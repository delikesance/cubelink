//! Types partagés entre `core`, `p2p` et `src-tauri`. Propriété : team lead.
//! Les agents ne modifient pas ce fichier : ils demandent un changement au team lead.

use serde::{Deserialize, Serialize};

/// Erreur commune. Les variantes sont volontairement larges ; chaque module ajoute du contexte via `Other`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("réseau : {0}")]
    Network(String),
    #[error("intégrité : hash attendu {expected}, obtenu {actual}")]
    Integrity { expected: String, actual: String },
    #[error("authentification : {0}")]
    Auth(String),
    #[error("introuvable : {0}")]
    NotFound(String),
    #[error("entrée invalide : {0}")]
    Invalid(String),
    #[error("annulé")]
    Cancelled,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Hash SHA-256 en hexadécimal minuscule (64 caractères). C'est le hash du manifeste.
pub type Sha256Hex = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Loader {
    Vanilla,
    NeoForge,
    Fabric,
    Quilt,
    Forge,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoaderSpec {
    pub loader: Loader,
    /// Version exacte du loader (ex. "21.1.77"). Vide pour Vanilla.
    pub version: String,
}

/// Profil de jeu. `Offline` sert aux tests E2E (drapeau de test, absent de l'interface publique).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Profile {
    Online { name: String, uuid: String },
    Offline { name: String, uuid: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instance {
    pub id: String,
    pub name: String,
    pub minecraft: String,
    pub loader: LoaderSpec,
    pub ram_mb: u32,
    pub mod_count: u32,
}

/// Un fichier du manifeste hôte.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestFile {
    /// Chemin relatif dans l'instance, avec `/` (ex. "mods/create.jar").
    pub path: String,
    pub size: u64,
    pub sha256: Sha256Hex,
    /// SHA-1 (40 hex minuscules), propre aux jars de `mods/`. REVENDICATION de l'hôte, vérifiée
    /// par l'invité à la réception ; sert à interroger Modrinth avant téléchargement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha1: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub name: String,
    pub minecraft: String,
    pub loader: LoaderSpec,
    pub java_major: u32,
    pub files: Vec<ManifestFile>,
}

/// Code ami (`mcl1-…`) : chaîne opaque. Le format est défini par `p2p::friend_code` (tâche N3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FriendCode(pub String);

/// Code de vérification : 4 groupes de 4 caractères hexadécimaux majuscules (ex. "3F9A · C21B · 77D0 · 5E14").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyCode(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrustLevel {
    Ask,
    Always,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Friend {
    /// Clé publique ed25519, hex.
    pub public_key: String,
    /// Pseudo Minecraft déclaré (non vérifié).
    pub name: String,
    pub trust: TrustLevel,
    pub added_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FriendRequest {
    pub public_key: String,
    pub name: String,
    pub verify_code: VerifyCode,
    pub received_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Presence {
    Online,
    Offline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransferPhase {
    Preparing,
    Receiving,
    Verifying,
    Launching,
    Paused,
    Failed,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferProgress {
    pub phase: TransferPhase,
    pub files_done: u32,
    pub files_total: u32,
    pub bytes_done: u64,
    pub bytes_total: u64,
    /// Octets par seconde, 0 si inconnu.
    pub speed_bps: u64,
    /// Vrai si le transfert passe par un relais.
    pub via_relay: bool,
    /// Message d'échec (phase `failed`), en français.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Paramètres de l'application (écran Paramètres). Forme alignée sur `src/services/types.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub language: String,
    pub default_ram_mb: u32,
    pub java_path: Option<String>,
    pub trust_default: TrustLevel,
    pub allow_relay: bool,
    pub close_on_launch: bool,
    /// Réglage du design : toujours passer par le relais.
    pub relay_only: bool,
    /// Réglage du design : envoyer des mods à mes amis.
    pub share_mods: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_roundtrip() {
        let m = Manifest {
            name: "Create: Aeronautics".into(),
            minecraft: "1.21.1".into(),
            loader: LoaderSpec { loader: Loader::NeoForge, version: "21.1.77".into() },
            java_major: 21,
            files: vec![ManifestFile { path: "mods/a.jar".into(), size: 1, sha256: "0".repeat(64), sha1: None }],
        };
        let s = serde_json::to_string(&m).unwrap();
        assert_eq!(serde_json::from_str::<Manifest>(&s).unwrap(), m);
        assert!(s.contains("\"neoforge\""));
    }
}
