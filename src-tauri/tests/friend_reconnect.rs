//! Reconnexion aux amis après redémarrage : les adresses de contact sont persistées dans `FriendsDb`,
//! rechargées au démarrage et rafraîchies. Deux nœuds loopback, relais désactivé, découverte publique
//! désactivée (comme en production) : sans adresse mémorisée, la sonde retombe sur un `EndpointId` nu.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use core_lib::types::Presence;
use cubelink_app::iroh_backend::{start_app, IrohBackend, IrohOptions};
use cubelink_app::state::{AppState, EventSink};
use p2p_lib::presence::PresenceConfig;
use p2p_lib::relay::RelayChoice;

struct Node {
    st: Arc<AppState>,
    be: Arc<IrohBackend>,
}

impl Node {
    /// Démarre (ou redémarre, avec le même dossier : même identité, même base d'amis).
    async fn start(root: &Path, name: &str) -> Node {
        let events: Arc<Mutex<Vec<(String, serde_json::Value)>>> = Arc::default();
        let sink: EventSink = Arc::new(move |n, v| events.lock().unwrap().push((n.to_string(), v)));
        let opts = IrohOptions {
            relay: RelayChoice::Disabled,
            advertise_addrs: Some(true),
            presence: PresenceConfig {
                interval: Duration::from_millis(300),
                timeout: Duration::from_secs(2),
                failures_to_offline: 2,
                max_concurrent: 4,
            },
            ..IrohOptions::default()
        };
        // Le magasin de blobs libère son verrou de façon asynchrone après l'arrêt : on réessaie brièvement.
        let mut tries = 0;
        let (st, be) = loop {
            match start_app(root, sink.clone(), opts.clone()).await {
                Ok(v) => break v,
                Err(e) if e.contains("already open") && tries < 50 => {
                    tries += 1;
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(e) => panic!("{e}"),
            }
        };
        st.auth_offline_login(name).unwrap();
        Node { st, be }
    }
    fn key(&self) -> String {
        self.st.identity.public_key_hex()
    }
    fn sees_online(&self, other: &str) -> bool {
        self.st.friend_list().unwrap().iter().any(|f| f.friend.public_key == other && f.presence == Presence::Online)
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn l_ami_revient_en_ligne_apres_redemarrage() {
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = Node::start(da.path(), "Alice").await;
    let b = Node::start(db.path(), "Bob").await;
    let b_key = b.key();

    // A demande B en ami (adresse issue du code) ; B accepte.
    a.st.friend_send_request(&b.st.friend_my_code().0).await.unwrap();
    wait_for("demande chez B", 20, || b.st.friend_requests(0, 10, None).unwrap().total == 1).await;
    b.st.friend_accept(&a.key()).await.unwrap();
    // B (l'accepteur) connaît déjà l'adresse du demandeur dès le retour de l'acceptation.
    assert!(b.st.friends.get_friend_addr(&a.key()).unwrap().is_some(), "adresse du demandeur mémorisée à l'acceptation");
    wait_for("ami chez A", 20, || a.st.friends.is_friend(&b_key).unwrap()).await;
    wait_for("B en ligne chez A", 20, || a.sees_online(&b_key)).await;

    // L'adresse de B a été persistée chez A.
    let stored = a.st.friends.get_friend_addr(&b_key).unwrap().expect("adresse de B persistée chez A");
    assert!(!stored.addrs.is_empty(), "indices directs mémorisés");

    // A redémarre (même dossier) : l'adresse doit être rechargée, B revient en ligne.
    a.be.shutdown().await;
    drop(a);
    let a2 = Node::start(da.path(), "Alice").await;
    assert!(a2.st.friends.is_friend(&b_key).unwrap());
    wait_for("B en ligne chez A après redémarrage de A", 20, || a2.sees_online(&b_key)).await;

    b.be.shutdown().await;
    a2.be.shutdown().await;
}

/// L'AUTRE nœud redémarre (B, nouveau port) : A garde une adresse périmée de B. B sonde A au démarrage
/// (adresse de A rechargée depuis sa base) ; A reçoit cette connexion d'un ami, rafraîchit l'adresse de B
/// (`set_addr_sink`) et peut de nouveau le joindre. Sans relais ni découverte, c'est le seul chemin :
/// A ne peut pas joindre B tant que B ne l'a pas contacté.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn l_ami_qui_redemarre_nous_rafraichit_son_adresse() {
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = Node::start(da.path(), "Alice").await;
    let b = Node::start(db.path(), "Bob").await;
    let (a_key, b_key) = (a.key(), b.key());
    a.st.friend_send_request(&b.st.friend_my_code().0).await.unwrap();
    wait_for("demande chez B", 20, || b.st.friend_requests(0, 10, None).unwrap().total == 1).await;
    b.st.friend_accept(&a_key).await.unwrap();
    wait_for("ami chez A", 20, || a.st.friends.is_friend(&b_key).unwrap()).await;
    wait_for("B en ligne chez A", 20, || a.sees_online(&b_key)).await;
    // B connaît aussi A (adresse observée lors de la sonde de A, persistée).
    wait_for("adresse de A chez B", 20, || b.st.friends.get_friend_addr(&a_key).unwrap().is_some()).await;
    let old = a.st.friends.get_friend_addr(&b_key).unwrap().unwrap();

    b.be.shutdown().await;
    drop(b);
    // Occupe l'ancien port de B (l'OS le redonnerait sinon) : B doit se relier sur un autre port.
    let old_ports: Vec<u16> = old.addrs.iter().filter_map(|a| a.parse::<std::net::SocketAddr>().ok()).map(|a| a.port()).collect();
    let _held: Vec<std::net::UdpSocket> = old_ports
        .iter()
        .flat_map(|p| [format!("0.0.0.0:{p}"), format!("[::]:{p}")])
        .filter_map(|a| std::net::UdpSocket::bind(a).ok())
        .collect();
    assert!(!_held.is_empty(), "ancien port de B réservé");
    let b2 = Node::start(db.path(), "Bob").await;
    // A reçoit la sonde de B et remplace l'adresse périmée (la présence peut ne jamais passer « hors ligne »
    // si le redémarrage est plus court que deux sondes).
    wait_for("adresse de B rafraîchie chez A", 30, || {
        a.st.friends.get_friend_addr(&b_key).unwrap().is_some_and(|n| n.addrs != old.addrs)
    })
    .await;
    wait_for("B en ligne chez A après redémarrage de B", 30, || a.sees_online(&b_key)).await;
    let new = a.st.friends.get_friend_addr(&b_key).unwrap().unwrap();
    assert_ne!(new.addrs, old.addrs, "l'adresse de B (nouveau port) a été rafraîchie chez A");

    b2.be.shutdown().await;
    a.be.shutdown().await;
}
