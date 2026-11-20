//! Sauvegarde chiffrée de l'identité (`.cubelink-id`) : export, import par phrase de passe, application au démarrage.
//!
//! # Format v1 (durable : ne jamais changer le sens d'un champ, ajouter une version)
//!
//! ```text
//! offset  taille  contenu
//!   0       4     magic « CLID »
//!   4       1     version du format (1)
//!   5       1     KDF (1 = Argon2id v1.3)
//!   6       4     m_cost en Kio (u32 LE)
//!  10       4     t_cost (u32 LE)
//!  14       4     parallélisme (u32 LE)
//!  18      16     sel
//!  34      24     nonce XChaCha20-Poly1305
//!  58       …     texte chiffré + étiquette Poly1305 (16 octets)
//! ```
//!
//! L'en-tête entier (octets 0 à 57) est l'AAD : altérer un paramètre fait échouer l'authentification. La clé de
//! 32 octets vient d'Argon2id sur la phrase de passe (normalisée NFKC). Le clair est un JSON
//! [`BackupPayload`] ; les champs inconnus sont ignorés, ce qui permet d'ajouter plus tard les clés de groupe Hearth
//! sans changer la version du conteneur.
//!
//! Une mauvaise phrase de passe et un fichier altéré sont indistinguables (l'AEAD ne les sépare pas) : même erreur.
//!
//! # Import
//!
//! Aucun remplacement à chaud : l'identité et la base des amis sont tenues par le réseau et par Hearth. [`stage_import`]
//! déchiffre, valide et dépose le résultat dans `import-pending/` ; [`apply_pending_import`] l'applique au démarrage
//! suivant, avant l'ouverture de l'identité. L'existant est gardé en `*.bak-<horodatage>`.

use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;
use zeroize::{Zeroize, Zeroizing};

use crate::friends_db::FriendsDb;
use crate::identity::Identity;

/// Extension conseillée du fichier de sauvegarde.
pub const BACKUP_EXTENSION: &str = "cubelink-id";
/// Nom du dossier d'import en attente, dans le dossier `p2p`.
pub const PENDING_DIR: &str = "import-pending";

const MAGIC: &[u8; 4] = b"CLID";
const FORMAT_VERSION: u8 = 1;
const KDF_ARGON2ID: u8 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
const HEADER_LEN: usize = 4 + 1 + 1 + 4 + 4 + 4 + SALT_LEN + NONCE_LEN;
const TAG_LEN: usize = 16;
/// Taille maximale acceptée à la lecture : une sauvegarde fait quelques Kio.
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// Longueur minimale de la phrase de passe, imposée à l'export seulement.
pub const MIN_PASSPHRASE_CHARS: usize = 8;

const KEY_FILE: &str = "identity.key";
const DB_FILE: &str = "friends.db";
const STAMP_FILE: &str = "stamp";

/// Coût d'Argon2id, écrit dans le fichier. Les bornes à la lecture empêchent un fichier hostile de réclamer
/// des gigaoctets de mémoire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KdfParams {
    pub m_cost_kib: u32,
    pub t_cost: u32,
    pub parallelism: u32,
}

impl KdfParams {
    /// 64 Mio, 3 passes, 1 voie : de l'ordre de 0,3 s sur un PC récent, coûteux à forcer.
    pub const DEFAULT: KdfParams = KdfParams { m_cost_kib: 64 * 1024, t_cost: 3, parallelism: 1 };
    const MAX_M_COST_KIB: u32 = 256 * 1024;
    const MAX_T_COST: u32 = 10;
    const MAX_PARALLELISM: u32 = 4;

    fn within_bounds(&self) -> bool {
        self.m_cost_kib <= Self::MAX_M_COST_KIB
            && self.t_cost <= Self::MAX_T_COST
            && self.parallelism <= Self::MAX_PARALLELISM
    }
}

/// Erreurs de sauvegarde et de restauration, avec un message affichable tel quel.
#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("La phrase de passe est trop courte : {MIN_PASSPHRASE_CHARS} caractères au minimum.")]
    WeakPassphrase,
    #[error("Ce fichier n'est pas une sauvegarde d'identité Cubelink.")]
    NotABackup,
    #[error("Cette sauvegarde vient d'une version plus récente de Cubelink : mets Cubelink à jour pour la restaurer.")]
    UnsupportedVersion(u8),
    #[error("Phrase de passe incorrecte, ou fichier abîmé.")]
    BadPassphraseOrCorrupt,
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Io(String),
}

fn io(what: &str, e: impl fmt::Display) -> BackupError {
    BackupError::Io(format!("{what} : {e}"))
}

/// Contenu clair d'une sauvegarde. Contient la clé secrète : jamais de `Debug` détaillé, mémoire effacée à la chute.
#[derive(Serialize, Deserialize, Default)]
pub struct BackupPayload {
    /// Clé secrète ed25519 (32 octets) en hexadécimal.
    #[serde(default)]
    pub identity_key: String,
    /// Clé publique correspondante, pour contrôler la cohérence après déchiffrement.
    #[serde(default)]
    pub public_key: String,
    /// Secondes depuis l'époque Unix à la création.
    #[serde(default)]
    pub created_at: u64,
    /// Image SQLite de `friends.db` (base64), si présente.
    #[serde(default)]
    pub friends_db: Option<String>,
}

impl fmt::Debug for BackupPayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BackupPayload")
            .field("public_key", &self.public_key)
            .field("identity_key", &"<masquée>")
            .field("friends_db", &self.friends_db.as_ref().map(|d| d.len()))
            .finish()
    }
}

impl Drop for BackupPayload {
    fn drop(&mut self) {
        self.identity_key.zeroize();
    }
}

