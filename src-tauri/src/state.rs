//! État de l'application : chemins, index du magasin, instances, identité, amis, session de jeu.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

use core_lib::auth::device::DeviceCode;
use core_lib::instance::InstanceStore;
use core_lib::launch_run::RunningGame;
use core_lib::store::index::StoreIndex;
use core_lib::types::{Friend, ManifestFile, Presence, Profile, Settings, TransferProgress, TrustLevel, VerifyCode};
use core_lib::unknown_mods::UnknownReport;
use p2p_lib::friend_code::FriendCodePayload;
use p2p_lib::friends_db::FriendsDb;
use p2p_lib::identity::Identity;
use p2p_lib::invite::InviteMessage;
use p2p_lib::transfer::plan::Plan;
use serde::Serialize;

use crate::p2p_backend::{BoxFuture, HostManifest, P2pBackend, ProgressFn, TransferControl};

/// Émetteur d'événements (Tauri en production, collecteur dans les tests).
pub type EventSink = Arc<dyn Fn(&str, serde_json::Value) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct Paths {
    pub root: PathBuf,
    pub instances: PathBuf,
    pub store: PathBuf,
    pub libraries: PathBuf,
    pub assets: PathBuf,
    pub versions: PathBuf,
    pub natives: PathBuf,
    pub java: PathBuf,
    pub loaders: PathBuf,
    pub logs: PathBuf,
    pub p2p: PathBuf,
    pub settings_file: PathBuf,
    /// Ancien fichier à compte unique (migré vers `accounts_file` puis supprimé).
    pub tokens_file: PathBuf,
    pub accounts_file: PathBuf,
    pub key_file: PathBuf,
    pub epoch_file: PathBuf,
}

impl Paths {
    pub fn new(root: &Path) -> Self {
        let r = root.to_path_buf();
        Self {
            instances: r.join("instances"),
            store: r.join("store"),
            libraries: r.join("libraries"),
            assets: r.join("assets"),
            versions: r.join("versions"),
            natives: r.join("natives"),
            java: r.join("java"),
            loaders: r.join("loaders"),
            logs: r.join("logs"),
            p2p: r.join("p2p"),
            settings_file: r.join("settings.json"),
            tokens_file: r.join("auth").join("tokens.bin"),
            accounts_file: r.join("auth").join("accounts.bin"),
            key_file: r.join("auth").join("tokens.key"),
            epoch_file: r.join("p2p").join("friend_code_epoch"),
            root: r,
        }
    }

    pub fn create_all(&self) -> std::io::Result<()> {
        for d in [
            &self.root,
            &self.instances,
            &self.store,
            &self.libraries,
            &self.assets,
            &self.versions,
            &self.natives,
            &self.java,
            &self.loaders,
            &self.logs,
            &self.p2p,
        ] {
            std::fs::create_dir_all(d)?;
        }
        std::fs::create_dir_all(self.tokens_file.parent().unwrap_or(&self.root))
    }
}

/// Transfert d'invité en cours.
pub struct JoinSession {
    pub control: Arc<TransferControl>,
    pub last: Arc<Mutex<Option<TransferProgress>>>,
}

pub struct AppState {
    pub paths: Paths,
    pub store: Arc<StoreIndex>,
    pub instances: Arc<InstanceStore>,
    pub identity: Arc<Identity>,
    pub friends: Arc<FriendsDb>,
    /// Dernière fois que chaque ami a été vu en ligne.
    pub last_seen: Arc<crate::last_seen::LastSeen>,
    /// Mods inconnus déjà acceptés (par sha256) : la case de confiance ne les redemande pas.
    pub trusted_mods: Arc<crate::trusted_mods::TrustedMods>,
    /// Par ami : le dernier `join_plan` montré (hash du manifeste, sha256 des fichiers qui demandent l'accord), vérifié
    /// au `join_start`.
    pub pending_trust: Mutex<std::collections::HashMap<String, crate::commands_join::PendingConsent>>,
    pub settings: Arc<Mutex<Settings>>,
    pub profile: Mutex<Option<Profile>>,
    pub pending_login: Mutex<Option<DeviceCode>>,
    /// Identifiant de l'instance en cours de préparation ou de jeu.
    pub game: Mutex<Option<String>>,
    /// Processus de jeu en cours (id de l'instance + poignée d'arrêt) ; vide pendant la préparation.
    pub game_handle: Mutex<Option<(String, Arc<RunningGame>)>>,
    /// Zone de jeu integree a la fenetre du launcher (Windows) : rideau et adoption de la fenetre du jeu.
    pub stage: crate::stage::Stage,
    pub join: Mutex<Option<JoinSession>>,
    /// Chemin du jar de l'agent LAN (ressource Tauri), s'il est présent.
    pub lan_agent: Mutex<Option<PathBuf>>,
    /// Vrai si l'agent LAN a fait planter la JVM : désactivé pour le reste de la session.
    pub lan_agent_off: std::sync::atomic::AtomicBool,
    /// État du partage hôte (voir `commands_host`).
    pub host: Mutex<Option<crate::commands_host::HostState>>,
    /// Invitations reçues d'amis (mémoire seulement, voir `invites`).
    pub invites: Mutex<Vec<core_lib::types::Invite>>,
    /// Dernier envoi d'invitation par ami (anti-spam côté émetteur).
    pub invites_sent: Mutex<std::collections::HashMap<String, std::time::Instant>>,
    /// Instance invitée créée par le dernier `join_start` réussi (lue par `game_join_friend`).
    pub last_join_instance: Mutex<Option<String>>,
    backend: RwLock<Arc<dyn P2pBackend>>,
    /// Fermeture du launcher et démarrage du jeu ne se croisent jamais (voir `QuitGate`).
    pub quit: QuitGate,
    /// Test : saute l'installation du jeu (Mojang, loader, Java) après un transfert d'invité.
    pub skip_game_install: AtomicBool,
    pub emit: EventSink,
}

