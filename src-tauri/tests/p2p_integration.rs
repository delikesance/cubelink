//! Deux AppState / IrohBackend en loopback (relais désactivé) : ami, présence, partage, join, coupure, refus.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use core_lib::types::{Loader, LoaderSpec, Presence, TransferPhase, TransferProgress};
use cubelink_app::iroh_backend::{start_app, IrohBackend, IrohOptions, ManifestUnknownFn, UnknownFn};
use cubelink_app::p2p_backend::P2pBackend;
use cubelink_app::state::{AppState, EventSink};
use p2p_lib::presence::PresenceConfig;
use p2p_lib::relay::RelayChoice;
use p2p_lib::transfer::resume::{ResumeConfig, RetryPolicy};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

type Events = Arc<Mutex<Vec<(String, serde_json::Value)>>>;

struct Node {
    _dir: tempfile::TempDir,
    st: Arc<AppState>,
    be: Arc<IrohBackend>,
    events: Events,
}

impl Node {
    async fn new(name: &str) -> Node {
        let dir = tempfile::tempdir().unwrap();
        let events: Events = Arc::default();
        let ev = events.clone();
        let sink: EventSink = Arc::new(move |n, v| ev.lock().unwrap().push((n.to_string(), v)));
        let unknown: UnknownFn = Arc::new(|files, _| {
            Box::pin(async move { files.into_iter().filter(|f| f.path.contains("homemade")).map(|f| f.path).collect() })
        });
        let pre: ManifestUnknownFn = Arc::new(|files| {
            Box::pin(async move {
                let mut r = core_lib::unknown_mods::UnknownReport::default();
                for f in files {
                    assert!(f.sha1.is_some(), "l'hôte annonce le sha1 des jars");
                    if f.path.contains("homemade") {
                        r.unknown.push(f.path);
                    }
                }
                r
            })
        });
        let opts = IrohOptions {
            manifest_unknown_finder: pre,
            relay: RelayChoice::Disabled,
            advertise_addrs: Some(true),
            unknown_finder: unknown,
            resume: ResumeConfig {
                retry: RetryPolicy { max_retries: 5, base: Duration::from_millis(20), cap: Duration::from_millis(100), jitter: 0.0 },
                ..ResumeConfig::default()
            },
            presence: PresenceConfig {
                interval: Duration::from_millis(300),
                timeout: Duration::from_secs(2),
                failures_to_offline: 2,
                max_concurrent: 4,
            },
            ..IrohOptions::default()
        };
        let (st, be) = start_app(dir.path(), sink, opts).await.unwrap();
        st.auth_offline_login(name, true).unwrap();
        st.skip_game_install.store(true, std::sync::atomic::Ordering::SeqCst);
        Node { _dir: dir, st, be, events }
    }
    fn key(&self) -> String {
        self.st.identity.public_key_hex()
    }
    fn events(&self, name: &str) -> Vec<serde_json::Value> {
        self.events.lock().unwrap().iter().filter(|(n, _)| n == name).map(|(_, v)| v.clone()).collect()
    }
    fn progress(&self) -> Vec<TransferProgress> {
        self.events("transfer://progress").into_iter().map(|v| serde_json::from_value(v).unwrap()).collect()
    }
}

async fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    for _ in 0..200 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("délai dépassé : {what}");
}

fn make_world(host: &Node) -> (String, Vec<(String, Vec<u8>)>) {
    let inst = host
        .st
        .instances
        .create("Monde de Bob", "1.21.1", LoaderSpec { loader: Loader::Fabric, version: "0.16.0".into() }, 4096)
        .unwrap();
    let gd = host.st.instances.game_dir(&inst.id).unwrap();
    std::fs::create_dir_all(gd.join("mods")).unwrap();
    let mut files = vec![];
    for (i, n) in ["mods/alpha.jar", "mods/beta.jar", "mods/homemade.jar"].iter().enumerate() {
        let data: Vec<u8> = (0..200_000u32 + i as u32 * 1000).map(|x| (x as usize * (i + 3) % 251) as u8).collect();
        std::fs::write(gd.join(n), &data).unwrap();
        files.push((n.to_string(), data));
    }
    (inst.id, files)
}

