//! Comptes multiples : fichier chiffré `accounts.bin` (même format et même clé que `tokens.bin`).
//! Les comptes hors ligne n'ont pas de jetons. Aucun appel réseau.

use std::fmt;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::tokens::{decrypt_blob, encrypt_blob, write_atomic, KeyProvider, StoredTokens};
use crate::types::{Error, Profile, Result};

pub const ACCOUNTS_VERSION: u32 = 1;

/// Jetons d'un compte en ligne. `Debug` masque les secrets.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountTokens {
    pub ms_refresh_token: String,
    pub ms_expires_at: i64,
    pub minecraft_token: String,
    pub minecraft_expires_at: i64,
}

impl fmt::Debug for AccountTokens {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccountTokens")
            .field("ms_refresh_token", &"***")
            .field("ms_expires_at", &self.ms_expires_at)
            .field("minecraft_token", &"***")
            .field("minecraft_expires_at", &self.minecraft_expires_at)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountEntry {
    /// UUID du compte (unique).
    pub id: String,
    pub profile: Profile,
    #[serde(default)]
    pub tokens: Option<AccountTokens>,
}

/// UUID d'un profil.
pub fn profile_uuid(p: &Profile) -> &str {
    match p {
        Profile::Online { uuid, .. } | Profile::Offline { uuid, .. } => uuid,
    }
}

/// Pseudo d'un profil.
pub fn profile_name(p: &Profile) -> &str {
    match p {
        Profile::Online { name, .. } | Profile::Offline { name, .. } => name,
    }
}

impl AccountEntry {
    /// Compte en ligne issu de jetons (l'id est l'UUID du profil).
    pub fn from_stored(t: StoredTokens) -> Self {
        Self {
            id: profile_uuid(&t.profile).to_string(),
            tokens: Some(AccountTokens {
                ms_refresh_token: t.ms_refresh_token,
                ms_expires_at: t.ms_expires_at,
                minecraft_token: t.minecraft_token,
                minecraft_expires_at: t.minecraft_expires_at,
            }),
            profile: t.profile,
        }
    }

    /// Compte hors ligne (sans jetons).
    pub fn offline(profile: Profile) -> Self {
        Self { id: profile_uuid(&profile).to_string(), profile, tokens: None }
    }

    /// Jetons au format de renouvellement ; `None` pour un compte hors ligne.
    pub fn to_stored(&self) -> Option<StoredTokens> {
        let t = self.tokens.as_ref()?;
        Some(StoredTokens {
            ms_refresh_token: t.ms_refresh_token.clone(),
            ms_expires_at: t.ms_expires_at,
            minecraft_token: t.minecraft_token.clone(),
            minecraft_expires_at: t.minecraft_expires_at,
            profile: self.profile.clone(),
        })
    }

    pub fn info(&self) -> AccountInfo {
        AccountInfo {
            id: self.id.clone(),
            kind: match self.profile {
                Profile::Online { .. } => AccountKind::Online,
                Profile::Offline { .. } => AccountKind::Offline,
            },
            name: profile_name(&self.profile).to_string(),
            uuid: profile_uuid(&self.profile).to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountKind {
    Online,
    Offline,
}

/// Compte tel que vu par l'interface (aucun secret).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountInfo {
    pub id: String,
    pub kind: AccountKind,
    pub name: String,
    pub uuid: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AccountsState {
    pub accounts: Vec<AccountInfo>,
    pub active: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountsFile {
    pub version: u32,
    pub active: Option<String>,
    pub accounts: Vec<AccountEntry>,
}

impl Default for AccountsFile {
    fn default() -> Self {
        Self { version: ACCOUNTS_VERSION, active: None, accounts: Vec::new() }
    }
}

impl AccountsFile {
    pub fn get(&self, id: &str) -> Option<&AccountEntry> {
        self.accounts.iter().find(|a| a.id == id)
    }

    pub fn active_entry(&self) -> Option<&AccountEntry> {
        self.active.as_deref().and_then(|id| self.get(id))
    }

    pub fn active_profile(&self) -> Option<Profile> {
        self.active_entry().map(|a| a.profile.clone())
    }

    /// Ajoute le compte, ou remplace celui de même id (même position).
    pub fn upsert(&mut self, entry: AccountEntry) {
        match self.accounts.iter_mut().find(|a| a.id == entry.id) {
            Some(slot) => *slot = entry,
            None => self.accounts.push(entry),
        }
    }

    /// Retire le compte ; s'il était actif, le premier restant devient actif (sinon aucun). Faux si absent.
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.accounts.len();
        self.accounts.retain(|a| a.id != id);
        if self.accounts.len() == before {
            return false;
        }
        if self.active.as_deref() == Some(id) {
            self.active = self.accounts.first().map(|a| a.id.clone());
        }
        true
    }

    pub fn state(&self) -> AccountsState {
        AccountsState { accounts: self.accounts.iter().map(AccountEntry::info).collect(), active: self.active.clone() }
    }
}

pub struct AccountStore<K: KeyProvider> {
    path: PathBuf,
    keys: K,
}

impl<K: KeyProvider> fmt::Debug for AccountStore<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccountStore").field("path", &self.path).finish()
    }
}

impl<K: KeyProvider> AccountStore<K> {
    pub fn new(path: impl Into<PathBuf>, key_provider: K) -> Self {
        Self { path: path.into(), keys: key_provider }
    }

    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    pub fn save(&self, file: &AccountsFile) -> Result<()> {
        let plain = serde_json::to_vec(file).map_err(|_| Error::Auth("sérialisation des comptes impossible".into()))?;
        write_atomic(&self.path, &encrypt_blob(&self.keys, &plain)?)
    }