/// Normalise la phrase (NFKC : « é » composé ou décomposé donne la même clé) dans un tampon effacé à la chute.
fn passphrase_bytes(p: &str) -> Zeroizing<Vec<u8>> {
    Zeroizing::new(p.nfkc().collect::<String>().into_bytes())
}

fn derive_key(pass: &[u8], salt: &[u8], p: KdfParams) -> Result<Zeroizing<[u8; 32]>, BackupError> {
    let params = Params::new(p.m_cost_kib, p.t_cost, p.parallelism, Some(32))
        .map_err(|e| BackupError::Invalid(format!("paramètres de dérivation invalides : {e}")))?;
    let mut key = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(pass, salt, key.as_mut())
        .map_err(|e| BackupError::Invalid(format!("dérivation de la clé impossible : {e}")))?;
    Ok(key)
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    rand::Rng::fill_bytes(&mut rand::rand_core::UnwrapErr(rand::rngs::SysRng), &mut b);
    b
}

fn cipher(key: &[u8; 32]) -> XChaCha20Poly1305 {
    XChaCha20Poly1305::new(key.into())
}

fn seal(payload: &[u8], passphrase: &str, params: KdfParams) -> Result<Vec<u8>, BackupError> {
    let salt = random_bytes::<SALT_LEN>();
    let nonce = random_bytes::<NONCE_LEN>();
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len() + TAG_LEN);
    out.extend_from_slice(MAGIC);
    out.push(FORMAT_VERSION);
    out.push(KDF_ARGON2ID);
    out.extend_from_slice(&params.m_cost_kib.to_le_bytes());
    out.extend_from_slice(&params.t_cost.to_le_bytes());
    out.extend_from_slice(&params.parallelism.to_le_bytes());
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce);
    debug_assert_eq!(out.len(), HEADER_LEN);
    let key = derive_key(&passphrase_bytes(passphrase), &salt, params)?;
    let ct = cipher(&key)
        .encrypt(&XNonce::from(nonce), Payload { msg: payload, aad: &out })
        .map_err(|_| BackupError::Invalid("chiffrement impossible".into()))?;
    out.extend_from_slice(&ct);
    Ok(out)
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// Déchiffre une sauvegarde. Ne fait aucun calcul coûteux avant d'avoir vérifié magic, version et bornes.
pub fn decrypt_backup(bytes: &[u8], passphrase: &str) -> Result<BackupPayload, BackupError> {
    if bytes.len() < HEADER_LEN + TAG_LEN || &bytes[..4] != MAGIC {
        return Err(BackupError::NotABackup);
    }
    if bytes[4] != FORMAT_VERSION {
        return Err(BackupError::UnsupportedVersion(bytes[4]));
    }
    if bytes[5] != KDF_ARGON2ID {
        return Err(BackupError::UnsupportedVersion(bytes[5]));
    }
    let params = KdfParams { m_cost_kib: u32_at(bytes, 6), t_cost: u32_at(bytes, 10), parallelism: u32_at(bytes, 14) };
    if !params.within_bounds() {
        return Err(BackupError::BadPassphraseOrCorrupt);
    }
    let (header, ct) = bytes.split_at(HEADER_LEN);
    let salt = &header[18..18 + SALT_LEN];
    let nonce: [u8; NONCE_LEN] = header[18 + SALT_LEN..].try_into().map_err(|_| BackupError::NotABackup)?;
    // Des paramètres invalides pour Argon2 (ex. mémoire trop petite) ne peuvent venir que d'un fichier abîmé.
    let key = derive_key(&passphrase_bytes(passphrase), salt, params).map_err(|_| BackupError::BadPassphraseOrCorrupt)?;
    let plain = Zeroizing::new(
        cipher(&key)
            .decrypt(&XNonce::from(nonce), Payload { msg: ct, aad: header })
            .map_err(|_| BackupError::BadPassphraseOrCorrupt)?,
    );
    serde_json::from_slice::<BackupPayload>(&plain).map_err(|_| BackupError::BadPassphraseOrCorrupt)
}

fn secret_of(payload: &BackupPayload) -> Result<Zeroizing<[u8; 32]>, BackupError> {
    let bad = || BackupError::Invalid("La clé de cette sauvegarde est illisible.".into());
    let raw = Zeroizing::new(hex::decode(&payload.identity_key).map_err(|_| bad())?);
    let arr: [u8; 32] = raw.as_slice().try_into().map_err(|_| bad())?;
    let secret = Zeroizing::new(arr);
    let public = hex::encode(SigningKey::from_bytes(&secret).verifying_key().to_bytes());
    if public != payload.public_key {
        return Err(BackupError::Invalid("Cette sauvegarde est incohérente : la clé publique ne correspond pas.".into()));
    }
    Ok(secret)
}

