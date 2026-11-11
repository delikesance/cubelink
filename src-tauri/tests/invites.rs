//! Invitations « rejoins mon monde » de bout en bout : deux nœuds loopback (relais désactivé), vrais handlers iroh.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use core_lib::types::{Loader, LoaderSpec};
use cubelink_app::iroh_backend::{start_app, IrohBackend, IrohOptions};
use cubelink_app::state::{AppState, EventSink};
use p2p_lib::presence::PresenceConfig;
use p2p_lib::relay::RelayChoice;

type Events = Arc<Mutex<Vec<(String, serde_json::Value)>>>;

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
    fn key(&self) -> String {
        self.st.identity.public_key_hex()
    }
    fn count(&self, ev: &str) -> usize {
        self.events.lock().unwrap().iter().filter(|(n, _)| n == ev).count()
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

/// A demande, B accepte : les deux sont amis.
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
async fn invitation_recue_limitee_annulee_et_refusee_si_non_ami() {
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let host = Node::start(da.path(), "Hote").await;
    let guest = Node::start(db.path(), "Invite").await;
    befriend(&host, &guest).await;
    wait_for("ami chez l'invité", 20, || guest.st.friends.is_friend(&host.key()).unwrap()).await;

    // Sans monde ouvert : refus clair.
    assert!(host.st.friend_invite(&guest.key()).await.unwrap_err().contains("monde"));

    let id = world(&host);
    host.st.host_start_sharing(&id, 0).await.unwrap();
    assert!(host.st.friend_invite(&guest.key()).await.is_err(), "partage sans monde ouvert");
    host.st.host_world_opened(&id, 54321);

    host.st.friend_invite(&guest.key()).await.unwrap();
    wait_for("invitation reçue", 10, || !guest.st.friend_invites().is_empty()).await;
    let q = guest.st.friend_invites();
    assert_eq!(q.len(), 1);
    assert_eq!((q[0].public_key.as_str(), q[0].world_name.as_str(), q[0].minecraft.as_str()), (host.key().as_str(), "Monde", "1.21.1"));
    assert!(!q[0].name.is_empty());
    assert_eq!(guest.count("friend://invite"), 1);
    assert!(guest.count("friend://invites") >= 1);

    // Le renvoi immédiat est limité côté émetteur ; la file reste à une seule invitation.
    assert!(host.st.friend_invite(&guest.key()).await.unwrap_err().contains("patientez"));
    assert_eq!(guest.st.friend_invites().len(), 1);

    // L'hôte arrête de partager : l'invitation est annulée chez l'ami.
    host.st.host_stop_sharing();
    wait_for("invitation annulée", 10, || guest.st.friend_invites().is_empty()).await;

    // L'invité retire l'hôte de ses amis : l'invitation est refusée, rien n'arrive.
    host.st.host_start_sharing(&id, 0).await.unwrap();
    host.st.host_world_opened(&id, 54321);
    guest.st.friends.remove_friend(&host.key()).unwrap();
    assert!(host.st.friend_invite(&guest.key()).await.is_err());
    assert!(guest.st.friend_invites().is_empty());

    host.be.shutdown().await;
    guest.be.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rejoindre_un_ami_echoue_vite_sans_toucher_au_jeu_en_cours() {
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let host = Node::start(da.path(), "Hote").await;
    let guest = Node::start(db.path(), "Invite").await;
    befriend(&guest, &host).await;
    // Un jeu « tourne » chez l'invité ; l'hôte ne partage rien : le jeu n'est pas fermé.
    *guest.st.game.lock().unwrap() = Some("en-cours".into());
    let e = guest.st.game_join_friend(&host.key(), None).await.unwrap_err();
    assert!(!e.is_empty());
    assert_eq!(guest.st.game.lock().unwrap().as_deref(), Some("en-cours"));
    *guest.st.game.lock().unwrap() = None;
    host.be.shutdown().await;
    guest.be.shutdown().await;
}
