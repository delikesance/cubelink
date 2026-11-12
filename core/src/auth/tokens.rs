//! Tâche A4 : stockage chiffré des jetons (AES-256-GCM, nonce aléatoire par écriture, écriture atomique).
//! Aucun appel réseau : seule la logique d'expiration est exposée (`needs_refresh`).
//! Format du fichier : `MAGIC (4) || nonce (12) || chiffré+tag`. Le magic est authentifié (AAD).

use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};

use crate::types::{Error, Profile, Result};

const MAGIC: &[u8; 4] = b"CLT1";
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;
const KEY_LEN: usize = 32;

/// Fournit la clé de chiffrement de 32 octets.
pub trait KeyProvider: Send + Sync {
    fn key(&self) -> Result<[u8; KEY_LEN]>;
}

/// Clé fixe en mémoire (tests, ou clé issue d'un coffre déjà déverrouillé).
pub struct StaticKeyProvider(pub [u8; KEY_LEN]);

impl KeyProvider for StaticKeyProvider {
    fn key(&self) -> Result<[u8; KEY_LEN]> {
        Ok(self.0)
    }
}

impl fmt::Debug for StaticKeyProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("StaticKeyProvider(***)")
    }
}

/// Clé stockée dans un fichier local, générée au premier usage.
/// Limite : un fichier-clé à côté des données ne protège pas contre un autre processus de l'utilisateur.
#[derive(Debug, Clone)]
pub struct FileKeyProvider {
    path: PathBuf,
}

impl FileKeyProvider {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    fn read_key(&self) -> Result<[u8; KEY_LEN]> {
        let bytes = fs::read(&self.path)?;
        <[u8; KEY_LEN]>::try_from(bytes.as_slice())
            .map_err(|_| Error::Auth("fichier-clé invalide (taille inattendue)".into()))
    }
}

impl KeyProvider for FileKeyProvider {
    fn key(&self) -> Result<[u8; KEY_LEN]> {
        match self.read_key() {
            Ok(k) => return Ok(k),
            Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        if let Some(dir) = self.path.parent() {
            if !dir.as_os_str().is_empty() {
                fs::create_dir_all(dir)?;
            }
        }
        let mut key = [0u8; KEY_LEN];
        OsRng.fill_bytes(&mut key);
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        match opts.open(&self.path) {
            Ok(mut f) => {
                f.write_all(&key)?;
                f.sync_all()?;
                Ok(key)
            }
            // Création concurrente : on relit la clé de l'autre.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => self.read_key(),
            Err(e) => Err(e.into()),
        }
    }
}

/// Jetons persistés. `Debug` masque les secrets.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredTokens {
    pub ms_refresh_token: String,
    /// Expiration du jeton d'accès Microsoft (secondes Unix).
    pub ms_expires_at: i64,
    pub minecraft_token: String,
    /// Expiration du jeton Minecraft (secondes Unix).
    pub minecraft_expires_at: i64,
    pub profile: Profile,
}

impl StoredTokens {
    /// Vrai si un des jetons expire dans `margin` secondes ou moins.
    pub fn needs_refresh(&self, now: i64, margin: i64) -> bool {
        let limit = now.saturating_add(margin);
        self.minecraft_expires_at <= limit || self.ms_expires_at <= limit
    }
}

impl fmt::Debug for StoredTokens {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoredTokens")
            .field("ms_refresh_token", &"***")
            .field("ms_expires_at", &self.ms_expires_at)
            .field("minecraft_token", &"***")
            .field("minecraft_expires_at", &self.minecraft_expires_at)
            .field("profile", &self.profile)
            .finish()
    }
}

pub struct TokenStore<K: KeyProvider> {
    path: PathBuf,
    keys: K,
}

impl<K: KeyProvider> fmt::Debug for TokenStore<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenStore").field("path", &self.path).finish()
    }
}

impl<K: KeyProvider> TokenStore<K> {
    pub fn new(path: impl Into<PathBuf>, key_provider: K) -> Self {
        Self { path: path.into(), keys: key_provider }
    }

    pub fn save(&self, tokens: &StoredTokens) -> Result<()> {
        let plain = serde_json::to_vec(tokens)
            .map_err(|_| Error::Auth("sérialisation des jetons impossible".into()))?;
        write_atomic(&self.path, &encrypt_blob(&self.keys, &plain)?)
    }

    /// `Ok(None)` si le fichier n'existe pas ; `Error::Auth` s'il est altéré, tronqué ou chiffré avec une autre clé.
    pub fn load(&self) -> Result<Option<StoredTokens>> {
        let data = match fs::read(&self.path) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let plain = decrypt_blob(&self.keys, &data)?;
        serde_json::from_slice(&plain)
            .map(Some)
            .map_err(|_| Error::Auth("contenu du fichier de jetons illisible".into()))
    }

    /// Supprime le fichier (sans erreur s'il n'existe pas).
    pub fn clear(&self) -> Result<()> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Vrai si rien n'est stocké ou si un jeton expire dans `margin` secondes.
    pub fn needs_refresh(&self, now: i64, margin: i64) -> Result<bool> {
        Ok(self.load()?.is_none_or(|t| t.needs_refresh(now, margin)))
    }
}

