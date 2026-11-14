//! Hearth : publication, rapatriement et matérialisation des mondes (voir `docs/hearth.md`, § 3.2, § 4, § 5).
//!
//! Un monde est une **collection iroh-blobs** (`chemin relatif → blob BLAKE3`, racine de type `HashSeq`). Deux
//! versions partagent tous les blobs inchangés : rapatrier la suivante ne transfère que les fichiers modifiés.
//!
//! * [`WorldStore::publish_dir`] importe les fichiers d'un dossier (sans `session.lock` ni `*.tmp`, sans lien
//!   symbolique) dans le magasin et fabrique la collection. Les fichiers dont la taille et la date n'ont pas bougé
//!   depuis la dernière publication ne sont pas relus (même précaution « fichier trop récent » que `git`).
//! * [`WorldStore::fetch`] rapatrie une collection depuis un ou plusieurs pairs (reprise sur ce qui est déjà là,
//!   bascule sur le pair suivant en cas d'échec) en trois temps bornés : la liste des hachages, les noms, puis les
//!   fichiers. Chaque temps a une limite d'octets ; le total est borné par la taille signée dans l'en-tête de version.
//! * [`WorldStore::materialize`] écrit le monde dans un dossier, de façon atomique (dossier voisin puis échange).
//!
//! # Contrôle d'accès
//!
//! iroh-blobs ne sait pas autoriser « ce pair, ce blob ». Hearth ne partage donc **jamais** le magasin de
//! transfert des mondes solo : il a son **propre magasin** et son **propre ALPN** ([`HEARTH_BLOBS_ALPN`]) ; le
//! `Router` de l'application l'enregistre avec [`WorldStore::blobs_handler`]. Ce handler est branché sur un
//! `EventSender` d'iroh-blobs qui interroge, **à chaque connexion et à chaque requête**, la table des collections
//! publiées :
//!
//! * connexion refusée si le pair n'est membre d'aucun groupe qui possède une collection ici ;
//! * requête `Get` refusée si le hachage demandé n'est pas la racine d'une collection d'un groupe **dont le pair est
//!   membre** (les enfants d'une collection ne se servent que par leur racine : un blob isolé n'est jamais servi) ;
//! * requêtes `GetMany`, `Observe` et `Push` toujours refusées (le pair ne peut ni lister ni écrire).
//!
//! L'appartenance vient de [`GroupMembers`] (l'état du groupe, tenu par l'application) : un membre retiré perd
//! l'accès à sa prochaine requête. La table est reconstruite au démarrage depuis les étiquettes du magasin
//! (`hearth/<groupe>/<collection>`), qui servent aussi à protéger les blobs du ramasse-miettes : tant que
//! l'application n'a pas appelé [`WorldStore::release`], les données de la version restent.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use futures_util::StreamExt;
use iroh::{Endpoint, EndpointAddr};
use iroh_blobs::api::blobs::{AddPathOptions, BlobStatus, ImportMode};
use iroh_blobs::format::collection::Collection;
use iroh_blobs::hashseq::HashSeq;
use iroh_blobs::protocol::{ChunkRanges, GetRequest};
use iroh_blobs::provider::events::{AbortReason, ConnectMode, EventMask, EventSender, ObserveMode, ProviderMessage, RequestMode, ThrottleMode};
use iroh_blobs::store::fs::options::Options as FsOptions;
use iroh_blobs::store::fs::FsStore;
use iroh_blobs::store::GcConfig;
use iroh_blobs::{BlobFormat, BlobsProtocol, Hash, HashAndFormat};
use rand::RngCore;
use thiserror::Error;
use tokio::task::JoinHandle;

use super::model::GroupId;
use super::proto::HEARTH_BLOBS_ALPN;
use super::version::BlobHash;
use crate::presence::PeerKey;
use crate::util::lock;

/// Nombre maximal de fichiers d'un monde.
pub const MAX_WORLD_FILES: usize = 100_000;
/// Profondeur maximale de dossiers d'un monde (un monde Minecraft en a moins de 8).
pub const MAX_WORLD_DEPTH: usize = 32;
/// Longueur maximale d'un chemin relatif (octets).
const MAX_PATH_BYTES: usize = 512;
/// Taille maximale du blob de noms d'une collection.
const MAX_META_BYTES: u64 = 32 * 1024 * 1024;
/// Marge de progression au-delà de la taille annoncée (enveloppes des arbres de hachage).
const PROGRESS_SLACK: u64 = 1024 * 1024;
/// Taille totale maximale rapatriée par défaut (64 Gio).
pub const DEFAULT_MAX_TOTAL: u64 = 64 * 1024 * 1024 * 1024;
/// Une version reçue ne peut faire plus de 3 fois la version précédente plus [`GROWTH_SLACK`] sans confirmation.
pub const GROWTH_FACTOR: u64 = 3;
/// Marge de croissance, en plus du triple de la version précédente (2 Gio).
pub const GROWTH_SLACK: u64 = 2 * 1024 * 1024 * 1024;
/// Espace libre à laisser sur le disque après un rapatriement (5 Gio).
pub const FREE_RESERVE: u64 = 5 * 1024 * 1024 * 1024;
/// Fichiers importés en parallèle.
const IMPORT_PARALLELISM: usize = 4;
/// Un fichier modifié depuis moins de 2 s peut l'être encore dans la même « tique » de l'horloge du système de
/// fichiers : il n'est pas mémorisé et sera relu à la prochaine publication.
const RACY_WINDOW: Duration = Duration::from_secs(2);
/// Délai de connexion à un pair de blobs.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Fréquence du ramasse-miettes du magasin.
const GC_INTERVAL: Duration = Duration::from_secs(10 * 60);
const TAG_PREFIX: &str = "hearth/";

