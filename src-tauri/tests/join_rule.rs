//! Règle de jonction (docs/hearth.md § 2) de bout en bout : trois nœuds loopback (relais désactivé), vrais handlers
//! iroh. Un ami ne rejoint la partie de l'hôte (manifeste, fichiers, tunnel) que s'il est invité pour cette partie ou si
//! sa demande à rejoindre a été acceptée ; l'accès tombe à `host_stop_sharing`.

mod common;
use std::time::Duration;

use common::{befriend, wait_for, world, Node};
use p2p_lib::join_request::JoinAnswer;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Faux monde LAN : écho TCP sur un port local libre.
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

/// Vrai si `guest` atteint le monde de l'hôte par le tunnel (le faux monde répond à son écho).
async fn tunnel_works(guest: &Node, host_key: &str) -> bool {
    let Ok(t) = guest.be.join_tunnel(host_key).await else { return false };
    let ok = async {
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", t.local_port)).await.ok()?;
        s.write_all(b"salut").await.ok()?;
        let mut buf = [0u8; 5];
        tokio::time::timeout(Duration::from_secs(3), s.read_exact(&mut buf)).await.ok()?.ok()?;
        (&buf == b"salut").then_some(())
    }
    .await
    .is_some();
    t.stop().await;
    ok
}

async fn manifest_ok(guest: &Node, host_key: &str) -> bool {
    guest.st.join_plan(host_key).await.is_ok()
}

