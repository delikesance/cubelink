//! Hearth côté application : serveurs de groupe P2P (voir `docs/hearth.md`).
//!
//! `HearthRuntime` tient l'état des groupes (disque + mémoire), la table des baux, les statuts des membres, la file
//! d'envoi et le magasin de blobs des mondes. Il branche le protocole `cubelink/hearth/1` (`HearthDelegate`) et
//! offre les opérations que les commandes (`commands_hearth.rs`) enchaînent : sonde et synchro de fond, gouvernance
//! (propositions, votes), versions du monde, cycle du serveur dédié (démarrage, points de reprise, arrêt).
//!
//! Écarts assumés par rapport à la spec :
//! * une seule file d'envoi pour tous les groupes (`hearth/outbox/`) au lieu d'une par groupe ;
//! * le monde de la tête est matérialisé dans `hearth/<groupe>/server/world` (le dossier que lit le serveur), pas dans
//!   un `world/` à part : une copie de moins sur le disque ;
//! * une fourche se résout sans toucher à `p2p::hearth::version` : voir [`effective_heads`].

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use core_lib::hearth::server::{log_message, InstallRequest, LineSink, ServerConfig, ServerInstaller};
use core_lib::instance::InstanceStore;
use core_lib::types::{
    HearthBlocker, HearthBranch, HearthCard, HearthDetail, HearthInvite, HearthMember, HearthMemberLite, HearthProposal,
    HearthSettings, HearthStateKind, HearthUnclean, HearthVersion, LoaderSpec,
};
use iroh::{Endpoint, EndpointAddr};
use p2p_lib::friends_db::FriendsDb;
use p2p_lib::hearth::lease::{
    arbitrate_marked, claim_outcome, decide_from, fence_action, new_lease_id, Arbitration, Claim, ClaimConflict, ClaimOutcome, ClaimReply,
    Clock, ConflictKind, Decision, FenceAction, FenceReason, FenceVerdict, HostFence, Lease, LeaseId, LeasePhase, LeaseTable, Observed,
    OnlineMember, ProbeRound, ReleaseReason, SystemClock, CLAIM_TIMEOUT, CONFLICT_WAIT, LEASE_RENEW_INTERVAL,
};
use p2p_lib::hearth::model::{GroupId, GroupOp, GroupState, OpHash, PackPin, Proposal, Sig, SignedOp};
use p2p_lib::hearth::outbox::{MemberReplica, Outbox, Replica};
use p2p_lib::hearth::proto::{HearthConn, HearthDelegate, NetError};
use p2p_lib::hearth::store::{Group, GroupKey, GroupStore};
use p2p_lib::hearth::sync::{human_size, FetchOptions, GroupMembers, PublishedWorld, SyncError, WorldStore};
use p2p_lib::hearth::version::{SignedVersion, VersionHeader, VersionId, VersionKind, VersionPoint};
use p2p_lib::identity::Identity;
use p2p_lib::presence::PeerKey;
use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;

use crate::state::{lock, now_secs, EventSink};

/// Erreur renvoyée par `hearth_play` quand l'EULA de Minecraft n'a pas encore été acceptée (l'écran la propose).
pub const EULA_REQUIRED: &str = "eula_required";
/// Délai entre deux sondes des membres.
pub const PROBE_INTERVAL: Duration = Duration::from_secs(60);
/// Délai d'une sonde d'un membre.
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);
/// Délai de la connexion et du `Hello` d'une ronde de sondage (par membre, en parallèle).
pub const ROUND_HELLO_TIMEOUT: Duration = Duration::from_secs(4);
/// Attente du bail d'un concurrent après un `ClaimConflict` sur un claim (il annonce son bail `Starting`).
const CLAIM_YIELD_WAIT: Duration = Duration::from_secs(12);
/// Durée pendant laquelle un premier « Jouer » sur un bail « au mieux » attend la confirmation (le second clic).
const BEST_EFFORT_CONFIRM: Duration = Duration::from_secs(120);
/// Période des points de reprise pendant une partie.
pub const CHECKPOINT_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// Jours sans nouvelles avant de pouvoir continuer sans un membre.
pub const CONTINUE_AFTER_DAYS: u32 = 7;
/// Versions plus jeunes que cela sont gardées par le ramasse-miettes des versions.
const KEEP_VERSIONS_SECS: u64 = 14 * 24 * 3600;
/// Invitations de groupe en attente au plus.
const MAX_PENDING_INVITES: usize = 16;
/// Invitations acceptées d'un même ami en 24 h.
const MAX_INVITES_PER_FRIEND_PER_DAY: usize = 3;
/// Une invitation en attente expire après 7 jours.
const INVITE_TTL_SECS: i64 = 7 * 24 * 3600;
/// Message des commandes refusées quand l'interrupteur est coupé.
pub const HEARTH_DISABLED: &str = "Les serveurs de groupe sont désactivés : active « Serveurs de groupe (expérimental) » dans les Réglages.";
/// Délai laissé au serveur d'un groupe oublié pour s'arrêter avant d'être tué.
const FORGET_STOP_GRACE: Duration = Duration::from_secs(20);
/// Délai laissé au serveur pour s'arrêter proprement (sauvegarde du monde).
const SERVER_STOP_GRACE: Duration = Duration::from_secs(90);
/// Attente maximale du démarrage du serveur d'un autre membre (bail `Starting`).
const START_WAIT_MAX: Duration = Duration::from_secs(15 * 60);
/// Pas d'observation de ce démarrage.
const START_WAIT_POLL: Duration = Duration::from_secs(2);

pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Lance une tâche sur le runtime courant, à défaut sur celui de Tauri.
pub(crate) fn spawn<F: Future<Output = ()> + Send + 'static>(f: F) -> JoinHandle<()> {
    match tokio::runtime::Handle::try_current() {
        Ok(h) => h.spawn(f),
        Err(_) => {
            let tauri::async_runtime::RuntimeHandle::Tokio(h) = tauri::async_runtime::handle();
            h.spawn(f)
        }
    }
}

pub fn key_hex(k: &PeerKey) -> String {
    hex::encode(k)
}

pub fn parse_key(s: &str) -> Option<PeerKey> {
    if s.len() != 64 {
        return None;
    }
    hex::decode(s).ok()?.try_into().ok()
}

fn short(k: &PeerKey) -> String {
    format!("Joueur {}", &hex::encode(k)[..6])
}

// ---------------------------------------------------------------------------------------------------------
// Réseau, serveur dédié : points d'injection
// ---------------------------------------------------------------------------------------------------------

/// Comment joindre un pair (`None` : clé qui n'est pas une identité valide).
pub type AddrOf = Arc<dyn Fn(&PeerKey) -> Option<EndpointAddr> + Send + Sync>;

/// Ce que Hearth demande au réseau : le point de terminaison iroh et l'adresse d'un pair.
#[derive(Clone)]
pub struct HearthNet {
    pub endpoint: Endpoint,
    pub addr_of: AddrOf,
}

/// Ce qu'il faut pour installer et lancer le serveur d'un groupe.
#[derive(Clone)]
pub struct ServerSpec {
    /// `hearth/<groupe>/server`.
    pub dir: PathBuf,
    pub minecraft: String,
    pub loader: LoaderSpec,
    /// Dossier `mods` de l'instance du groupe (les mods « client seulement » en sont retirés).
    pub mods_src: PathBuf,
    pub java_root: PathBuf,
    pub java_override: Option<PathBuf>,
    pub jar_cache: PathBuf,
    pub ram_mb: u32,
    pub accept_eula: bool,
    /// Chaque ligne de la sortie du serveur.
    pub on_line: LineSink,
    /// Étapes d'installation, en français.
    pub progress: LineSink,
}

/// Un serveur dédié en marche (réel, ou factice dans les tests).
pub trait RunningServer: Send + Sync {
    fn port(&self) -> u16;
    /// Dossier du monde que lit le serveur.
    fn world_dir(&self) -> PathBuf;
    /// `save-off` puis `save-all flush` : le monde ne bouge plus sur le disque.
    fn flush_begin(&self) -> BoxFut<'_, Result<(), String>>;
    /// `save-on`.
    fn flush_end(&self) -> BoxFut<'_, Result<(), String>>;
    /// `stop`, puis arrêt forcé après `grace`.
    fn stop(&self, grace: Duration) -> BoxFut<'_, ()>;
    /// Le serveur s'est arrêté (plantage ou `stop`).
    fn wait_exit(&self) -> BoxFut<'_, ()>;
}

pub trait ServerRunner: Send + Sync {
    fn start(&self, spec: ServerSpec) -> BoxFut<'static, Result<Arc<dyn RunningServer>, String>>;
}

/// Serveur dédié réel : installation (Mojang, Fabric, Quilt, Forge, NeoForge) puis processus Java.
pub struct RealRunner;

struct RealServer(Arc<core_lib::hearth::server::DedicatedServer>);

/// Les futurs de `core::hearth::server` ne sont pas `Send` (un prédicat de recherche dans la sortie du serveur n'est pas
/// `Sync`) : on les attend sur un fil bloquant dédié, qui n'exige pas `Send`.
fn run_local<T: Send + 'static, F: Future<Output = T>>(f: impl FnOnce() -> F + Send + 'static) -> BoxFut<'static, T> {
    let h = tokio::runtime::Handle::current();
    Box::pin(async move {
        match tokio::task::spawn_blocking(move || h.block_on(f())).await {
            Ok(v) => v,
            Err(e) => std::panic::resume_unwind(e.into_panic()),
        }
    })
}

impl RunningServer for RealServer {
    fn port(&self) -> u16 {
        self.0.port()
    }
    fn world_dir(&self) -> PathBuf {
        self.0.world_dir()
    }
    fn flush_begin(&self) -> BoxFut<'_, Result<(), String>> {
        let s = self.0.clone();
        run_local(move || async move {
            s.save_off().await.map_err(|e| crate::error::user_msg(&e))?;
            s.save_all_flush().await.map_err(|e| crate::error::user_msg(&e))
        })
    }
    fn flush_end(&self) -> BoxFut<'_, Result<(), String>> {
        let s = self.0.clone();
        run_local(move || async move { s.save_on().await.map_err(|e| crate::error::user_msg(&e)) })
    }
    fn stop(&self, grace: Duration) -> BoxFut<'_, ()> {
        Box::pin(async move {
            self.0.stop(grace).await;
        })
    }
    fn wait_exit(&self) -> BoxFut<'_, ()> {
        Box::pin(async move {
            self.0.wait_exit().await;
        })
    }
}

impl ServerRunner for RealRunner {
    fn start(&self, spec: ServerSpec) -> BoxFut<'static, Result<Arc<dyn RunningServer>, String>> {
        run_local(move || async move {
            let e = |e: core_lib::types::Error| crate::error::user_msg(&e);
            let installed = ServerInstaller::new()
                .install(&InstallRequest {
                    dir: spec.dir.clone(),
                    minecraft: spec.minecraft.clone(),
                    loader: spec.loader.clone(),
                    java_root: spec.java_root.clone(),
                    java_override: spec.java_override.clone(),
                    jar_cache: Some(spec.jar_cache.clone()),
                    progress: Some(spec.progress.clone()),
                })
                .await
                .map_err(e)?;
            if spec.loader.loader != core_lib::types::Loader::Vanilla {
                let (src, dst) = (spec.mods_src.clone(), spec.dir.join("mods"));
                tokio::task::spawn_blocking(move || {
                    core_lib::hearth::server::sync_server_mods(&src, &dst, &HashMap::new())
                })
                .await
                .map_err(|_| "préparation des mods du serveur interrompue".to_string())?
                .map_err(e)?;
            }
            let java = core_lib::hearth::server::resolve_java(&spec.java_root, installed.java_major, spec.java_override.as_deref())
                .await
                .map_err(e)?;
            let mut cfg = ServerConfig::new(spec.dir.clone(), java, spec.ram_mb);
            cfg.accept_eula = spec.accept_eula;
            cfg.on_line = Some(spec.on_line.clone());
            let server = core_lib::hearth::server::DedicatedServer::start(&cfg, &installed).await.map_err(e)?;
            Ok(Arc::new(RealServer(Arc::new(server))) as Arc<dyn RunningServer>)
        })
    }
}

/// Ouvre le jeu du joueur sur un serveur (`--quickPlayMultiplayer` vers `127.0.0.1:port`). Remplaçable dans les tests.
pub trait ClientLauncher: Send + Sync {
    fn launch(&self, st: &Arc<crate::state::AppState>, instance: &str, port: u16) -> BoxFut<'static, Result<(), String>>;
}

/// Lanceur réel : la boucle de lancement du jeu (`launch_with`).
pub struct RealLauncher;

impl ClientLauncher for RealLauncher {
    fn launch(&self, st: &Arc<crate::state::AppState>, instance: &str, port: u16) -> BoxFut<'static, Result<(), String>> {
        let (st, instance) = (st.clone(), instance.to_string());
        Box::pin(async move { st.launch_with(&instance, Some(("127.0.0.1".to_string(), port))).await })
    }
}

// ---------------------------------------------------------------------------------------------------------
// Persistance locale
// ---------------------------------------------------------------------------------------------------------

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct GroupMeta {
    /// Instance locale du modpack du groupe.
    #[serde(default)]
    instance_id: Option<String>,
    #[serde(default)]
    keep_background: bool,
    /// Version dont le monde est déjà matérialisé dans `server/world`.
    #[serde(default)]
    materialized: Option<String>,
    /// Dernière fois (secondes Unix) qu'on a vu chaque membre en ligne.
    #[serde(default)]
    seen: BTreeMap<String, i64>,
    /// Membres qui n'ont pas encore reçu leur invitation.
    #[serde(default)]
    invite_pending: BTreeSet<String>,
    /// Changement de modpack proposé d'ici : (empreinte du manifeste, instance). L'instance liée ne change qu'une fois
    /// `PackUpdate` appliqué à la chaîne.
    #[serde(default)]
    pending_pack: Option<(String, String)>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Meta {
    /// Interrupteur de sécurité (docs/hearth.md § 12.1 n° 63) : `false` par défaut, Hearth est inerte.
    #[serde(default)]
    hearth_enabled: bool,
    #[serde(default)]
    eula_accepted: bool,
    #[serde(default)]
    groups: BTreeMap<String, GroupMeta>,
}

fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(data)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)
}

/// Dernier état connu d'un membre.
#[derive(Debug, Clone, Default)]
pub struct MemberStatus {
    pub online: bool,
    pub op_seq: u64,
    /// Version la plus avancée qu'il dit avoir (parmi celles qu'on connaît).
    pub latest: Option<VersionPoint>,
}

/// Invitation de groupe reçue, en attente de la réponse de l'utilisateur.
struct PendingInvite {
    from: PeerKey,
    ops: Vec<SignedOp>,
    key: GroupKey,
    state: GroupState,
    received_at: i64,
}

/// Où en est le lancement d'un groupe sur ce PC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Play {
    Launching { steps: &'static [&'static str], index: u8 },
    Joined { holder: PeerKey },
}

pub const HOST_STEPS: &[&str] = &["Mise à jour du monde", "Démarrage du serveur", "Ouverture du jeu"];
pub const JOIN_STEPS: &[&str] = &["Connexion à l'hôte", "Ouverture du jeu"];

/// Partie hébergée par ce PC.
pub struct ActiveHost {
    pub group: GroupId,
    pub instance_id: String,
    pub server: Arc<dyn RunningServer>,
    pub started: Instant,
    /// Dernière version de la chaîne de cette partie : parent de la suivante.
    tip: Mutex<Option<(VersionId, u64)>>,
    pub lease: Mutex<Lease>,
    /// Dernier renouvellement du bail (horloges murale et monotone) : auto-exclusion après veille.
    fence: Mutex<HostFence>,
    /// Exclu après une veille : la partie finit comme branche sans bail (`LeaseRelease { Suspend }`).
    fenced: AtomicBool,
    players: Arc<Mutex<BTreeSet<String>>>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    stopping: AtomicBool,
}

