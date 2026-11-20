//! Jeton du code ami (ticket #53) : « Générer un nouveau code » révoque les anciens codes pour les NOUVELLES
//! demandes d'ami, sans toucher aux amis déjà acceptés. Trois nœuds loopback réels (QUIC, relais désactivé).

use std::time::Duration;

use core_lib::types::Presence;
use p2p_lib::friend_code;

mod common;
use common::{wait_for, Node};

impl Node {
    fn sees_online(&self, other: &str) -> bool {
        self.st.friend_list().unwrap().iter().any(|f| f.friend.public_key == other && f.presence == Presence::Online)
    }
}

/// Sonde de présence rapide : la présence d'un ami doit se voir (et rester) en quelques secondes.
async fn start(root: &std::path::Path, name: &str) -> Node {
    Node::start_with(root, name, Duration::from_millis(300)).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ancien_code_refuse_nouveau_accepte_ami_existant_intact() {
    let (da, db, dc) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = start(da.path(), "Alice").await;
    let b = start(db.path(), "Bob").await;
    let c = start(dc.path(), "Carol").await;

    // A et B sont amis et se voient en ligne.
    common::befriend(&b, &a).await;
    wait_for("A en ligne chez B", 20, || b.sees_online(&a.key())).await;
    wait_for("B en ligne chez A", 20, || a.sees_online(&b.key())).await;

    let requests_before = a.count("friend://request");

    // Carol a reçu le code d'Alice ; Alice génère ensuite un nouveau code.
    let old_code = a.st.friend_my_code();
    let new_code = a.st.friend_regenerate_code().unwrap();
    assert_ne!(old_code, new_code);
    let (old_token, new_token) = (friend_code::parse(&old_code.0).unwrap().token, friend_code::parse(&new_code.0).unwrap().token);
    assert_ne!(old_token, new_token, "un jeton neuf, pas seulement de nouvelles adresses");
    assert_eq!(friend_code::parse(&a.st.friend_my_code().0).unwrap().token, new_token);

    // L'ancien code est refusé pour une NOUVELLE demande, avec un message clair, autant de fois qu'on réessaie
    // (le refus ne consomme pas le quota de demandes par pair, fixé à 3).
    for i in 0..5 {
        let e = c.st.friend_send_request(&old_code.0).await.unwrap_err();
        assert!(e.contains("n'est plus valide") && e.contains("son code actuel"), "essai {i} : {e}");
        assert!(!e.contains(&hex::encode(old_token)), "le jeton ne fuit pas dans le message");
    }
    assert_eq!(a.st.friend_requests(0, 10, None).unwrap().total, 0, "aucune demande stockée chez Alice");
    assert_eq!(a.count("friend://request"), requests_before, "aucun événement chez Alice");
    assert!(!c.st.friends.is_friend(&a.key()).unwrap());

    // Le nouveau code passe (malgré les cinq refus), Alice accepte : Carol devient amie.
    c.st.friend_send_request(&new_code.0).await.unwrap();
    wait_for("demande de Carol chez Alice", 20, || a.st.friend_requests(0, 10, None).unwrap().total == 1).await;
    a.st.friend_accept(&c.key()).await.unwrap();
    wait_for("Alice amie de Carol", 20, || c.st.friends.is_friend(&a.key()).unwrap()).await;

    // L'ami déjà accepté n'a pas été touché : toujours ami, toujours en ligne des deux côtés, dans la durée
    // (plusieurs sondes de présence après la régénération).
    assert!(a.st.friends.is_friend(&b.key()).unwrap() && b.st.friends.is_friend(&a.key()).unwrap());
    for _ in 0..8 {
        assert!(b.sees_online(&a.key()) && a.sees_online(&b.key()), "présence perdue après la régénération");
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    // Un redémarrage garde le jeton (fichier) : le code reste valide pour une nouvelle demande.
    a.be.shutdown().await;
    drop(a);
    let a2 = start(da.path(), "Alice").await;
    assert_eq!(friend_code::parse(&a2.st.friend_my_code().0).unwrap().token, new_token);
    let d = tempfile::tempdir().unwrap();
    let dave = start(d.path(), "Dave").await;
    dave.st.friend_send_request(&a2.st.friend_my_code().0).await.unwrap();
    wait_for("demande de Dave chez Alice", 20, || a2.st.friend_requests(0, 10, None).unwrap().total == 1).await;

    for n in [&a2, &b, &c, &dave] {
        n.be.shutdown().await;
    }
}
