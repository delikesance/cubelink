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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Empreinte CurseForge (murmur2) des jars de `mods/`. REVENDICATION de l'hôte, vérifiée à la réception ;
    /// sert à reconnaître avant téléchargement les mods publiés sur CurseForge et absents de Modrinth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cf_fingerprint: Option<u32>,
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

/// Invitation reçue d'un ami à rejoindre son monde (file en mémoire, jamais persistée).
/// Dates en secondes depuis l'epoch Unix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

// ---------- Hearth : serveurs de groupe (docs/hearth.md, docs/contrats.md) ----------

/// État visible d'un serveur de groupe (carte de l'accueil).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HearthStateKind {
    /// Personne ne joue.
    Idle,
    /// Un autre membre héberge : on peut le rejoindre.
    Hosted,
    /// Le monde se met à jour sur ce PC (`progress` en pour cent).
    Syncing,
    /// Aucun membre joignable : le jeu hors ligne est interdit sur un serveur de groupe.
    Blocked,
    /// La partie d'un membre ne s'est pas terminée proprement : le groupe choisit.
    Unclean,
    /// Deux versions du monde existent : le groupe en choisit une.
    Fork,
    /// Lancement en cours (`step`, `step_index`).
    Launching,
    /// Ce PC héberge le serveur.
    Hosting,
    /// Le modpack du groupe n'est pas installé sur ce PC.
    PackMissing,
}

/// Un membre d'un groupe, tel que l'interface l'affiche.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HearthMemberLite {
    pub public_key: String,
    pub name: String,
    pub online: bool,
}

/// Partie mal terminée : qui, et jusqu'où on peut reprendre.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HearthUnclean {
    pub holder: HearthMemberLite,
    /// Date (secondes Unix) de son dernier point de reprise connu de nous ; `None` : aucun.
    pub checkpoint_at: Option<i64>,
}

/// Carte d'un serveur de groupe sur l'accueil (événement `hearth://state`, commande `hearth_list`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HearthCard {
    /// Identifiant du groupe (32 caractères hexadécimaux).
    pub id: String,
    pub name: String,
    pub minecraft: String,
    pub loader: String,
    pub state: HearthStateKind,
    /// Titulaire du bail (état `hosted` ou `hosting`).
    pub host: Option<HearthMemberLite>,
    /// Joueurs connectés au serveur (nous compris quand c'est notre serveur).
    pub players: u32,
    /// Avancement de la mise à jour du monde, en pour cent (état `syncing`).
    pub progress: Option<u8>,
    /// Étape du lancement (état `launching`) : libellé et rang (0 à `step_count - 1`).
    pub step: Option<String>,
    pub step_index: Option<u8>,
    pub step_count: u8,
    pub unclean: Option<HearthUnclean>,
    /// Membres en ligne autres que nous.
    pub members_online: Vec<HearthMemberLite>,
    pub members_total: u32,
    pub is_creator: bool,
    /// Le modpack du groupe est installé ici (on peut héberger).
    pub can_host: bool,
    /// Instance locale du modpack du groupe.
    pub instance_id: Option<String>,
    /// Numéro de la version du monde la plus récente connue.
    pub version: Option<u64>,
}

/// Invitation à un groupe reçue d'un ami (événement `hearth://invites`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HearthInvite {
    pub group_id: String,
    pub name: String,
    pub from: HearthMemberLite,
    pub members: u32,
    pub minecraft: String,
    pub loader: String,
    pub received_at: i64,
}

/// Monde d'une instance, proposé à la création d'un groupe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HearthWorld {
    pub name: String,
    pub size_bytes: u64,
}

/// Membre sur la page du serveur.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HearthMember {
    pub public_key: String,
    pub name: String,
    pub online: bool,
    pub is_me: bool,
    pub is_creator: bool,
    /// `ok` (a la version la plus récente), `behind` (en retard), `unknown` (pas encore sondé).
    pub sync: String,
    /// Numéro de la version la plus récente qu'il dit avoir.
    pub version: Option<u64>,
    /// Jours depuis qu'on l'a vu en ligne (`None` : jamais vu, ou en ligne).
    pub unseen_days: Option<u32>,
}

/// Version du monde (historique).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HearthVersion {
    pub id: String,
    pub number: u64,
    /// `session`, `checkpoint` ou `restore`.
    pub kind: String,
    pub author: String,
    pub author_name: String,
    pub created_at: i64,
    pub played_secs: u64,
    pub total_size: u64,
    pub is_head: bool,
    /// Branche mise de côté (époque révolue) : restaurable, jamais tête.
    pub side_branch: bool,
}

/// Décision du groupe en attente de votes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HearthProposal {
    pub id: String,
    /// `continue_without`, `add_member`, `remove_member`, `pack_update` ou `leave`.
    pub kind: String,
    pub subject: Option<HearthMemberLite>,
    pub approvals: u32,
    pub needed: u32,
    pub i_voted: bool,
    pub can_vote: bool,
}

/// Bannière « X avait la version la plus récente et n'est pas revenu depuis N jours ».
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HearthBlocker {
    pub member: HearthMemberLite,
    pub days: u32,
}

/// Une des deux versions d'une fourche.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HearthBranch {
    pub version_id: String,
    pub number: u64,
    pub author_name: String,
    pub created_at: i64,
    pub played_secs: u64,
    pub total_size: u64,
}

/// Réglages d'un groupe pour ce PC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HearthSettings {
    /// Garder le serveur ouvert quand mon jeu se ferme.
    pub keep_background: bool,
    /// Place prise par le groupe sur ce disque (octets).
    pub disk_bytes: u64,
    /// Versions en attente d'envoi chez les autres membres.
    pub outbox_pending: u32,
    /// Nom de l'instance locale du modpack.
    pub instance_name: Option<String>,
}

/// Page du serveur (commande `hearth_detail`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HearthDetail {
    pub card: HearthCard,
    pub members: Vec<HearthMember>,
    pub versions: Vec<HearthVersion>,
    pub proposals: Vec<HearthProposal>,
    pub blocker: Option<HearthBlocker>,
    pub fork: Vec<HearthBranch>,
    pub settings: HearthSettings,
}

/// Événement `hearth://handover` : l'hôte vient d'arrêter le serveur pendant que nous jouions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HearthHandover {
    pub group_id: String,
    pub name: String,
    pub from_name: String,
}

/// Événement `hearth://host_prompt` : notre jeu s'est fermé alors que d'autres joueurs sont encore sur notre serveur.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HearthHostPrompt {
    pub group_id: String,
    pub name: String,
    pub players: u32,
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
    /// Inviter automatiquement tous mes amis en ligne quand mon monde s'ouvre (voir docs/hearth.md § 2). `false` par
    /// défaut : seuls les amis invités un par un, ou dont la demande à rejoindre est acceptée, peuvent entrer.
    #[serde(default)]
    pub open_to_all_friends: bool,
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
        assert!(!s.open_to_all_friends, "anciens réglages : pas d'invitation automatique de tous les amis");
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
