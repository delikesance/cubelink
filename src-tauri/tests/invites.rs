//! Invitations « rejoins mon monde » de bout en bout : deux nœuds loopback (relais désactivé), vrais handlers iroh.

mod common;
use common::{befriend, wait_for, world, Node};

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