/// Écrit `data` dans `dest` de façon atomique (temporaire puis renommage, remplace l'existant), droits 0600 sous unix.
fn write_replace(dest: &Path, data: &[u8]) -> Result<(), BackupError> {
    let mut name = dest.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(format!(".{}.tmp", hex::encode(random_bytes::<4>())));
    let tmp = dest.with_file_name(name);
    let res = (|| -> Result<(), BackupError> {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp).map_err(|e| io("création du fichier", e))?;
        f.write_all(data).map_err(|e| io("écriture du fichier", e))?;
        f.sync_all().map_err(|e| io("écriture du fichier", e))?;
        drop(f);
        std::fs::rename(&tmp, dest).map_err(|e| io("remplacement du fichier", e))
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

/// Exporte l'identité et la liste d'amis dans `dest`, chiffrées par `passphrase`. Renvoie la clé publique (hex).
/// `scratch` : dossier privé pour la copie temporaire de la base.
pub fn export_identity(
    identity: &Identity,
    friends: &FriendsDb,
    scratch: &Path,
    passphrase: &str,
    dest: &Path,
) -> Result<String, BackupError> {
    export_identity_with(identity, friends, scratch, passphrase, dest, KdfParams::DEFAULT)
}

/// Comme [`export_identity`] avec un coût Argon2 choisi (tests : coût réduit).
#[doc(hidden)]
pub fn export_identity_with(
    identity: &Identity,
    friends: &FriendsDb,
    scratch: &Path,
    passphrase: &str,
    dest: &Path,
    params: KdfParams,
) -> Result<String, BackupError> {
    if passphrase.nfkc().count() < MIN_PASSPHRASE_CHARS {
        return Err(BackupError::WeakPassphrase);
    }
    if dest.is_dir() {
        return Err(BackupError::Invalid("Choisis un nom de fichier, pas un dossier.".into()));
    }
    let snapshot = friends.snapshot_bytes(scratch).map_err(|e| BackupError::Io(e.to_string()))?;
    let secret = Zeroizing::new(identity.secret_bytes());
    let public_key = identity.public_key_hex();
    let payload = BackupPayload {
        identity_key: hex::encode(secret.as_ref()),
        public_key: public_key.clone(),
        created_at: crate::util::now_secs(),
        friends_db: Some(data_encoding::BASE64.encode(&snapshot)),
    };
    let plain = Zeroizing::new(serde_json::to_vec(&payload).map_err(|e| BackupError::Invalid(e.to_string()))?);
    let sealed = seal(&plain, passphrase, params)?;
    write_replace(dest, &sealed)?;
    Ok(public_key)
}

fn read_backup_file(path: &Path) -> Result<Vec<u8>, BackupError> {
    let meta = std::fs::metadata(path).map_err(|e| io("lecture du fichier", e))?;
    if !meta.is_file() {
        return Err(BackupError::NotABackup);
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(BackupError::NotABackup);
    }
    std::fs::read(path).map_err(|e| io("lecture du fichier", e))
}

/// Déchiffre et valide la sauvegarde `src`, puis la dépose dans `p2p_dir/import-pending/` pour le prochain démarrage.
/// Ne touche ni à l'identité ni à la base en cours. Renvoie la clé publique de l'identité à restaurer.
pub fn stage_import(p2p_dir: &Path, src: &Path, passphrase: &str) -> Result<String, BackupError> {
    let bytes = read_backup_file(src)?;
    let payload = decrypt_backup(&bytes, passphrase)?;
    let secret = secret_of(&payload)?;
    let db_bytes = match &payload.friends_db {
        Some(b64) => Some(
            data_encoding::BASE64
                .decode(b64.as_bytes())
                .map_err(|_| BackupError::Invalid("La liste d'amis de cette sauvegarde est illisible.".into()))?,
        ),
        None => None,
    };

    std::fs::create_dir_all(p2p_dir).map_err(|e| io("création du dossier", e))?;
    let tmp = p2p_dir.join(format!("{PENDING_DIR}.tmp"));
    let pending = p2p_dir.join(PENDING_DIR);
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| io("création du dossier", e))?;
    let res = (|| -> Result<(), BackupError> {
        if let Some(db) = &db_bytes {
            let db_path = tmp.join(DB_FILE);
            write_replace(&db_path, db)?;
            // La base doit s'ouvrir (schéma connu, pas de version plus récente) : sinon on refuse avant tout changement.
            let opened = FriendsDb::open(&db_path).map_err(|_| {
                BackupError::Invalid("La liste d'amis de cette sauvegarde est illisible ou vient d'une version plus récente.".into())
            })?;
            drop(opened);
            // Fermée proprement, la base n'a plus de journal WAL à côté.
            let _ = std::fs::remove_file(tmp.join("friends.db-wal"));
            let _ = std::fs::remove_file(tmp.join("friends.db-shm"));
        }
        write_replace(&tmp.join(KEY_FILE), secret.as_ref())?;
        write_replace(&tmp.join(STAMP_FILE), crate::util::now_secs().to_string().as_bytes())?;
        let _ = std::fs::remove_dir_all(&pending);
        std::fs::rename(&tmp, &pending).map_err(|e| io("dépôt de la restauration", e))
    })();
    if res.is_err() {
        let _ = std::fs::remove_dir_all(&tmp);
    }
    res?;
    Ok(payload.public_key.clone())
}

/// Une restauration est en attente de redémarrage.
pub fn has_pending_import(p2p_dir: &Path) -> bool {
    p2p_dir.join(PENDING_DIR).join(KEY_FILE).is_file()
}

fn same_content(a: &Path, b: &Path) -> bool {
    matches!((std::fs::read(a), std::fs::read(b)), (Ok(x), Ok(y)) if x == y)
}

fn bak_name(path: &Path, stamp: &str) -> PathBuf {
    let mut n = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    n.push(format!(".bak-{stamp}"));
    path.with_file_name(n)
}

/// Copie `from` en `to` de façon atomique (remplace `to`).
fn copy_replace(from: &Path, to: &Path) -> Result<(), BackupError> {
    let data = Zeroizing::new(std::fs::read(from).map_err(|e| io("lecture du fichier", e))?);
    write_replace(to, &data)
}

/// Nombre de démarrages après lesquels une restauration qui échoue toujours sur une erreur d'E/S est abandonnée.
const MAX_APPLY_LAUNCHES: u32 = 3;
/// Fichier compteur déposé dans `import-pending/` après un échec d'E/S.
const ATTEMPTS_FILE: &str = "attempts";
/// Réessais au sein d'un même démarrage (Windows : l'ancien processus peut encore tenir `friends.db` ouverte
/// pendant quelques instants après `app.restart()`), et pause entre deux.
const APPLY_RETRIES: u32 = 20;
const APPLY_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(250);

/// Fichiers `friends.db*` de `dir` encore ouverts par ce processus (Linux : `/proc/self/fd`).
/// Ailleurs que sous Linux, aucune mesure n'est possible : la liste est vide (la CI Windows vérifie le renommage pour de vrai).
#[cfg(all(test, not(target_os = "linux")))]
fn open_db_files(_dir: &Path) -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(all(test, target_os = "linux"))]
fn open_db_files(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter_map(|e| std::fs::read_link(e.ok()?.path()).ok())
        .filter(|p| p.parent() == Some(dir) && p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("friends.db")))
        .collect()
}