    /// `Ok(None)` si le fichier n'existe pas ; `Error::Auth` s'il est altéré, tronqué, d'une version inconnue ou chiffré avec une autre clé.
    pub fn load(&self) -> Result<Option<AccountsFile>> {
        let data = match fs::read(&self.path) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let plain = decrypt_blob(&self.keys, &data)?;
        let file: AccountsFile =
            serde_json::from_slice(&plain).map_err(|_| Error::Auth("contenu du fichier des comptes illisible".into()))?;
        if file.version > ACCOUNTS_VERSION {
            return Err(Error::Auth("fichier des comptes d'une version plus récente".into()));
        }
        Ok(Some(file))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::tokens::StaticKeyProvider;

    fn online(n: &str, u: &str) -> AccountEntry {
        AccountEntry::from_stored(StoredTokens {
            ms_refresh_token: "rt-secret".into(),
            ms_expires_at: 10,
            minecraft_token: "mc-secret".into(),
            minecraft_expires_at: 20,
            profile: Profile::Online { name: n.into(), uuid: u.into() },
        })
    }

    #[test]
    fn roundtrip_sans_clair_et_debug_masque() {
        let d = tempfile::tempdir().unwrap();
        let s = AccountStore::new(d.path().join("accounts.bin"), StaticKeyProvider([7; 32]));
        assert_eq!(s.load().unwrap(), None);
        let mut f = AccountsFile::default();
        f.upsert(online("Steve", "u-1"));
        f.upsert(AccountEntry::offline(Profile::Offline { name: "Bob".into(), uuid: "u-2".into() }));
        f.active = Some("u-1".into());
        s.save(&f).unwrap();
        assert_eq!(s.load().unwrap(), Some(f.clone()));
        let raw = fs::read(d.path().join("accounts.bin")).unwrap();
        assert!(!raw.windows(9).any(|w| w == b"mc-secret"));
        assert!(!format!("{f:?}").contains("mc-secret") && !format!("{f:?}").contains("rt-secret"));
        assert!(s.exists());
    }

    #[test]
    fn mauvaise_cle_et_version_future_refusees() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("accounts.bin");
        AccountStore::new(&p, StaticKeyProvider([1; 32])).save(&AccountsFile::default()).unwrap();
        assert!(matches!(AccountStore::new(&p, StaticKeyProvider([2; 32])).load(), Err(Error::Auth(_))));
        let s = AccountStore::new(&p, StaticKeyProvider([1; 32]));
        s.save(&AccountsFile { version: 99, ..Default::default() }).unwrap();
        assert!(matches!(s.load(), Err(Error::Auth(_))));
    }

    #[test]
    fn upsert_remove_et_etat() {
        let mut f = AccountsFile::default();
        f.upsert(online("A", "1"));
        f.upsert(online("B", "2"));
        f.upsert(online("A2", "1"));
        assert_eq!(f.accounts.len(), 2);
        assert_eq!(profile_name(&f.accounts[0].profile), "A2", "même position");
        f.active = Some("1".into());
        assert!(f.remove("1"));
        assert_eq!(f.active.as_deref(), Some("2"));
        assert!(!f.remove("zz"));
        assert!(f.remove("2"));
        assert_eq!(f.active, None);
        assert_eq!(f.state(), AccountsState::default());
    }

    #[test]
    fn serialisation_info() {
        let v = serde_json::to_value(AccountEntry::offline(Profile::Offline { name: "Bob".into(), uuid: "u".into() }).info()).unwrap();
        assert_eq!(v, serde_json::json!({"id":"u","kind":"offline","name":"Bob","uuid":"u"}));
    }
}
