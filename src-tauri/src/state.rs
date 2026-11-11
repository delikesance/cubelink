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
    pub tokens_file: PathBuf,
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
    pub settings: Arc<Mutex<Settings>>,
    pub profile: Mutex<Option<Profile>>,
    pub pending_login: Mutex<Option<DeviceCode>>,
    /// Identifiant de l'instance en cours de préparation ou de jeu.
    pub game: Mutex<Option<String>>,
    /// Processus de jeu en cours (id de l'instance + poignée d'arrêt) ; vide pendant la préparation.
    pub game_handle: Mutex<Option<(String, Arc<RunningGame>)>>,
    pub join: Mutex<Option<JoinSession>>,
    /// Chemin du jar de l'agent LAN (ressource Tauri), s'il est présent.
    pub lan_agent: Mutex<Option<PathBuf>>,
    /// Vrai si l'agent LAN a fait planter la JVM : désactivé pour le reste de la session.
    pub lan_agent_off: std::sync::atomic::AtomicBool,
    /// État du partage hôte (voir `commands_host`).
    pub host: Mutex<Option<crate::commands_host::HostState>>,
    backend: RwLock<Arc<dyn P2pBackend>>,
    /// Test : saute l'installation du jeu (Mojang, loader, Java) après un transfert d'invité.
    pub skip_game_install: AtomicBool,
    pub emit: EventSink,
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
    fn notify_accepted(&self, friend: Friend) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move { self.wait().await?.notify_accepted(friend).await })
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
        Ok(Self {
            instances: Arc::new(InstanceStore::new(paths.instances.clone())),
            store: Arc::new(store),
            identity: Arc::new(identity),
            friends: Arc::new(friends),
            settings: Arc::new(Mutex::new(settings)),
            profile: Mutex::new(None),
            pending_login: Mutex::new(None),
            game: Mutex::new(None),
            game_handle: Mutex::new(None),
            join: Mutex::new(None),
            lan_agent: Mutex::new(None),
            lan_agent_off: std::sync::atomic::AtomicBool::new(false),
            host: Mutex::new(None),
            backend: RwLock::new(backend),
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