/// Erreurs de synchronisation des mondes (messages affichables).
#[derive(Debug, Error)]
pub enum SyncError {
    #[error("erreur disque ({context}) : {message}")]
    Io { context: String, message: String },
    #[error("magasin de blobs : {0}")]
    Store(String),
    #[error("transfert impossible : {0}")]
    Net(String),
    #[error("monde invalide : {0}")]
    Invalid(String),
    #[error("le monde reçu ne fait pas la taille annoncée ({got} octets au lieu de {expected})")]
    SizeMismatch { expected: u64, got: u64 },
    #[error("monde trop gros ({0} octets)")]
    TooLarge(u64),
    #[error("aucun membre joignable ne peut fournir cette version")]
    NoPeer,
    #[error("cette version fait {} contre {} pour la précédente : elle demande une confirmation", human_size(*.expected), human_size(*.previous))]
    Oversize { expected: u64, previous: u64 },
    #[error("pas assez de place sur le disque : il faut {} de libre et il en reste {}", human_size(*.need), human_size(*.free))]
    NoSpace { need: u64, free: u64 },
}

/// Taille lisible (« 3,5 Go »), pour les messages.
pub fn human_size(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= GIB {
        format!("{:.1} Go", b / GIB).replace('.', ",")
    } else {
        format!("{:.0} Mo", (b / MIB).max(1.0))
    }
}

fn io_err(context: impl Into<String>) -> impl FnOnce(std::io::Error) -> SyncError {
    let context = context.into();
    move |e| SyncError::Io { context, message: e.to_string() }
}

fn store_err(e: impl std::fmt::Display) -> SyncError {
    SyncError::Store(e.to_string())
}

// ---------------------------------------------------------------------------------------------------------
// Validation des chemins
// ---------------------------------------------------------------------------------------------------------

fn check_segment(seg: &str) -> Result<(), String> {
    if seg.is_empty() {
        return Err("segment vide".into());
    }
    if seg == "." || seg == ".." {
        return Err("segment '.' ou '..'".into());
    }
    if seg.chars().count() > 255 {
        return Err("segment trop long".into());
    }
    if seg.chars().any(|c| c.is_control() || "<>:\"|?*\\".contains(c)) {
        return Err("caractère interdit".into());
    }
    if seg.ends_with('.') || seg.ends_with(' ') {
        return Err("segment finissant par '.' ou espace".into());
    }
    let stem = seg.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || ((stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4 && matches!(stem.as_bytes()[3], b'1'..=b'9'));
    if reserved {
        return Err("nom réservé Windows".into());
    }
    Ok(())
}

/// Valide un chemin relatif d'un monde (séparateur `/`) : en forme NFC, ni absolu, ni `..`, ni nom réservé Windows,
/// ni caractère interdit, ni exécutable, borné en longueur et en profondeur. Même fonction à la publication
/// ([`scan_world`]) et à la réception ([`check_names`]).
pub fn check_world_path(path: &str) -> Result<(), String> {
    if !unicode_normalization::is_nfc(path) {
        return Err("chemin non normalisé (NFC)".into());
    }
    if path.is_empty() {
        return Err("chemin vide".into());
    }
    if path.len() > MAX_PATH_BYTES {
        return Err("chemin trop long".into());
    }
    if path.contains('\\') {
        return Err("contient '\\'".into());
    }
    if path.starts_with('/') {
        return Err("chemin absolu".into());
    }
    let b = path.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return Err("lettre de lecteur".into());
    }
    let segs: Vec<&str> = path.split('/').collect();
    if segs.len() > MAX_WORLD_DEPTH {
        return Err("trop de dossiers imbriqués".into());
    }
    for s in segs {
        check_segment(s)?;
    }
    if core_lib::mrpack::has_dangerous_ext(path) {
        return Err("fichier exécutable".into());
    }
    Ok(())
}

/// Fichier qui n'entre jamais dans un monde publié : `session.lock` (verrou du jeu) et `*.tmp`.
pub fn is_excluded(name: &str) -> bool {
    core_lib::hearth::world::is_excluded(name)
}

/// Clé de collision d'un chemin sur un disque insensible à la casse : minuscules du chemin NFC.
pub fn collision_key(path: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    path.nfc().collect::<String>().to_lowercase()
}

/// Détecte les collisions d'un ensemble de chemins : doublons (casse comprise) et fichier/dossier de même nom
/// (`a` et `a/b`).
fn check_collisions<'a>(names: impl Iterator<Item = &'a str>) -> Result<usize, String> {
    let mut files = HashSet::new();
    let mut dirs = HashSet::new();
    let mut n = 0usize;
    for name in names {
        let key = collision_key(name);
        if !files.insert(key.clone()) {
            return Err(format!("doublon : {name:?}"));
        }
        let mut prefix = String::new();
        let parts: Vec<&str> = key.split('/').collect();
        for seg in &parts[..parts.len() - 1] {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(seg);
            dirs.insert(prefix.clone());
        }
        n += 1;
    }
    if let Some(clash) = files.iter().find(|f| dirs.contains(*f)) {
        return Err(format!("{clash:?} est à la fois un fichier et un dossier"));
    }
    Ok(n)
}

/// Valide les noms d'une collection : chacun sûr, aucune collision (casse, forme Unicode, fichier/dossier).
fn check_names<'a>(names: impl Iterator<Item = &'a str>) -> Result<usize, SyncError> {
    let names: Vec<&str> = names.collect();
    for name in &names {
        check_world_path(name).map_err(|why| SyncError::Invalid(format!("chemin {name:?} refusé : {why}")))?;
    }
    check_collisions(names.into_iter()).map_err(SyncError::Invalid)
}

// ---------------------------------------------------------------------------------------------------------
// Contrôle d'accès
// ---------------------------------------------------------------------------------------------------------

/// Appartenance aux groupes, fournie par l'application (état des groupes en mémoire).
pub trait GroupMembers: Send + Sync + 'static {
    fn is_member(&self, group: &GroupId, key: &PeerKey) -> bool;
}

/// Collections servies : racine → groupes qui la possèdent.
#[derive(Default)]
struct Index {
    roots: Mutex<HashMap<Hash, BTreeSet<GroupId>>>,
}

impl Index {
    fn register(&self, group: GroupId, root: Hash) {
        lock(&self.roots).entry(root).or_default().insert(group);
    }

    fn unregister(&self, group: &GroupId, root: &Hash) {
        let mut roots = lock(&self.roots);
        if let Some(set) = roots.get_mut(root) {
            set.remove(group);
            if set.is_empty() {
                roots.remove(root);
            }
        }
    }