impl ActiveHost {
    pub fn players(&self) -> u32 {
        lock(&self.players).len() as u32
    }
    pub fn port(&self) -> u16 {
        self.server.port()
    }
}

/// Joueur qui entre ou sort, d'après une ligne de la sortie du serveur. Seul un message de journal compte (jamais une
/// ligne de chat : `<Pseudo> …`).
pub fn parse_player_event(line: &str) -> Option<(bool, String)> {
    let msg = log_message(line.trim_end());
    let (name, joined) = match msg.strip_suffix(" joined the game") {
        Some(n) => (n, true),
        None => (msg.strip_suffix(" left the game")?, false),
    };
    let ok = (1..=16).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    ok.then(|| (joined, name.to_string()))
}

/// Têtes de la lignée, une par monde distinct. Une fourche se résout par une version `Restore` du monde gardé posée
/// sur chaque tête écartée ; la résolution reste vraie après les parties suivantes (voir
/// `VersionGraph::effective_heads`).
pub fn effective_heads(g: &p2p_lib::hearth::version::VersionGraph, state: &GroupState) -> Vec<SignedVersion> {
    g.effective_heads(state).into_iter().cloned().collect()
}

// ---------------------------------------------------------------------------------------------------------
// Le runtime
// ---------------------------------------------------------------------------------------------------------

pub struct HearthRuntime {
    me: PeerKey,
    identity: Arc<Identity>,
    root: PathBuf,
    friends: Arc<FriendsDb>,
    instances: Arc<InstanceStore>,
    access: Arc<crate::authz::SessionAccess>,
    emit: EventSink,
    store: GroupStore,
    groups: Mutex<HashMap<GroupId, Arc<Mutex<Group>>>>,
    leases: Mutex<LeaseTable>,
    status: Mutex<HashMap<(GroupId, PeerKey), MemberStatus>>,
    meta: Mutex<Meta>,
    invites: Mutex<Vec<PendingInvite>>,
    /// Invitations reçues (ami, secondes Unix) sur les dernières 24 h : quota par ami, même si elles sont écartées.
    invite_log: Mutex<Vec<(PeerKey, i64)>>,
    outbox: Arc<Outbox>,
    net: OnceLock<HearthNet>,
    world: OnceLock<WorldStore>,
    runner: Mutex<Arc<dyn ServerRunner>>,
    launcher: Mutex<Arc<dyn ClientLauncher>>,
    play: Mutex<HashMap<GroupId, Play>>,
    active: Mutex<HashMap<GroupId, Arc<ActiveHost>>>,
    /// Collections dont on a tout le contenu (répond à `HaveVersion`, qui est synchrone).
    complete: Mutex<HashSet<[u8; 32]>>,
    /// Mondes en cours de rapatriement : pour cent.
    fetching: Mutex<HashMap<GroupId, u8>>,
    /// Groupes dont une sonde tourne déjà.
    probing: Mutex<HashSet<GroupId>>,
    background: Mutex<Vec<JoinHandle<()>>>,
    drain: Mutex<Option<p2p_lib::hearth::outbox::DrainTask>>,
    /// Hôtes dont le bail est annoncé en phase `Starting` (installation et démarrage du serveur en cours).
    starting: Mutex<HashMap<GroupId, Arc<StartingHost>>>,
    /// Versions annoncées dont la taille dépasse le seuil de croissance : refusées tant qu'une personne n'a pas confirmé
    /// (un deuxième « Jouer » après l'avertissement vaut confirmation).
    oversize: Mutex<HashSet<VersionId>>,
    /// Fin d'une partie hébergée qui s'arrête d'elle-même (plantage) : l'application arrête aussi le partage, l'accès et
    /// l'état de lancement.
    end_hook: Mutex<Option<EndHook>>,
    /// Une seule fermeture générale à la fois : la seconde attend la première.
    stop_all: tokio::sync::Mutex<()>,
    /// Horloges (injectables : les tests font « dormir » la machine sans attendre).
    clock: Mutex<Arc<dyn Clock>>,
    /// Groupes dont un premier « Jouer » a reçu un bail « au mieux » : le second clic vaut confirmation.
    best_effort: Mutex<HashMap<GroupId, Instant>>,
    me_weak: Weak<HearthRuntime>,
}

/// Issue de la première phase de la prise du bail ([`HearthRuntime::begin_starting`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Begin {
    /// Le bail `Starting` est annoncé : on prépare le serveur.
    Hosting,
    /// Un autre membre a la main (bail connu, claim concurrent gagnant, ou départage perdu) : le rejoindre.
    Join(PeerKey),
}

/// Issue d'un tour de renouvellement du bail d'un serveur prêt ([`HearthRuntime::renew_tick`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenewTick {
    Renewed,
    /// Exclu après une veille, rien ne contredit : reprise avec un nouveau `lease_id`.
    Resumed,
    /// Exclu après une veille, un autre bail ou une version plus récente existe : le serveur s'arrête, la session est
    /// publiée comme branche sans bail.
    Fenced,
}

/// Début du message d'un bail « au mieux » (voir [`HearthRuntime::begin_starting`]).
pub const BEST_EFFORT_NOTE: &str = "Le groupe n'est pas assez joignable pour une prise du serveur sûre : si un autre membre joue de son côté, il y aura deux versions à départager.";

/// Réaction de l'application à la fin d'une partie hébergée (voir `HearthRuntime::set_end_hook`).
pub type EndHook = Arc<dyn Fn(GroupId) -> BoxFut<'static, ()> + Send + Sync>;

/// Ce que `forget_group` laisse à nettoyer après le retrait de l'état en mémoire.
struct ForgetTail {
    host: Option<Arc<ActiveHost>>,
    starting: Option<Arc<StartingHost>>,
    collections: HashSet<[u8; 32]>,
}

/// Bail `Starting` d'un hôte et sa tâche de renouvellement.
pub struct StartingHost {
    lease: Mutex<Lease>,
    task: Mutex<Option<JoinHandle<()>>>,
    fence: Mutex<HostFence>,
    /// Exclu après une veille pendant la préparation : un autre bail ou une version plus récente existe.
    fenced: AtomicBool,
}

impl GroupMembers for HearthRuntime {
    fn is_member(&self, group: &GroupId, key: &PeerKey) -> bool {
        if !self.enabled() {
            return false;
        }
        self.state(group).is_some_and(|s| s.is_member(key))
    }
}

impl HearthRuntime {
    /// Ouvre `<root>/hearth` : groupes, file d'envoi, métadonnées.
    pub fn new(
        root: &Path,
        identity: Arc<Identity>,
        friends: Arc<FriendsDb>,
        instances: Arc<InstanceStore>,
        access: Arc<crate::authz::SessionAccess>,
        emit: EventSink,
    ) -> Result<Arc<Self>, String> {
        let dir = root.join("hearth");
        let store = GroupStore::open(&dir).map_err(|e| e.to_string())?;
        let outbox = Arc::new(Outbox::open(dir.join("outbox")).map_err(|e| e.to_string())?);
        let meta: Meta = std::fs::read(dir.join("meta.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let mut groups = HashMap::new();
        for id in store.list().map_err(|e| e.to_string())? {
            match store.open_group(&id) {
                Ok(g) => {
                    groups.insert(id, Arc::new(Mutex::new(g)));
                }
                Err(e) => eprintln!("[cubelink] groupe {id} ignoré : {e}"),
            }
        }
        let me = identity.public_key().to_bytes();
        Ok(Arc::new_cyclic(|w| Self {
            me,
            identity,
            root: dir,
            friends,
            instances,
            access,
            emit,
            store,
            groups: Mutex::new(groups),
            leases: Mutex::new(LeaseTable::new()),
            status: Mutex::new(HashMap::new()),
            meta: Mutex::new(meta),
            invites: Mutex::new(Vec::new()),
            invite_log: Mutex::new(Vec::new()),
            outbox,
            net: OnceLock::new(),
            world: OnceLock::new(),
            runner: Mutex::new(Arc::new(RealRunner)),
            launcher: Mutex::new(Arc::new(RealLauncher)),
            play: Mutex::new(HashMap::new()),
            active: Mutex::new(HashMap::new()),
            complete: Mutex::new(HashSet::new()),
            fetching: Mutex::new(HashMap::new()),
            probing: Mutex::new(HashSet::new()),
            background: Mutex::new(Vec::new()),
            drain: Mutex::new(None),
            starting: Mutex::new(HashMap::new()),
            oversize: Mutex::new(HashSet::new()),
            end_hook: Mutex::new(None),
            stop_all: tokio::sync::Mutex::new(()),
            clock: Mutex::new(Arc::new(SystemClock)),
            best_effort: Mutex::new(HashMap::new()),
            me_weak: w.clone(),
        }))
    }

    fn arc(&self) -> Arc<HearthRuntime> {
        // Le runtime vit dans un `Arc` du début à la fin : le `Weak` ne peut pas être mort pendant un appel.
        self.me_weak.upgrade().expect("HearthRuntime vit dans un Arc")
    }

    pub fn me(&self) -> PeerKey {
        self.me
    }

    pub fn outbox(&self) -> &Arc<Outbox> {
        &self.outbox
    }

    /// Branche le réseau et le magasin de blobs (au démarrage du backend) ; lance les boucles de fond.
    pub fn attach(self: &Arc<Self>, net: HearthNet, world: WorldStore) {
        let _ = self.net.set(net);
        let _ = self.world.set(world);
        self.start_background();
    }

    /// Remplace le lanceur de serveur (tests).
    pub fn set_runner(&self, r: Arc<dyn ServerRunner>) {
        *lock(&self.runner) = r;
    }

    /// Remplace le lanceur du jeu (tests).
    /// Branche la réaction de l'application à une partie dont le serveur s'arrête seul (plantage).
    pub fn set_end_hook(&self, h: EndHook) {
        *lock(&self.end_hook) = Some(h);
    }

    /// Les groupes dont ce PC héberge le serveur.
    pub fn active_groups(&self) -> Vec<GroupId> {
        lock(&self.active).keys().copied().collect()
    }

    /// Verrou de la fermeture générale (voir `AppState::hearth_stop_all`) : le second appelant attend le premier.
    pub async fn stop_all_guard(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.stop_all.lock().await
    }

    pub fn set_launcher(&self, l: Arc<dyn ClientLauncher>) {
        *lock(&self.launcher) = l;
    }

    pub fn launcher(&self) -> Arc<dyn ClientLauncher> {
        lock(&self.launcher).clone()
    }

    /// Le monde de cette version est-il en entier sur ce PC ?
    pub fn world_complete(&self, v: &SignedVersion) -> bool {
        lock(&self.complete).contains(&v.header.collection)
    }

    /// Versions encore en file d'envoi.
    pub fn pending_outbox(&self) -> usize {
        self.outbox.len()
    }

    pub fn world(&self) -> Result<&WorldStore, String> {
        self.world.get().ok_or_else(|| "le réseau entre amis est en cours de démarrage, réessayez dans un instant".to_string())
    }

    fn net(&self) -> Result<&HearthNet, String> {
        self.net.get().ok_or_else(|| "le réseau entre amis est en cours de démarrage, réessayez dans un instant".to_string())
    }

    pub fn emit_json<T: Serialize>(&self, name: &str, payload: &T) {
        if let Ok(v) = serde_json::to_value(payload) {
            (self.emit)(name, v);
        }
    }

    /// Arrête les boucles de fond (fermeture de l'application, tests).
    pub async fn shutdown(&self) {
        for h in std::mem::take(&mut *lock(&self.background)) {
            h.abort();
        }
        let hosts: Vec<Arc<ActiveHost>> = lock(&self.active).values().cloned().collect();
        for h in hosts {
            h.stopping.store(true, Ordering::SeqCst);
            for t in std::mem::take(&mut *lock(&h.tasks)) {
                t.abort();
            }
            // Un serveur qui survit à Cubelink garde le monde verrouillé : il est arrêté proprement (le monde est
            // publié par `stop_all_hosting`, qui passe avant dans la fermeture normale de l'application).
            h.server.stop(SERVER_STOP_GRACE).await;
        }
        let starting: Vec<Arc<StartingHost>> = lock(&self.starting).drain().map(|(_, s)| s).collect();
        for s in starting {
            if let Some(t) = lock(&s.task).take() {
                t.abort();
            }
        }
        if let Ok(w) = self.world() {
            let _ = w.shutdown().await;
        }
    }

    // --- accès aux groupes ----------------------------------------------------------------------------

    fn group(&self, id: &GroupId) -> Option<Arc<Mutex<Group>>> {
        lock(&self.groups).get(id).cloned()
    }

    fn with_group<R>(&self, id: &GroupId, f: impl FnOnce(&mut Group) -> R) -> Result<R, String> {
        let g = self.group(id).ok_or("groupe introuvable")?;
        let mut guard = lock(&g);
        Ok(f(&mut guard))
    }

    pub fn state(&self, id: &GroupId) -> Option<GroupState> {
        self.group(id).map(|g| lock(&g).state().clone())
    }

    pub fn group_ids(&self) -> Vec<GroupId> {
        let mut v: Vec<GroupId> = lock(&self.groups).keys().copied().collect();
        v.sort();
        v
    }

    pub fn parse_id(s: &str) -> Result<GroupId, String> {
        GroupId::from_hex(s).ok_or_else(|| "identifiant de groupe invalide".to_string())
    }

    pub fn require_state(&self, id: &GroupId) -> Result<GroupState, String> {
        self.state(id).ok_or_else(|| "ce groupe n'existe plus".to_string())
    }

    fn others(&self, st: &GroupState) -> Vec<PeerKey> {
        st.members.iter().copied().filter(|k| *k != self.me).collect()
    }

    pub fn name_of(&self, k: &PeerKey) -> String {
        if *k == self.me {
            return "Toi".to_string();
        }
        match self.friends.get_friend(&key_hex(k)) {
            Ok(Some(f)) if !f.name.is_empty() => f.name,
            _ => short(k),
        }
    }

    fn lite(&self, st: &GroupState, k: &PeerKey) -> HearthMemberLite {
        HearthMemberLite { public_key: key_hex(k), name: self.name_of(k), online: *k == self.me || self.is_online(&st.id, k) }
    }

    /// Test : fait croire que `key` n'a pas donné signe de vie depuis `days` jours (la règle des 7 jours se joue sur
    /// l'horloge, qu'un test ne peut pas avancer).
    #[doc(hidden)]
    pub fn test_set_last_seen_days_ago(&self, gid: &GroupId, key: &PeerKey, days: i64) {
        let t = now_secs() - days * 86_400;
        lock(&self.meta).groups.entry(gid.to_hex()).or_default().seen.insert(key_hex(key), t);
    }

    pub fn is_online(&self, gid: &GroupId, k: &PeerKey) -> bool {
        lock(&self.status).get(&(*gid, *k)).is_some_and(|s| s.online)
    }

    fn mark_seen(&self, gid: &GroupId, k: &PeerKey) {
        let now = now_secs();
        let mut meta = lock(&self.meta);
        meta.groups.entry(gid.to_hex()).or_default().seen.insert(key_hex(k), now);
    }

    fn save_meta(&self) {
        let data = serde_json::to_vec_pretty(&*lock(&self.meta)).unwrap_or_default();
        if let Err(e) = write_atomic(&self.root.join("meta.json"), &data) {
            eprintln!("[cubelink] métadonnées Hearth non enregistrées : {e}");
        }
    }

    fn update_meta<R>(&self, gid: &GroupId, f: impl FnOnce(&mut GroupMeta) -> R) -> R {
        let r = f(lock(&self.meta).groups.entry(gid.to_hex()).or_default());
        self.save_meta();
        r
    }

    fn meta_of(&self, gid: &GroupId) -> GroupMeta {
        lock(&self.meta).groups.get(&gid.to_hex()).cloned().unwrap_or_default()
    }

    pub fn instance_of(&self, gid: &GroupId) -> Option<String> {
        self.meta_of(gid).instance_id.filter(|id| self.instances.get(id).is_ok())
    }

    pub fn set_instance(&self, gid: &GroupId, instance_id: Option<String>) {
        self.update_meta(gid, |m| m.instance_id = instance_id);
        self.emit_state();
    }

    pub fn keep_background(&self, gid: &GroupId) -> bool {
        self.meta_of(gid).keep_background
    }

    pub fn set_keep_background(&self, gid: &GroupId, on: bool) {
        self.update_meta(gid, |m| m.keep_background = on);
    }

    /// Interrupteur de sécurité : Hearth est-il actif sur ce PC ?
    pub fn enabled(&self) -> bool {
        lock(&self.meta).hearth_enabled
    }

    /// Active ou coupe Hearth. Coupé : l'accueil masque la section, les commandes refusent et le protocole
    /// `cubelink/hearth/1` refuse tous les pairs, et les sondes, invitations en attente, synchros et envois de la file sont inertes ; un serveur déjà lancé n'est pas touché (l'arrêt reste possible).
    pub fn set_enabled(&self, on: bool) {
        lock(&self.meta).hearth_enabled = on;
        self.save_meta();
        self.emit_state();
    }

    /// Refus clair des commandes `hearth_*` tant que l'interrupteur est coupé.
    pub fn require_enabled(&self) -> Result<(), String> {
        if self.enabled() {
            Ok(())
        } else {
            Err(HEARTH_DISABLED.to_string())
        }
    }

    pub fn eula_accepted(&self) -> bool {
        lock(&self.meta).eula_accepted
    }

    pub fn accept_eula(&self) {
        lock(&self.meta).eula_accepted = true;
        self.save_meta();
    }

    pub fn materialized(&self, gid: &GroupId) -> Option<VersionId> {
        self.meta_of(gid).materialized.and_then(|s| VersionId::from_hex(&s))
    }

    pub fn set_materialized_version(&self, gid: &GroupId, v: VersionId) {
        self.set_materialized(gid, Some(v));
    }

    fn set_materialized(&self, gid: &GroupId, v: Option<VersionId>) {
        self.update_meta(gid, |m| m.materialized = v.map(|v| v.to_hex()));
    }

    pub fn server_dir(&self, gid: &GroupId) -> PathBuf {
        self.store.group_dir(gid).join("server")
    }

    pub fn jar_cache(&self) -> PathBuf {
        self.root.join("jars")
    }

    // --- état des parties ------------------------------------------------------------------------------

    pub fn set_play(&self, gid: &GroupId, p: Option<Play>) {
        {
            let mut m = lock(&self.play);
            match p {
                Some(p) => {
                    m.insert(*gid, p);
                }
                None => {
                    m.remove(gid);
                }
            }
        }
        self.emit_state();
    }

    /// Démarre un lancement si aucun n'est en cours pour ce groupe (atomique).
    pub fn try_begin_play(&self, gid: &GroupId, steps: &'static [&'static str]) -> bool {
        {
            let mut m = lock(&self.play);
            if m.contains_key(gid) {
                return false;
            }
            m.insert(*gid, Play::Launching { steps, index: 0 });
        }
        self.emit_state();
        true
    }

    pub fn play_of(&self, gid: &GroupId) -> Option<Play> {
        lock(&self.play).get(gid).cloned()
    }

    pub fn active(&self, gid: &GroupId) -> Option<Arc<ActiveHost>> {
        lock(&self.active).get(gid).cloned()
    }

    pub fn any_active(&self) -> Option<Arc<ActiveHost>> {
        lock(&self.active).values().next().cloned()
    }
}

// ---------------------------------------------------------------------------------------------------------
// Réseau : connexions, sonde, synchro
// ---------------------------------------------------------------------------------------------------------

type Visit = Arc<dyn Fn(Arc<HearthConn>) -> BoxFut<'static, Result<(), NetError>> + Send + Sync>;

impl HearthRuntime {
    async fn connect(&self, key: &PeerKey) -> Result<HearthConn, String> {
        let net = self.net()?;
        let addr = (net.addr_of)(key).ok_or("clé de membre invalide")?;
        HearthConn::connect(&net.endpoint, addr).await.map_err(|e| e.to_string())
    }

