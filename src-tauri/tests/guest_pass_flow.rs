//! Laissez-passer d'invité de bout en bout : deux nœuds loopback (relais désactivé), vrais handlers iroh.
//! Invitation visible chez l'hôte, entrée au premier manifeste, retrait par l'hôte (manifeste et tunnel refusés,
//! invitation retirée chez l'ami), puis demande à rejoindre qui arrive bien chez l'hôte au lieu d'être acceptée
//! d'office, et demande en cours visible chez l'invité.

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
        while let Ok((mut s, _)) = l.accept().await {
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

async fn tunnel_works(guest: &Node, host_key: &str) -> bool {
    let Ok(t) = guest.be.join_tunnel(host_key).await else {
        return false;
    };
    let ok = async {
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", t.local_port))
            .await
            .ok()?;
        s.write_all(b"salut").await.ok()?;
        let mut buf = [0u8; 5];
        tokio::time::timeout(Duration::from_secs(3), s.read_exact(&mut buf))
            .await
            .ok()?
            .ok()?;
        (&buf == b"salut").then_some(())
    }
    .await
    .is_some();
    t.stop().await;
    ok
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invitation_visible_retrait_puis_demande_qui_arrive_chez_l_hote() {
    let (dh, dg) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let host = Node::start(dh.path(), "Hote").await;
    let guest = Node::start(dg.path(), "Invite").await;
    befriend(&guest, &host).await;
    wait_for("ami chez l'hôte", 20, || {
        host.st.friends.is_friend(&guest.key()).unwrap()
    })
    .await;
    let (hk, gk) = (host.key(), guest.key());
    let id = world(&host);
    host.st
        .host_start_sharing(&id, u32::from(echo_server().await))
        .await
        .unwrap();

    // 1. Invitation : visible chez l'hôte, pas encore entrée ; un second envoi est refusé tant qu'elle attend.
    host.st.friend_invite(&gk).await.unwrap();
    let g = host.st.host_guests();
    assert_eq!(g.len(), 1);
    assert_eq!(
        (g[0].public_key.as_str(), g[0].entered),
        (gk.as_str(), false)
    );
    assert!(g[0].expires_at.is_some());
    assert!(host.count("host://guests") >= 1);
    assert!(host
        .st
        .friend_invite(&gk)
        .await
        .unwrap_err()
        .contains("déjà envoyée"));
    wait_for("invitation reçue", 10, || {
        !guest.st.friend_invites().is_empty()
    })
    .await;

    // 2. Une demande d'un ami déjà invité est acceptée sans déranger l'hôte (il est vraiment invité).
    assert_eq!(
        guest.st.friend_join_request(&hk).await.unwrap(),
        JoinAnswer::Accepted
    );
    assert_eq!(host.count("friend://join_request"), 0);

    // 3. Premier manifeste servi : l'ami est « entré », son accès ne périme plus.
    assert!(
        guest.st.join_plan(&hk).await.is_ok(),
        "manifeste accordé à l'invité"
    );
    wait_for("invité entré", 10, || {
        host.st.host_guests().first().is_some_and(|g| g.entered)
    })
    .await;
    assert_eq!(host.st.host_guests()[0].expires_at, None);
    assert!(tunnel_works(&guest, &hk).await, "tunnel accordé à l'invité");

    // 4. L'hôte retire l'accès : manifeste et tunnel refusés, invitation retirée chez l'ami.
    host.st.host_guest_revoke(&gk).unwrap();
    assert!(host.st.host_guests().is_empty());
    assert!(
        guest.st.join_plan(&hk).await.is_err(),
        "manifeste refusé après retrait"
    );
    assert!(
        !tunnel_works(&guest, &hk).await,
        "tunnel refusé après retrait"
    );
    wait_for("invitation retirée chez l'ami", 10, || {
        guest.st.friend_invites().is_empty()
    })
    .await;

    // 5. Nouvelle demande : elle arrive chez l'hôte (événement, donc bandeau et notification), et l'invité la voit en cours.
    let g2 = guest.st.clone();
    let hk2 = hk.clone();
    let ask = tokio::spawn(async move { g2.friend_join_request(&hk2).await });
    wait_for("demande chez l'hôte", 10, || {
        host.count("friend://join_request") == 1
    })
    .await;
    assert_eq!(host.st.friend_join_requests()[0].public_key, gk);
    assert_eq!(
        guest.st.friend_join_asks(),
        vec![hk.clone()],
        "demande en cours visible chez l'invité"
    );
    host.st.friend_join_request_answer(&gk, true).unwrap();
    assert_eq!(ask.await.unwrap().unwrap(), JoinAnswer::Accepted);
    assert!(guest.st.friend_join_asks().is_empty());
    let g = host.st.host_guests();
    assert_eq!(
        (g.len(), g[0].entered),
        (1, false),
        "accès donné par la demande, pas encore utilisé"
    );
    assert!(guest.st.join_plan(&hk).await.is_ok());

    host.be.shutdown().await;
    guest.be.shutdown().await;
}