/// Applique une restauration déposée par [`stage_import`]. À appeler au tout début du démarrage, avant l'ouverture
/// de l'identité et de la base. Sans restauration en attente : ne fait rien (`Ok(false)`).
/// `hearth_dir` : dossier des groupes Hearth, lié à l'identité ; quand la clé change, il est mis de côté en
/// `hearth.bak-<horodatage>` (sinon des groupes de l'ancienne clé resteraient chargés sous la nouvelle).
///
/// Reprenable : le dossier en attente n'est supprimé qu'à la fin ; rejouée après un arrêt brutal, l'application
/// saute ce qui est déjà en place. L'ancienne identité n'est jamais absente du disque (copie `.bak`, puis
/// remplacement atomique). En cas d'échec, les anciens fichiers sont remis. Une erreur d'E/S (fichier verrouillé)
/// est réessayée quelques secondes, puis la restauration reste en attente pour le démarrage suivant (trois au
/// plus) ; une restauration invalide est abandonnée tout de suite.
pub fn apply_pending_import(p2p_dir: &Path, hearth_dir: Option<&Path>) -> Result<bool, BackupError> {
    apply_pending_import_with(p2p_dir, hearth_dir, APPLY_RETRIES, APPLY_RETRY_DELAY)
}

fn apply_pending_import_with(
    p2p_dir: &Path,
    hearth_dir: Option<&Path>,
    retries: u32,
    delay: std::time::Duration,
) -> Result<bool, BackupError> {
    let pending = p2p_dir.join(PENDING_DIR);
    // Un dépôt interrompu avant son renommage final n'est jamais appliqué.
    let _ = std::fs::remove_dir_all(p2p_dir.join(format!("{PENDING_DIR}.tmp")));
    if !pending.exists() {
        return Ok(false);
    }
    let launches: u32 = std::fs::read_to_string(pending.join(ATTEMPTS_FILE)).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    let mut res = apply_inner(p2p_dir, hearth_dir, &pending);
    for _ in 0..retries {
        if !matches!(res, Err(BackupError::Io(_))) {
            break;
        }
        std::thread::sleep(delay);
        res = apply_inner(p2p_dir, hearth_dir, &pending);
    }
    match &res {
        // Fichier encore verrouillé : la restauration reste en attente, rejouée au prochain démarrage.
        Err(BackupError::Io(_)) if launches + 1 < MAX_APPLY_LAUNCHES => {
            let _ = std::fs::write(pending.join(ATTEMPTS_FILE), (launches + 1).to_string());
        }
        // Réussie, invalide ou trop de démarrages ratés : terminée, on ne la rejoue pas.
        _ => {
            let _ = std::fs::remove_dir_all(&pending);
        }
    }
    res.map(|()| true)
}

