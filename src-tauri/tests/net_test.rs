//! Test de connexion (Paramètres → Réseau) sur le vrai réseau : IPv6 publique, port ouvert sur la box (UPnP). Aucun
//! serveur tiers contacté ; le résultat dépend de la box de la machine de test.

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
    // Le résultat dépend de la box : on vérifie seulement que le test se termine dans le délai (5 s de mappage au plus).
    assert!(t0.elapsed() < std::time::Duration::from_secs(15), "test trop long : {r:?}");
    be.shutdown().await;
}
