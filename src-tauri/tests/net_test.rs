//! Test de connexion (Paramètres → Réseau) sur le vrai réseau : relais n0, rapport iroh. Réseau requis.

use std::sync::Arc;
use std::time::Instant;

use cubelink_app::iroh_backend::{start_app, IrohOptions};
use cubelink_app::p2p_backend::P2pBackend;
use cubelink_app::state::EventSink;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "réseau réel"]
async fn net_test_reel() {
    let dir = tempfile::tempdir().unwrap();
    let sink: EventSink = Arc::new(|_, _| {});
    let (_st, be) = start_app(dir.path(), sink, IrohOptions::default()).await.unwrap();
    let t0 = Instant::now();
    let r = be.net_test().await.unwrap();
    println!("{:?} en {:?}", r, t0.elapsed());
    assert!(r.relay_connected || r.udp, "ni relais ni UDP : {r:?}");
    be.shutdown().await;
}