/// Chiffre `plain` au format `MAGIC || nonce || chiffré+tag` (partagé avec le stockage des comptes).
pub(crate) fn encrypt_blob<K: KeyProvider>(keys: &K, plain: &[u8]) -> Result<Vec<u8>> {
    let key = keys.key()?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));
    let mut nonce = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce);
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: plain, aad: MAGIC })
        .map_err(|_| Error::Auth("chiffrement des jetons impossible".into()))?;
    let mut out = Vec::with_capacity(MAGIC.len() + NONCE_LEN + ct.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Inverse de `encrypt_blob`.
pub(crate) fn decrypt_blob<K: KeyProvider>(keys: &K, data: &[u8]) -> Result<Vec<u8>> {
    if data.len() < MAGIC.len() + NONCE_LEN + TAG_LEN || &data[..MAGIC.len()] != MAGIC {
        return Err(Error::Auth("fichier de jetons invalide ou tronqué".into()));
    }
    let key = keys.key()?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));
    let (nonce, ct) = data[MAGIC.len()..].split_at(NONCE_LEN);
    cipher
        .decrypt(Nonce::from_slice(nonce), Payload { msg: ct, aad: MAGIC })
        .map_err(|_| Error::Auth("déchiffrement impossible : fichier altéré ou clé incorrecte".into()))
}

pub(crate) fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            fs::create_dir_all(dir)?;
        }
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let res = (|| -> std::io::Result<()> {
        let mut f = opts.open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if let Err(e) = res {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> StoredTokens {
        StoredTokens {
            ms_refresh_token: "refresh-secret".into(),
            ms_expires_at: 2_000,
            minecraft_token: "mc-secret".into(),
            minecraft_expires_at: 1_000,
            profile: Profile::Online { name: "Steve".into(), uuid: "u-1".into() },
        }
    }

    fn store(dir: &Path, key: u8) -> TokenStore<StaticKeyProvider> {
        TokenStore::new(dir.join("tokens.bin"), StaticKeyProvider([key; 32]))
    }

    #[test]
    fn roundtrip_and_no_plaintext() {
        let d = tempfile::tempdir().unwrap();
        let s = store(d.path(), 1);
        s.save(&sample()).unwrap();
        assert_eq!(s.load().unwrap(), Some(sample()));
        let raw = fs::read(d.path().join("tokens.bin")).unwrap();
        assert!(!raw.windows(9).any(|w| w == b"mc-secret"));
        assert!(!d.path().join("tokens.bin.tmp").exists());
    }

    #[test]
    fn nonce_differs_per_write() {
        let d = tempfile::tempdir().unwrap();
        let s = store(d.path(), 1);
        s.save(&sample()).unwrap();
        let a = fs::read(d.path().join("tokens.bin")).unwrap();
        s.save(&sample()).unwrap();
        let b = fs::read(d.path().join("tokens.bin")).unwrap();
        assert_ne!(a[4..16], b[4..16]);
    }

    #[test]
    fn tampered_rejected() {
        let d = tempfile::tempdir().unwrap();
        let s = store(d.path(), 1);
        s.save(&sample()).unwrap();
        let p = d.path().join("tokens.bin");
        let mut raw = fs::read(&p).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 0x01;
        fs::write(&p, &raw).unwrap();
        assert!(matches!(s.load(), Err(Error::Auth(_))));
    }

    #[test]
    fn wrong_key_rejected() {
        let d = tempfile::tempdir().unwrap();
        store(d.path(), 1).save(&sample()).unwrap();
        assert!(matches!(store(d.path(), 2).load(), Err(Error::Auth(_))));
    }

    #[test]
    fn truncated_rejected() {
        let d = tempfile::tempdir().unwrap();
        let s = store(d.path(), 1);
        s.save(&sample()).unwrap();
        let p = d.path().join("tokens.bin");
        let raw = fs::read(&p).unwrap();
        for len in [0, 3, 10, 20, raw.len() - 1] {
            fs::write(&p, &raw[..len]).unwrap();
            assert!(matches!(s.load(), Err(Error::Auth(_))), "len {len}");
        }
    }

    #[test]
    fn missing_is_none_and_clear() {
        let d = tempfile::tempdir().unwrap();
        let s = store(d.path(), 1);
        assert_eq!(s.load().unwrap(), None);
        s.clear().unwrap();
        s.save(&sample()).unwrap();
        s.clear().unwrap();
        assert_eq!(s.load().unwrap(), None);
    }

    #[test]
    fn needs_refresh_logic() {
        let t = sample(); // mc expire à 1000, ms à 2000
        assert!(!t.needs_refresh(0, 100));
        assert!(t.needs_refresh(900, 100));
        assert!(t.needs_refresh(1_500, 0));
        let d = tempfile::tempdir().unwrap();
        let s = store(d.path(), 1);
        assert!(s.needs_refresh(0, 0).unwrap());
        s.save(&t).unwrap();
        assert!(!s.needs_refresh(0, 100).unwrap());
        assert!(s.needs_refresh(950, 100).unwrap());
    }

    #[test]
    fn debug_masks_secrets() {
        let out = format!("{:?}", sample());
        assert!(!out.contains("refresh-secret") && !out.contains("mc-secret"));
    }

    #[test]
    fn file_key_provider_generates_and_is_stable() {
        let d = tempfile::tempdir().unwrap();
        let kp = FileKeyProvider::new(d.path().join("sub").join("key.bin"));
        let k1 = kp.key().unwrap();
        assert_eq!(k1, kp.key().unwrap());
        let s = TokenStore::new(d.path().join("t.bin"), kp);
        s.save(&sample()).unwrap();
        let s2 = TokenStore::new(
            d.path().join("t.bin"),
            FileKeyProvider::new(d.path().join("sub").join("key.bin")),
        );
        assert_eq!(s2.load().unwrap(), Some(sample()));
    }

    #[test]
    fn corrupt_key_file_rejected() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("key.bin");
        fs::write(&p, b"short").unwrap();
        assert!(matches!(FileKeyProvider::new(p).key(), Err(Error::Auth(_))));
    }
}