/// Fermeture du launcher contre démarrage du jeu. Un java créé pendant que le launcher sort lui survivrait sans
/// personne pour le suivre (ni rideau, ni journal, ni arrêt propre) : dès que la fermeture est demandée plus aucun
/// jeu ne démarre, et un démarrage déjà engagé (du `spawn` jusqu'à ce que le jeu soit suivi) est attendu avant de
/// sortir. Chaque côté écrit son drapeau PUIS lit l'autre : au moins l'un des deux voit l'autre.
#[derive(Default)]
pub struct QuitGate {
    quitting: AtomicBool,
    spawning: AtomicBool,
}

/// Démarrage du jeu en cours ; le lâcher rend la main à une fermeture qui attend.
pub struct SpawnPass<'a>(&'a QuitGate);

impl Drop for SpawnPass<'_> {
    fn drop(&mut self) {
        self.0.spawning.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

impl QuitGate {
    /// La fermeture du launcher est demandée. Vrai : un démarrage de jeu est engagé, il faut attendre `spawning()`
    /// faux avant de sortir.
    pub fn begin_quit(&self) -> bool {
        self.quitting.store(true, std::sync::atomic::Ordering::SeqCst);
        self.spawning()
    }

    /// La fermeture n'a pas quitté l'application (Cubelink masqué en fond) : les lancements suivants sont de nouveau permis.
    pub fn cancel_quit(&self) {
        self.quitting.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn quitting(&self) -> bool {
        self.quitting.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn spawning(&self) -> bool {
        self.spawning.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Avant de créer le processus du jeu. `None` : le launcher se ferme, le jeu ne doit pas démarrer.
    pub fn enter_spawn(&self) -> Option<SpawnPass<'_>> {
        self.spawning.store(true, std::sync::atomic::Ordering::SeqCst);
        let pass = SpawnPass(self);
        if self.quitting() {
            return None;
        }
        Some(pass)
    }
}

/// Délai maximal d'attente du backend P2P par une commande avant de renvoyer « en démarrage ».
const BACKEND_WAIT: std::time::Duration = std::time::Duration::from_secs(30);
const STARTING_MSG: &str = "le réseau entre amis est en cours de démarrage, réessayez dans un instant";

type Ready = Option<Result<Arc<dyn P2pBackend>, String>>;

/// Backend P2P en cours de démarrage : l'UI s'affiche tout de suite ; les opérations réseau asynchrones
/// attendent (bornées) que le vrai backend soit prêt, les lectures synchrones répondent « hors ligne ».
pub struct PendingBackend {
    tx: tokio::sync::watch::Sender<Ready>,
    rx: tokio::sync::watch::Receiver<Ready>,
    name: Mutex<Option<String>>,
}

impl Default for PendingBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl PendingBackend {
    pub fn new() -> Self {
        let (tx, rx) = tokio::sync::watch::channel(None);
        Self { tx, rx, name: Mutex::new(None) }
    }

    /// Le backend réel est prêt : débloque les commandes en attente et rejoue le pseudo déclaré entre-temps.
    pub fn ready(&self, be: Arc<dyn P2pBackend>) {
        if let Some(n) = lock(&self.name).take() {
            be.set_display_name(&n);
        }
        self.tx.send_replace(Some(Ok(be)));
    }

    /// Le démarrage a échoué : les commandes en attente reçoivent ce message.
    pub fn failed(&self, msg: String) {
        self.tx.send_replace(Some(Err(msg)));
    }

    fn current(&self) -> Option<Arc<dyn P2pBackend>> {
        match &*self.rx.borrow() {
            Some(Ok(b)) => Some(b.clone()),
            _ => None,
        }
    }

    async fn wait(&self) -> Result<Arc<dyn P2pBackend>, String> {
        let mut rx = self.rx.clone();
        let fut = rx.wait_for(|v| v.is_some());
        let got = tokio::time::timeout(BACKEND_WAIT, fut).await.map_err(|_| STARTING_MSG.to_string())?;
        let v = got.map_err(|_| STARTING_MSG.to_string())?;
        match v.as_ref() {
            Some(Ok(b)) => Ok(b.clone()),
            Some(Err(e)) => Err(e.clone()),
            None => Err(STARTING_MSG.to_string()),
        }
    }
}

impl P2pBackend for PendingBackend {
    fn presence(&self, public_key: &str) -> Presence {
        self.current().map(|b| b.presence(public_key)).unwrap_or(Presence::Offline)
    }
    fn set_display_name(&self, name: &str) {
        match self.current() {
            Some(b) => b.set_display_name(name),
            None => *lock(&self.name) = Some(name.to_string()),
        }
    }
    fn code_hints(&self) -> (Option<String>, Vec<String>) {
        self.current().map(|b| b.code_hints()).unwrap_or((None, vec![]))
    }
    fn send_request(&self, target: FriendCodePayload, my_name: String) -> BoxFuture<'_, Result<VerifyCode, String>> {
        Box::pin(async move { self.wait().await?.send_request(target, my_name).await })
    }
    fn accept_request(&self, public_key: String) -> BoxFuture<'_, Result<Option<Friend>, String>> {
        Box::pin(async move { self.wait().await?.accept_request(public_key).await })
    }
    fn refuse_request(&self, public_key: String) -> BoxFuture<'_, Result<bool, String>> {
        Box::pin(async move { self.wait().await?.refuse_request(public_key).await })
    }
    fn fetch_manifest(&self, friend_public_key: String) -> BoxFuture<'_, Result<HostManifest, String>> {
        Box::pin(async move { self.wait().await?.fetch_manifest(friend_public_key).await })
    }
    fn find_unknown_mods(&self, files: Vec<ManifestFile>, game_dir: PathBuf) -> BoxFuture<'_, Vec<String>> {
        Box::pin(async move {
            match self.wait().await {
                Ok(b) => b.find_unknown_mods(files, game_dir).await,
                Err(_) => vec![],
            }
        })
    }
    fn find_unknown_from_manifest(&self, files: Vec<ManifestFile>) -> BoxFuture<'_, UnknownReport> {
        Box::pin(async move {
            match self.wait().await {
                Ok(b) => b.find_unknown_from_manifest(files).await,
                Err(_) => UnknownReport::default(),
            }
        })
    }
    fn send_invite(&self, public_key: String, msg: InviteMessage) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move { self.wait().await?.send_invite(public_key, msg).await })
    }
    fn host_start_sharing(&self, instance_id: String, world_port: u16) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move { self.wait().await?.host_start_sharing(instance_id, world_port).await })
    }
    fn host_stop_sharing(&self) {
        if let Some(b) = self.current() {
            b.host_stop_sharing();
        }
    }
    fn host_set_world_port(&self, port: u16) {
        if let Some(b) = self.current() {
            b.host_set_world_port(port);
        }
    }
    fn join_open_tunnel(&self, friend_public_key: String) -> BoxFuture<'_, Result<u16, String>> {
        Box::pin(async move { self.wait().await?.join_open_tunnel(friend_public_key).await })
    }
    fn join_close_tunnel(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if let Some(b) = self.current() {
                b.join_close_tunnel().await;
            }
        })
    }
    fn receive_files(
        &self,
        friend_public_key: String,
        transfer_id: String,
        plan: Plan,
        progress: ProgressFn,
        control: Arc<TransferControl>,
    ) -> BoxFuture<'_, Result<(), core_lib::types::Error>> {
        Box::pin(async move {
            let b = self.wait().await.map_err(core_lib::types::Error::Other)?;
            b.receive_files(friend_public_key, transfer_id, plan, progress, control).await
        })
    }
}

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn default_settings() -> Settings {
    Settings {
        language: "fr".into(),
        default_ram_mb: 4096,
        java_path: None,
        trust_default: TrustLevel::Ask,
        allow_relay: true,
        close_on_launch: false,
        relay_only: false,
        share_mods: true,
        discovery: true,
        lan_auto: true,
        overlay_enabled: true,
        overlay_hotkey: core_lib::types::DEFAULT_OVERLAY_HOTKEY.into(),
        embedded_game: true,
        start_with_os: true,
    }
}

