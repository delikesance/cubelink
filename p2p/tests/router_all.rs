//! Un seul `Router` iroh par endpoint porte tous les protocoles : ami, présence, xfer, blobs,
//! manifeste, tunnel. Deux endpoints loopback, relais désactivé, aucun réseau externe.
//! `cargo test -p cubelink-p2p --test router_all -- --ignored`

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use core_lib::types::{Error, Presence, Loader, LoaderSpec, Manifest, ManifestFile};
use iroh::endpoint::presets;
use iroh::protocol::Router;
use iroh::{Endpoint, RelayMode, SecretKey};
use p2p_lib::friend_code;
use p2p_lib::friend_proto::{self, FriendConfig, FriendEvent, FriendService};
use p2p_lib::friends_db::FriendsDb;
use p2p_lib::identity::Identity;
use p2p_lib::manifest_channel::{fetch_manifest, ManifestServer, MANIFEST_ALPN};
use p2p_lib::presence::{PresenceService, PRESENCE_ALPN};
use p2p_lib::relay::{endpoint_addr_from_payload, endpoint_addr_info};
use p2p_lib::transfer::send::{
    AllowList, BlobTransport, FetchRequest, Host, IrohTransport, ServeIndex, ServeLimits, CONTROL_ALPN,
};
use p2p_lib::tunnel::{AllowKeys, GuestTunnel, HostTunnel, TunnelConfig, TUNNEL_ALPN};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

struct Node {
    _dir: tempfile::TempDir,
    ident: Arc<Identity>,
    ep: Endpoint,
    db: Arc<FriendsDb>,
    friends: FriendService,
    events: Arc<Mutex<Vec<FriendEvent>>>,
    presence: Arc<PresenceService>,
    dir_path: std::path::PathBuf,
}

fn all_alpns() -> Vec<Vec<u8>> {
    [friend_proto::ALPN, PRESENCE_ALPN, CONTROL_ALPN, iroh_blobs::ALPN, MANIFEST_ALPN, TUNNEL_ALPN]
        .iter()
        .map(|a| a.to_vec())
        .collect()
}

async fn node() -> Node {
    let dir = tempfile::tempdir().unwrap();
    let ident = Arc::new(Identity::load_or_create(dir.path()).unwrap());
    let ep = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .secret_key(SecretKey::from_bytes(&ident.secret_bytes()))
        .alpns(all_alpns())
        .bind()
        .await
        .unwrap();
    let db = Arc::new(FriendsDb::open_in_memory().unwrap());
    let events: Arc<Mutex<Vec<FriendEvent>>> = Arc::default();
    let ev = events.clone();
    let sink: friend_proto::EventSink = Arc::new(move |e| ev.lock().unwrap().push(e));
    let friends = FriendService::attach(ep.clone(), ident.clone(), db.clone(), sink, FriendConfig::default()).unwrap();
    let presence = Arc::new(PresenceService::start(ep.clone(), db.clone(), Arc::new(|_| {})));
    let dir_path = dir.path().to_path_buf();
    Node { _dir: dir, ident, ep, db, friends, events, presence, dir_path }
}

fn sha(d: &[u8]) -> String {
    hex::encode(Sha256::digest(d))
}