fn request_events(host: &Node) -> usize {
    host.count("friend://join_request")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn regle_de_jonction_invitation_demande_et_fin_de_partie() {
    let (dh, dg, dg2) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let host = Node::start(dh.path(), "Hote").await;
    let guest = Node::start(dg.path(), "Invite").await;
    let guest2 = Node::start(dg2.path(), "Demandeur").await;
    befriend(&guest, &host).await;
    befriend(&guest2, &host).await;
    wait_for("amis chez l'hôte", 20, || host.st.friends.is_friend(&guest.key()).unwrap() && host.st.friends.is_friend(&guest2.key()).unwrap()).await;
    let hk = host.key();

    // Rien n'est partagé : une demande est refusée tout de suite, sans rien afficher chez l'hôte.
    assert_eq!(guest.st.friend_join_request(&hk).await.unwrap(), JoinAnswer::Refused);
    assert_eq!(request_events(&host), 0);

    // L'hôte partage son monde (fichiers + monde LAN ouvert).
    let id = world(&host);
    host.st.host_start_sharing(&id, u32::from(echo_server().await)).await.unwrap();

    // 1. Ami non invité : ni manifeste, ni tunnel.
    assert!(!manifest_ok(&guest, &hk).await, "manifeste refusé à un ami non invité");
    assert!(!tunnel_works(&guest, &hk).await, "tunnel refusé à un ami non invité");
    assert!(!manifest_ok(&guest2, &hk).await);

    // 2. Demande refusée : toujours rien. La même clé ne peut pas redemander avant 10 min.
    let g = guest.st.clone();
    let hk2 = hk.clone();
    let ask = tokio::spawn(async move { g.friend_join_request(&hk2).await });
    wait_for("demande chez l'hôte", 10, || request_events(&host) == 1).await;
    let pending = host.st.friend_join_requests();
    assert_eq!((pending.len(), pending[0].public_key.as_str()), (1, guest.key().as_str()));
    assert!(!pending[0].name.is_empty());
    // Une seconde demande pendant l'attente est rejetée côté invité (une seule à la fois).
    assert!(guest.st.friend_join_request(&hk).await.unwrap_err().contains("déjà"));
    host.st.friend_join_request_answer(&guest.key(), false).unwrap();
    assert_eq!(ask.await.unwrap().unwrap(), JoinAnswer::Refused);
    wait_for("file des demandes vidée", 10, || host.st.friend_join_requests().is_empty()).await;
    assert!(!manifest_ok(&guest, &hk).await);
    assert!(!tunnel_works(&guest, &hk).await);
    // Redemande trop tôt : refus immédiat, l'hôte n'est pas dérangé.
    assert_eq!(guest.st.friend_join_request(&hk).await.unwrap(), JoinAnswer::Refused);
    assert_eq!(request_events(&host), 1, "pas de nouvelle demande chez l'hôte");
    // Décider d'une demande qui n'existe plus échoue clairement.
    assert!(host.st.friend_join_request_answer(&guest.key(), true).unwrap_err().contains("plus valable"));
    assert!(!manifest_ok(&guest, &hk).await, "accepter une demande inexistante ne donne rien");

    // 3. Demande acceptée : le demandeur obtient manifeste et tunnel.
    let g2 = guest2.st.clone();
    let hk2 = hk.clone();
    let ask = tokio::spawn(async move { g2.friend_join_request(&hk2).await });
    wait_for("demande de Demandeur", 10, || request_events(&host) == 2).await;
    assert!(host.events.lock().unwrap().iter().any(|(n, v)| n == "friend://join_requests" && v.as_array().is_some_and(|a| a.len() == 1)));
    assert!(!manifest_ok(&guest2, &hk).await, "pas d'accès tant que l'hôte n'a pas répondu");
    host.st.friend_join_request_answer(&guest2.key(), true).unwrap();
    assert_eq!(ask.await.unwrap().unwrap(), JoinAnswer::Accepted);
    assert!(manifest_ok(&guest2, &hk).await, "manifeste accordé après acceptation");
    assert!(tunnel_works(&guest2, &hk).await, "tunnel accordé après acceptation");
    // Déjà accepté : une nouvelle demande est acceptée sans déranger l'hôte.
    assert_eq!(guest2.st.friend_join_request(&hk).await.unwrap(), JoinAnswer::Accepted);
    assert_eq!(request_events(&host), 2);
    // L'autre ami reste dehors.
    assert!(!manifest_ok(&guest, &hk).await);

    // 4. Invitation : l'ami refusé plus tôt entre aussitôt (une invitation n'est pas limitée par le refus).
    host.st.friend_invite(&guest.key()).await.unwrap();
    wait_for("invitation reçue", 10, || !guest.st.friend_invites().is_empty()).await;
    assert!(manifest_ok(&guest, &hk).await, "manifeste accordé à l'invité");
    assert!(tunnel_works(&guest, &hk).await, "tunnel accordé à l'invité");

    // 5. Fin de partie : l'accès est retiré pour tous ; la partie suivante repart de zéro.
    host.st.host_stop_sharing();
    assert!(!manifest_ok(&guest, &hk).await);
    assert!(!manifest_ok(&guest2, &hk).await);
    assert!(!tunnel_works(&guest, &hk).await);
    assert!(!tunnel_works(&guest2, &hk).await);
    host.st.host_start_sharing(&id, u32::from(echo_server().await)).await.unwrap();
    assert!(!manifest_ok(&guest, &hk).await, "l'invitation de la partie précédente ne vaut plus");
    assert!(!manifest_ok(&guest2, &hk).await, "la demande acceptée de la partie précédente ne vaut plus");
    assert!(!tunnel_works(&guest2, &hk).await);

    host.be.shutdown().await;
    guest.be.shutdown().await;
    guest2.be.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn non_ami_ferme_sans_reponse_et_arret_du_partage_annule_les_demandes() {
    let (dh, dg, dc) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let host = Node::start(dh.path(), "Hote").await;
    let guest = Node::start(dg.path(), "Invite").await;
    let stranger = Node::start(dc.path(), "Inconnu").await;
    befriend(&guest, &host).await;
    wait_for("ami chez l'hôte", 20, || host.st.friends.is_friend(&guest.key()).unwrap()).await;
    let id = world(&host);
    host.st.host_start_sharing(&id, u32::from(echo_server().await)).await.unwrap();
    let hk = host.key();

    // Un non-ami n'a aucune réponse, et l'hôte ne voit rien. Il ne passe pas non plus par `friend_join_request`
    // (qui exige un ami) : on utilise directement le protocole.
    stranger.be.add_peer_addr(host.be.endpoint_addr());
    let r = cubelink_app::p2p_backend::P2pBackend::send_join_request(stranger.be.as_ref(), hk.clone()).await;
    assert!(r.is_err(), "{r:?}");
    assert_eq!(request_events(&host), 0);
    assert!(host.st.friend_join_requests().is_empty());

    // Demande en attente, puis l'hôte arrête de partager : la demande tombe (expirée), la file se vide.
    let g = guest.st.clone();
    let hk2 = hk.clone();
    let ask = tokio::spawn(async move { g.friend_join_request(&hk2).await });
    wait_for("demande chez l'hôte", 10, || request_events(&host) == 1).await;
    host.st.host_stop_sharing();
    assert_eq!(ask.await.unwrap().unwrap(), JoinAnswer::Expired);
    assert!(host.st.friend_join_requests().is_empty());
    // Plus rien n'est partagé : accepter ne donne rien.
    assert!(host.st.friend_join_request_answer(&guest.key(), true).is_err());
    assert!(!host.st.access.contains(&hex::decode(guest.key()).unwrap().try_into().unwrap()));

    host.be.shutdown().await;
    guest.be.shutdown().await;
    stranger.be.shutdown().await;
}

/// Une connexion déjà établie ne survit pas au retrait de l'accès : fin de partie, l'invité est coupé en cours de jeu.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn un_tunnel_deja_ouvert_est_coupe_a_la_fin_de_partie() {
    let (dh, dg) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let host = Node::start(dh.path(), "Hote").await;
    let guest = Node::start(dg.path(), "Invite").await;
    befriend(&guest, &host).await;
    wait_for("ami chez l'hôte", 20, || host.st.friends.is_friend(&guest.key()).unwrap()).await;
    let hk = host.key();
    let id = world(&host);
    host.st.host_start_sharing(&id, u32::from(echo_server().await)).await.unwrap();
    host.st.friend_invite(&guest.key()).await.unwrap();
    wait_for("invitation reçue", 10, || !guest.st.friend_invites().is_empty()).await;

    let t = guest.be.join_tunnel(&hk).await.expect("tunnel accordé à l'invité");
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", t.local_port)).await.unwrap();
    s.write_all(b"salut").await.unwrap();
    let mut buf = [0u8; 5];
    tokio::time::timeout(Duration::from_secs(3), s.read_exact(&mut buf)).await.unwrap().unwrap();
    assert_eq!(&buf, b"salut");

    // Fin de partie : la connexion en cours doit tomber d'elle-même, sans que l'invité ne fasse rien.
    host.st.host_stop_sharing();
    let coupe = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let mut b = [0u8; 8];
            match s.read(&mut b).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            if s.write_all(b"encore").await.is_err() {
                break;
            }
        }
    })
    .await;
    assert!(coupe.is_ok(), "le tunnel établi aurait dû être coupé après la révocation");
    t.stop().await;
    assert!(!tunnel_works(&guest, &hk).await, "plus de nouveau tunnel non plus");

    host.be.shutdown().await;
    guest.be.shutdown().await;
}
