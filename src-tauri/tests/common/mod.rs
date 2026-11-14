//! Outils partagés des tests d'intégration : un nœud (AppState + IrohBackend) en loopback, relais désactivé.
#![allow(dead_code)]

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use core_lib::types::{Loader, LoaderSpec, TransferProgress};
use cubelink_app::iroh_backend::{start_app, IrohBackend, IrohOptions};
use cubelink_app::state::{AppState, EventSink};
use p2p_lib::presence::PresenceConfig;
use p2p_lib::relay::RelayChoice;

pub type Events = Arc<Mutex<Vec<(String, serde_json::Value)>>>;

pub struct Node {
    pub st: Arc<AppState>,
    pub be: Arc<IrohBackend>,
    pub events: Events,
}

impl Node {
    /// Sonde de présence quasi inactive (1 h) : les tests pilotent la présence eux-mêmes.
    pub async fn start(root: &Path, name: &str) -> Node {
        Node::start_with(root, name, Duration::from_secs(3600)).await
    }

    /// Démarre (ou redémarre, avec le même dossier : même identité, même base d'amis) avec l'intervalle de sonde donné.
    pub async fn start_with(root: &Path, name: &str, presence_interval: Duration) -> Node {
        let events: Events = Arc::default();
        let ev = events.clone();
        let sink: EventSink = Arc::new(move |n, v| ev.lock().unwrap().push((n.to_string(), v)));
        let opts = IrohOptions {
            relay: RelayChoice::Disabled,
            advertise_addrs: Some(true),
            presence: PresenceConfig {
                interval: presence_interval,
                timeout: Duration::from_secs(2),
                failures_to_offline: 2,
                max_concurrent: 4,
            },
            ..IrohOptions::default()
        };
        // Le magasin de blobs libère son verrou de façon asynchrone après l'arrêt : on réessaie brièvement.
        let mut tries = 0;
        let (st, be) = loop {
            match start_app(root, sink.clone(), opts.clone()).await {
                Ok(v) => break v,
                Err(e) if e.contains("already open") && tries < 50 => {
                    tries += 1;
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(e) => panic!("{e}"),
            }
        };
        st.auth_offline_login(name).unwrap();
        st.skip_game_install.store(true, std::sync::atomic::Ordering::SeqCst);
        Node { st, be, events }
    }

    pub fn key(&self) -> String {
        self.st.identity.public_key_hex()
    }

    pub fn count(&self, ev: &str) -> usize {
        self.events.lock().unwrap().iter().filter(|(n, _)| n == ev).count()
    }

    pub fn progress(&self) -> Vec<TransferProgress> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|(n, _)| n == "transfer://progress")
            .map(|(_, v)| serde_json::from_value(v.clone()).unwrap())
            .collect()
    }
}

pub async fn wait_for(what: &str, secs: u64, mut f: impl FnMut() -> bool) {
    for _ in 0..secs * 10 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("délai dépassé : {what}");
}

/// A et B deviennent amis (A demande, B accepte).
pub async fn befriend(a: &Node, b: &Node) {
    a.st.friend_send_request(&b.st.friend_my_code().0).await.unwrap();
    wait_for("demande chez B", 20, || b.st.friend_requests(0, 10, None).unwrap().total == 1).await;
    b.st.friend_accept(&a.key()).await.unwrap();
    wait_for("ami chez A", 20, || a.st.friends.is_friend(&b.key()).unwrap()).await;
}

/// Instance vanilla « Monde » de l'hôte avec un mod de 5 000 octets ; renvoie son identifiant.
pub fn world(host: &Node) -> String {
    let inst = host.st.instances.create("Monde", "1.21.1", LoaderSpec { loader: Loader::Vanilla, version: String::new() }, 2048).unwrap();
    let gd = host.st.instances.game_dir(&inst.id).unwrap();
    std::fs::create_dir_all(gd.join("mods")).unwrap();
    std::fs::write(gd.join("mods/a.jar"), vec![7u8; 5000]).unwrap();
    inst.id
}