async fn echo_server() -> u16 {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else { break };
            tokio::spawn(async move {
                let mut b = [0u8; 64];
                while let Ok(n) = s.read(&mut b).await {
                    if n == 0 || s.write_all(&b[..n]).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    port
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn amis_presence_partage_join_coupure_et_refus() {
    let a = Node::new("Alice").await;
    let b = Node::new("Bob").await;

    // 1. A envoie une demande à B par code ami ; B reçoit l'événement et accepte.
    let code_a = a.st.friend_send_request(&b.st.friend_my_code().0).await.unwrap();
    wait_for("friend://request chez B", || !b.events("friend://request").is_empty()).await;
    let req = &b.events("friend://request")[0];
    assert_eq!(req["public_key"], a.key());
    assert_eq!(req["verify_code"], serde_json::json!(code_a.0), "même code de vérification des deux côtés");
    assert_eq!(b.st.friend_requests(0, 10, None).unwrap().total, 1);
    b.st.friend_accept(&a.key()).await.unwrap();
    wait_for("ami chez A", || a.st.friends.is_friend(&b.key()).unwrap()).await;
    assert!(b.st.friends.is_friend(&a.key()).unwrap());

    // 2. Présence : B apparaît « en ligne » chez A, avec un événement.
    wait_for("B en ligne", || a.st.friend_list().unwrap().iter().any(|f| f.presence == Presence::Online)).await;
    assert!(a.events("friend://presence").iter().any(|e| e["public_key"] == b.key() && e["presence"] == "online"));

    // 3. B partage un monde (2 mods + un mod maison) et expose un faux monde LAN.
    let (inst_id, files) = make_world(&b);
    let world_port = echo_server().await;
    b.be.host_start_sharing(inst_id, world_port).await.unwrap();

    // 4. Plan puis join avec une coupure au 2e fichier.
    let plan = a.st.join_plan(&b.key()).await.unwrap();
    assert_eq!((plan.manifest.files.len(), plan.cached, plan.to_receive), (3, 0, 3));
    assert_eq!(plan.unknown_mods, vec!["mods/homemade.jar".to_string()], "avertissement avant téléchargement");
    assert!(plan.unknown_unchecked.is_empty());
    a.be.inject_cut_at_fetch(2);
    a.st.join_start(&b.key()).await.unwrap();
    wait_for("transfert terminé", || a.progress().iter().any(|p| matches!(p.phase, TransferPhase::Done | TransferPhase::Failed))).await;
    let prog = a.progress();
    let last = prog.last().unwrap();
    assert_eq!(last.phase, TransferPhase::Done, "{last:?}");
    assert!(a.be.cut_consumed(), "la coupure injectée a eu lieu, puis reprise");
    assert!(prog.iter().any(|p| p.phase == TransferPhase::Receiving && p.bytes_done > 0));
    assert!(!last.via_relay);
    assert_eq!(last.files_done, 3);
    // Avertissement fiable calculé côté invité après réception.
    let um = a.events("transfer://unknown_mods");
    assert_eq!(um.len(), 1);
    assert_eq!(um[0]["unknown_mods"], serde_json::json!(["mods/homemade.jar"]));

    // 5. Instance invitée créée, fichiers identiques (hash vérifiés à la réception), hôte intact.
    let guests = a.st.instances.list();
    assert_eq!(guests.len(), 1);
    assert!(guests[0].name.contains("invité"));
    let gd = a.st.instances.game_dir(&guests[0].id).unwrap();
    for (n, data) in &files {
        assert_eq!(&std::fs::read(gd.join(n)).unwrap(), data, "{n}");
    }
    assert_eq!(b.st.instances.list().len(), 1);
    let again = a.st.join_plan(&b.key()).await.unwrap();
    assert_eq!((again.cached, again.to_receive), (3, 0));

    // 6. Tunnel : A atteint le monde de B (faux serveur d'écho) par le tunnel.
    let tun = a.be.join_tunnel(&b.key()).await.unwrap();
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", tun.local_port)).await.unwrap();
    s.write_all(b"salut").await.unwrap();
    let mut buf = [0u8; 5];
    tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut buf)).await.unwrap().unwrap();
    assert_eq!(&buf, b"salut");
    drop(s);
    tun.stop().await;

    // 7. Un pair non ami est refusé (manifeste, tunnel).
    let c = Node::new("Carl").await;
    c.be.add_peer_addr(b.be.endpoint_addr());
    assert!(c.be.fetch_manifest(b.key()).await.is_err());
    if let Ok(t) = c.be.join_tunnel(&b.key()).await {
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", t.local_port)).await.unwrap();
        let _ = s.write_all(b"x").await;
        let mut buf = [0u8; 1];
        let r = tokio::time::timeout(Duration::from_secs(3), s.read(&mut buf)).await;
        assert!(!matches!(r, Ok(Ok(n)) if n > 0), "un non-ami ne doit rien recevoir du monde");
    }

    // 8. share_mods = false : même un ami n'obtient plus le manifeste.
    b.st.settings.lock().unwrap().share_mods = false;
    assert!(a.st.join_plan(&b.key()).await.is_err());
    b.be.host_stop_sharing();
    a.be.shutdown().await;
    b.be.shutdown().await;
    c.be.shutdown().await;
}

/// Join complet via les handlers de p2p partagés dans UN Router : changement d'instance partagée à chaud
/// (`handlers_dynamic`), arrêt du partage, adresses du code ami (relais désactivé).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn join_handlers_partages_instance_dynamique() {
    let a = Node::new("Alice").await;
    let b = Node::new("Bob").await;
    // Relais désactivé : le code ami porte les adresses directes ; sinon (advertise Some(false)) il n'en porte pas.
    let payload = p2p_lib::friend_code::parse(&b.st.friend_my_code().0).unwrap();
    assert!(!payload.addrs.is_empty(), "relais désactivé : adresses directes dans le code");
    assert!(payload.relay.is_none());
    a.st.friend_send_request(&b.st.friend_my_code().0).await.unwrap();
    wait_for("demande chez B", || !b.events("friend://request").is_empty()).await;
    b.st.friend_accept(&a.key()).await.unwrap();
    wait_for("ami chez A", || a.st.friends.is_friend(&b.key()).unwrap()).await;

    let (first, _) = make_world(&b);
    b.be.host_start_sharing(first, 0).await.unwrap();
    assert_eq!(a.st.join_plan(&b.key()).await.unwrap().manifest.files.len(), 3);

    // Seconde instance (1 fichier) partagée sans recréer le Router.
    let inst2 = b.st.instances.create("Autre", "1.21.1", LoaderSpec { loader: Loader::Fabric, version: "0.16.0".into() }, 4096).unwrap();
    let gd2 = b.st.instances.game_dir(&inst2.id).unwrap();
    std::fs::create_dir_all(gd2.join("mods")).unwrap();
    let data: Vec<u8> = (0..50_000u32).map(|x| (x % 199) as u8).collect();
    std::fs::write(gd2.join("mods/solo.jar"), &data).unwrap();
    b.be.host_start_sharing(inst2.id.clone(), 0).await.unwrap();
    let plan = a.st.join_plan(&b.key()).await.unwrap();
    assert_eq!(plan.manifest.files.len(), 1);

    a.st.join_start(&b.key()).await.unwrap();
    wait_for("transfert terminé", || a.progress().iter().any(|p| matches!(p.phase, TransferPhase::Done | TransferPhase::Failed))).await;
    assert_eq!(a.progress().last().unwrap().phase, TransferPhase::Done);
    let guests = a.st.instances.list();
    assert_eq!(guests.len(), 1);
    let gd = a.st.instances.game_dir(&guests[0].id).unwrap();
    assert_eq!(std::fs::read(gd.join("mods/solo.jar")).unwrap(), data);

    // Plus rien de partagé : manifeste refusé.
    b.be.host_stop_sharing();
    assert!(a.st.join_plan(&b.key()).await.is_err());
    a.be.shutdown().await;
    b.be.shutdown().await;
}