async fn echo_world() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            tokio::spawn(async move {
                let (mut r, mut w) = s.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    port
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "ouvre de vrais sockets UDP locaux ; lancer avec --ignored"]
async fn one_router_carries_every_protocol() {
    let (host, guest) = (node().await, node().await);
    let guest_hex = guest.ident.public_key_hex();
    let guest_key = guest.ident.public_key().to_bytes();

    // Hôte : fichier + manifeste partagés.
    let game = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(game.path().join("mods")).unwrap();
    let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(game.path().join("mods/a.jar"), &data).unwrap();
    let manifest = Manifest {
        name: "Monde".into(),
        minecraft: "1.21.1".into(),
        loader: LoaderSpec { loader: Loader::Fabric, version: "0.16.0".into() },
        java_major: 21,
        files: vec![ManifestFile { path: "mods/a.jar".into(), size: data.len() as u64, sha256: sha(&data), sha1: None, cf_fingerprint: None }],
    };

    let transport = IrohTransport::new(host.ep.clone(), host.dir_path.join("host-blobs")).await.unwrap();
    let index = ServeIndex::from_manifest(&manifest, game.path()).unwrap();
    let xfer_host = Host::new(index, Arc::new(AllowList(vec![guest_hex.clone()])), ServeLimits::default());
    let (control, blobs) = transport.handlers(xfer_host);

    let msrv = ManifestServer::new(Arc::new(AllowList(vec![guest_hex.clone()])));
    msrv.set(Some(&manifest)).unwrap();

    let world = echo_world().await;
    let tunnel = HostTunnel::attach(Arc::new(AllowKeys(vec![guest_key])), TunnelConfig::default());
    tunnel.set_world_port(world);

    let router_host = Router::builder(host.ep.clone())
        .accept(friend_proto::ALPN, host.friends.handler())
        .accept(PRESENCE_ALPN, host.presence.handler())
        .accept(CONTROL_ALPN, control)
        .accept(iroh_blobs::ALPN, blobs)
        .accept(MANIFEST_ALPN, msrv.handler())
        .accept(TUNNEL_ALPN, tunnel.handler())
        .spawn();
    let router_guest = Router::builder(guest.ep.clone())
        .accept(friend_proto::ALPN, guest.friends.handler())
        .accept(PRESENCE_ALPN, guest.presence.handler())
        .spawn();

    // Adresse de l'hôte via le code ami (EndpointAddrInfo -> payload -> EndpointAddr).
    let payload = endpoint_addr_info(&host.ep.addr()).into_payload(host.ident.public_key().to_bytes(), 0);
    assert!(!payload.addrs.is_empty());
    let code = friend_code::encode(&payload);
    let parsed = friend_code::parse(&code).unwrap();
    let host_addr = endpoint_addr_from_payload(&parsed).unwrap();
    assert_eq!(host_addr.id, host.ep.id());

    // 1) ami : l'invité demande, l'hôte accepte.
    guest.friends.send_request(&code).await.unwrap();
    for _ in 0..100 {
        if host.db.request_count().unwrap() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(host.db.request_count().unwrap(), 1);
    host.friends.accept(&guest_hex).await.unwrap();
    for _ in 0..100 {
        if guest.db.is_friend(&host.ident.public_key_hex()).unwrap() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(guest.db.is_friend(&host.ident.public_key_hex()).unwrap());
    assert!(guest.events.lock().unwrap().iter().any(|e| matches!(e, FriendEvent::Accepted(_))));

    // 2) présence.
    guest.presence.add_addr(host_addr.clone());
    assert_eq!(guest.presence.probe_now(host.ident.public_key().to_bytes()).await, Presence::Online);

    // 3) manifeste (+ pair non autorisé refusé).
    let got = fetch_manifest(&guest.ep, host_addr.clone()).await.unwrap();
    assert_eq!(got, manifest);
    let stranger = node().await;
    assert!(matches!(fetch_manifest(&stranger.ep, host_addr.clone()).await, Err(Error::Auth(_))));

    // 4) xfer + blobs.
    let gt = IrohTransport::new(guest.ep.clone(), guest.dir_path.join("guest-blobs")).await.unwrap();
    let peer = gt.add_peer(host_addr.clone());
    let dest = guest.dir_path.join("a.part");
    let req = FetchRequest { sha256: sha(&data), size: data.len() as u64, dest: dest.clone(), resume_from: 0 };
    gt.fetch(&peer, &req, &|_| {}, &AtomicBool::new(false)).await.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), data);
    let st = IrohTransport::new(stranger.ep.clone(), stranger.dir_path.join("s")).await.unwrap();
    let p2 = st.add_peer(host_addr.clone());
    assert!(st.fetch(&p2, &req, &|_| {}, &AtomicBool::new(false)).await.is_err());

    // 5) tunnel.
    let gtun = GuestTunnel::connect(guest.ep.clone(), host_addr.clone()).await.unwrap();
    let mut s = TcpStream::connect(("127.0.0.1", gtun.local_port)).await.unwrap();
    s.write_all(b"hello tunnel").await.unwrap();
    let mut buf = [0u8; 12];
    tokio::time::timeout(Duration::from_secs(10), s.read_exact(&mut buf)).await.unwrap().unwrap();
    assert_eq!(&buf, b"hello tunnel");
    assert!(tunnel.stats().total_connections >= 1);

    gtun.stop().await;
    tunnel.stop().await;
    let _ = router_guest.shutdown().await;
    let _ = router_host.shutdown().await;
}
