//! Erreurs distinctes côté invité : ami hors ligne, ne partage pas encore, partage de mods désactivé,
//! pas (ou plus) ami. Deux nœuds loopback, relais désactivé.

use std::time::{Duration, Instant};

use cubelink_app::p2p_backend::P2pBackend;
use p2p_lib::reach::Reach;

mod common;
use common::{allow, befriend, world, Node};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raisons_distinctes_cote_invite() {
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let host = Node::start(da.path(), "Hote").await;
    let guest = Node::start(db.path(), "Invite").await;
    befriend(&guest, &host).await;
    let hk = host.key();
    // Règle de jonction : l'invité est invité pour cette partie (sinon il n'obtient rien, voir join_rule.rs).
    allow(&host, &guest);

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