impl AppState {
    pub fn new(root: &Path, emit: EventSink, backend: Arc<dyn P2pBackend>) -> Result<Self, String> {
        let paths = Paths::new(root);
        paths.create_all().map_err(|_| "impossible de créer le dossier de données".to_string())?;
        let store = StoreIndex::open(paths.store.join("index.sqlite")).map_err(|e| crate::error::user_msg(&e))?;
        let identity = Identity::load_or_create(&paths.p2p).map_err(|e| crate::error::user_msg_any(&e))?;
        let friends = FriendsDb::open(paths.p2p.join("friends.db")).map_err(|e| crate::error::user_msg(&e))?;
        let settings = crate::commands_settings::load_settings(&paths.settings_file);
        let stage = crate::stage::Stage::default();
        stage.set_wanted(settings.embedded_game);
        let last_seen = Arc::new(crate::last_seen::LastSeen::open(paths.p2p.join("last_seen.json")));
        let trusted_mods = Arc::new(crate::trusted_mods::TrustedMods::open(paths.p2p.join("trusted_mods.json")));
        Ok(Self {
            last_seen,
            trusted_mods,
            pending_trust: Mutex::new(std::collections::HashMap::new()),
            instances: Arc::new(InstanceStore::new(paths.instances.clone())),
            store: Arc::new(store),
            identity: Arc::new(identity),
            friends: Arc::new(friends),
            settings: Arc::new(Mutex::new(settings)),
            profile: Mutex::new(None),
            pending_login: Mutex::new(None),
            game: Mutex::new(None),
            game_handle: Mutex::new(None),
            stage,
            join: Mutex::new(None),
            lan_agent: Mutex::new(None),
            lan_agent_off: std::sync::atomic::AtomicBool::new(false),
            host: Mutex::new(None),
            invites: Mutex::new(Vec::new()),
            invites_sent: Mutex::new(std::collections::HashMap::new()),
            last_join_instance: Mutex::new(None),
            backend: RwLock::new(backend),
            quit: QuitGate::default(),
            skip_game_install: AtomicBool::new(false),
            emit,
            paths,
        })
    }

