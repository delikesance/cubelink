//! Erreurs distinctes côté invité : ami hors ligne, ne partage pas encore, partage de mods désactivé,
//! pas (ou plus) ami. Deux nœuds loopback, relais désactivé.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use core_lib::types::{Loader, LoaderSpec};
use cubelink_app::iroh_backend::{start_app, IrohBackend, IrohOptions};
use cubelink_app::p2p_backend::P2pBackend;
use cubelink_app::state::{AppState, EventSink};
use p2p_lib::presence::PresenceConfig;
use p2p_lib::reach::Reach;
use p2p_lib::relay::RelayChoice;

struct Node {
    st: Arc<AppState>,
    be: Arc<IrohBackend>,
}

impl Node {
    async fn start(root: &Path, name: &str) -> Node {
        let events: Arc<Mutex<Vec<(String, serde_json::Value)>>> = Arc::default();
        let sink: EventSink = Arc::new(move |n, v| events.lock().unwrap().push((n.to_string(), v)));
        let opts = IrohOptions {
            relay: RelayChoice::Disabled,
            advertise_addrs: Some(true),
            presence: PresenceConfig {
                interval: Duration::from_secs(3600),
                timeout: Duration::from_secs(2),
                failures_to_offline: 2,
                max_concurrent: 4,
            },
            ..IrohOptions::default()
        };
        let (st, be) = start_app(root, sink, opts).await.unwrap();
        st.auth_offline_login(name).unwrap();
        st.skip_game_install.store(true, std::sync::atomic::Ordering::SeqCst);
        Node { st, be }
    }
    fn key(&self) -> String {
        self.st.identity.public_key_hex()
    }
}

async fn wait_for(what: &str, secs: u64, mut f: impl FnMut() -> bool) {
    for _ in 0..secs * 10 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("délai dépassé : {what}");
}

/// A et B deviennent amis (A demande, B accepte).
async fn befriend(a: &Node, b: &Node) {
    a.st.friend_send_request(&b.st.friend_my_code().0).await.unwrap();
    wait_for("demande chez B", 20, || b.st.friend_requests(0, 10, None).unwrap().total == 1).await;
    b.st.friend_accept(&a.key()).await.unwrap();
    wait_for("ami chez A", 20, || a.st.friends.is_friend(&b.key()).unwrap()).await;
}

fn world(host: &Node) -> String {
    let inst = host.st.instances.create("Monde", "1.21.1", LoaderSpec { loader: Loader::Vanilla, version: String::new() }, 2048).unwrap();
    let gd = host.st.instances.game_dir(&inst.id).unwrap();
    std::fs::create_dir_all(gd.join("mods")).unwrap();
    std::fs::write(gd.join("mods/a.jar"), vec![7u8; 5000]).unwrap();
    inst.id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raisons_distinctes_cote_invite() {
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let host = Node::start(da.path(), "Hote").await;
    let guest = Node::start(db.path(), "Invite").await;
    befriend(&guest, &host).await;
    let hk = host.key();

    // 1. Hôte en ligne mais rien d'ouvert : message dédié, pour le plan comme pour le tunnel.
    let e = guest.st.join_plan(&hk).await.unwrap_err();
    assert_eq!(e, Reach::NothingShared.message(), "plan : {e}");
    let e = guest.st.join_open_tunnel(&hk).await.unwrap_err();
    assert_eq!(e, Reach::NothingShared.message(), "tunnel : {e}");

    // 2. Monde partagé : le plan fonctionne.
    let id = world(&host);
    host.be.host_start_sharing(id, 0).await.unwrap();
    assert!(guest.st.join_plan(&hk).await.is_ok());
    // Fichiers partagés mais port monde 0 : le tunnel est refusé avec le message dédié.
    let e = guest.st.join_open_tunnel(&hk).await.unwrap_err();
    assert_eq!(e, Reach::NothingShared.message(), "tunnel sans monde : {e}");

    // 3. Partage de mods désactivé chez l'hôte (après coup) : message dédié.
    let mut s = host.st.settings_get();
    s.share_mods = false;
    host.st.settings_set(s.clone()).unwrap();
    let e = guest.st.join_plan(&hk).await.unwrap_err();
    assert_eq!(e, Reach::SharingOff.message(), "{e}");
    s.share_mods = true;
    host.st.settings_set(s).unwrap();
    assert!(guest.st.join_plan(&hk).await.is_ok());

    // 4. L'hôte retire l'invité de ses amis : « pas ami », pas « hors ligne ».
    host.st.friends.remove_friend(&guest.key()).unwrap();
    let t = Instant::now();
    let e = guest.st.join_plan(&hk).await.unwrap_err();
    assert_eq!(e, Reach::NotFriend.message(), "{e}");
    assert!(t.elapsed() < Duration::from_secs(5), "échec rapide : {:?}", t.elapsed());

    host.be.shutdown().await;
    guest.be.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ami_hors_ligne_echoue_vite_avec_un_message_clair() {
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let host = Node::start(da.path(), "Hote").await;
    let guest = Node::start(db.path(), "Invite").await;
    befriend(&guest, &host).await;
    let hk = host.key();
    host.be.shutdown().await;
    drop(host);

    let t = Instant::now();
    let e = guest.st.join_plan(&hk).await.unwrap_err();
    assert_eq!(e, Reach::Offline.message(), "{e}");
    let e2 = guest.st.join_open_tunnel(&hk).await.unwrap_err();
    assert_eq!(e2, Reach::Offline.message(), "{e2}");
    // Deux échecs, chacun borné par REACH_TIMEOUT (8 s) : jamais 30 s.
    assert!(t.elapsed() < Duration::from_secs(20), "{:?}", t.elapsed());
    guest.be.shutdown().await;
}