    fn groups(&self) -> BTreeSet<GroupId> {
        lock(&self.roots).values().flatten().copied().collect()
    }

    fn contains(&self, group: &GroupId, root: &Hash) -> bool {
        lock(&self.roots).get(root).is_some_and(|s| s.contains(group))
    }
}

/// Tâche qui répond aux événements d'iroh-blobs (connexion, requête) selon la table et l'appartenance.
async fn gate_loop(
    mut rx: tokio::sync::mpsc::Receiver<ProviderMessage>,
    index: Arc<Index>,
    members: Arc<dyn GroupMembers>,
) {
    let mut peers: HashMap<u64, PeerKey> = HashMap::new();
    let deny = Err(AbortReason::Permission);
    while let Some(msg) = rx.recv().await {
        match msg {
            ProviderMessage::ClientConnected(m) => {
                let ok = m.inner.endpoint_id.map(|id| *id.as_bytes()).filter(|key| index.groups().iter().any(|g| members.is_member(g, key)));
                let res = match ok {
                    Some(key) => {
                        peers.insert(m.inner.connection_id, key);
                        Ok(())
                    }
                    None => deny,
                };
                m.tx.send(res).await.ok();
            }
            ProviderMessage::ConnectionClosed(m) => {
                peers.remove(&m.inner.connection_id);
            }
            ProviderMessage::GetRequestReceived(m) => {
                let hash = m.inner.request.hash;
                let allowed = peers
                    .get(&m.inner.connection_id)
                    .is_some_and(|key| index.groups().iter().any(|g| index.contains(g, &hash) && members.is_member(g, key)));
                m.tx.send(if allowed { Ok(()) } else { deny }).await.ok();
            }
            ProviderMessage::GetManyRequestReceived(m) => {
                m.tx.send(deny).await.ok();
            }
            ProviderMessage::PushRequestReceived(m) => {
                m.tx.send(deny).await.ok();
            }
            ProviderMessage::ObserveRequestReceived(m) => {
                m.tx.send(deny).await.ok();
            }
            ProviderMessage::Throttle(m) => {
                m.tx.send(Ok(())).await.ok();
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------------------------------------
// Magasin des mondes
// ---------------------------------------------------------------------------------------------------------

/// Ce qu'on retient d'un fichier déjà importé.
#[derive(Clone)]
struct Stamp {
    size: u64,
    mtime: SystemTime,
    hash: Hash,
}

struct Inner {
    store: FsStore,
    index: Arc<Index>,
    stamps: Mutex<HashMap<PathBuf, Stamp>>,
    gate: Mutex<Option<JoinHandle<()>>>,
    events: EventSender,
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(h) = lock(&self.gate).take() {
            h.abort();
        }
    }
}

/// Fichier importé (ou réutilisé) : l'étiquette temporaire le protège du ramasse-miettes jusqu'à la collection.
struct ImportedFile {
    rel: String,
    hash: Hash,
    size: u64,
    fresh: bool,
    tag: iroh_blobs::api::TempTag,
}

/// Résultat de [`WorldStore::publish_dir`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedWorld {
    /// Racine de la collection (à mettre dans `VersionHeader::collection`).
    pub collection: BlobHash,
    /// Somme des tailles des fichiers (à mettre dans `VersionHeader::total_size`).
    pub total_size: u64,
    /// Nombre de fichiers de la collection.
    pub files: usize,
    /// Fichiers réellement relus et importés (les autres étaient inchangés).
    pub imported: usize,
    /// Fichiers ignorés parce qu'exécutables (jamais publiés).
    pub skipped_unsafe: Vec<String>,
}

/// Résultat de [`WorldStore::fetch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchReport {
    pub files: usize,
    /// Fichiers qu'il manquait avant le rapatriement.
    pub missing_files: usize,
    /// Octets reçus du réseau (liste des hachages, noms et fichiers manquants).
    pub downloaded_bytes: u64,
    pub total_size: u64,
}

/// Options de [`WorldStore::fetch`].
#[derive(Clone)]
pub struct FetchOptions {
    /// Taille totale maximale acceptée (en plus de la taille signée de la version).
    pub max_total: u64,
    /// Appelé avec le nombre d'octets reçus depuis le début (monotone, au plus quelques fois par seconde).
    pub on_progress: Option<Arc<dyn Fn(u64) + Send + Sync>>,
    /// Taille de la version précédente (le parent de celle qu'on rapatrie), si on la connaît : au-delà de
    /// `GROWTH_FACTOR × précédente + GROWTH_SLACK`, la version est refusée tant que `confirmed` est faux.
    pub previous_total: Option<u64>,
    /// Une personne a confirmé le téléchargement malgré la taille (jamais pour l'espace disque).
    pub confirmed: bool,
    /// Espace libre mesuré sur le disque du magasin, s'il l'est : il doit rester `FREE_RESERVE` après le rapatriement.
    pub free_bytes: Option<u64>,
}

impl Default for FetchOptions {
    fn default() -> Self {
        Self { max_total: DEFAULT_MAX_TOTAL, on_progress: None, previous_total: None, confirmed: false, free_bytes: None }
    }
}

/// Garde-fous d'une version annoncée par un pair, avant tout octet reçu : croissance démesurée (confirmation
/// nécessaire) et espace disque (jamais contournable). Une grande `total_size` ne coûte à l'émetteur que sa signature.
pub fn check_incoming(expected_total: u64, opts: &FetchOptions) -> Result<(), SyncError> {
    if expected_total > opts.max_total {
        return Err(SyncError::TooLarge(expected_total));
    }
    if let Some(free) = opts.free_bytes {
        let need = expected_total.saturating_add(FREE_RESERVE);
        if free < need {
            return Err(SyncError::NoSpace { need, free });
        }
    }
    if let Some(previous) = opts.previous_total {
        let allowed = previous.saturating_mul(GROWTH_FACTOR).saturating_add(GROWTH_SLACK);
        if !opts.confirmed && expected_total > allowed {
            return Err(SyncError::Oversize { expected: expected_total, previous });
        }
    }
    Ok(())
}

/// Magasin de blobs des mondes Hearth : publie, rapatrie, matérialise, et sert aux membres seulement.
#[derive(Clone)]
pub struct WorldStore {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for WorldStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WorldStore")
    }
}

