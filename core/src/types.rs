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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum Loader {
    Vanilla,
    NeoForge,
    Fabric,
    Quilt,
    Forge,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct LoaderSpec {
    pub loader: Loader,
    /// Version exacte du loader (ex. "21.1.77"). Vide pour Vanilla.
    pub version: String,
}

/// Profil de jeu. `Offline` sert aux tests E2E (drapeau de test, absent de l'interface publique).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Profile {
    Online { name: String, uuid: String },
    Offline { name: String, uuid: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct Instance {
    pub id: String,
    pub name: String,
    pub minecraft: String,
    pub loader: LoaderSpec,
    pub ram_mb: u32,
    pub mod_count: u32,
    /// Java propre à l'instance (surcharge le réglage global). `None` : réglage global.
    #[serde(default)]
    pub java_path: Option<String>,
    /// Arguments JVM supplémentaires, séparés par des espaces.
    #[serde(default)]
    pub jvm_args: String,
    /// Illustrations du modpack (icône, captures, couleur). Absent des anciens `instance.json`.
    #[serde(default)]
    pub art: Option<InstanceArt>,
}

/// Illustrations d'une instance. Les fichiers sont dans `<dossier de l'instance>/art/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct InstanceArt {
    /// "curseforge", "modrinth" ou "custom".
    pub source: String,
    pub project_id: String,
    pub title: String,
    pub author: String,
    pub summary: String,
    /// "#rrggbb" en minuscules.
    #[serde(default)]
    pub color: Option<String>,
    /// Nom de fichier dans `art/`.
    #[serde(default)]
    pub icon: Option<String>,
    /// Noms de fichiers dans `art/`, la capture « featured » en premier (6 au plus).
    #[serde(default)]
    pub screenshots: Vec<String>,
    #[serde(default)]
    pub page_url: Option<String>,
}

/// Un fichier du manifeste hôte.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ManifestFile {
    /// Chemin relatif dans l'instance, avec `/` (ex. "mods/create.jar").
    pub path: String,
    pub size: u64,
    pub sha256: Sha256Hex,
    /// SHA-1 (40 hex minuscules), propre aux jars de `mods/`. REVENDICATION de l'hôte, vérifiée
    /// par l'invité à la réception ; sert à interroger Modrinth avant téléchargement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha1: Option<String>,
    /// Empreinte CurseForge (murmur2) des jars de `mods/`. REVENDICATION de l'hôte, vérifiée à la réception ;
    /// sert à reconnaître avant téléchargement les mods publiés sur CurseForge et absents de Modrinth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cf_fingerprint: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct Manifest {
    pub name: String,
    pub minecraft: String,
    pub loader: LoaderSpec,
    pub java_major: u32,
    pub files: Vec<ManifestFile>,
}

/// Code ami (`mcl1-…`) : chaîne opaque. Le format est défini par `p2p::friend_code` (tâche N3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct FriendCode(pub String);

/// Code de vérification : 4 groupes de 4 caractères hexadécimaux majuscules (ex. "3F9A · C21B · 77D0 · 5E14").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct VerifyCode(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum TrustLevel {
    Ask,
    Always,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct Friend {
    /// Clé publique ed25519, hex.
    pub public_key: String,
    /// Pseudo Minecraft déclaré (non vérifié).
    pub name: String,
    pub trust: TrustLevel,
    pub added_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct FriendRequest {
    pub public_key: String,
    pub name: String,
    pub verify_code: VerifyCode,
    pub received_at: i64,
}

/// Invitation reçue d'un ami à rejoindre son monde (file en mémoire, jamais persistée).
/// Dates en secondes depuis l'epoch Unix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct Invite {
    pub id: String,
    pub public_key: String,
    pub name: String,
    pub world_name: String,
    pub minecraft: String,
    pub received_at: i64,
    pub expires_at: i64,
    /// Modpack (instance) de l'ami, si son launcher l'envoie.
    #[serde(default)]
    pub pack: Option<String>,
    /// Icône du monde (`data:image/png;base64,…`), contrôlée à la réception.
    #[serde(default)]
    pub world_icon: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum Presence {
    Online,
    Offline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
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
    /// Découverte d'adresses publique (pkarr + DNS n0) : retrouver un ami dont l'adresse a changé.
    /// Publie la clé publique, le relais et les IP auprès de n0. Lu à la création de l'endpoint
    /// (redémarrage requis). `true` par défaut (fichiers de réglages anciens : `true`).
    #[serde(default = "default_discovery")]
    pub discovery: bool,
    /// Ouvrir automatiquement mon monde solo au réseau local (agent Java Cubelink, instances `Own`) pour que mes
    /// amis puissent le rejoindre sans manipulation. `true` par défaut (fichiers de réglages anciens : `true`).
    #[serde(default = "default_lan_auto")]
    pub lan_auto: bool,
    /// Panneau superposé au jeu (amis, invitations) ouvert par un raccourci global. `true` par défaut.
    #[serde(default = "default_overlay_enabled")]
    pub overlay_enabled: bool,
    /// Raccourci global du panneau (format du plugin global-shortcut, p. ex. `Ctrl+Shift+C`).
    #[serde(default = "default_overlay_hotkey")]
    pub overlay_hotkey: String,
    /// Jeu intégré (Windows) : le jeu s'affiche dans la fenêtre du launcher. Lu au lancement du jeu : un jeu déjà
    /// ouvert n'est pas touché. `true` par défaut (fichiers de réglages anciens : `true`).
    #[serde(default = "default_embedded_game")]
    pub embedded_game: bool,
}

pub const DEFAULT_OVERLAY_HOTKEY: &str = "Ctrl+Shift+C";

fn default_overlay_enabled() -> bool {
    true
}

fn default_overlay_hotkey() -> String {
    DEFAULT_OVERLAY_HOTKEY.to_string()
}

fn default_discovery() -> bool {
    true
}

fn default_lan_auto() -> bool {
    true
}

fn default_embedded_game() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anciens_reglages_sans_overlay_prennent_les_defauts() {
        let old = r#"{"language":"fr","default_ram_mb":4096,"java_path":null,"trust_default":"ask","allow_relay":true,"close_on_launch":false,"relay_only":false,"share_mods":true}"#;
        let s: Settings = serde_json::from_str(old).unwrap();
        assert!(s.overlay_enabled);
        assert_eq!(s.overlay_hotkey, "Ctrl+Shift+C");
        assert!(s.discovery && s.lan_auto);
        assert!(s.embedded_game);
    }

    #[test]
    fn manifest_roundtrip() {
        let m = Manifest {
            name: "Create: Aeronautics".into(),
            minecraft: "1.21.1".into(),
            loader: LoaderSpec { loader: Loader::NeoForge, version: "21.1.77".into() },
            java_major: 21,
            files: vec![ManifestFile { path: "mods/a.jar".into(), size: 1, sha256: "0".repeat(64), sha1: None, cf_fingerprint: None }],
        };
        let s = serde_json::to_string(&m).unwrap();
        assert_eq!(serde_json::from_str::<Manifest>(&s).unwrap(), m);
        assert!(s.contains("\"neoforge\""));
    }
}