    /// Adresses des membres en ligne (autres que nous), pour rapatrier un monde.
    fn online_addrs(&self, gid: &GroupId, prefer: Option<PeerKey>) -> Vec<EndpointAddr> {
        let (Some(st), Ok(net)) = (self.state(gid), self.net()) else { return Vec::new() };
        let mut keys: Vec<PeerKey> = self.others(&st).into_iter().filter(|k| self.is_online(gid, k)).collect();
        if let Some(p) = prefer {
            if let Some(i) = keys.iter().position(|k| *k == p) {
                keys.swap(0, i);
            } else if st.is_member(&p) {
                keys.insert(0, p);
            }
        }
        keys.iter().filter_map(|k| (net.addr_of)(k)).collect()
    }

    /// Exécute `f` auprès des autres membres (ceux en ligne seulement si `only_online`). Rend le nombre de succès.
    pub async fn broadcast(self: &Arc<Self>, gid: GroupId, only_online: bool, f: Visit) -> usize {
        let Some(st) = self.state(&gid) else { return 0 };
        let mut set = tokio::task::JoinSet::new();
        for k in self.others(&st).into_iter().filter(|k| !only_online || self.is_online(&gid, k)) {
            let (rt, f) = (self.clone(), f.clone());
            set.spawn(async move {
                let conn = tokio::time::timeout(PROBE_TIMEOUT, rt.connect(&k)).await.ok()?.ok()?;
                tokio::time::timeout(PROBE_TIMEOUT, f(Arc::new(conn))).await.ok()?.ok()
            });
        }
        let mut ok = 0;
        while let Some(r) = set.join_next().await {
            if matches!(r, Ok(Some(()))) {
                ok += 1;
            }
        }
        ok
    }

    /// Sonde tous les membres d'un groupe (état, chaîne, versions, bail). Une sonde à la fois par groupe.
    pub fn probe_group(self: &Arc<Self>, gid: GroupId) -> BoxFut<'static, ()> {
        // Futur boxé : une sonde peut déclencher une proposition, qui en finalisant relance une sonde (récursion asynchrone).
        let rt = self.clone();
        Box::pin(async move { rt.probe_group_inner(gid).await })
    }

    async fn probe_group_inner(self: &Arc<Self>, gid: GroupId) {
        if !self.enabled() {
            return;
        }
        if lock(&self.probing).contains(&gid) {
            // Une sonde tourne déjà : on attend son résultat plutôt que de décider sur un état périmé.
            self.wait_probe_free(&gid).await;
            return;
        }
        let _ = self.run_probe(gid).await;
    }