fn tag_name(group: &GroupId, collection: &Hash) -> String {
    format!("{TAG_PREFIX}{}/{}", group.to_hex(), collection.to_hex())
}

fn parse_tag(name: &[u8]) -> Option<(GroupId, Hash)> {
    let s = std::str::from_utf8(name).ok()?.strip_prefix(TAG_PREFIX)?;
    let (g, h) = s.split_once('/')?;
    let hash: Hash = h.parse().ok()?;
    Some((GroupId::from_hex(g)?, hash))
}

impl WorldStore {
    /// Ouvre (ou crée) le magasin dans `dir` (par exemple `<données>/hearth/blobs`). Doit être appelé depuis un
    /// runtime tokio : la tâche de contrôle d'accès y est lancée.
    pub async fn open(dir: impl AsRef<Path>, members: Arc<dyn GroupMembers>) -> Result<Self, SyncError> {
        let root = dir.as_ref();
        std::fs::create_dir_all(root).map_err(io_err(format!("création de {}", root.display())))?;
        let mut opts = FsOptions::new(root);
        // Ramasse-miettes : seules les données sans étiquette ni étiquette temporaire (imports et rapatriements
        // abandonnés) sont supprimées.
        opts.gc = Some(GcConfig { interval: GC_INTERVAL, add_protected: None });
        let store = FsStore::load_with_opts(root.join("blobs.db"), opts).await.map_err(store_err)?;

        let index = Arc::new(Index::default());
        let mut tags = store.tags().list_prefix(TAG_PREFIX).await.map_err(store_err)?;
        while let Some(t) = tags.next().await {
            let t = t.map_err(store_err)?;
            if let (Some((group, hash)), BlobFormat::HashSeq) = (parse_tag(&t.name.0), t.format) {
                if hash == t.hash {
                    index.register(group, hash);
                }
            }
        }
        drop(tags);

        // Connexion, requête `Get` : interceptées. `GetMany`, `Push` : désactivées. `Observe` : interceptée (refusée).
        let mask = EventMask {
            connected: ConnectMode::Intercept,
            get: RequestMode::Intercept,
            get_many: RequestMode::Disabled,
            push: RequestMode::Disabled,
            observe: ObserveMode::Intercept,
            throttle: ThrottleMode::None,
        };
        let (events, rx) = EventSender::channel(64, mask);
        let gate = tokio::spawn(gate_loop(rx, index.clone(), members));
        Ok(Self {
            inner: Arc::new(Inner { store, index, stamps: Mutex::new(HashMap::new()), gate: Mutex::new(Some(gate)), events }),
        })
    }

    /// Handler iroh-blobs à enregistrer dans le `Router` sous [`HEARTH_BLOBS_ALPN`] (jamais sous
    /// `iroh_blobs::ALPN`, réservé au transfert des mondes solo). L'endpoint doit déclarer cet ALPN.
    pub fn blobs_handler(&self) -> BlobsProtocol {
        BlobsProtocol::new(&self.inner.store, Some(self.inner.events.clone()))
    }

    fn store(&self) -> &iroh_blobs::api::Store {
        &self.inner.store
    }

    /// Arrête le magasin (à appeler avant de supprimer son dossier).
    pub async fn shutdown(&self) -> Result<(), SyncError> {
        self.store().shutdown().await.map_err(store_err)
    }

    // --- publication -----------------------------------------------------------------------------------