    pub fn backend(&self) -> Arc<dyn P2pBackend> {
        self.backend.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn set_backend(&self, b: Arc<dyn P2pBackend>) {
        *self.backend.write().unwrap_or_else(|e| e.into_inner()) = b;
    }

    pub fn emit_event<T: Serialize>(&self, name: &str, payload: &T) {
        if let Ok(v) = serde_json::to_value(payload) {
            (self.emit)(name, v);
        }
    }
}

#[cfg(test)]
mod quit_tests {
    use super::QuitGate;

    #[test]
    fn fermeture_demandee_plus_aucun_jeu_ne_demarre() {
        let g = QuitGate::default();
        let pass = g.enter_spawn();
        assert!(pass.is_some() && g.spawning(), "hors fermeture : le jeu demarre");
        drop(pass);
        assert!(!g.spawning());
        assert!(!g.begin_quit(), "aucun demarrage engage : le launcher sort tout de suite");
        assert!(g.quitting());
        assert!(g.enter_spawn().is_none(), "fermeture demandee : pas de java derriere le launcher");
        assert!(!g.spawning(), "le refus ne retient pas la fermeture");
    }

    #[test]
    fn fermeture_pendant_le_demarrage_attend_puis_le_jeu_est_arrete() {
        let g = QuitGate::default();
        let pass = g.enter_spawn().expect("demarrage engage avant la fermeture");
        assert!(g.begin_quit(), "le launcher doit attendre la fin du demarrage");
        // le lanceur relit le drapeau une fois le processus cree : il l'arrete, puis rend la main
        assert!(g.quitting());
        drop(pass);
        assert!(!g.spawning(), "la fermeture peut se terminer");
    }
}

#[cfg(test)]
pub mod test_support {
    use super::*;
    use crate::p2p_backend::UnavailableBackend;

    pub type Events = Arc<Mutex<Vec<(String, serde_json::Value)>>>;

    pub fn test_state(dir: &Path) -> (Arc<AppState>, Events) {
        let events: Events = Arc::new(Mutex::new(Vec::new()));
        let ev = events.clone();
        let sink: EventSink = Arc::new(move |n, v| lock(&ev).push((n.to_string(), v)));
        (Arc::new(AppState::new(dir, sink, Arc::new(UnavailableBackend)).unwrap()), events)
    }
}
