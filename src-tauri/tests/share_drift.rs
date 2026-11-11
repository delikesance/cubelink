//! Fichiers de l'hôte modifiés entre le calcul du manifeste et le transfert (config réécrite par le jeu,
//! mod remplacé) : l'invité ne doit jamais recevoir un fichier qui ne correspond pas au manifeste annoncé.

use std::path::Path;
use std::sync::{Arc, Mutex};

type Events = Arc<Mutex<Vec<(String, serde_json::Value)>>>;
use std::time::Duration;

use core_lib::types::{Loader, LoaderSpec, TransferPhase, TransferProgress};
use cubelink_app::iroh_backend::{start_app, IrohBackend, IrohOptions};
use cubelink_app::p2p_backend::P2pBackend;
use cubelink_app::state::{AppState, EventSink};
use p2p_lib::presence::PresenceConfig;
use p2p_lib::relay::RelayChoice;

struct Node {
    st: Arc<AppState>,
    be: Arc<IrohBackend>,
    events: Events,
}

impl Node {
    async fn start(root: &Path, name: &str) -> Node {
        let events: Events = Arc::default();
        let ev = events.clone();
        let sink: EventSink = Arc::new(move |n, v| ev.lock().unwrap().push((n.to_string(), v)));
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
        Node { st, be, events }
    }
    fn progress(&self) -> Vec<TransferProgress> {
        self.events.lock().unwrap().iter().filter(|(n, _)| n == "transfer://progress").map(|(_, v)| serde_json::from_value(v.clone()).unwrap()).collect()
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
    std::fs::create_dir_all(gd.join("config")).unwrap();
    std::fs::write(gd.join("config/mod.json"), b"{\"v\":1}").unwrap();
    inst.id
}


/// Lance un join et renvoie la dernière progression terminale (Done ou Failed).
async fn join(guest: &Node, host_key: &str) -> TransferProgress {
    let before = guest.progress().len();
    guest.st.join_start(host_key).await.unwrap();
    for _ in 0..300 {
        let p = guest.progress();
        if let Some(t) = p[before..].iter().find(|p| matches!(p.phase, TransferPhase::Done | TransferPhase::Failed)) {
            return t.clone();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("transfert non terminé");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn config_reecrite_par_le_jeu_apres_le_manifeste_ne_fait_pas_echouer() {
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let host = Node::start(da.path(), "Hote").await;
    let guest = Node::start(db.path(), "Invite").await;
    befriend(&guest, &host).await;
    let id = world(&host);
    host.be.host_start_sharing(id.clone(), 0).await.unwrap();
    // Le jeu démarre et réécrit sa config APRÈS le calcul du manifeste.
    let gd = host.st.instances.game_dir(&id).unwrap();
    std::fs::write(gd.join("config/mod.json"), b"{\"v\":2,\"window\":[1,2,3]}").unwrap();

    let t = join(&guest, &host.key()).await;
    assert_eq!(t.phase, TransferPhase::Done, "{t:?}");
    let g = guest.st.instances.list().into_iter().next().expect("instance invitée");
    let content = std::fs::read(guest.st.instances.game_dir(&g.id).unwrap().join("config/mod.json")).unwrap();
    assert_eq!(content, b"{\"v\":1}", "l'invité reçoit exactement le fichier annoncé (instantané)");
    host.be.shutdown().await;
    guest.be.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mod_remplace_apres_le_manifeste_echec_clair_puis_reessai_avec_manifeste_frais() {
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let host = Node::start(da.path(), "Hote").await;
    let guest = Node::start(db.path(), "Invite").await;
    befriend(&guest, &host).await;
    let id = world(&host);
    host.be.host_start_sharing(id.clone(), 0).await.unwrap();
    let gd = host.st.instances.game_dir(&id).unwrap();
    // Un mod est mis à jour pendant que le partage est actif : le manifeste annoncé est périmé.
    std::fs::write(gd.join("mods/a.jar"), vec![9u8; 6000]).unwrap();

    // Jamais de fichier non conforme : le premier essai échoue avec une raison lisible (pas d'« empreinte »).
    let t = join(&guest, &host.key()).await;
    assert_eq!(t.phase, TransferPhase::Failed, "{t:?}");
    let msg = t.error.clone().unwrap_or_default();
    assert!(msg.contains("a changé chez ton ami"), "{msg}");
    assert!(guest.st.instances.list().is_empty(), "rien d'installé après un échec");

    // L'hôte a recalculé son manifeste : « Réessayer » reprend un état frais et réussit.
    for _ in 0..50 {
        let plan = guest.st.join_plan(&host.key()).await.unwrap();
        if plan.manifest.files.iter().any(|f| f.path == "mods/a.jar" && f.size == 6000) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let t = join(&guest, &host.key()).await;
    assert_eq!(t.phase, TransferPhase::Done, "{t:?}");
    let g = guest.st.instances.list().into_iter().next().expect("instance invitée");
    assert_eq!(std::fs::read(guest.st.instances.game_dir(&g.id).unwrap().join("mods/a.jar")).unwrap(), vec![9u8; 6000]);
    host.be.shutdown().await;
    guest.be.shutdown().await;
}