    /// Publie le contenu de `dir` comme collection du groupe `group`. L'appelant a arrêté l'écriture du monde
    /// (`save-off` + `save-all flush`, ou jeu arrêté). La collection est étiquetée (protégée du ramasse-miettes) et
    /// servie aux membres du groupe.
    pub async fn publish_dir(&self, group: &GroupId, dir: &Path) -> Result<PublishedWorld, SyncError> {
        let scan_dir = dir.to_path_buf();
        let scanned = tokio::task::spawn_blocking(move || scan_world(&scan_dir))
            .await
            .map_err(|e| SyncError::Invalid(format!("parcours interrompu : {e}")))??;
        if scanned.files.is_empty() {
            return Err(SyncError::Invalid("le dossier du monde est vide".into()));
        }
        let started = SystemTime::now();

        // Import (en parallèle borné) ; les étiquettes temporaires gardent les blobs jusqu'à l'étiquette de la
        // collection.
        let results: Vec<Result<ImportedFile, SyncError>> =
            futures_util::stream::iter(scanned.files.into_iter().map(|f| {
                let this = self.clone();
                async move { this.import_file(f, started).await }
            }))
            .buffer_unordered(IMPORT_PARALLELISM)
            .collect()
            .await;
        let mut entries: Vec<(String, Hash, u64)> = Vec::new();
        let mut keep = Vec::new();
        let mut imported = 0usize;
        for r in results {
            let f = r?;
            imported += usize::from(f.fresh);
            entries.push((f.rel, f.hash, f.size));
            keep.push(f.tag);
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        let total_size: u64 = entries.iter().map(|e| e.2).sum();
        let files = entries.len();
        let collection: Collection = entries.into_iter().map(|(n, h, _)| (n, h)).collect();
        let root_tag = collection.store(self.store()).await.map_err(store_err)?;
        let root = root_tag.hash();
        self.store()
            .tags()
            .set(tag_name(group, &root), HashAndFormat::hash_seq(root))
            .await
            .map_err(store_err)?;
        self.inner.index.register(*group, root);
        drop(keep);
        drop(root_tag);
        Ok(PublishedWorld { collection: *root.as_bytes(), total_size, files, imported, skipped_unsafe: scanned.skipped_unsafe })
    }

    /// Importe un fichier (ou réutilise le blob déjà importé d'un fichier inchangé).
    async fn import_file(
        &self,
        f: ScannedFile,
        started: SystemTime,
    ) -> Result<ImportedFile, SyncError> {
        let cached = lock(&self.inner.stamps).get(&f.path).cloned();
        if let Some(s) = cached.filter(|s| s.size == f.size && s.mtime == f.mtime) {
            if let Ok(BlobStatus::Complete { size }) = self.store().blobs().status(s.hash).await {
                if size == s.size {
                    let tag = self.store().tags().temp_tag(HashAndFormat::raw(s.hash)).await.map_err(store_err)?;
                    return Ok(ImportedFile { rel: f.rel, hash: s.hash, size, fresh: false, tag });
                }
            }
        }
        let tag = self
            .store()
            .blobs()
            .add_path_with_opts(AddPathOptions { path: f.path.clone(), format: BlobFormat::Raw, mode: ImportMode::Copy })
            .temp_tag()
            .await
            .map_err(|e| SyncError::Store(format!("import de {} : {e}", f.rel)))?;
        let hash = tag.hash();
        // La taille est celle du blob importé (le fichier a pu changer depuis le parcours).
        let size = match self.store().blobs().status(hash).await.map_err(store_err)? {
            BlobStatus::Complete { size } => size,
            _ => return Err(SyncError::Store(format!("import de {} incomplet", f.rel))),
        };
        // Un fichier modifié à l'instant même peut encore changer sans que sa date bouge : pas de mémorisation.
        let calm = f.mtime.checked_add(RACY_WINDOW).is_some_and(|t| t < started);
        if calm && size == f.size {
            lock(&self.inner.stamps).insert(f.path, Stamp { size, mtime: f.mtime, hash });
        }
        Ok(ImportedFile { rel: f.rel, hash, size, fresh: true, tag })
    }

    /// Cesse de protéger et de servir une collection (version écartée par le ramasse-miettes des versions). Les
    /// blobs qu'aucune autre collection n'utilise sont supprimés au prochain passage du ramasse-miettes.
    pub async fn release(&self, group: &GroupId, collection: &BlobHash) -> Result<(), SyncError> {
        let root = Hash::from_bytes(*collection);
        self.inner.index.unregister(group, &root);
        self.store().tags().delete(tag_name(group, &root)).await.map_err(store_err)?;
        Ok(())
    }

    /// Cette collection est-elle présente en entier (liste, noms et fichiers) ?
    pub async fn is_complete(&self, collection: &BlobHash) -> bool {
        let root = Hash::from_bytes(*collection);
        match self.store().remote().local(HashAndFormat::hash_seq(root)).await {
            Ok(info) => info.is_complete(),
            Err(_) => false,
        }
    }

    /// Étiquette et sert une collection déjà complète pour `group` (cas d'une version qu'un autre groupe ou une
    /// restauration réutilise).
    pub async fn adopt(&self, group: &GroupId, collection: &BlobHash) -> Result<(), SyncError> {
        if !self.is_complete(collection).await {
            return Err(SyncError::Invalid("collection incomplète".into()));
        }
        let root = Hash::from_bytes(*collection);
        self.store().tags().set(tag_name(group, &root), HashAndFormat::hash_seq(root)).await.map_err(store_err)?;
        self.inner.index.register(*group, root);
        Ok(())
    }

    // --- rapatriement ----------------------------------------------------------------------------------

    /// Rapatrie la collection `collection` du groupe `group` depuis `peers` (essayés dans l'ordre ; le travail
    /// déjà reçu est conservé d'un pair à l'autre). `expected_total` est la taille signée dans l'en-tête de la
    /// version : le monde reçu doit la faire exactement, et rien de plus n'est accepté du réseau.
    pub async fn fetch(
        &self,
        endpoint: &Endpoint,
        group: &GroupId,
        peers: &[EndpointAddr],
        collection: &BlobHash,
        expected_total: u64,
        opts: &FetchOptions,
    ) -> Result<FetchReport, SyncError> {
        check_incoming(expected_total, opts)?;
        let root = Hash::from_bytes(*collection);
        // Protège ce qui arrive du ramasse-miettes pendant le travail.
        let _pin = self.store().tags().temp_tag(HashAndFormat::hash_seq(root)).await.map_err(store_err)?;
        let mut last_err = SyncError::NoPeer;
        let mut downloaded_total = 0u64;
        for addr in peers {
            let before = downloaded_total;
            match self.fetch_from(endpoint, addr, root, expected_total, opts, &mut downloaded_total).await {
                Ok(mut report) => {
                    report.downloaded_bytes = downloaded_total;
                    self.store()
                        .tags()
                        .set(tag_name(group, &root), HashAndFormat::hash_seq(root))
                        .await
                        .map_err(store_err)?;
                    self.inner.index.register(*group, root);
                    return Ok(report);
                }
                // Un monde qui ment (noms, taille) est refusé net : inutile d'essayer un autre pair avec le même
                // en-tête signé.
                Err(e @ (SyncError::Invalid(_) | SyncError::SizeMismatch { .. } | SyncError::TooLarge(_) | SyncError::Oversize { .. } | SyncError::NoSpace { .. })) => return Err(e),
                Err(e) => {
                    tracing::debug!(pair = %addr.id, recu = downloaded_total - before, "rapatriement échoué : {e}");
                    last_err = e;
                }
            }
        }
        Err(last_err)
    }

    async fn fetch_from(
        &self,
        endpoint: &Endpoint,
        addr: &EndpointAddr,
        root: Hash,
        expected_total: u64,
        opts: &FetchOptions,
        downloaded: &mut u64,
    ) -> Result<FetchReport, SyncError> {
        let conn = tokio::time::timeout(CONNECT_TIMEOUT, endpoint.connect(addr.clone(), HEARTH_BLOBS_ALPN))
            .await
            .map_err(|_| SyncError::Net("pair injoignable".into()))?
            .map_err(|e| SyncError::Net(format!("pair injoignable : {e}")))?;
        let result = self.fetch_on(&conn, root, expected_total, opts, downloaded).await;
        conn.close(0u32.into(), b"");
        result
    }

    /// Exécute une requête en ne demandant que ce qui manque ; coupe dès que `limit` octets sont dépassés.
    async fn run_request(
        &self,
        conn: &iroh::endpoint::Connection,
        request: GetRequest,
        limit: u64,
        base: u64,
        opts: &FetchOptions,
        downloaded: &mut u64,
    ) -> Result<(), SyncError> {
        let local = self.store().remote().local_for_request(request).await.map_err(store_err)?;
        if local.is_complete() {
            return Ok(());
        }
        let mut stream = self.store().remote().execute_get(conn.clone(), local.missing()).stream();
        let mut last = 0u64;
        let mut reported_at = std::time::Instant::now();
        while let Some(item) = stream.next().await {
            match item {
                iroh_blobs::api::remote::GetProgressItem::Progress(n) => {
                    if n > limit {
                        return Err(SyncError::Invalid(format!("le pair envoie plus que prévu ({n} > {limit} octets)")));
                    }
                    *downloaded += n.saturating_sub(last);
                    last = n;
                    if let Some(cb) = &opts.on_progress {
                        if reported_at.elapsed() >= Duration::from_millis(200) {
                            reported_at = std::time::Instant::now();
                            cb(base + last);
                        }
                    }
                }
                iroh_blobs::api::remote::GetProgressItem::Done(_) => {
                    if let Some(cb) = &opts.on_progress {
                        cb(base + last);
                    }
                    return Ok(());
                }
                iroh_blobs::api::remote::GetProgressItem::Error(e) => return Err(SyncError::Net(format!("{e}"))),
            }
        }
        Err(SyncError::Net("transfert interrompu".into()))
    }

    async fn fetch_on(
        &self,
        conn: &iroh::endpoint::Connection,
        root: Hash,
        expected_total: u64,
        opts: &FetchOptions,
        downloaded: &mut u64,
    ) -> Result<FetchReport, SyncError> {
        let start = *downloaded;
        let limit_list = (MAX_WORLD_FILES as u64 + 1) * 32 + PROGRESS_SLACK;
        // 1) la liste des hachages (blob racine seul).
        self.run_request(conn, GetRequest::blob(root), limit_list, 0, opts, downloaded).await?;
        let list = self.store().blobs().get_bytes(root).await.map_err(store_err)?;
        let seq = HashSeq::new(list).ok_or_else(|| SyncError::Invalid("liste de hachages mal formée".into()))?;
        if seq.is_empty() || seq.len() > MAX_WORLD_FILES + 1 {
            return Err(SyncError::Invalid(format!("nombre de fichiers invalide ({})", seq.len().saturating_sub(1))));
        }
        // 2) les noms (premier enfant), puis validation avant de rapatrier le moindre fichier.
        let meta_req = GetRequest::builder().root(ChunkRanges::all()).child(0, ChunkRanges::all()).build(root);
        self.run_request(conn, meta_req, limit_list + MAX_META_BYTES, 0, opts, downloaded).await?;
        let coll = Collection::load(root, self.store()).await.map_err(|e| SyncError::Invalid(format!("collection illisible : {e}")))?;
        let files = check_names(coll.iter().map(|(n, _)| n.as_str()))?;
        // Ce qu'il manquait avant le gros du transfert (mesuré sur les fichiers).
        let mut missing_files = 0usize;
        for (_, h) in coll.iter() {
            if !matches!(self.store().blobs().status(*h).await, Ok(BlobStatus::Complete { .. })) {
                missing_files += 1;
            }
        }
        // 3) les fichiers manquants, plafonnés par la taille signée de la version.
        let limit_files = expected_total.saturating_add(limit_list + MAX_META_BYTES + PROGRESS_SLACK);
        let base = *downloaded - start;
        self.run_request(conn, GetRequest::all(root), limit_files, base, opts, downloaded).await?;
        // Contrôle final : tout est là, et la taille est celle qui a été signée.
        let mut got = 0u64;
        for (name, h) in coll.iter() {
            match self.store().blobs().status(*h).await.map_err(store_err)? {
                BlobStatus::Complete { size } => got += size,
                _ => return Err(SyncError::Net(format!("fichier incomplet : {name}"))),
            }
        }
        if got != expected_total {
            return Err(SyncError::SizeMismatch { expected: expected_total, got });
        }
        Ok(FetchReport { files, missing_files, downloaded_bytes: *downloaded - start, total_size: got })
    }

    // --- matérialisation -------------------------------------------------------------------------------

    /// Écrit la collection dans `dest`, dossier **géré par Hearth** : il est remplacé en entier, de façon
    /// atomique (le monde est écrit dans un dossier voisin, puis échangé ; en cas d'échec l'ancien reste). Les
    /// fichiers sont des **copies** : jamais de lien dur vers le magasin, le jeu écrit dans les siens.
    pub async fn materialize(&self, collection: &BlobHash, dest: &Path) -> Result<usize, SyncError> {
        let root = Hash::from_bytes(*collection);
        let coll = Collection::load(root, self.store()).await.map_err(|e| SyncError::Invalid(format!("collection illisible : {e}")))?;
        let files = check_names(coll.iter().map(|(n, _)| n.as_str()))?;
        let parent = dest.parent().ok_or_else(|| SyncError::Invalid("dossier de destination sans parent".into()))?;
        std::fs::create_dir_all(parent).map_err(io_err(format!("création de {}", parent.display())))?;
        let base = dest.file_name().and_then(|n| n.to_str()).ok_or_else(|| SyncError::Invalid("nom de destination invalide".into()))?.to_string();
        cleanup_stale(parent, &base);
        let nonce = format!("{:016x}", rand::rngs::OsRng.next_u64());
        let tmp = parent.join(format!("{base}.hearth-tmp-{nonce}"));
        let old = parent.join(format!("{base}.hearth-old-{nonce}"));
        std::fs::create_dir_all(&tmp).map_err(io_err(format!("création de {}", tmp.display())))?;
        let written = async {
            for (name, hash) in coll.iter() {
                let target = tmp.join(name);
                if let Some(dir) = target.parent() {
                    std::fs::create_dir_all(dir).map_err(io_err(format!("création de {}", dir.display())))?;
                }
                self.store().blobs().export(*hash, &target).await.map_err(|e| SyncError::Store(format!("export de {name} : {e}")))?;
            }
            Ok::<(), SyncError>(())
        }
        .await;
        if let Err(e) = written {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(e);
        }
        // Échange : l'ancien dossier est mis de côté, le neuf prend sa place ; sur échec on remet l'ancien.
        let had_old = dest.exists();
        if had_old {
            if let Err(e) = std::fs::rename(dest, &old) {
                let _ = std::fs::remove_dir_all(&tmp);
                return Err(io_err(format!("remplacement de {}", dest.display()))(e));
            }
        }
        if let Err(e) = std::fs::rename(&tmp, dest) {
            if had_old {
                let _ = std::fs::rename(&old, dest);
            }
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(io_err(format!("remplacement de {}", dest.display()))(e));
        }
        if had_old {
            let _ = std::fs::remove_dir_all(&old);
        }
        Ok(files)
    }
}

/// Supprime les dossiers temporaires qu'un arrêt brutal a laissés à côté de `base`.
fn cleanup_stale(parent: &Path, base: &str) {
    let (tmp, old) = (format!("{base}.hearth-tmp-"), format!("{base}.hearth-old-"));
    let Ok(rd) = std::fs::read_dir(parent) else { return };
    for e in rd.filter_map(Result::ok) {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with(&tmp) || name.starts_with(&old) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

// ---------------------------------------------------------------------------------------------------------
// Parcours du dossier
// ---------------------------------------------------------------------------------------------------------

struct ScannedFile {
    rel: String,
    path: PathBuf,
    size: u64,
    mtime: SystemTime,
}

struct Scanned {
    files: Vec<ScannedFile>,
    skipped_unsafe: Vec<String>,
}

/// Liste les fichiers du monde : jamais de lien symbolique, exclusions, chemins validés, bornes.
/// Collisions de casse ou d'Unicode : garde le fichier au mtime le plus récent et signale l'autre. Un fichier qui est
/// aussi le dossier d'un autre est signalé.
fn exclude_collisions(out: &mut Scanned) {
    let mut files = std::mem::take(&mut out.files);
    files.sort_by(|a, b| b.mtime.cmp(&a.mtime).then_with(|| a.rel.cmp(&b.rel)));
    let mut seen = HashSet::new();
    let mut kept = Vec::new();
    for f in files {
        if seen.insert(collision_key(&f.rel)) {
            kept.push(f);
        } else {
            out.skipped_unsafe.push(f.rel);
        }
    }
    let mut dirs = HashSet::new();
    for f in &kept {
        let key = collision_key(&f.rel);
        let mut prefix = String::new();
        let parts: Vec<&str> = key.split('/').collect();
        for seg in &parts[..parts.len() - 1] {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(seg);
            dirs.insert(prefix.clone());
        }
    }
    for f in kept {
        if dirs.contains(&collision_key(&f.rel)) {
            out.skipped_unsafe.push(f.rel);
        } else {
            out.files.push(f);
        }
    }
}

fn scan_world(root: &Path) -> Result<Scanned, SyncError> {
    let mut out = Scanned { files: Vec::new(), skipped_unsafe: Vec::new() };
    let mut stack: Vec<(PathBuf, String, usize)> = vec![(root.to_path_buf(), String::new(), 0)];
    while let Some((dir, prefix, depth)) = stack.pop() {
        if depth >= MAX_WORLD_DEPTH {
            return Err(SyncError::Invalid("dossiers trop imbriqués".into()));
        }
        let rd = std::fs::read_dir(&dir).map_err(io_err(format!("lecture de {}", dir.display())))?;
        for entry in rd {
            let entry = entry.map_err(io_err(format!("lecture de {}", dir.display())))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let ft = entry.file_type().map_err(io_err(format!("lecture de {name}")))?;
            // Les liens symboliques ne sont jamais suivis : un lien ne doit pas faire sortir le monde de son dossier.
            if ft.is_symlink() {
                continue;
            }
            let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
            if ft.is_dir() {
                stack.push((entry.path(), rel, depth + 1));
                continue;
            }
            if !ft.is_file() || is_excluded(&name) {
                continue;
            }
            if core_lib::mrpack::has_dangerous_ext(&rel) {
                out.skipped_unsafe.push(rel);
                continue;
            }
            // Un chemin invalide (par exemple non NFC) est exclu et signalé, il ne bloque pas la publication.
            if check_world_path(&rel).is_err() {
                out.skipped_unsafe.push(rel);
                continue;
            }
            if out.files.len() >= MAX_WORLD_FILES {
                return Err(SyncError::Invalid(format!("trop de fichiers (plus de {MAX_WORLD_FILES})")));
            }
            let meta = entry.metadata().map_err(io_err(format!("lecture de {rel}")))?;
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            out.files.push(ScannedFile { rel, path: entry.path(), size: meta.len(), mtime });
        }
    }
    exclude_collisions(&mut out);
    check_collisions(out.files.iter().map(|f| f.rel.as_str())).map_err(SyncError::Invalid)?;
    out.files.sort_by(|a, b| a.rel.cmp(&b.rel));
    out.skipped_unsafe.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn une_version_annoncee_demesuree_est_refusee_avant_tout_telechargement() {
        // Une grande `total_size` ne coûte qu'une signature : 60 Gio contre 6 Gio avant (limite 3 × 6 + 2 = 20 Gio).
        let opts = FetchOptions { previous_total: Some(6 * GIB), ..FetchOptions::default() };
        assert!(check_incoming(20 * GIB, &opts).is_ok());
        assert!(matches!(check_incoming(20 * GIB + 1, &opts), Err(SyncError::Oversize { .. })));
        let msg = check_incoming(32 * GIB, &opts).unwrap_err().to_string();
        assert!(msg.contains("32,0 Go") && msg.contains("6,0 Go"), "{msg}");
        // Une confirmation lève la croissance, pas le plafond absolu.
        let confirmed = FetchOptions { confirmed: true, ..opts.clone() };
        assert!(check_incoming(60 * GIB, &confirmed).is_ok());
        assert!(matches!(check_incoming(DEFAULT_MAX_TOTAL + 1, &confirmed), Err(SyncError::TooLarge(_))));
        // Sans version précédente connue, pas de rapport à comparer.
        assert!(check_incoming(60 * GIB, &FetchOptions::default()).is_ok());
    }

    #[test]
    fn l_espace_libre_est_verifie_et_jamais_contournable() {
        let opts = FetchOptions { free_bytes: Some(12 * GIB), confirmed: true, ..FetchOptions::default() };
        assert!(check_incoming(7 * GIB, &opts).is_ok(), "7 Go + 5 Go de réserve tiennent dans 12 Go");
        let err = check_incoming(7 * GIB + 1, &opts).unwrap_err();
        assert!(matches!(err, SyncError::NoSpace { free, .. } if free == 12 * GIB), "{err}");
        assert!(err.to_string().contains("12,0 Go"));
        // Un disque inconnu ne bloque pas.
        assert!(check_incoming(50 * GIB, &FetchOptions { confirmed: true, ..FetchOptions::default() }).is_ok());
        assert_eq!(human_size(3 * GIB / 2), "1,5 Go");
        assert_eq!(human_size(10 * 1024 * 1024), "10 Mo");
    }

    #[test]
    fn world_paths_are_validated() {
        for ok in ["level.dat", "region/r.0.0.mca", "DIM-1/data/raids.dat", "playerdata/0123-abcd.dat", "datapacks/a b/pack.mcmeta"] {
            assert!(check_world_path(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "/etc/passwd",
            "../level.dat",
            "region/../../x",
            "a\\b",
            "C:/x",
            "region//x",
            "CON",
            "region/nul.txt",
            "a/COM1.dat",
            "fin.",
            "fin /x",
            "a/b?c",
            "tool.exe",
            "mods/run.bat",
            "x\u{0}y",
        ] {
            assert!(check_world_path(bad).is_err(), "{bad:?} aurait dû être refusé");
        }
        assert!(check_world_path(&"a/".repeat(40)).is_err(), "trop profond");
        assert!(check_world_path(&"a".repeat(600)).is_err(), "trop long");
    }

    #[test]
    fn names_reject_duplicates_even_by_case() {
        assert!(check_names(["a/b", "a/c"].into_iter()).is_ok());
        assert!(check_names(["a/b", "A/B"].into_iter()).is_err());
        assert!(check_names(["a/b", "a/b"].into_iter()).is_err());
    }

    #[test]
    fn nfc_obligatoire_et_collision_nfc() {
        let compose = "region/caf\u{e9}.dat";
        let decompose = "region/cafe\u{301}.dat";
        assert!(check_world_path(compose).is_ok());
        assert!(check_world_path(decompose).is_err(), "NFD refusé");
        assert_eq!(collision_key(compose), collision_key(decompose));
        assert!(check_collisions(["region/caf\u{e9}.dat", "REGION/CAF\u{c9}.dat"].into_iter()).is_err());
    }

    #[test]
    fn noms_reserves_et_caracteres_windows() {
        for bad in ["CON.txt", "a/nul", "a/Com1.x", "lpt9.dat", "a/b:c", "a/b*", "a/b<", "a/b|c", "a/\"b", "D:/x", "a/b."] {
            assert!(check_world_path(bad).is_err(), "{bad:?}");
        }
        assert!(check_world_path("a/COM0.dat").is_ok());
    }

    #[test]
    fn collision_fichier_dossier() {
        assert!(check_names(["a", "a/b"].into_iter()).is_err());
        assert!(check_names(["a/b", "a"].into_iter()).is_err());
        assert!(check_names(["A", "a/b"].into_iter()).is_err());
        assert!(check_names(["a/b", "a/c", "ab"].into_iter()).is_ok());
    }

    #[test]
    fn exclusions() {
        assert!(is_excluded("session.lock"));
        assert!(is_excluded("SESSION.LOCK"));
        assert!(is_excluded("level.dat.TMP"));
        assert!(!is_excluded("level.dat"));
        assert!(!is_excluded("tmp.dat"));
    }

    #[test]
    fn scan_skips_locks_links_and_executables() {
        let d = tempfile::tempdir().unwrap();
        let w = d.path();
        std::fs::create_dir_all(w.join("region")).unwrap();
        std::fs::write(w.join("level.dat"), b"x").unwrap();
        std::fs::write(w.join("session.lock"), b"x").unwrap();
        std::fs::write(w.join("region/r.0.0.mca"), b"y").unwrap();
        std::fs::write(w.join("region/r.0.0.mca.tmp"), b"y").unwrap();
        std::fs::write(w.join("evil.exe"), b"y").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc", w.join("lien")).unwrap();
            std::os::unix::fs::symlink("/etc/hostname", w.join("lien.dat")).unwrap();
        }
        let s = scan_world(w).unwrap();
        let names: Vec<&str> = s.files.iter().map(|f| f.rel.as_str()).collect();
        assert_eq!(names, vec!["level.dat", "region/r.0.0.mca"]);
        assert_eq!(s.skipped_unsafe, vec!["evil.exe".to_string()]);
    }

    #[test]
    fn scan_exclut_les_chemins_invalides_et_les_collisions_sans_echouer() {
        let d = tempfile::tempdir().unwrap();
        let w = d.path();
        std::fs::write(w.join("level.dat"), b"x").unwrap();
        // « é » décomposé (NFD) : non NFC.
        std::fs::write(w.join("e\u{301}.dat"), b"x").unwrap();
        std::fs::write(w.join("Data.txt"), b"x").unwrap();
        std::fs::write(w.join("data.txt"), b"x").unwrap();
        let s = scan_world(w).unwrap();
        assert!(s.files.iter().any(|f| f.rel == "level.dat"));
        assert!(s.skipped_unsafe.contains(&"e\u{301}.dat".to_string()), "{:?}", s.skipped_unsafe);
        assert_eq!(s.files.iter().filter(|f| f.rel.to_lowercase() == "data.txt").count(), 1);
        assert_eq!(s.skipped_unsafe.len(), 2);
    }

    #[test]
    fn tag_names_roundtrip() {
        let g = GroupId([3; 16]);
        let h = Hash::new(b"x");
        assert_eq!(parse_tag(tag_name(&g, &h).as_bytes()), Some((g, h)));
        assert_eq!(parse_tag(b"autre/chose"), None);
        assert_eq!(parse_tag(b"hearth/zz/yy"), None);
    }
}
