//! État de l'application : chemins, index du magasin, instances, identité, amis, session de jeu.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

use core_lib::auth::device::DeviceCode;
use core_lib::instance::InstanceStore;
use core_lib::store::index::StoreIndex;
use core_lib::types::{Profile, Settings, TransferProgress, TrustLevel};
use p2p_lib::friends_db::FriendsDb;
use p2p_lib::identity::Identity;
use serde::Serialize;

use crate::p2p_backend::{P2pBackend, TransferControl};

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
    pub join: Mutex<Option<JoinSession>>,
    backend: RwLock<Arc<dyn P2pBackend>>,
    /// Test : saute l'installation du jeu (Mojang, loader, Java) après un transfert d'invité.
    pub skip_game_install: AtomicBool,
    pub emit: EventSink,
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
            join: Mutex::new(None),
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