    async fn wait_probe_free(&self, gid: &GroupId) {
        for _ in 0..300 {
            if !lock(&self.probing).contains(gid) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Ronde de sondage complète avant une décision : un `Hello` en parallèle à **tous** les membres (connexion et
    /// réponse en [`ROUND_HELLO_TIMEOUT`] chacun), puis le rattrapage des opérations et des versions. Rend qui a répondu
    /// (avec sa version la plus avancée) et qui est injoignable : `decide` ne se fie jamais au seul cache. Si une sonde de
    /// fond tourne déjà, on attend qu'elle finisse et on fait **notre** ronde.
    pub async fn probe_round(self: &Arc<Self>, gid: GroupId) -> ProbeRound {
        loop {
            self.wait_probe_free(&gid).await;
            if lock(&self.probing).insert(gid) {
                break;
            }
        }
        self.run_probe_guarded(gid).await
    }

    async fn run_probe(self: &Arc<Self>, gid: GroupId) -> ProbeRound {
        if !lock(&self.probing).insert(gid) {
            return ProbeRound::default();
        }
        self.run_probe_guarded(gid).await
    }

    /// Le verrou `probing` du groupe est pris par l'appelant ; il est relâché ici.
    async fn run_probe_guarded(self: &Arc<Self>, gid: GroupId) -> ProbeRound {
        let started = Instant::now();
        let mut round = ProbeRound::default();
        if let Some(st) = self.state(&gid) {
            let mut set = tokio::task::JoinSet::new();
            for k in self.others(&st) {
                let rt = self.clone();
                set.spawn(async move { (k, rt.probe_member(gid, k).await) });
            }
            while let Some(r) = set.join_next().await {
                let Ok((k, ok)) = r else { continue };
                if ok {
                    let latest = lock(&self.status).get(&(gid, k)).and_then(|s| s.latest);
                    round.answers.push(OnlineMember { key: k, latest });
                } else {
                    round.unreachable.push(k);
                }
            }
        }
        round.answers.sort_by_key(|m| m.key);
        round.unreachable.sort();
        round.elapsed = started.elapsed();
        lock(&self.probing).remove(&gid);
        self.save_meta();
        self.outbox.wake();
        self.emit_state();
        let rt = self.clone();
        spawn(async move {
            rt.maybe_take_over(gid).await;
            rt.sync_heads(gid, None).await;
        });
        round
    }

    /// Le créateur a été écarté il y a plus de 30 jours (`ContinueWithout`) et le groupe est toujours gouverné par lui :
    /// le plus petit des autres membres propose de passer à la majorité (les autres votent depuis la page du serveur). Une
    /// seule proposition à la fois, et jamais quand le créateur est de retour.
    async fn maybe_take_over(self: &Arc<Self>, gid: GroupId) {
        if !self.enabled() {
            return;
        }
        let Some(st) = self.state(&gid) else { return };
        if st.governance != p2p_lib::hearth::model::Governance::Creator || st.creator == self.me || !st.is_member(&self.me) {
            return;
        }
        let now = u64::try_from(now_secs()).unwrap_or(0);
        if !st.creator_dormant(now) || self.is_online(&gid, &st.creator) {
            return;
        }
        if st.members.iter().filter(|k| **k != st.creator).min() != Some(&self.me) {
            return;
        }
        let open = self.with_group(&gid, |g| g.proposals().any(|p| matches!(p.op, GroupOp::SetGovernance { .. }))).unwrap_or(true);
        if !open {
            let _ = self.propose(gid, GroupOp::SetGovernance { at: now }).await;
        }
    }

    pub async fn probe_all(self: &Arc<Self>) {
        if !self.enabled() {
            return;
        }
        for gid in self.group_ids() {
            self.probe_group(gid).await;
        }
    }

    async fn probe_member(self: &Arc<Self>, gid: GroupId, key: PeerKey) -> bool {
        let r = tokio::time::timeout(PROBE_TIMEOUT * 2, self.probe_inner(gid, key)).await;
        let ok = matches!(r, Ok(Ok(())));
        if !ok {
            lock(&self.status).entry((gid, key)).or_default().online = false;
        }
        ok
    }

    async fn probe_inner(self: &Arc<Self>, gid: GroupId, key: PeerKey) -> Result<(), String> {
        if !self.enabled() {
            return Err("Hearth est coupé".into());
        }
        if self.meta_of(&gid).invite_pending.contains(&key_hex(&key)) {
            // Sans effet tant que l'invité n'est pas joignable : on retentera à la prochaine sonde.
            if self.send_group_invite(gid, key).await.is_ok() {
                self.update_meta(&gid, |m| {
                    m.invite_pending.remove(&key_hex(&key));
                });
            }
        }
        let (seq, heads) = self.with_group(&gid, |g| (g.state().op_seq, g.heads()))?;
        // Connexion et `Hello` : 4 s au plus (le rattrapage qui suit a son propre délai).
        let (conn, st) = tokio::time::timeout(ROUND_HELLO_TIMEOUT, async {
            let conn = self.connect(&key).await?;
            let st = conn.hello(&gid, seq, &heads).await.map_err(|e| e.to_string())?;
            Ok::<_, String>((conn, st))
        })
        .await
        .map_err(|_| "délai dépassé".to_string())??;
        if st.op_seq > seq {
            let ops = conn.fetch_ops(&gid, seq).await.map_err(|e| e.to_string())?;
            self.apply_ops(gid, &ops)?;
        }
        let missing = self.with_group(&gid, |g| st.heads.iter().any(|h| !g.graph().contains(h)))?;
        if missing {
            let after = self.with_group(&gid, |g| g.heads().iter().filter_map(|h| g.graph().get(h)).map(|v| v.header.number).min().unwrap_or(0))?;
            let vs = conn.fetch_versions(&gid, after.saturating_sub(1)).await.map_err(|e| e.to_string())?;
            self.with_group(&gid, |g| g.add_versions(vs).map(|_| ()).map_err(|e| e.to_string()))??;
        }
        let latest = self.with_group(&gid, |g| st.heads.iter().filter_map(|h| g.graph().get(h)).map(|v| v.header.point()).max())?;
        lock(&self.status).insert((gid, key), MemberStatus { online: true, op_seq: st.op_seq, latest });
        self.mark_seen(&gid, &key);
        match st.lease {
            Some(l) if l.holder == key => {
                // Seul le titulaire annonce son propre bail : le bail d'un tiers rapporté par un membre n'est pas cru.
                let _ = self.with_group(&gid, |g| lock(&self.leases).observe(g.state(), g.graph(), &key, l, Instant::now()));
            }
            // Le membre répond et n'héberge rien : le bail qu'on gardait de lui est périmé (fin de partie dont le
            // `LeaseRelease` s'est perdu). Sans cela il expirerait en « partie mal terminée » alors qu'il est en ligne.
            None => {
                if lock(&self.leases).release(&gid, &key) {
                    self.emit_state();
                }
            }
            Some(_) => {}
        }
        Ok(())
    }

    /// Reprend auprès de `key` les opérations et les versions qui nous manquent.
    pub async fn pull_from(self: &Arc<Self>, gid: GroupId, key: PeerKey) -> Result<(), String> {
        let conn = self.connect(&key).await?;
        let seq = self.with_group(&gid, |g| g.state().op_seq)?;
        let ops = conn.fetch_ops(&gid, seq).await.map_err(|e| e.to_string())?;
        self.apply_ops(gid, &ops)?;
        let after = self.with_group(&gid, |g| g.graph().iter().map(|(_, v)| v.header.number).max().unwrap_or(0))?;
        let vs = conn.fetch_versions(&gid, after.saturating_sub(1)).await.map_err(|e| e.to_string())?;
        self.with_group(&gid, |g| g.add_versions(vs).map(|_| ()).map_err(|e| e.to_string()))??;
        self.emit_state();
        Ok(())
    }

    /// Applique des opérations reçues et en tire les conséquences (accès du tunnel, départ forcé, bail annulé).
    fn apply_ops(&self, gid: GroupId, ops: &[SignedOp]) -> Result<(), String> {
        if ops.is_empty() {
            return Ok(());
        }
        let (before, after) = self
            .with_group(&gid, |g| {
                let before = g.state().clone();
                g.append_ops(ops).map(|_| (before, g.state().clone())).map_err(|e| e.to_string())
            })??;
        self.after_change(&before, &after);
        if ops.iter().any(|o| matches!(o.op, GroupOp::Leave)) && before.members != after.members {
            self.after_leave(&after);
        }
        Ok(())
    }

    /// Un membre est parti : la clé du groupe change. Un seul membre s'en charge (le plus petit identifiant parmi ceux qui
    /// restent), pour que deux membres ne génèrent pas deux clés différentes.
    fn after_leave(&self, after: &GroupState) {
        if after.members.iter().min() == Some(&self.me) {
            let (rt, gid) = (self.arc(), after.id);
            spawn(async move { rt.rotate_key(gid).await });
        }
    }

    fn after_change(&self, before: &GroupState, after: &GroupState) {
        let gid = after.id;
        if self.active(&gid).is_some() {
            self.access.allow_members(gid, after.members.iter().copied().filter(|k| *k != self.me));
            for gone in before.members.difference(&after.members) {
                self.access.revoke_member(&gid, gone);
            }
        }
        for c in after.continuations.iter().skip(before.continuations.len()) {
            lock(&self.leases).revoke(&gid, &c.absent);
        }
        if !after.is_member(&self.me) {
            // Retiré du groupe : on l'oublie ici (le monde copié reste dans les sauvegardes de l'utilisateur).
            let _ = self.forget_group(&gid);
        }
        self.emit_state();
    }

    /// Cherche, chez les membres en ligne, les mondes des têtes qu'on n'a pas en entier (synchro de fond).
    pub async fn sync_heads(self: &Arc<Self>, gid: GroupId, prefer: Option<PeerKey>) {
        if !self.enabled() {
            return;
        }
        if self.active(&gid).is_some() || lock(&self.fetching).contains_key(&gid) {
            return;
        }
        let Some(heads) = self.heads_of(&gid) else { return };
        for v in heads {
            if let Err(e) = self.fetch_version(gid, &v, prefer).await {
                eprintln!("[cubelink] synchro du groupe {gid} : {e}");
                break;
            }
        }
    }

    /// Rapatrie le monde d'une version (sans l'écrire sur le disque de jeu). Sans effet s'il est déjà là.
    pub async fn fetch_version(self: &Arc<Self>, gid: GroupId, v: &SignedVersion, prefer: Option<PeerKey>) -> Result<(), String> {
        self.fetch_version_as(gid, v, prefer, false).await
    }

    /// Comme [`HearthRuntime::fetch_version`], pour une action explicite de la personne (Jouer) : une version dont la
    /// taille dépasse le seuil de croissance, déjà signalée par un premier refus, est alors téléchargée. L'espace
    /// disque, lui, n'est jamais contourné.
    pub async fn fetch_version_user(self: &Arc<Self>, gid: GroupId, v: &SignedVersion, prefer: Option<PeerKey>) -> Result<(), String> {
        let confirmed = lock(&self.oversize).contains(&v.id());
        self.fetch_version_as(gid, v, prefer, confirmed).await
    }

    async fn fetch_version_as(self: &Arc<Self>, gid: GroupId, v: &SignedVersion, prefer: Option<PeerKey>, confirmed: bool) -> Result<(), String> {
        let coll = v.header.collection;
        if lock(&self.complete).contains(&coll) {
            return Ok(());
        }
        let world = self.world()?.clone();
        if world.is_complete(&coll).await {
            let _ = world.adopt(&gid, &coll).await;
            lock(&self.complete).insert(coll);
            return Ok(());
        }
        let addrs = self.online_addrs(&gid, prefer);
        if addrs.is_empty() {
            return Err("aucun membre du groupe n'est joignable pour récupérer le monde".into());
        }
        {
            let mut f = lock(&self.fetching);
            if f.contains_key(&gid) {
                return Err("une mise à jour du monde est déjà en cours".into());
            }
            f.insert(gid, 0);
        }
        self.emit_state();
        let (rt, total) = (self.clone(), v.header.total_size.max(1));
        let on_progress: Arc<dyn Fn(u64) + Send + Sync> = Arc::new(move |bytes| {
            let pct = (bytes.saturating_mul(100) / total).min(99) as u8;
            let changed = {
                let mut f = lock(&rt.fetching);
                match f.get_mut(&gid) {
                    Some(p) if *p != pct => {
                        *p = pct;
                        true
                    }
                    _ => false,
                }
            };
            if changed {
                rt.emit_state();
            }
        });
        let net = self.net()?.clone();
        // Garde-fous avant le premier octet : une grande `total_size` ne coûte à l'émetteur que sa signature.
        let previous_total = v.header.parent.and_then(|p| self.version(&gid, &p)).map(|p| p.header.total_size);
        let opts = FetchOptions {
            on_progress: Some(on_progress),
            previous_total,
            confirmed,
            free_bytes: free_space(&self.root),
            ..FetchOptions::default()
        };
        let res = world.fetch(&net.endpoint, &gid, &addrs, &coll, v.header.total_size, &opts).await;
        lock(&self.fetching).remove(&gid);
        let out = match res {
            Ok(_) => {
                lock(&self.complete).insert(coll);
                lock(&self.oversize).remove(&v.id());
                Ok(())
            }
            Err(SyncError::Oversize { expected, previous }) => {
                lock(&self.oversize).insert(v.id());
                Err(format!(
                    "La partie de {} fait {} contre {} avant. Elle n'est pas téléchargée toute seule : relance Jouer si tu veux la récupérer quand même.",
                    self.name_of(&v.header.author),
                    human_size(expected),
                    human_size(previous),
                ))
            }
            Err(e) => Err(e.to_string()),
        };
        self.emit_state();
        out
    }

    /// Fait le point sur ce qu'on a déjà en entier (démarrage).
    async fn scan_complete(self: &Arc<Self>) {
        let Ok(world) = self.world() else { return };
        for gid in self.group_ids() {
            let colls: Vec<[u8; 32]> =
                self.with_group(&gid, |g| g.graph().iter().map(|(_, v)| v.header.collection).collect()).unwrap_or_default();
            for c in colls {
                if world.is_complete(&c).await {
                    lock(&self.complete).insert(c);
                }
            }
        }
        self.emit_state();
    }

    fn start_background(self: &Arc<Self>) {
        let rt = self.clone();
        let probe = spawn(async move {
            rt.scan_complete().await;
            loop {
                rt.probe_all().await;
                tokio::time::sleep(PROBE_INTERVAL).await;
            }
        });
        lock(&self.background).push(probe);
        let rt = self.clone();
        let reach: Arc<dyn Fn() -> Vec<Arc<dyn Replica>> + Send + Sync> = Arc::new(move || rt.reachable_replicas());
        *lock(&self.drain) = Some(self.outbox.spawn_drain_loop(reach, Duration::from_secs(30)));
    }

    /// Les membres joignables, comme cibles de la file d'envoi.
    fn reachable_replicas(&self) -> Vec<Arc<dyn Replica>> {
        if !self.enabled() {
            return Vec::new();
        }
        let Ok(net) = self.net() else { return Vec::new() };
        let mut by_key: BTreeMap<PeerKey, BTreeSet<GroupId>> = BTreeMap::new();
        for gid in self.group_ids() {
            let Some(st) = self.state(&gid) else { continue };
            for k in self.others(&st).into_iter().filter(|k| self.is_online(&gid, k)) {
                by_key.entry(k).or_default().insert(gid);
            }
        }
        by_key
            .into_iter()
            .filter_map(|(k, groups)| {
                let addr = (net.addr_of)(&k)?;
                Some(Arc::new(MemberReplica::new(net.endpoint.clone(), addr, groups)) as Arc<dyn Replica>)
            })
            .collect()
    }

    // --- têtes et versions ------------------------------------------------------------------------------

    /// Têtes de la lignée (un monde par tête distincte).
    pub fn heads_of(&self, gid: &GroupId) -> Option<Vec<SignedVersion>> {
        self.with_group(gid, |g| effective_heads(g.graph(), g.state())).ok()
    }

    /// La version actuelle : la tête unique. `None` : aucune version, ou fourche.
    pub fn head(&self, gid: &GroupId) -> Option<SignedVersion> {
        let mut h = self.heads_of(gid)?;
        (h.len() == 1).then(|| h.remove(0))
    }

    pub fn is_fork(&self, gid: &GroupId) -> bool {
        self.heads_of(gid).is_some_and(|h| h.len() > 1)
    }

    pub fn version(&self, gid: &GroupId, id: &VersionId) -> Option<SignedVersion> {
        self.with_group(gid, |g| g.graph().get(id).cloned()).ok().flatten()
    }

    /// Publie le contenu de `dir` comme nouvelle version du groupe.
    pub async fn publish_version(
        self: &Arc<Self>,
        gid: GroupId,
        dir: &Path,
        kind: VersionKind,
        parent: Option<(VersionId, u64)>,
        played_secs: u64,
    ) -> Result<SignedVersion, String> {
        let pw: PublishedWorld = self.world()?.publish_dir(&gid, dir).await.map_err(|e| e.to_string())?;
        self.record_version(gid, kind, parent, played_secs, pw.collection, pw.total_size)
    }

    /// Signe et enregistre une version dont le contenu est déjà dans le magasin, puis la met en file d'envoi.
    pub fn record_version(
        &self,
        gid: GroupId,
        kind: VersionKind,
        parent: Option<(VersionId, u64)>,
        played_secs: u64,
        collection: [u8; 32],
        total_size: u64,
    ) -> Result<SignedVersion, String> {
        let state = self.require_state(&gid)?;
        let header = VersionHeader {
            group_id: gid,
            number: parent.map_or(1, |p| p.1 + 1),
            parent: parent.map(|p| p.0),
            epoch: if parent.is_some() { state.epoch } else { 0 },
            author: self.me,
            pack_hash: state.pack.manifest_hash.clone(),
            created_at: now_secs().max(0) as u64,
            played_secs,
            kind,
            collection,
            total_size,
        };
        let v = SignedVersion::sign(&*self.identity, header).map_err(|e| e.to_string())?;
        self.with_group(&gid, |g| g.add_version(v.clone()).map(|_| ()).map_err(|e| e.to_string()))??;
        lock(&self.complete).insert(collection);
        self.outbox.enqueue(v.clone()).map_err(|e| e.to_string())?;
        self.outbox.wake();
        self.emit_state();
        Ok(v)
    }

    /// Restaure `from` : une nouvelle version, enfant de la tête, qui reprend son contenu. Rien n'est supprimé.
    pub async fn restore(self: &Arc<Self>, gid: GroupId, from: VersionId) -> Result<SignedVersion, String> {
        if self.active(&gid).is_some() {
            return Err("arrête le serveur avant de restaurer une version".into());
        }
        let src = self.version(&gid, &from).ok_or("version introuvable")?;
        let head = self.head(&gid).ok_or("deux versions existent : choisis d'abord laquelle garder")?;
        if head.id() == from || head.header.collection == src.header.collection {
            return Err("c'est déjà la version actuelle".into());
        }
        self.fetch_version_user(gid, &src, None).await?;
        self.record_version(
            gid,
            VersionKind::Restore { from },
            Some((head.id(), head.header.number)),
            0,
            src.header.collection,
            src.header.total_size,
        )
    }

    /// Fourche : garde la version `keep`. Chaque autre tête reçoit une version `Restore` de `keep`.
    pub async fn resolve_fork(self: &Arc<Self>, gid: GroupId, keep: VersionId) -> Result<(), String> {
        let kept = self.version(&gid, &keep).ok_or("version introuvable")?;
        // Les têtes encore en jeu : celles qu'une résolution précédente a absorbées ne reçoivent pas de nouvelle `Restore`.
        let open: Vec<SignedVersion> = self.with_group(&gid, |g| g.graph().open_heads(g.state()).into_iter().cloned().collect())?;
        if !open.iter().any(|h| h.header.collection == kept.header.collection) {
            return Err("cette version n'est pas l'une des deux en jeu".into());
        }
        self.fetch_version_user(gid, &kept, None).await?;
        for h in open.into_iter().filter(|h| h.header.collection != kept.header.collection) {
            self.record_version(
                gid,
                VersionKind::Restore { from: keep },
                Some((h.id(), h.header.number)),
                0,
                kept.header.collection,
                kept.header.total_size,
            )?;
        }
        Ok(())
    }

    /// Libère les mondes des vieilles versions (gardées : tête, parent, moins de 14 jours, file d'envoi).
    pub async fn gc_versions(&self, gid: GroupId) {
        let Ok(world) = self.world() else { return };
        let now = now_secs().max(0) as u64;
        let mut keep: HashSet<[u8; 32]> = self.outbox.pinned_collections().into_iter().map(|(_, c)| c).collect();
        let all: Vec<SignedVersion> = self.with_group(&gid, |g| g.graph().iter().map(|(_, v)| v.clone()).collect()).unwrap_or_default();
        let mut keep_ids: HashSet<VersionId> = HashSet::new();
        for h in self.heads_of(&gid).unwrap_or_default() {
            keep_ids.insert(h.id());
            keep_ids.extend(h.header.parent);
        }
        keep_ids.extend(self.materialized(&gid));
        for v in &all {
            if keep_ids.contains(&v.id()) || v.header.created_at + KEEP_VERSIONS_SECS > now {
                keep.insert(v.header.collection);
            }
        }
        let mut released = HashSet::new();
        for v in &all {
            let c = v.header.collection;
            if !keep.contains(&c) && released.insert(c) && lock(&self.complete).remove(&c) {
                let _ = world.release(&gid, &c).await;
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------------------
// Protocole : ce que les autres membres nous demandent
// ---------------------------------------------------------------------------------------------------------

impl HearthDelegate for HearthRuntime {
    fn is_friend(&self, key: &PeerKey) -> bool {
        self.enabled() && self.friends.is_friend(&key_hex(key)).unwrap_or(false)
    }

    fn group_state(&self, group: &GroupId) -> Option<GroupState> {
        // Interrupteur coupé : aucun groupe n'est visible des pairs, donc tout est refusé (`Deny`).
        if !self.enabled() {
            return None;
        }
        self.state(group)
    }

    fn ops_after(&self, group: &GroupId, after: u64, limit: usize) -> Vec<SignedOp> {
        self.with_group(group, |g| g.ops_after(after, limit)).unwrap_or_default()
    }

    fn versions_after(&self, group: &GroupId, after_number: u64, limit: usize) -> Vec<SignedVersion> {
        self.with_group(group, |g| g.versions_after(after_number, limit)).unwrap_or_default()
    }

    fn heads(&self, group: &GroupId) -> Vec<VersionId> {
        self.with_group(group, |g| g.heads()).unwrap_or_default()
    }

    fn lease(&self, group: &GroupId) -> Option<Lease> {
        // Un serveur prêt, sinon un serveur en préparation : un membre qui nous sonde sait qu'il y a une partie.
        self.active(group)
            .map(|a| lock(&a.lease).clone())
            .or_else(|| lock(&self.starting).get(group).map(|s| lock(&s.lease).clone()))
    }

    fn have_version(&self, group: &GroupId, version: &VersionId) -> bool {
        let coll = self.with_group(group, |g| g.graph().get(version).map(|v| v.header.collection)).ok().flatten();
        coll.is_some_and(|c| lock(&self.complete).contains(&c))
    }

    fn on_status(&self, from: PeerKey, group: &GroupId, op_seq: u64, heads: &[VersionId]) {
        let latest = self
            .with_group(group, |g| heads.iter().filter_map(|h| g.graph().get(h)).map(|v| v.header.point()).max())
            .ok()
            .flatten();
        lock(&self.status).insert((*group, from), MemberStatus { online: true, op_seq, latest });
        self.mark_seen(group, &from);
        let (mine, known) = self
            .with_group(group, |g| (g.state().op_seq, heads.iter().all(|h| g.graph().contains(h))))
            .unwrap_or((0, true));
        if op_seq > mine || !known {
            let (rt, gid) = (self.arc(), *group);
            spawn(async move {
                let _ = rt.pull_from(gid, from).await;
                rt.sync_heads(gid, Some(from)).await;
            });
        }
        self.emit_state();
    }

    fn on_lease(&self, from: PeerKey, lease: Lease) {
        let gid = lease.group_id;
        let base = lease.base_version;
        let r = self.with_group(&gid, |g| {
            let unknown_base = base.is_some_and(|b| !g.graph().contains(&b));
            (lock(&self.leases).observe(g.state(), g.graph(), &from, lease, Instant::now()), unknown_base)
        });
        let Ok((r, unknown_base)) = r else { return };
        if unknown_base {
            // Une base inconnue est d'abord synchronisée (elle ne donne aucun avantage en attendant).
            let rt = self.arc();
            spawn(async move {
                let _ = rt.pull_from(gid, from).await;
                rt.sync_heads(gid, Some(from)).await;
            });
        }
        if matches!(r, Ok(Observed::Accepted)) {
            // Je suis moi-même en train d'héberger : si ce bail gagne et que personne ne joue sur mon serveur, je cède.
            if self.active(&gid).is_some_and(|a| a.players() == 0) && self.arbitrate_now(&gid).is_some() {
                eprintln!("[cubelink] un autre bail l'emporte sur celui du groupe {gid} : ce serveur s'arrête");
                let rt = self.arc();
                spawn(async move { rt.abort_hosting(gid).await });
            }
            self.emit_state();
        }
    }

    fn on_claim(&self, from: PeerKey, claim: Claim) -> Result<ClaimReply, String> {
        let gid = claim.group_id;
        // J'héberge déjà (serveur prêt ou en préparation) : mon bail est la réponse.
        if let Some(mine) = self.lease(&gid) {
            return Ok(ClaimReply::Conflict(ClaimConflict {
                kind: ConflictKind::Lease,
                holder: mine.holder,
                lease_id: mine.lease_id,
                base_version: mine.base_version,
                base_number: mine.base_number,
            }));
        }
        self.with_group(&gid, |g| lock(&self.leases).claim(g.state(), g.graph(), &from, &claim, Instant::now()))?.map_err(|e| e.to_string())
    }

    fn on_lease_release(
        &self,
        from: PeerKey,
        group: &GroupId,
        lease_id: LeaseId,
        last_seq: u64,
        _reason: ReleaseReason,
        version: Option<VersionId>,
    ) {
        lock(&self.leases).release_lease(group, &from, &lease_id, last_seq, Instant::now());
        let gid = *group;
        // `last_seq == 0` : un candidat qui renonce à son claim, aucune partie n'avait été annoncée.
        if let (Some(Play::Joined { holder }), Some(st)) = (self.play_of(&gid), self.state(&gid)) {
            if last_seq > 0 && holder == from && self.active(&gid).is_none() {
                self.emit_json(
                    "hearth://handover",
                    &core_lib::types::HearthHandover { group_id: gid.to_hex(), name: st.name.clone(), from_name: self.name_of(&from) },
                );
            }
        }
        if version.is_some() {
            let rt = self.arc();
            spawn(async move {
                let _ = rt.pull_from(gid, from).await;
                rt.sync_heads(gid, Some(from)).await;
            });
        }
        self.emit_state();
    }

    fn on_invite(&self, from: PeerKey, ops: Vec<SignedOp>, key: GroupKey) -> Result<(), String> {
        let state = p2p_lib::hearth::model::replay(&ops).map_err(|e| e.to_string())?;
        let gid = state.id;
        if self.group(&gid).is_some() {
            // Un membre nous renvoie le groupe : clé renouvelée (après le retrait d'un membre). L'émetteur doit être
            // membre dans NOTRE état courant : un ami qui n'en fait pas partie ne change pas la clé d'un groupe connu.
            let (before, after) = self.with_group(&gid, |g| {
                if !g.state().is_member(&from) {
                    return Err("l'émetteur n'est pas membre du groupe".to_string());
                }
                let before = g.state().clone();
                g.append_ops(&ops).map_err(|e| e.to_string())?;
                g.set_key(key).map_err(|e| e.to_string())?;
                Ok::<_, String>((before, g.state().clone()))
            })??;
            self.after_change(&before, &after);
            return Ok(());
        }
        let now = now_secs();
        {
            // Quota : 3 invitations par ami et par 24 h (celles qui sont écartées comptent aussi).
            let mut log = lock(&self.invite_log);
            log.retain(|(_, t)| now - *t < 24 * 3600);
            if log.iter().filter(|(k, _)| *k == from).count() >= MAX_INVITES_PER_FRIEND_PER_DAY {
                return Err("trop d'invitations de ce membre aujourd'hui".into());
            }
            log.push((from, now));
        }
        {
            let mut inv = lock(&self.invites);
            inv.retain(|i| now - i.received_at < INVITE_TTL_SECS);
            // Une seule invitation par groupe : la plus récente remplace l'ancienne.
            inv.retain(|i| i.state.id != gid);
            // File pleine : la plus ancienne est évincée.
            while inv.len() >= MAX_PENDING_INVITES {
                inv.remove(0);
            }
            inv.push(PendingInvite { from, ops, key, state, received_at: now });
        }
        self.emit_invites();
        Ok(())
    }

    fn on_proposal(&self, from: PeerKey, group: &GroupId, proposal: Proposal) -> Result<(), String> {
        let status = self.with_group(group, |g| g.put_proposal(proposal, &from).map_err(|e| e.to_string()))??;
        self.after_vote(*group, status.id, status.ready);
        Ok(())
    }

    fn on_vote(&self, from: PeerKey, group: &GroupId, proposal_id: OpHash, sig: Sig) -> Result<(), String> {
        let status = self.with_group(group, |g| g.add_vote(&proposal_id, from, sig).map_err(|e| e.to_string()))??;
        self.after_vote(*group, status.id, status.ready);
        Ok(())
    }

    fn on_version_offer(&self, from: PeerKey, version: SignedVersion) -> Result<(), String> {
        let gid = version.header.group_id;
        let report = self.with_group(&gid, |g| g.add_versions(vec![version.clone()]).map_err(|e| e.to_string()))??;
        if let Some((_, why)) = report.rejected.first() {
            // Parent inconnu : on rattrape la lignée auprès de l'émetteur avant de réessayer.
            let (rt, v) = (self.arc(), version);
            let unknown_parent = why.contains("parente inconnue");
            if !unknown_parent {
                return Err(why.clone());
            }
            spawn(async move {
                if rt.pull_from(gid, from).await.is_ok() && rt.version(&gid, &v.id()).is_some() {
                    let _ = rt.fetch_version(gid, &v, Some(from)).await;
                }
            });
            return Ok(());
        }
        let rt = self.arc();
        spawn(async move {
            let _ = rt.fetch_version(gid, &version, Some(from)).await;
        });
        self.emit_state();
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------------------
// Groupes : création, invitations, gouvernance
// ---------------------------------------------------------------------------------------------------------

impl HearthRuntime {
    fn emit_invites(&self) {
        self.emit_json("hearth://invites", &self.invites());
    }

    pub fn invites(&self) -> Vec<HearthInvite> {
        let now = now_secs();
        let mut pending = lock(&self.invites);
        pending.retain(|i| now - i.received_at < INVITE_TTL_SECS);
        let list: Vec<HearthInvite> = pending
            .iter()
            .map(|i| HearthInvite {
                group_id: i.state.id.to_hex(),
                name: i.state.name.clone(),
                from: HearthMemberLite { public_key: key_hex(&i.from), name: self.name_of(&i.from), online: true },
                members: i.state.members.len() as u32,
                minecraft: i.state.pack.minecraft.clone(),
                loader: format!("{:?}", i.state.pack.loader.loader).to_lowercase(),
                received_at: i.received_at,
            })
            .collect();
        list
    }

    /// L'utilisateur accepte l'invitation : le groupe est créé ici avec la clé reçue.
    pub fn accept_invite(self: &Arc<Self>, gid: &GroupId) -> Result<(), String> {
        let inv = {
            let mut l = lock(&self.invites);
            l.retain(|i| now_secs() - i.received_at < INVITE_TTL_SECS);
            let i = l.iter().position(|i| i.state.id == *gid).ok_or("cette invitation n'existe plus")?;
            l.remove(i)
        };
        let group = self.store.import(&inv.ops, inv.key).map_err(|e| e.to_string())?;
        lock(&self.groups).insert(*gid, Arc::new(Mutex::new(group)));
        self.emit_invites();
        self.emit_state();
        let rt = self.clone();
        let (gid, from) = (*gid, inv.from);
        spawn(async move {
            rt.probe_group(gid).await;
            rt.sync_heads(gid, Some(from)).await;
        });
        Ok(())
    }

    pub fn dismiss_invite(&self, gid: &GroupId) {
        lock(&self.invites).retain(|i| i.state.id != *gid);
        self.emit_invites();
    }

    /// Crée le groupe (opération `Create` signée, clé de groupe aléatoire) et prévoit les invitations.
    pub fn create_group(self: &Arc<Self>, name: &str, pack: PackPin, others: &[PeerKey], instance_id: &str) -> Result<GroupId, String> {
        let gid = GroupId::random();
        let set: BTreeSet<PeerKey> = others.iter().copied().collect();
        let op = SignedOp::create(&*self.identity, gid, name, pack, &set).map_err(|e| e.to_string())?;
        let group = self.store.create(&op, GroupKey::random()).map_err(|e| e.to_string())?;
        lock(&self.groups).insert(gid, Arc::new(Mutex::new(group)));
        self.update_meta(&gid, |m| {
            m.instance_id = Some(instance_id.to_string());
            m.invite_pending = set.iter().map(key_hex).collect();
        });
        self.emit_state();
        Ok(gid)
    }

    /// Envoie les invitations qui n'ont pas encore été livrées (les amis hors ligne la recevront plus tard).
    pub async fn send_pending_invites(self: &Arc<Self>, gid: GroupId) -> Vec<PeerKey> {
        let pending: Vec<PeerKey> = self.meta_of(&gid).invite_pending.iter().filter_map(|k| parse_key(k)).collect();
        let mut failed = Vec::new();
        for k in pending {
            match tokio::time::timeout(Duration::from_secs(40), self.send_group_invite(gid, k)).await {
                Ok(Ok(())) => {
                    self.update_meta(&gid, |m| {
                        m.invite_pending.remove(&key_hex(&k));
                    });
                }
                _ => failed.push(k),
            }
        }
        failed
    }

    async fn send_group_invite(&self, gid: GroupId, key: PeerKey) -> Result<(), String> {
        let (ops, gk) = self.with_group(&gid, |g| (g.ops().to_vec(), g.key().clone()))?;
        let conn = self.connect(&key).await?;
        conn.send_invite(&ops, &gk).await.map_err(|e| e.to_string())
    }

    /// Oublie le groupe sur ce PC (départ, retrait). L'état en mémoire disparaît tout de suite ; puis, dans l'ordre : le
    /// serveur actif est arrêté (tué après un délai), l'accès du groupe est fermé, les collections du groupe sont libérées
    /// dans le magasin de blobs, enfin les données du groupe sont supprimées du disque.
    pub fn forget_group(&self, gid: &GroupId) -> Result<(), String> {
        let rt = self.arc();
        let tail = self.forget_now(gid);
        let gid = *gid;
        spawn(async move {
            if let Err(e) = rt.forget_tail(gid, tail).await {
                eprintln!("[cubelink] groupe {gid} : suppression incomplète : {e}");
            }
        });
        Ok(())
    }

    /// Comme [`Self::forget_group`], en attendant la fin du ménage (arrêt du serveur, blobs, disque).
    pub async fn forget_group_wait(self: &Arc<Self>, gid: &GroupId) -> Result<(), String> {
        let tail = self.forget_now(gid);
        self.forget_tail(*gid, tail).await
    }

    /// Partie synchrone : retire l'état du groupe de la mémoire et rend ce qu'il reste à nettoyer.
    fn forget_now(&self, gid: &GroupId) -> ForgetTail {
        let collections: HashSet<[u8; 32]> = self
            .with_group(gid, |g| g.graph().iter().map(|(_, v)| v.header.collection).collect())
            .unwrap_or_default();
        let host = lock(&self.active).remove(gid);
        if let Some(h) = &host {
            h.stopping.store(true, Ordering::SeqCst);
            for t in std::mem::take(&mut *lock(&h.tasks)) {
                t.abort();
            }
        }
        let starting = self.take_starting_entry(gid);
        // Plus de membre : l'accès est fermé tout de suite, avant même l'arrêt du serveur.
        self.access.close_group(gid);
        lock(&self.groups).remove(gid);
        lock(&self.leases).forget_group(gid);
        lock(&self.status).retain(|(g, _), _| g != gid);
        lock(&self.play).remove(gid);
        lock(&self.best_effort).remove(gid);
        for e in self.outbox.pending().into_iter().filter(|e| e.group_id() == *gid) {
            let _ = self.outbox.remove(&e.id());
        }
        lock(&self.meta).groups.remove(&gid.to_hex());
        self.save_meta();
        self.emit_state();
        ForgetTail { host, starting, collections }
    }

    async fn forget_tail(self: &Arc<Self>, gid: GroupId, tail: ForgetTail) -> Result<(), String> {
        if let Some(h) = &tail.host {
            // `stop` tue le processus si l'arrêt propre dépasse le délai.
            h.server.stop(FORGET_STOP_GRACE).await;
        }
        let _ = tail.starting;
        if tail.host.is_some() {
            // Le partage du tunnel et l'état de lancement se ferment comme à la fin d'une partie.
            let hook = lock(&self.end_hook).clone();
            if let Some(hook) = hook {
                hook(gid).await;
            }
        }
        self.access.close_group(&gid);
        lock(&self.play).remove(&gid);
        if let Ok(world) = self.world() {
            for c in &tail.collections {
                lock(&self.complete).remove(c);
                let _ = world.release(&gid, c).await;
            }
        }
        let r = self.store.delete(&gid).map_err(|e| e.to_string());
        self.emit_state();
        r
    }

    /// Propose une opération (ou l'applique tout de suite si ma voix suffit).
    pub async fn propose(self: &Arc<Self>, gid: GroupId, op: GroupOp) -> Result<(), String> {
        let leaving = matches!(op, GroupOp::Leave);
        self.check_continue_vote(&gid, &op)?;
        let (p, status) = self.with_group(&gid, |g| {
            g.check_continue_base(&op, true).map_err(|e| e.to_string())?;
            let mut p = Proposal::new(g.state(), op).map_err(|e| e.to_string())?;
            p.vote(g.state(), &*self.identity).map_err(|e| e.to_string())?;
            let status = g.put_proposal(p.clone(), &self.me).map_err(|e| e.to_string())?;
            Ok::<_, String>((p, status))
        })??;
        let visit: Visit = Arc::new({
            let p = p.clone();
            move |c| {
                let p = p.clone();
                Box::pin(async move { c.propose(&gid, &p).await })
            }
        });
        if status.ready {
            if leaving {
                // Mon départ doit parvenir à au moins un membre AVANT que j'oublie le groupe : une fois parti, je ne
                // peux plus servir la chaîne, et les autres me compteraient pour toujours dans leurs quorums.
                let others = self.state(&gid).map_or(0, |s| self.others(&s).len());
                if others > 0 && self.broadcast(gid, false, visit).await == 0 {
                    return Err("Personne du groupe n'a pu être joint : ton départ n'a pas pu leur être annoncé. Réessaie quand un ami est connecté.".into());
                }
            }
            return self.finalize(gid, status.id).await;
        }
        self.broadcast(gid, true, visit).await;
        self.emit_state();
        Ok(())
    }

    /// Propose un nouveau modpack pour le groupe. L'instance `instance_id` ne devient l'instance liée qu'une fois
    /// l'opération `PackUpdate` appliquée (tout de suite si ma voix suffit, sinon au vote qui atteint le quota).
    pub async fn propose_pack_update(self: &Arc<Self>, gid: GroupId, pack: PackPin, instance_id: &str) -> Result<(), String> {
        let hash = pack.manifest_hash.clone();
        self.update_meta(&gid, |m| m.pending_pack = Some((hash.clone(), instance_id.to_string())));
        let r = self.propose(gid, GroupOp::PackUpdate { pack }).await;
        if r.is_err() {
            self.update_meta(&gid, |m| {
                if m.pending_pack.as_ref().is_some_and(|(h, _)| *h == hash) {
                    m.pending_pack = None;
                }
            });
        }
        r
    }

    /// Règles locales avant de signer un `ContinueWithout` : le membre écarté ne doit pas avoir donné signe de vie depuis
    /// 7 jours (nous ne l'avons pas vu, il n'est pas en ligne), et l'heure de décision ne vient pas du futur.
    fn check_continue_vote(&self, gid: &GroupId, op: &GroupOp) -> Result<(), String> {
        if let GroupOp::SetGovernance { .. } = op {
            // Le créateur écarté est revenu : on ne lui prend pas la main (il peut aussi la céder lui-même).
            let creator = self.state(gid).map(|s| s.creator);
            return match creator {
                Some(c) if c != self.me && self.is_online(gid, &c) => Err(format!("{} est de retour : le groupe n'a plus à décider sans lui.", self.name_of(&c))),
                _ => Ok(()),
            };
        }
        let GroupOp::ContinueWithout { absent, at, .. } = op else { return Ok(()) };
        let name = self.name_of(absent);
        if self.is_online(gid, absent) {
            return Err(format!("{name} est en ligne : on ne continue pas sans lui."));
        }
        let now = now_secs();
        let seen = self.meta_of(gid).seen.get(&key_hex(absent)).copied();
        if seen.is_some_and(|t| now - t < CONTINUE_AFTER_DAYS as i64 * 86_400) {
            return Err(format!("{name} a donné signe de vie il y a moins de {CONTINUE_AFTER_DAYS} jours : on ne continue pas sans lui."));
        }
        if i64::try_from(*at).map_or(true, |a| a > now + 86_400) {
            return Err("la date de décision de cette proposition est dans le futur : l'horloge de son auteur est fausse".into());
        }
        Ok(())
    }

    /// Mon vote sur une proposition en attente.
    pub async fn vote(self: &Arc<Self>, gid: GroupId, id: OpHash) -> Result<(), String> {
        let (sig, status) = self.with_group(&gid, |g| {
            let p = g.proposal(&id).ok_or("cette proposition n'existe plus")?.clone();
            self.check_continue_vote(&gid, &p.op)?;
            g.check_continue_base(&p.op, true).map_err(|e| e.to_string())?;
            let sig = p.sign(&*self.identity);
            let status = g.add_vote(&id, self.me, sig).map_err(|e| e.to_string())?;
            Ok::<_, String>((sig, status))
        })??;
        if status.ready {
            return self.finalize(gid, id).await;
        }
        let visit: Visit = Arc::new(move |c| Box::pin(async move { c.vote(&gid, id, sig).await }));
        self.broadcast(gid, true, visit).await;
        self.emit_state();
        Ok(())
    }

    fn after_vote(&self, gid: GroupId, id: OpHash, ready: bool) {
        self.emit_state();
        if ready {
            let rt = self.arc();
            spawn(async move {
                let _ = rt.finalize(gid, id).await;
            });
        }
    }

    /// La proposition a son quota de voix : elle devient une opération de la chaîne.
    async fn finalize(self: &Arc<Self>, gid: GroupId, id: OpHash) -> Result<(), String> {
        let (op, before, after) = self.with_group(&gid, |g| {
            let before = g.state().clone();
            let op = match g.finalize(&id) {
                Ok(op) => op,
                // Un autre chemin (vote reçu en même temps) l'a déjà finalisée.
                Err(p2p_lib::hearth::store::StoreError::UnknownProposal) => return Ok(None),
                Err(e) => return Err(e.to_string()),
            };
            Ok::<_, String>(Some((op, before, g.state().clone())))
        })??
        .map_or((None, None, None), |(o, b, a)| (Some(o), Some(b), Some(a)));
        let (Some(op), Some(before), Some(after)) = (op, before, after) else { return Ok(()) };
        self.after_change(&before, &after);
        match &op.op {
            GroupOp::AddMember { key } => {
                self.update_meta(&gid, |m| {
                    m.invite_pending.insert(key_hex(key));
                });
                let rt = self.clone();
                spawn(async move {
                    rt.send_pending_invites(gid).await;
                });
            }
            GroupOp::RemoveMember { key } => {
                lock(&self.status).remove(&(gid, *key));
                self.access.revoke_member(&gid, key);
                self.rotate_key(gid).await;
            }
            GroupOp::Leave => self.after_leave(&after),
            GroupOp::PackUpdate { pack } => {
                let wanted = self.meta_of(&gid).pending_pack.filter(|(h, _)| *h == pack.manifest_hash);
                if let Some((_, instance)) = wanted {
                    self.update_meta(&gid, |m| m.pending_pack = None);
                    self.set_instance(&gid, Some(instance));
                }
            }
            _ => {}
        }
        // Les autres membres rattrapent la chaîne au prochain échange.
        let rt = self.clone();
        spawn(async move { rt.probe_group(gid).await });
        Ok(())
    }

    /// Nouvelle clé de groupe après le retrait d'un membre, transmise aux membres restants.
    async fn rotate_key(self: &Arc<Self>, gid: GroupId) {
        let Ok(new) = Ok::<_, ()>(GroupKey::random()) else { return };
        let Ok((ops, st)) = self.with_group(&gid, |g| {
            let _ = g.set_key(new.clone());
            (g.ops().to_vec(), g.state().clone())
        }) else {
            return;
        };
        for k in self.others(&st) {
            if let Ok(conn) = self.connect(&k).await {
                let _ = conn.send_invite(&ops, &new).await;
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------------------
// Cycle d'une partie : décision, serveur dédié, bail, points de reprise, arrêt
// ---------------------------------------------------------------------------------------------------------

/// Ce dont le runtime a besoin pour héberger.
#[derive(Clone)]
pub struct HostParams {
    pub instance_id: String,
    pub minecraft: String,
    pub loader: LoaderSpec,
    pub mods_src: PathBuf,
    pub ram_mb: u32,
    pub java_override: Option<PathBuf>,
    pub java_root: PathBuf,
}

impl HearthRuntime {
    /// La décision du bouton Jouer d'après le **cache** (sans réseau) : pour l'affichage et les choix qui n'agissent pas
    /// sur le monde. Avant de jouer, on décide sur une ronde fraîche : [`HearthRuntime::decision_for`].
    pub fn decision(&self, gid: &GroupId) -> Option<Decision> {
        let st = self.state(gid)?;
        let mut round = ProbeRound::default();
        for k in self.others(&st) {
            match lock(&self.status).get(&(*gid, k)).cloned() {
                Some(s) if s.online => round.answers.push(OnlineMember { key: k, latest: s.latest }),
                _ => round.unreachable.push(k),
            }
        }
        self.decision_for(gid, &round)
    }

    /// La décision du bouton Jouer d'après une ronde de sondage qui vient d'avoir lieu ([`HearthRuntime::probe_round`]) :
    /// les réponses et les injoignables, jamais le cache seul.
    pub fn decision_for(&self, gid: &GroupId, round: &ProbeRound) -> Option<Decision> {
        let (st, graph) = self.with_group(gid, |g| (g.state().clone(), g.graph().clone())).ok()?;
        let table = lock(&self.leases);
        Some(decide_from(&st, &graph, &table, &self.me, round, false, Instant::now()))
    }

    /// Bail valide de ce titulaire dans la table (diagnostic et tests).
    pub fn lease_of_holder(&self, gid: &GroupId, holder: &PeerKey) -> Option<Lease> {
        lock(&self.leases).lease_of(gid, holder, Instant::now())
    }

    /// Horloges (injectables pour les tests : veille simulée).
    pub fn set_clock(&self, c: Arc<dyn Clock>) {
        *lock(&self.clock) = c;
    }

    fn clock(&self) -> Arc<dyn Clock> {
        lock(&self.clock).clone()
    }

    /// Bail signé d'une nouvelle partie (`lease_id` imposé), à l'époque courante du groupe.
    fn new_signed_lease(&self, gid: GroupId, phase: LeasePhase, base: Option<(VersionId, u64)>, lease_id: LeaseId) -> Lease {
        let l = match phase {
            LeasePhase::Starting => Lease::starting(gid, self.me, base),
            LeasePhase::Ready => Lease::new(gid, self.me, base),
        };
        self.resign(l.with_lease_id(lease_id))
    }

    /// Signe de nouveau un bail modifié, à l'époque courante.
    fn resign(&self, l: Lease) -> Lease {
        let epoch = self.state(&l.group_id).map_or(l.epoch, |s| s.epoch);
        l.at_epoch(epoch).signed(&*self.identity)
    }

    /// Point (époque, numéro) d'une version du graphe local.
    fn point_of(&self, gid: &GroupId, v: Option<VersionId>) -> Option<VersionPoint> {
        let id = v?;
        self.with_group(gid, |g| g.graph().get(&id).map(|x| x.header.point())).ok().flatten()
    }

    /// Prépare le monde du groupe pour un serveur : la tête matérialisée dans `server/world`. Un nouveau groupe sans
    /// version n'a rien à écrire (le serveur crée le monde).
    pub async fn prepare_world(self: &Arc<Self>, gid: GroupId, head: Option<&SignedVersion>) -> Result<(), String> {
        let Some(h) = head else { return Ok(()) };
        let dest = self.server_dir(&gid).join("world");
        if self.materialized(&gid) == Some(h.id()) && dest.join("level.dat").is_file() {
            return Ok(());
        }
        self.fetch_version_user(gid, h, None).await?;
        let world = self.world()?;
        world.materialize(&h.header.collection, &dest).await.map_err(|e| e.to_string())?;
        self.set_materialized(&gid, Some(h.id()));
        Ok(())
    }

    /// Installe et démarre le serveur dédié du groupe. `base` : la version dont part la partie.
    pub async fn start_server(
        self: &Arc<Self>,
        gid: GroupId,
        p: HostParams,
        base: Option<(VersionId, u64)>,
    ) -> Result<Arc<ActiveHost>, String> {
        if let Some(a) = self.any_active() {
            return Err(if a.group == gid { "le serveur de ce groupe tourne déjà".into() } else { "un autre serveur de groupe tourne déjà sur ce PC".to_string() });
        }
        let players: Arc<Mutex<BTreeSet<String>>> = Arc::default();
        let (pl, rt) = (players.clone(), self.clone());
        let on_line: LineSink = Arc::new(move |line| {
            if let Some((joined, name)) = parse_player_event(line) {
                {
                    let mut set = lock(&pl);
                    if joined {
                        set.insert(name);
                    } else {
                        set.remove(&name);
                    }
                }
                rt.emit_state();
            }
        });
        let spec = ServerSpec {
            dir: self.server_dir(&gid),
            minecraft: p.minecraft,
            loader: p.loader,
            mods_src: p.mods_src,
            java_root: p.java_root,
            java_override: p.java_override,
            jar_cache: self.jar_cache(),
            ram_mb: p.ram_mb,
            accept_eula: self.eula_accepted(),
            on_line,
            progress: Arc::new(|_| {}),
        };
        let runner = lock(&self.runner).clone();
        let server = runner.start(spec).await?;
        // Le bail annoncé en préparation devient le bail du serveur prêt (même partie : `seq` continue).
        let starting = self.take_starting_entry(&gid);
        if starting.as_ref().is_some_and(|e| e.fenced.load(Ordering::SeqCst)) {
            // Exclu après une veille pendant la préparation : un autre bail ou une version plus récente existe.
            server.stop(Duration::from_secs(20)).await;
            self.emit_state();
            return Err("Un autre membre a pris la main pendant la veille de ce PC : le serveur n'est pas lancé. Réessaie dans un instant.".into());
        }
        if self.state(&gid).is_none() {
            // Groupe supprimé pendant la préparation : pas de serveur orphelin.
            server.stop(Duration::from_secs(20)).await;
            return Err("Le groupe a été supprimé pendant le démarrage du serveur.".into());
        }
        let lease = match starting {
            // Même partie (même `lease_id`, `seq` continue), signée de nouveau en phase `Ready`.
            Some(e) => self.resign(lock(&e.lease).ready()),
            None => self.new_signed_lease(gid, LeasePhase::Ready, base, new_lease_id()),
        };
        let host = Arc::new(ActiveHost {
            group: gid,
            instance_id: p.instance_id,
            server,
            started: Instant::now(),
            tip: Mutex::new(base),
            lease: Mutex::new(lease),
            fence: Mutex::new(HostFence::new(&*self.clock())),
            fenced: AtomicBool::new(false),
            players,
            tasks: Mutex::new(Vec::new()),
            stopping: AtomicBool::new(false),
        });
        lock(&self.active).insert(gid, host.clone());
        Ok(host)
    }

    /// Envoie `lease` aux membres en ligne.
    async fn broadcast_lease(self: &Arc<Self>, gid: GroupId, lease: Lease) {
        let visit: Visit = Arc::new(move |c| {
            let l = lease.clone();
            Box::pin(async move { c.send_lease(&l).await })
        });
        self.broadcast(gid, true, visit).await;
    }

    /// Mon bail du moment (serveur prêt, sinon serveur en préparation) face aux baux des autres membres : `Some(holder)` si
    /// un autre bail l'emporte (phase, puis ascendance des versions de départ, puis plus petite clé).
    fn arbitrate_now(&self, gid: &GroupId) -> Option<PeerKey> {
        let mine = self
            .active(gid)
            .map(|a| lock(&a.lease).clone())
            .or_else(|| lock(&self.starting).get(gid).map(|s| lock(&s.lease).clone()))?;
        let others = lock(&self.leases).active(gid, Instant::now());
        let (verdict, fork) = self.with_group(gid, |g| arbitrate_marked(&mine, &others, g.graph())).ok()?;
        if fork {
            eprintln!("[cubelink] le groupe {gid} a deux parties sur des branches différentes : la fourche sera visible");
        }
        match verdict {
            Arbitration::Yield { to } => Some(to),
            Arbitration::Keep => None,
        }
    }

    /// Phase 1 de la prise du bail : un `LeaseClaim` en parallèle à tous les membres (5 s chacun, relais compris). Rend
    /// le nombre d'acquittements (moi exclu), les conflits reçus et les membres qui n'ont pas répondu.
    async fn claim_round(self: &Arc<Self>, gid: GroupId, claim: &Claim) -> (usize, Vec<ClaimConflict>, Vec<PeerKey>) {
        let Some(st) = self.state(&gid) else { return (0, Vec::new(), Vec::new()) };
        let mut set = tokio::task::JoinSet::new();
        for k in self.others(&st) {
            let (rt, claim) = (self.clone(), claim.clone());
            set.spawn(async move {
                let r = async {
                    let conn = tokio::time::timeout(CLAIM_TIMEOUT, rt.connect(&k)).await.ok()?.ok()?;
                    tokio::time::timeout(CLAIM_TIMEOUT, conn.claim_lease(&claim)).await.ok()?.ok()
                }
                .await;
                (k, r)
            });
        }
        let (mut acks, mut conflicts, mut silent) = (0, Vec::new(), Vec::new());
        while let Some(r) = set.join_next().await {
            match r {
                Ok((_, Some(ClaimReply::Ack))) => acks += 1,
                Ok((_, Some(ClaimReply::Conflict(c)))) => conflicts.push(c),
                Ok((k, None)) => silent.push(k),
                Err(_) => {}
            }
        }
        (acks, conflicts, silent)
    }

    /// Un premier « Jouer » a-t-il reçu un bail « au mieux » il y a moins de [`BEST_EFFORT_CONFIRM`] ? (Le second clic vaut
    /// confirmation ; la confirmation est consommée.)
    fn take_best_effort_confirmation(&self, gid: &GroupId) -> bool {
        lock(&self.best_effort).remove(gid).is_some_and(|t| t.elapsed() < BEST_EFFORT_CONFIRM)
    }

    /// Un bail « au mieux » attend la confirmation de la personne (un second « Jouer »).
    pub fn best_effort_pending(&self, gid: &GroupId) -> bool {
        lock(&self.best_effort).get(gid).is_some_and(|t| t.elapsed() < BEST_EFFORT_CONFIRM)
    }

    /// Un concurrent a la main : on cède. Le verrou du claim tombe ici et chez les témoins.
    async fn cede_claim(self: &Arc<Self>, gid: GroupId, lease_id: LeaseId) {
        lock(&self.leases).unlock_claim(&gid, &lease_id);
        let visit: Visit =
            Arc::new(move |c| Box::pin(async move { c.release_lease(&gid, lease_id, 0, ReleaseReason::Handover, None).await }));
        self.broadcast(gid, true, visit).await;
    }

    /// Premier temps de la prise du bail, en deux phases :
    ///
    /// 1. **Claim** : `LeaseClaim` à tous les membres joignables ; majorité stricte des membres (moi compris) = bail
    ///    **sûr**, sinon **au mieux** : la première fois, une erreur dédiée ([`HearthRuntime::best_effort_pending`]) demande
    ///    la confirmation (un second « Jouer » vaut confirmation). Un `ClaimConflict` fait céder : on rejoint le bail
    ///    existant, ou on attend l'annonce du concurrent.
    /// 2. **Annonce** d'un bail `Starting` aux membres en ligne dès qu'on a choisi d'héberger (l'installation et le démarrage
    ///    d'un gros pack prennent des minutes), attente d'une contre-annonce, puis renouvellement tant que le serveur n'est
    ///    pas prêt.
    pub async fn begin_starting(self: &Arc<Self>, gid: GroupId, base: Option<(VersionId, u64)>) -> Result<Begin, String> {
        let confirmed = self.take_best_effort_confirmation(&gid);
        let lease = self.new_signed_lease(gid, LeasePhase::Starting, base, new_lease_id());
        let claim = Claim::of(&lease);
        let members = self.state(&gid).map_or(1, |s| s.members.len());
        let graph = self.with_group(&gid, |g| g.graph().clone())?;
        let own = lock(&self.leases).lock_own_claim(&graph, &claim, Instant::now());
        let outcome = match own {
            Some(c) => ClaimOutcome::Conflict(c),
            None => {
                let (acks, conflicts, _silent) = self.claim_round(gid, &claim).await;
                claim_outcome(&claim, members, acks, &conflicts, &graph)
            }
        };
        match outcome {
            ClaimOutcome::Conflict(c) => {
                self.cede_claim(gid, claim.lease_id).await;
                return self.yield_to(gid, c).await;
            }
            ClaimOutcome::BestEffort { acks, needed, members } if !confirmed => {
                self.cede_claim(gid, claim.lease_id).await;
                lock(&self.best_effort).insert(gid, Instant::now());
                self.emit_state();
                return Err(format!(
                    "{BEST_EFFORT_NOTE} ({acks} membre(s) sur {members} joignable(s), {needed} nécessaires pour une prise sûre.) Clique encore sur Jouer pour héberger quand même."
                ));
            }
            ClaimOutcome::BestEffort { .. } | ClaimOutcome::Safe { .. } => {}
        }
        let entry = Arc::new(StartingHost {
            lease: Mutex::new(lease.clone()),
            task: Mutex::new(None),
            fence: Mutex::new(HostFence::new(&*self.clock())),
            fenced: AtomicBool::new(false),
        });
        lock(&self.starting).insert(gid, entry.clone());
        self.broadcast_lease(gid, lease).await;
        // Mon bail est annoncé : il protège désormais lui-même, le verrou de mon claim n'a plus lieu d'être.
        lock(&self.leases).unlock_claim(&gid, &claim.lease_id);
        tokio::time::sleep(CONFLICT_WAIT).await;
        if let Some(to) = self.arbitrate_now(&gid) {
            self.end_starting(gid).await;
            return Ok(Begin::Join(to));
        }
        let (rt, e) = (self.clone(), entry.clone());
        let task = spawn(async move {
            loop {
                tokio::time::sleep(LEASE_RENEW_INTERVAL).await;
                if !rt.renew_starting(gid, &e).await {
                    break;
                }
            }
        });
        *lock(&entry.task) = Some(task);
        self.emit_state();
        Ok(Begin::Hosting)
    }

    /// Un `ClaimConflict` : un bail déjà connu se rejoint ; un claim concurrent (il annonce son bail `Starting` dans
    /// l'instant) s'attend quelques secondes, puis se rejoint.
    async fn yield_to(self: &Arc<Self>, gid: GroupId, c: ClaimConflict) -> Result<Begin, String> {
        if c.kind == ConflictKind::Lease {
            return Ok(Begin::Join(c.holder));
        }
        let deadline = Instant::now() + CLAIM_YIELD_WAIT;
        loop {
            // Le concurrent annonce son bail à tous : le nôtre l'apprend par `on_lease`, ou par une ronde de sondage.
            if let Some(l) = lock(&self.leases).active(&gid, Instant::now()).into_iter().find(|l| l.holder != self.me) {
                return Ok(Begin::Join(l.holder));
            }
            if Instant::now() >= deadline {
                return Err(format!("{} démarre déjà la partie : réessaie dans un instant.", self.name_of(&c.holder)));
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    /// Un tour de renouvellement du bail `Starting`. Faux : arrêter la boucle (exclu après une veille).
    async fn renew_starting(self: &Arc<Self>, gid: GroupId, e: &Arc<StartingHost>) -> bool {
        let clock = self.clock();
        let verdict = lock(&e.fence).check(&*clock);
        if let FenceVerdict::Excluded(reason) = verdict {
            self.log_fence(&gid, reason);
            let base = lock(&e.lease).base_version;
            if self.fence_stops(gid, base).await {
                e.fenced.store(true, Ordering::SeqCst);
                eprintln!("[cubelink] groupe {gid} : un autre bail ou une version plus récente existe, la préparation du serveur est abandonnée");
                return false;
            }
            // Rien ne contredit : on reprend la préparation avec une nouvelle partie (`lease_id`).
            let next = {
                let mut l = lock(&e.lease);
                let fresh = Lease { seq: 1, ..l.clone() }.with_lease_id(new_lease_id());
                *l = self.resign(fresh);
                l.clone()
            };
            lock(&e.fence).mark(&*clock);
            self.broadcast_lease(gid, next).await;
            return true;
        }
        lock(&e.fence).mark(&*clock);
        let next = {
            let mut l = lock(&e.lease);
            *l = self.resign(l.renewed());
            l.clone()
        };
        self.broadcast_lease(gid, next).await;
        true
    }

    fn log_fence(&self, gid: &GroupId, reason: FenceReason) {
        match reason {
            FenceReason::ClockSkew { skew_secs } => {
                eprintln!("[cubelink] groupe {gid} : écart de {skew_secs} s entre les horloges depuis le dernier renouvellement (veille ?) : bail non renouvelé");
            }
            FenceReason::Stale { elapsed_secs } => {
                eprintln!("[cubelink] groupe {gid} : {elapsed_secs} s depuis le dernier renouvellement du bail : bail non renouvelé");
            }
        }
    }

    /// Ronde `Hello` d'un hôte qui ne doit pas renouveler : vrai si un autre bail valide ou une version plus récente que
    /// la pointe de ma partie existe (il faut s'arrêter), faux si rien ne la contredit.
    async fn fence_stops(self: &Arc<Self>, gid: GroupId, tip_version: Option<VersionId>) -> bool {
        let round = self.probe_round(gid).await;
        let others = lock(&self.leases).active(&gid, Instant::now());
        let tip = self.point_of(&gid, tip_version);
        fence_action(&others, &round, tip) == FenceAction::StopAndFork
    }

    /// Un tour de renouvellement du bail du serveur prêt, avec auto-exclusion après veille : avant chaque renouvellement,
    /// l'écart entre l'horloge murale et l'horloge monotone depuis le dernier renouvellement (et le temps écoulé) est
    /// comparé à 60 s et au `ttl`. Au-delà, le bail n'est **pas** renouvelé : une ronde `Hello` décide. Un autre bail ou une
    /// version plus récente : `save-all flush`, `stop`, la session est publiée comme branche sans bail (`Suspend`).
    /// Sinon l'hôte reprend avec un nouveau `lease_id`.
    pub async fn renew_tick(self: &Arc<Self>, host: &Arc<ActiveHost>) -> RenewTick {
        let gid = host.group;
        let clock = self.clock();
        let verdict = lock(&host.fence).check(&*clock);
        let FenceVerdict::Excluded(reason) = verdict else {
            lock(&host.fence).mark(&*clock);
            let next = {
                let mut l = lock(&host.lease);
                *l = self.resign(l.renewed());
                l.clone()
            };
            self.broadcast_lease(gid, next).await;
            return RenewTick::Renewed;
        };
        self.log_fence(&gid, reason);
        let tip = lock(&host.tip).map(|t| t.0);
        if self.fence_stops(gid, tip).await {
            eprintln!("[cubelink] groupe {gid} : un autre bail ou une version plus récente existe, ce serveur s'arrête et publie sa session comme branche");
            host.fenced.store(true, Ordering::SeqCst);
            // `save-all flush`, puis `save-on` : l'arrêt qui suit enregistre le monde (`save-off` l'en empêcherait).
            if host.server.flush_begin().await.is_ok() {
                let _ = host.server.flush_end().await;
            }
            // Tâche détachée : la fin de partie abandonne les tâches de l'hôte, dont celle-ci.
            let (rt, g) = (self.clone(), gid);
            spawn(async move { rt.host_ended(g).await });
            return RenewTick::Fenced;
        }
        let next = {
            let mut l = lock(&host.lease);
            let fresh = Lease { seq: 1, phase: LeasePhase::Ready, ..l.clone() }.with_lease_id(new_lease_id());
            *l = self.resign(fresh);
            l.clone()
        };
        lock(&host.fence).mark(&*clock);
        self.broadcast_lease(gid, next).await;
        RenewTick::Resumed
    }

    /// La préparation du serveur est abandonnée (échec, ou un autre membre garde la main) : le bail `Starting` est relâché.
    pub async fn end_starting(self: &Arc<Self>, gid: GroupId) {
        let Some(entry) = self.take_starting_entry(&gid) else { return };
        let l = lock(&entry.lease).clone();
        lock(&self.leases).unlock_claim(&gid, &l.lease_id);
        let visit: Visit =
            Arc::new(move |c| Box::pin(async move { c.release_lease(&gid, l.lease_id, l.seq, ReleaseReason::Handover, None).await }));
        self.broadcast(gid, true, visit).await;
        self.emit_state();
    }

    /// Retire le bail `Starting` (sans le relâcher) et arrête son renouvellement : le serveur prend le relais.
    fn take_starting_entry(&self, gid: &GroupId) -> Option<Arc<StartingHost>> {
        let entry = lock(&self.starting).remove(gid)?;
        if let Some(t) = lock(&entry.task).take() {
            t.abort();
        }
        Some(entry)
    }

    /// Second temps : le serveur répond. Annonce le bail en phase `Ready` aux membres en ligne et attend une contre-annonce.
    /// `Some(holder)` : un autre membre garde la main (un serveur prêt avant le nôtre, ou, à phase égale, une version de
    /// départ plus avancée, puis la plus petite clé) : il faut le rejoindre.
    pub async fn announce_lease(self: &Arc<Self>, host: &Arc<ActiveHost>) -> Option<PeerKey> {
        let lease = lock(&host.lease).clone();
        let gid = host.group;
        self.broadcast_lease(gid, lease).await;
        tokio::time::sleep(p2p_lib::hearth::lease::CONFLICT_WAIT).await;
        self.arbitrate_now(&gid)
    }

    /// Attend que le serveur du titulaire soit prêt (son bail passe de `Starting` à `Ready`). Erreur si son bail disparaît
    /// (serveur qui n'a pas démarré, titulaire parti) ou si l'attente dépasse le délai de démarrage.
    pub async fn wait_ready(&self, gid: &GroupId, holder: &PeerKey) -> Result<(), String> {
        let deadline = Instant::now() + START_WAIT_MAX;
        loop {
            let phase = lock(&self.leases).lease_of(gid, holder, Instant::now()).map(|l| l.phase);
            match phase {
                Some(LeasePhase::Ready) => return Ok(()),
                Some(LeasePhase::Starting) if Instant::now() < deadline => {}
                _ => return Err(format!("La partie de {} ne s'est pas lancée : réessaie dans un instant.", self.name_of(holder))),
            }
            tokio::time::sleep(START_WAIT_POLL).await;
        }
    }

    /// Preuve de service : la connexion au tunnel de `holder` a échoué. Au deuxième échec en trois minutes, son bail est
    /// ignoré quinze minutes (« Héberger quand même » devient possible).
    pub fn service_failed(&self, gid: &GroupId, holder: &PeerKey) {
        if lock(&self.leases).report_unreachable(gid, holder, Instant::now()) {
            eprintln!("[cubelink] le serveur annoncé par {} ne répond pas : son bail est ignoré", self.name_of(holder));
        }
        self.emit_state();
    }

    /// La connexion au tunnel de `holder` a réussi.
    pub fn service_ok(&self, gid: &GroupId, holder: &PeerKey) {
        lock(&self.leases).report_reachable(gid, holder);
    }

    /// Lance le renouvellement du bail, les points de reprise et la surveillance du serveur.
    pub fn run_hosting(self: &Arc<Self>, host: &Arc<ActiveHost>) {
        let (rt, h) = (self.clone(), host.clone());
        let renew = spawn(async move {
            loop {
                tokio::time::sleep(LEASE_RENEW_INTERVAL).await;
                if rt.renew_tick(&h).await == RenewTick::Fenced {
                    break;
                }
            }
        });
        let (rt, h) = (self.clone(), host.clone());
        let checkpoints = spawn(async move {
            loop {
                tokio::time::sleep(CHECKPOINT_INTERVAL).await;
                if let Err(e) = rt.checkpoint(&h).await {
                    eprintln!("[cubelink] point de reprise du groupe {} : {e}", h.group);
                }
            }
        });
        let (rt, h) = (self.clone(), host.clone());
        let watch = spawn(async move {
            h.server.wait_exit().await;
            if !h.stopping.load(Ordering::SeqCst) {
                eprintln!("[cubelink] le serveur du groupe {} s'est arrêté seul", h.group);
                // Tâche détachée : la fin de partie abandonne les tâches de l'hôte, dont celle-ci. Lancée d'ici, elle serait
                // annulée à son premier point d'attente (la publication) et le plantage ne serait jamais traité jusqu'au bout.
                let gid = h.group;
                spawn(async move { rt.host_ended(gid).await });
            }
        });
        lock(&host.tasks).extend([renew, checkpoints, watch]);
    }

    /// Le serveur d'une partie s'est arrêté seul : l'application ferme tout (partage, accès, état de lancement) ; sans
    /// application branchée, on termine seulement la partie (publication, bail).
    async fn host_ended(self: &Arc<Self>, gid: GroupId) {
        let hook = lock(&self.end_hook).clone();
        match hook {
            Some(h) => h(gid).await,
            None => {
                let _ = self.finish_hosting(gid).await;
            }
        }
    }

    /// Point de reprise : le serveur cesse d'écrire, le monde est publié, le serveur reprend.
    pub async fn checkpoint(self: &Arc<Self>, host: &Arc<ActiveHost>) -> Result<SignedVersion, String> {
        let begun = host.server.flush_begin().await;
        let published = if begun.is_ok() {
            let dir = host.server.world_dir();
            let tip = *lock(&host.tip);
            Some(self.publish_version(host.group, &dir, VersionKind::Checkpoint, tip, host.started.elapsed().as_secs()).await)
        } else {
            None
        };
        // `save-on` est toujours tenté, même si `save-off` n'a pas confirmé (la commande a pu partir sans réponse).
        let resumed = host.server.flush_end().await;
        // La version est déjà enregistrée et en file d'envoi : elle devient le parent de la suivante quoi qu'il arrive
        // ensuite. Sinon la `Session` finale aurait le même parent et le groupe verrait une fourche après une partie normale.
        if let Some(Ok(v)) = &published {
            *lock(&host.tip) = Some((v.id(), v.header.number));
        }
        begun?;
        let v = published.ok_or("point de reprise non fait")??;
        resumed?;
        Ok(v)
    }

    /// Abandonne l'hébergement sans rien publier (un autre membre a gardé la main) : le serveur est arrêté, le bail relâché.
    pub async fn abort_hosting(self: &Arc<Self>, gid: GroupId) {
        let Some(host) = lock(&self.active).remove(&gid) else { return };
        host.stopping.store(true, Ordering::SeqCst);
        for t in std::mem::take(&mut *lock(&host.tasks)) {
            t.abort();
        }
        host.server.stop(Duration::from_secs(20)).await;
        let l = lock(&host.lease).clone();
        let visit: Visit =
            Arc::new(move |c| Box::pin(async move { c.release_lease(&gid, l.lease_id, l.seq, ReleaseReason::Handover, None).await }));
        self.broadcast(gid, true, visit).await;
        self.access.close_group(&gid);
        self.emit_state();
    }

    /// Le groupe a tranché une partie mal terminée : son bail ne compte plus.
    pub fn revoke_lease(&self, gid: &GroupId, holder: &PeerKey) {
        lock(&self.leases).revoke(gid, holder);
        self.emit_state();
    }

    /// Fin de la partie : le serveur s'arrête, le monde est publié (version `Session`), le bail est relâché.
    /// Rend la version publiée.
    pub async fn finish_hosting(self: &Arc<Self>, gid: GroupId) -> Result<Option<SignedVersion>, String> {
        let host = lock(&self.active).remove(&gid).ok_or("aucun serveur n'est lancé pour ce groupe")?;
        host.stopping.store(true, Ordering::SeqCst);
        for t in std::mem::take(&mut *lock(&host.tasks)) {
            t.abort();
        }
        host.server.stop(SERVER_STOP_GRACE).await;
        let dir = host.server.world_dir();
        let tip = *lock(&host.tip);
        let published = if dir.is_dir() {
            self.publish_version(gid, &dir, VersionKind::Session, tip, host.started.elapsed().as_secs()).await.map(Some)
        } else {
            Ok(None)
        };
        if let Ok(Some(v)) = &published {
            self.set_materialized(&gid, Some(v.id()));
        }
        let vid = published.as_ref().ok().and_then(|v| v.as_ref().map(SignedVersion::id));
        let l = lock(&host.lease).clone();
        let reason = if host.fenced.load(Ordering::SeqCst) { ReleaseReason::Suspend } else { ReleaseReason::Stopped };
        let visit: Visit = Arc::new(move |c| {
            let reason = reason.clone();
            Box::pin(async move { c.release_lease(&gid, l.lease_id, l.seq, reason, vid).await })
        });
        self.broadcast(gid, true, visit).await;
        self.access.close_group(&gid);
        self.gc_versions(gid).await;
        self.emit_state();
        published
    }
}

// ---------------------------------------------------------------------------------------------------------
// Cartes et page du serveur
// ---------------------------------------------------------------------------------------------------------

fn loader_name(l: &LoaderSpec) -> String {
    format!("{:?}", l.loader).to_lowercase()
}

impl HearthRuntime {
    pub fn emit_state(&self) {
        self.emit_json("hearth://state", &self.cards());
    }

    pub fn cards(&self) -> Vec<HearthCard> {
        let mut v: Vec<HearthCard> = self.group_ids().iter().filter_map(|g| self.card(g)).collect();
        v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.id.cmp(&b.id)));
        v
    }

    pub fn card(&self, gid: &GroupId) -> Option<HearthCard> {
        let st = self.state(gid)?;
        let heads = self.heads_of(gid).unwrap_or_default();
        let instance_id = self.instance_of(gid);
        let online: Vec<HearthMemberLite> =
            self.others(&st).iter().filter(|k| self.is_online(gid, k)).map(|k| self.lite(&st, k)).collect();
        let mut card = HearthCard {
            id: gid.to_hex(),
            name: st.name.clone(),
            minecraft: st.pack.minecraft.clone(),
            loader: loader_name(&st.pack.loader),
            state: HearthStateKind::Idle,
            host: None,
            players: 0,
            progress: None,
            step: None,
            step_index: None,
            step_count: 0,
            unclean: None,
            members_online: online.clone(),
            members_total: st.members.len() as u32,
            is_creator: st.creator == self.me,
            can_host: instance_id.is_some(),
            instance_id: instance_id.clone(),
            version: heads.first().map(|h| h.header.number),
        };
        let decision = self.decision(gid);
        let holder_of = |k: &PeerKey| Some(self.lite(&st, k));
        if let Some(a) = self.active(gid) {
            card.state = HearthStateKind::Hosting;
            card.host = holder_of(&self.me);
            card.players = a.players();
        }
        if let Some(Play::Launching { steps, index }) = self.play_of(gid) {
            card.state = HearthStateKind::Launching;
            card.step = steps.get(usize::from(index)).map(|s| (*s).to_string());
            card.step_index = Some(index);
            card.step_count = steps.len() as u8;
            return Some(card);
        }
        if card.state == HearthStateKind::Hosting {
            return Some(card);
        }
        if let Some(p) = lock(&self.fetching).get(gid).copied() {
            card.state = HearthStateKind::Syncing;
            card.progress = Some(p);
        } else if heads.len() > 1 {
            card.state = HearthStateKind::Fork;
        } else if instance_id.is_none() {
            card.state = HearthStateKind::PackMissing;
        } else {
            match decision {
                Some(Decision::Join(h)) => {
                    card.state = HearthStateKind::Hosted;
                    card.host = holder_of(&h);
                    card.players = online.len() as u32;
                }
                Some(Decision::Unclean { holder, last_checkpoint }) => {
                    let head_no = heads.first().map_or(0, |h| h.header.number);
                    let at = last_checkpoint
                        .and_then(|id| self.version(gid, &id))
                        .filter(|v| v.header.number >= head_no)
                        .map(|v| v.header.created_at as i64);
                    card.state = HearthStateKind::Unclean;
                    card.unclean = Some(HearthUnclean { holder: self.lite(&st, &holder), checkpoint_at: at });
                }
                Some(Decision::Blocked) => card.state = HearthStateKind::Blocked,
                Some(Decision::Sync(_)) => {
                    card.state = HearthStateKind::Syncing;
                    card.progress = Some(0);
                }
                Some(Decision::Host) | None => {}
            }
        }
        if let (HearthStateKind::PackMissing, Some(Decision::Join(h))) = (card.state, decision) {
            card.host = holder_of(&h);
        }
        Some(card)
    }

    /// Page du serveur. Le calcul de la place disque lit le dossier : à appeler hors du fil asynchrone principal.
    pub fn detail(&self, gid: &GroupId) -> Result<HearthDetail, String> {
        let st = self.require_state(gid)?;
        let card = self.card(gid).ok_or("ce groupe n'existe plus")?;
        let (graph, proposals) = self.with_group(gid, |g| (g.graph().clone(), g.proposals().cloned().collect::<Vec<_>>()))?;
        let heads = effective_heads(&graph, &st);
        let head = (heads.len() == 1).then(|| heads[0].clone());
        let my_latest = graph.latest_by_number(&st).map(|v| v.header.point());
        let now = now_secs();
        let meta = self.meta_of(gid);
        let unseen = |k: &PeerKey| -> Option<u32> {
            if *k == self.me || self.is_online(gid, k) {
                return None;
            }
            meta.seen.get(&key_hex(k)).map(|t| ((now - t).max(0) / 86_400) as u32)
        };
        let members: Vec<HearthMember> = st
            .members
            .iter()
            .map(|k| {
                let s = lock(&self.status).get(&(*gid, *k)).cloned().unwrap_or_default();
                let latest = if *k == self.me { my_latest } else { s.latest };
                let sync = match (latest, my_latest) {
                    (Some(l), Some(m)) if l >= m => "ok",
                    (Some(_), Some(_)) => "behind",
                    (None, None) if *k == self.me || s.online => "ok",
                    (Some(_), None) => "ok",
                    _ => "unknown",
                };
                HearthMember {
                    public_key: key_hex(k),
                    name: self.name_of(k),
                    online: *k == self.me || s.online,
                    is_me: *k == self.me,
                    is_creator: *k == st.creator,
                    sync: sync.to_string(),
                    version: latest.map(|p| p.number),
                    unseen_days: unseen(k),
                }
            })
            .collect();
        let mainline: HashSet<VersionId> = head.as_ref().map(|h| graph.ancestry(&h.id()).into_iter().collect()).unwrap_or_default();
        let head_ids: HashSet<VersionId> = heads.iter().map(SignedVersion::id).collect();
        let mut versions: Vec<HearthVersion> = graph
            .iter()
            .map(|(id, v)| HearthVersion {
                id: id.to_hex(),
                number: v.header.number,
                kind: match v.header.kind {
                    VersionKind::Session => "session",
                    VersionKind::Checkpoint => "checkpoint",
                    VersionKind::Restore { .. } => "restore",
                }
                .to_string(),
                author: key_hex(&v.header.author),
                author_name: self.name_of(&v.header.author),
                created_at: v.header.created_at as i64,
                played_secs: v.header.played_secs,
                total_size: v.header.total_size,
                is_head: head_ids.contains(id),
                side_branch: v.header.epoch < st.epoch && head.is_some() && !mainline.contains(id),
            })
            .collect();
        versions.sort_by(|a, b| b.number.cmp(&a.number).then(a.id.cmp(&b.id)));
        let proposals = proposals
            .iter()
            .map(|p| {
                let (approvals, needed) = p.approvals(&st);
                let i_voted = p.sigs.iter().any(|(k, _)| *k == self.me);
                let (kind, subject) = match &p.op {
                    GroupOp::ContinueWithout { absent, .. } => ("continue_without", Some(*absent)),
                    GroupOp::AddMember { key } => ("add_member", Some(*key)),
                    GroupOp::RemoveMember { key } => ("remove_member", Some(*key)),
                    GroupOp::PackUpdate { .. } => ("pack_update", None),
                    GroupOp::Leave => ("leave", None),
                    GroupOp::SetGovernance { .. } => ("set_governance", None),
                    GroupOp::Create { .. } => ("create", None),
                };
                let excluded = matches!(&p.op, GroupOp::ContinueWithout { absent, .. } if *absent == self.me);
                HearthProposal {
                    id: hex::encode(p.id()),
                    kind: kind.to_string(),
                    subject: subject.map(|k| self.lite(&st, &k)),
                    approvals: approvals as u32,
                    needed: needed as u32,
                    i_voted,
                    can_vote: !i_voted && !excluded && st.is_member(&self.me),
                }
            })
            .collect();
        let blocker = head.as_ref().and_then(|h| {
            let a = h.header.author;
            let days = blocking_days(a == self.me, st.is_member(&a), unseen(&a))?;
            Some(HearthBlocker { member: self.lite(&st, &a), days })
        });
        let fork: Vec<HearthBranch> = if heads.len() > 1 {
            heads
                .iter()
                .map(|v| HearthBranch {
                    version_id: v.id().to_hex(),
                    number: v.header.number,
                    author_name: self.name_of(&v.header.author),
                    created_at: v.header.created_at as i64,
                    played_secs: v.header.played_secs,
                    total_size: v.header.total_size,
                })
                .collect()
        } else {
            Vec::new()
        };
        let settings = HearthSettings {
            keep_background: meta.keep_background,
            disk_bytes: dir_size(&self.store.group_dir(gid)),
            outbox_pending: self.outbox.pending().iter().filter(|e| e.group_id() == *gid).count() as u32,
            instance_name: self.instance_of(gid).and_then(|i| self.instances.get(&i).ok()).map(|i| i.name),
        };
        Ok(HearthDetail { card, members, versions, proposals, blocker, fork, settings })
    }

    /// Le membre qui bloque le groupe (bannière « Continuer sans X »), s'il y en a un.
    pub fn blocker(&self, gid: &GroupId) -> Option<PeerKey> {
        self.detail(gid).ok()?.blocker.and_then(|b| parse_key(&b.member.public_key))
    }

    /// Propose de continuer sans `absent` : repart de la version la plus récente que nous avons.
    pub async fn propose_continue(self: &Arc<Self>, gid: GroupId, absent: PeerKey) -> Result<(), String> {
        let st = self.require_state(&gid)?;
        if !st.is_member(&absent) || absent == self.me {
            return Err("ce membre ne peut pas être écarté".into());
        }
        let b = self.blocker(&gid).ok_or("personne ne bloque le groupe pour l'instant")?;
        if b != absent {
            return Err("ce membre n'est pas celui qui bloque le groupe".into());
        }
        // Une proposition identique existe déjà : on vote pour elle.
        let existing = self.with_group(&gid, |g| {
            g.proposals().find(|p| matches!(&p.op, GroupOp::ContinueWithout { absent: a, .. } if *a == absent)).map(Proposal::id)
        })?;
        if let Some(id) = existing {
            return self.vote(gid, id).await;
        }
        let base = self.head(&gid).ok_or("aucune version connue pour repartir")?.id();
        // Dernier signe de vie de l'absent tel que nous le connaissons ; sans cela, rien n'atteste son absence.
        let seen = self.meta_of(&gid).seen.get(&key_hex(&absent)).copied().ok_or("on ne sait pas depuis quand ce membre est absent")?;
        let (absent_since, at) = (u64::try_from(seen).unwrap_or(0), u64::try_from(now_secs()).unwrap_or(0));
        self.propose(gid, GroupOp::ContinueWithout { absent, base_version: base, absent_since, at }).await
    }
}

/// Nombre de jours d'absence de l'auteur de la version la plus récente, s'il bloque le groupe : un autre membre,
/// actuellement dans le groupe, pas revenu depuis `CONTINUE_AFTER_DAYS` jours au moins.
fn blocking_days(author_is_me: bool, is_member: bool, unseen_days: Option<u32>) -> Option<u32> {
    let days = unseen_days?;
    (!author_is_me && is_member && days >= CONTINUE_AFTER_DAYS).then_some(days)
}

/// Espace libre (octets) sur le disque qui porte `path`, s'il se mesure ; `None` sinon (la vérification est alors
/// sautée, jamais un refus).
pub fn free_space(path: &Path) -> Option<u64> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        extern "system" {
            fn GetDiskFreeSpaceExW(dir: *const u16, available: *mut u64, total: *mut u64, free: *mut u64) -> i32;
        }
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        let mut available = 0u64;
        // SAFETY : `wide` est une chaîne terminée par zéro qui vit pendant l'appel ; les pointeurs de sortie inutiles sont nuls
        // (permis par l'API) et `available` est un entier valide.
        let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, std::ptr::null_mut(), std::ptr::null_mut()) };
        (ok != 0).then_some(available)
    }
    #[cfg(not(windows))]
    {
        let out = std::process::Command::new("df").arg("-Pk").arg(path).output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let kib: u64 = text.lines().nth(1)?.split_whitespace().nth(3)?.parse().ok()?;
        kib.checked_mul(1024)
    }
}

/// Taille d'un dossier, bornée (un dossier immense ne fige pas l'écran).
fn dir_size(root: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![root.to_path_buf()];
    let mut seen = 0usize;
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            seen += 1;
            if seen > 300_000 {
                return total;
            }
            let Ok(t) = e.file_type() else { continue };
            if t.is_dir() {
                stack.push(e.path());
            } else if t.is_file() {
                total += e.metadata().map_or(0, |m| m.len());
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joueur_qui_entre_ou_sort_seulement_depuis_un_message_de_journal() {
        assert_eq!(
            parse_player_event("[12:00:01] [Server thread/INFO]: Alice joined the game"),
            Some((true, "Alice".into()))
        );
        assert_eq!(parse_player_event("[12:00:01] [Server thread/INFO]: Bob_2 left the game"), Some((false, "Bob_2".into())));
        // Un message de chat ne se fait pas passer pour une entrée.
        assert_eq!(parse_player_event("[12:00:01] [Server thread/INFO]: <Mallory> Eve joined the game"), None);
        assert_eq!(parse_player_event("[12:00:01] [Server thread/INFO]: Eve was slain"), None);
        assert_eq!(parse_player_event("[12:00:01] [Server thread/INFO]: [Rcon] Eve joined the game"), None);
        assert_eq!(parse_player_event("[12:00:01] [Server thread/INFO]: a b joined the game"), None);
    }

    #[test]
    fn bannière_seulement_apres_sept_jours_pour_un_autre_membre() {
        // Moins de sept jours, ou jamais vu : pas de bannière.
        assert_eq!(blocking_days(false, true, Some(CONTINUE_AFTER_DAYS - 1)), None);
        assert_eq!(blocking_days(false, true, None), None);
        // Sept jours et plus : « X n'est pas revenu depuis N jours ».
        assert_eq!(blocking_days(false, true, Some(CONTINUE_AFTER_DAYS)), Some(CONTINUE_AFTER_DAYS));
        assert_eq!(blocking_days(false, true, Some(30)), Some(30));
        // Jamais pour moi, ni pour quelqu'un qui n'est plus dans le groupe.
        assert_eq!(blocking_days(true, true, Some(30)), None);
        assert_eq!(blocking_days(false, false, Some(30)), None);
    }

    #[test]
    fn taille_de_dossier_bornee_et_recursive() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("a/b")).unwrap();
        std::fs::write(d.path().join("x.bin"), [0u8; 10]).unwrap();
        std::fs::write(d.path().join("a/b/y.bin"), [0u8; 32]).unwrap();
        assert_eq!(dir_size(d.path()), 42);
        assert_eq!(dir_size(&d.path().join("absent")), 0);
    }
}