fn apply_inner(p2p_dir: &Path, hearth_dir: Option<&Path>, pending: &Path) -> Result<(), BackupError> {
    // Contrainte Windows rejouée sous Linux : rien ne doit tenir `friends.db*` ouvert pendant l'échange de fichiers.
    #[cfg(all(test, target_os = "linux"))]
    assert_eq!(open_db_files(p2p_dir), Vec::<PathBuf>::new(), "friends.db* est encore ouvert : Windows refuserait le renommage");
    let new_key = pending.join(KEY_FILE);
    let new_db = pending.join(DB_FILE);
    let key_ok = std::fs::metadata(&new_key).map(|m| m.len() == 32).unwrap_or(false);
    if !key_ok {
        return Err(BackupError::Invalid("restauration en attente incomplète, ignorée".into()));
    }
    let stamp = std::fs::read_to_string(pending.join(STAMP_FILE))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .unwrap_or_else(|| crate::util::now_secs().to_string());

    let cur_db = p2p_dir.join(DB_FILE);
    let cur_key = p2p_dir.join(KEY_FILE);
    let side_files = [p2p_dir.join("friends.db-wal"), p2p_dir.join("friends.db-shm")];
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new(); // (sauvegarde, emplacement d'origine)

    let step = (|| -> Result<(), BackupError> {
        // 1. Amis : l'ancienne base (et son journal) part en .bak, la nouvelle est copiée en place.
        if new_db.is_file() && !(cur_db.is_file() && same_content(&new_db, &cur_db)) {
            for orig in std::iter::once(&cur_db).chain(side_files.iter()) {
                if orig.exists() {
                    let bak = bak_name(orig, &stamp);
                    std::fs::rename(orig, &bak).map_err(|e| io("sauvegarde de l'ancienne liste d'amis", e))?;
                    moved.push((bak, orig.clone()));
                }
            }
            copy_replace(&new_db, &cur_db)?;
        }
        // 2. Identité, en dernier : c'est le point de validation. La clé actuelle est copiée en .bak d'abord.
        if !(cur_key.is_file() && same_content(&new_key, &cur_key)) {
            // Les groupes Hearth sont liés à l'ancienne clé : mis de côté avant qu'elle change (reprenable : absent = fait).
            if let Some(h) = hearth_dir.filter(|h| h.exists()) {
                let bak = bak_name(h, &stamp);
                std::fs::rename(h, &bak).map_err(|e| io("mise de côté des groupes Hearth", e))?;
                moved.push((bak, h.to_path_buf()));
            }
            if cur_key.exists() {
                copy_replace(&cur_key, &bak_name(&cur_key, &stamp))?;
            }
            copy_replace(&new_key, &cur_key)?;
        }
        Ok(())
    })();

    if let Err(e) = step {
        // Retour arrière : les anciens fichiers reprennent leur place, l'ancienne identité reste en service.
        for (bak, orig) in moved.iter().rev() {
            let _ = std::fs::remove_file(orig);
            let _ = std::fs::rename(bak, orig);
        }
        return Err(e);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_lib::types::{Friend, TrustLevel};

    /// Coût réduit : les tests vérifient la logique, pas la dureté d'Argon2.
    const FAST: KdfParams = KdfParams { m_cost_kib: 64, t_cost: 1, parallelism: 1 };
    const PASS: &str = "correct cheval pile";

    fn key(c: char) -> String {
        c.to_string().repeat(64)
    }

    fn setup(dir: &Path) -> (Identity, FriendsDb) {
        let id = Identity::load_or_create(dir).unwrap();
        let db = FriendsDb::open(dir.join("friends.db")).unwrap();
        db.add_friend(&Friend { public_key: key('a'), name: "Alice".into(), trust: TrustLevel::Always, added_at: 42 }).unwrap();
        db.add_friend(&Friend { public_key: key('b'), name: "Bob é".into(), trust: TrustLevel::Ask, added_at: 43 }).unwrap();
        (id, db)
    }

    fn export(dir: &Path, id: &Identity, db: &FriendsDb, pass: &str) -> PathBuf {
        let out = dir.join("moi.cubelink-id");
        export_identity_with(id, db, dir, pass, &out, FAST).unwrap();
        out
    }

    #[test]
    fn roundtrip_restores_same_key_and_friends() {
        let src = tempfile::tempdir().unwrap();
        let (id, db) = setup(src.path());
        let file = export(src.path(), &id, &db, PASS);

        let dst = tempfile::tempdir().unwrap();
        let pk = stage_import(dst.path(), &file, PASS).unwrap();
        assert_eq!(pk, id.public_key_hex());
        assert!(has_pending_import(dst.path()));
        assert!(apply_pending_import(dst.path(), None).unwrap());
        assert!(!dst.path().join(PENDING_DIR).exists());

        let id2 = Identity::load_or_create(dst.path()).unwrap();
        assert_eq!(id2.public_key_hex(), id.public_key_hex());
        let db2 = FriendsDb::open(dst.path().join("friends.db")).unwrap();
        assert_eq!(db2.list_friends().unwrap(), db.list_friends().unwrap());
    }

    #[test]
    fn export_returns_public_key_and_file_has_no_plaintext_secret() {
        let d = tempfile::tempdir().unwrap();
        let (id, db) = setup(d.path());
        let file = export(d.path(), &id, &db, PASS);
        let raw = std::fs::read(&file).unwrap();
        assert_eq!(&raw[..4], b"CLID");
        let secret_hex = hex::encode(id.secret_bytes());
        let hay = String::from_utf8_lossy(&raw);
        assert!(!hay.contains(&secret_hex) && !hay.contains("Alice"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn export_refuses_short_passphrase_and_directory() {
        let d = tempfile::tempdir().unwrap();
        let (id, db) = setup(d.path());
        let out = d.path().join("x.cubelink-id");
        assert!(matches!(export_identity_with(&id, &db, d.path(), "court", &out, FAST), Err(BackupError::WeakPassphrase)));
        assert!(!out.exists());
        assert!(matches!(export_identity_with(&id, &db, d.path(), PASS, d.path(), FAST), Err(BackupError::Invalid(_))));
    }

    #[test]
    fn export_leaves_no_scratch_files_and_overwrites() {
        let d = tempfile::tempdir().unwrap();
        let (id, db) = setup(d.path());
        let scratch = d.path().join("scratch");
        let out = d.path().join("a.cubelink-id");
        export_identity_with(&id, &db, &scratch, PASS, &out, FAST).unwrap();
        export_identity_with(&id, &db, &scratch, PASS, &out, FAST).unwrap();
        assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0);
        assert!(std::fs::read_dir(d.path()).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().ends_with(".tmp")));
    }

    #[test]
    fn wrong_passphrase_is_rejected() {
        let d = tempfile::tempdir().unwrap();
        let (id, db) = setup(d.path());
        let bytes = std::fs::read(export(d.path(), &id, &db, PASS)).unwrap();
        assert!(matches!(decrypt_backup(&bytes, "une autre phrase"), Err(BackupError::BadPassphraseOrCorrupt)));
    }

    #[test]
    fn unicode_equivalent_passphrases_match() {
        let d = tempfile::tempdir().unwrap();
        let (id, db) = setup(d.path());
        let composed = "été passé là-bas";
        let decomposed = "e\u{301}te\u{301} passe\u{301} la\u{300}-bas";
        let bytes = std::fs::read(export(d.path(), &id, &db, composed)).unwrap();
        assert!(decrypt_backup(&bytes, decomposed).is_ok());
    }

    #[test]
    fn every_zone_tampered_is_rejected() {
        let d = tempfile::tempdir().unwrap();
        let (id, db) = setup(d.path());
        let bytes = std::fs::read(export(d.path(), &id, &db, PASS)).unwrap();
        assert!(decrypt_backup(&bytes, PASS).is_ok());
        // Octet par zone : version et KDF (refusés avant tout calcul), paramètres, sel, nonce, corps, étiquette.
        for at in [0usize, 4, 5, 6, 10, 14, 18, 34, 58, bytes.len() / 2, bytes.len() - 1] {
            let mut b = bytes.clone();
            b[at] ^= 0x01;
            assert!(decrypt_backup(&b, PASS).is_err(), "octet {at} altéré accepté");
        }
    }

    #[test]
    fn truncated_empty_and_foreign_files_are_rejected() {
        let d = tempfile::tempdir().unwrap();
        let (id, db) = setup(d.path());
        let bytes = std::fs::read(export(d.path(), &id, &db, PASS)).unwrap();
        assert!(matches!(decrypt_backup(&[], PASS), Err(BackupError::NotABackup)));
        assert!(matches!(decrypt_backup(b"PK\x03\x04 pas une sauvegarde du tout, un zip peut-etre......", PASS), Err(BackupError::NotABackup)));
        assert!(matches!(decrypt_backup(&bytes[..HEADER_LEN], PASS), Err(BackupError::NotABackup)));
        assert!(decrypt_backup(&bytes[..bytes.len() - 1], PASS).is_err());
        assert!(decrypt_backup(&bytes[..bytes.len() / 2], PASS).is_err());
    }

    #[test]
    fn unknown_version_is_reported_before_any_kdf() {
        let d = tempfile::tempdir().unwrap();
        let (id, db) = setup(d.path());
        let mut bytes = std::fs::read(export(d.path(), &id, &db, PASS)).unwrap();
        bytes[4] = 2;
        assert!(matches!(decrypt_backup(&bytes, PASS), Err(BackupError::UnsupportedVersion(2))));
    }

    #[test]
    fn hostile_kdf_params_are_refused_without_allocating() {
        let d = tempfile::tempdir().unwrap();
        let (id, db) = setup(d.path());
        let mut bytes = std::fs::read(export(d.path(), &id, &db, PASS)).unwrap();
        bytes[6..10].copy_from_slice(&u32::MAX.to_le_bytes()); // m_cost = 4 Tio
        let t = std::time::Instant::now();
        assert!(matches!(decrypt_backup(&bytes, PASS), Err(BackupError::BadPassphraseOrCorrupt)));
        assert!(t.elapsed() < std::time::Duration::from_secs(2));
        let mut bytes2 = std::fs::read(export(d.path(), &id, &db, PASS)).unwrap();
        bytes2[10..14].copy_from_slice(&1000u32.to_le_bytes()); // t_cost
        assert!(decrypt_backup(&bytes2, PASS).is_err());
    }

    #[test]
    fn two_exports_differ() {
        let d = tempfile::tempdir().unwrap();
        let (id, db) = setup(d.path());
        let a = std::fs::read(export(d.path(), &id, &db, PASS)).unwrap();
        let b = std::fs::read(export(d.path(), &id, &db, PASS)).unwrap();
        assert_ne!(a, b);
        assert_ne!(a[18..58], b[18..58], "sel et nonce aléatoires");
    }

    #[test]
    fn inconsistent_public_key_is_refused_at_staging() {
        let d = tempfile::tempdir().unwrap();
        let payload = BackupPayload {
            identity_key: hex::encode([7u8; 32]),
            public_key: key('0'),
            created_at: 1,
            friends_db: None,
        };
        let sealed = seal(&serde_json::to_vec(&payload).unwrap(), PASS, FAST).unwrap();
        let f = d.path().join("faux.cubelink-id");
        std::fs::write(&f, sealed).unwrap();
        let p2p = d.path().join("p2p");
        assert!(matches!(stage_import(&p2p, &f, PASS), Err(BackupError::Invalid(_))));
        assert!(!p2p.join(PENDING_DIR).exists());
    }

    #[test]
    fn corrupt_or_future_friends_db_is_refused_and_nothing_changes() {
        let d = tempfile::tempdir().unwrap();
        let sk = SigningKey::from_bytes(&[9u8; 32]);
        let mk = |db: Vec<u8>| {
            let p = BackupPayload {
                identity_key: hex::encode([9u8; 32]),
                public_key: hex::encode(sk.verifying_key().to_bytes()),
                created_at: 1,
                friends_db: Some(data_encoding::BASE64.encode(&db)),
            };
            let f = d.path().join("b.cubelink-id");
            std::fs::write(&f, seal(&serde_json::to_vec(&p).unwrap(), PASS, FAST).unwrap()).unwrap();
            f
        };
        let p2p = d.path().join("p2p");
        std::fs::create_dir_all(&p2p).unwrap();
        // Base corrompue.
        let f = mk(b"ceci n'est pas du SQLite, loin de la".repeat(20));
        assert!(matches!(stage_import(&p2p, &f, PASS), Err(BackupError::Invalid(_))));
        // Schéma plus récent.
        let fut = d.path().join("fut.db");
        {
            let c = rusqlite::Connection::open(&fut).unwrap();
            c.pragma_update(None, "user_version", crate::friends_db::SCHEMA_VERSION + 5).unwrap();
            c.execute_batch("CREATE TABLE t(x);").unwrap();
        }
        let f = mk(std::fs::read(&fut).unwrap());
        assert!(matches!(stage_import(&p2p, &f, PASS), Err(BackupError::Invalid(_))));
        assert!(!p2p.join(PENDING_DIR).exists());
        assert!(!p2p.join(format!("{PENDING_DIR}.tmp")).exists());
    }

    #[test]
    fn snapshot_includes_uncheckpointed_wal_rows() {
        let d = tempfile::tempdir().unwrap();
        let (_id, db) = setup(d.path());
        // Les lignes sont dans le journal WAL, pas forcément dans le fichier principal.
        let bytes = db.snapshot_bytes(&d.path().join("s")).unwrap();
        let f = d.path().join("copy.db");
        std::fs::write(&f, bytes).unwrap();
        let copy = FriendsDb::open(&f).unwrap();
        assert_eq!(copy.list_friends().unwrap().len(), 2);
    }

    #[test]
    fn apply_without_pending_is_a_noop() {
        let d = tempfile::tempdir().unwrap();
        assert!(!apply_pending_import(d.path(), None).unwrap());
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 0);
    }

    #[test]
    fn apply_keeps_bak_of_existing_identity_and_friends() {
        let src = tempfile::tempdir().unwrap();
        let (id, db) = setup(src.path());
        let file = export(src.path(), &id, &db, PASS);

        let dst = tempfile::tempdir().unwrap();
        let old = Identity::load_or_create(dst.path()).unwrap();
        let old_db = FriendsDb::open(dst.path().join("friends.db")).unwrap();
        old_db.add_friend(&Friend { public_key: key('c'), name: "Carole".into(), trust: TrustLevel::Ask, added_at: 1 }).unwrap();
        // Arrêt brutal simulé : le journal WAL et le `-shm` d'une base non fermée restent sur le disque. On les fige par
        // copie pendant que la connexion vit, puis la connexion est fermée : aucun descripteur ne reste ouvert
        // (sous Windows il bloquerait le renommage ; l'application s'exécute avant toute ouverture de la base).
        let crashed = dst.path().join("crashed");
        std::fs::create_dir_all(&crashed).unwrap();
        for n in ["friends.db", "friends.db-wal", "friends.db-shm"] {
            let _ = std::fs::copy(dst.path().join(n), crashed.join(n));
        }
        drop(old_db);
        for n in ["friends.db", "friends.db-wal", "friends.db-shm"] {
            let _ = std::fs::remove_file(dst.path().join(n));
            if crashed.join(n).exists() {
                std::fs::rename(crashed.join(n), dst.path().join(n)).unwrap();
            }
        }
        std::fs::remove_dir(&crashed).unwrap();
        assert!(dst.path().join("friends.db-wal").exists(), "précondition : un journal WAL traîne");
        assert!(open_db_files(dst.path()).is_empty(), "précondition : aucune connexion ouverte");

        stage_import(dst.path(), &file, PASS).unwrap();
        assert!(apply_pending_import(dst.path(), None).unwrap());

        let names: Vec<String> = std::fs::read_dir(dst.path()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert!(names.iter().any(|n| n.starts_with("identity.key.bak-")), "{names:?}");
        assert!(names.iter().any(|n| n.starts_with("friends.db.bak-")), "{names:?}");
        let bak_key = names.iter().find(|n| n.starts_with("identity.key.bak-")).unwrap();
        assert_eq!(std::fs::read(dst.path().join(bak_key)).unwrap(), old.secret_bytes());
        let now = Identity::load_or_create(dst.path()).unwrap();
        assert_eq!(now.public_key_hex(), id.public_key_hex());
        let friends = FriendsDb::open(dst.path().join("friends.db")).unwrap().list_friends().unwrap();
        assert_eq!(friends.len(), 2);
        assert!(friends.iter().all(|f| f.name != "Carole"));
        // L'ancienne base, remise en place avec son journal, contient encore Carole.
        let restored = dst.path().join("old");
        std::fs::create_dir_all(&restored).unwrap();
        for n in names.iter().filter(|n| n.starts_with("friends.db")) {
            let target = n.split(".bak-").next().unwrap();
            std::fs::rename(dst.path().join(n), restored.join(target)).unwrap();
        }
        let old_friends = FriendsDb::open(restored.join("friends.db")).unwrap().list_friends().unwrap();
        assert!(old_friends.iter().any(|f| f.name == "Carole"));
    }

    #[test]
    fn apply_is_resumable_after_a_crash() {
        let src = tempfile::tempdir().unwrap();
        let (id, db) = setup(src.path());
        let file = export(src.path(), &id, &db, PASS);
        let dst = tempfile::tempdir().unwrap();
        let _old = Identity::load_or_create(dst.path()).unwrap();
        stage_import(dst.path(), &file, PASS).unwrap();
        let pending = dst.path().join(PENDING_DIR);
        // Arrêt brutal juste après l'étape « amis » : la base est en place, l'identité pas encore.
        std::fs::copy(pending.join(DB_FILE), dst.path().join(DB_FILE)).unwrap();
        assert!(apply_pending_import(dst.path(), None).unwrap());
        assert_eq!(Identity::load_or_create(dst.path()).unwrap().public_key_hex(), id.public_key_hex());
        assert_eq!(FriendsDb::open(dst.path().join(DB_FILE)).unwrap().list_friends().unwrap().len(), 2);
        // Rejouée en entier : sans rien de nouveau à faire (pas de pending), toujours sans effet.
        assert!(!apply_pending_import(dst.path(), None).unwrap());
    }

    #[test]
    fn apply_failure_rolls_back_and_keeps_the_restore_for_the_next_launch() {
        let src = tempfile::tempdir().unwrap();
        let (id, db) = setup(src.path());
        let file = export(src.path(), &id, &db, PASS);
        let dst = tempfile::tempdir().unwrap();
        let (old_id, old_db) = setup(dst.path());
        old_db.add_friend(&Friend { public_key: key('d'), name: "Dora".into(), trust: TrustLevel::Ask, added_at: 1 }).unwrap();
        drop(old_db);
        stage_import(dst.path(), &file, PASS).unwrap();
        // Panne à l'étape « identité » : un dossier occupe le nom du fichier .bak voulu (équivalent d'un fichier verrouillé).
        let pending_stamp = std::fs::read_to_string(dst.path().join(PENDING_DIR).join(STAMP_FILE)).unwrap();
        let obstacle = dst.path().join(format!("identity.key.bak-{}", pending_stamp.trim()));
        std::fs::create_dir_all(&obstacle).unwrap();
        let none = std::time::Duration::ZERO;
        assert!(apply_pending_import_with(dst.path(), None, 2, none).is_err());
        // Erreur d'E/S : la restauration n'est pas perdue, elle est rejouée au démarrage suivant.
        assert!(has_pending_import(dst.path()));
        assert_eq!(Identity::load_or_create(dst.path()).unwrap().public_key_hex(), old_id.public_key_hex());
        let friends = FriendsDb::open(dst.path().join(DB_FILE)).unwrap().list_friends().unwrap();
        assert!(friends.iter().any(|f| f.name == "Dora"), "{friends:?}");
        // Le verrou disparaît : le démarrage suivant applique la restauration.
        std::fs::remove_dir_all(&obstacle).unwrap();
        assert!(apply_pending_import_with(dst.path(), None, 0, none).unwrap());
        assert!(!dst.path().join(PENDING_DIR).exists());
        assert_eq!(Identity::load_or_create(dst.path()).unwrap().public_key_hex(), id.public_key_hex());
    }

    #[test]
    fn apply_gives_up_after_too_many_failed_launches() {
        let src = tempfile::tempdir().unwrap();
        let (id, db) = setup(src.path());
        let file = export(src.path(), &id, &db, PASS);
        let dst = tempfile::tempdir().unwrap();
        let (old_id, old_db) = setup(dst.path());
        drop(old_db);
        stage_import(dst.path(), &file, PASS).unwrap();
        let stamp = std::fs::read_to_string(dst.path().join(PENDING_DIR).join(STAMP_FILE)).unwrap();
        std::fs::create_dir_all(dst.path().join(format!("identity.key.bak-{}", stamp.trim()))).unwrap();
        let none = std::time::Duration::ZERO;
        for _ in 0..MAX_APPLY_LAUNCHES - 1 {
            assert!(apply_pending_import_with(dst.path(), None, 0, none).is_err());
            assert!(has_pending_import(dst.path()));
        }
        assert!(apply_pending_import_with(dst.path(), None, 0, none).is_err());
        assert!(!dst.path().join(PENDING_DIR).exists());
        assert_eq!(Identity::load_or_create(dst.path()).unwrap().public_key_hex(), old_id.public_key_hex());
    }

    #[test]
    fn hearth_groups_are_set_aside_when_the_key_changes() {
        let src = tempfile::tempdir().unwrap();
        let (id, db) = setup(src.path());
        let file = export(src.path(), &id, &db, PASS);
        let dst = tempfile::tempdir().unwrap();
        let _old = Identity::load_or_create(dst.path()).unwrap();
        let hearth = dst.path().join("hearth-test");
        std::fs::create_dir_all(&hearth).unwrap();
        std::fs::write(hearth.join("meta.json"), b"{}").unwrap();
        stage_import(dst.path(), &file, PASS).unwrap();
        assert!(apply_pending_import(dst.path(), Some(&hearth)).unwrap());
        assert!(!hearth.exists());
        let stamp_bak = std::fs::read_dir(dst.path()).unwrap().filter_map(|e| e.ok()).any(|e| {
            e.file_name().to_string_lossy().starts_with("hearth-test.bak-") && e.path().join("meta.json").is_file()
        });
        assert!(stamp_bak);
    }

    #[test]
    fn hearth_groups_are_kept_when_the_key_is_unchanged() {
        let src = tempfile::tempdir().unwrap();
        let (id, db) = setup(src.path());
        let file = export(src.path(), &id, &db, PASS);
        // Restaurer sur place : la connexion de l'application est fermée (elle l'est avant le redémarrage).
        drop(db);
        let hearth = src.path().join("hearth-test");
        std::fs::create_dir_all(&hearth).unwrap();
        stage_import(src.path(), &file, PASS).unwrap();
        assert!(apply_pending_import(src.path(), Some(&hearth)).unwrap());
        assert!(hearth.exists());
    }

    #[test]
    fn partial_staging_directory_is_never_applied() {
        let d = tempfile::tempdir().unwrap();
        let _ = Identity::load_or_create(d.path()).unwrap();
        let tmp = d.path().join(format!("{PENDING_DIR}.tmp"));
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join(KEY_FILE), [1u8; 32]).unwrap();
        assert!(!apply_pending_import(d.path(), None).unwrap());
        assert!(!tmp.exists());
    }

    #[test]
    fn incomplete_pending_is_dropped_without_touching_the_identity() {
        let d = tempfile::tempdir().unwrap();
        let id = Identity::load_or_create(d.path()).unwrap();
        let p = d.path().join(PENDING_DIR);
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join(KEY_FILE), b"trop court").unwrap();
        assert!(apply_pending_import(d.path(), None).is_err());
        assert!(!p.exists());
        assert_eq!(Identity::load_or_create(d.path()).unwrap().public_key_hex(), id.public_key_hex());
    }

    #[test]
    fn payload_debug_hides_secret() {
        let p = BackupPayload { identity_key: hex::encode([5u8; 32]), public_key: key('1'), created_at: 0, friends_db: None };
        assert!(!format!("{p:?}").contains(&hex::encode([5u8; 32])));
    }
}
