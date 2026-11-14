//! Demande à rejoindre (ALPN `cubelink/joinreq/1`, voir docs/hearth.md § 2).
//!
//! Un ami qui n'a pas reçu d'invitation demande à entrer dans la partie en cours. Protocole minuscule, sans donnée
//! personnelle (même esprit que `invite.rs` et `presence.rs`) :
//!
//! 1. l'invité ouvre un flux bidirectionnel, écrit l'octet `REQUEST` puis termine son flux ;
//! 2. l'hôte garde le flux ouvert (au plus `ANSWER_WINDOW`) pendant que son utilisateur décide, puis répond UN octet :
//!    `ACCEPTED`, `REFUSED` ou `EXPIRED` ;
//! 3. l'invité lit la réponse et ferme.
//!
//! Sécurité : l'émetteur est `conn.remote_id()` (jamais le contenu du message) ; un non-ami est refusé sans un octet
//! de réponse ; le message est borné et exact ; au plus UNE demande en attente par pair et `MAX_PENDING` au total ;
//! après un refus, le même pair ne peut pas redemander avant `REFUSE_COOLDOWN` (réponse `REFUSED` immédiate, l'hôte
//! n'est pas dérangé) ; rien de partagé : `REFUSED` immédiat. L'accès est accordé par l'hôte (`JoinPolicy::grant`)
//! AVANT l'envoi de `ACCEPTED`, pour que l'invité qui rejoint tout de suite trouve la porte ouverte.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::{Endpoint, EndpointAddr};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use tokio::time::timeout;

use crate::presence::{FriendLookup, PeerKey};
use crate::reach::{Reach, CLOSE_DENIED};

/// ALPN du protocole de demande à rejoindre.
pub const JOIN_REQUEST_ALPN: &[u8] = b"cubelink/joinreq/1";
/// Durée maximale pendant laquelle l'hôte garde la demande ouverte avant de répondre `EXPIRED`.
pub const ANSWER_WINDOW: Duration = Duration::from_secs(120);
/// Délai avant qu'un pair refusé puisse redemander.
pub const REFUSE_COOLDOWN: Duration = Duration::from_secs(10 * 60);
/// Nombre maximal de demandes en attente (tous pairs confondus).
pub const MAX_PENDING: usize = 32;
/// Nombre maximal de refus retenus (les plus anciens sont oubliés).
const MAX_COOLDOWNS: usize = 1024;
/// Taille maximale du message de demande (octets).
const MAX_REQUEST_BYTES: usize = 8;
/// Marge laissée à l'invité au-delà de `ANSWER_WINDOW` avant de conclure que la demande a expiré.
const CLIENT_GRACE: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const IO_TIMEOUT: Duration = Duration::from_secs(5);

const REQUEST: u8 = 1;
const ACCEPTED: u8 = 1;
const REFUSED: u8 = 2;
const EXPIRED: u8 = 3;

const CLOSE_BAD: u32 = 2;

/// Réponse de l'hôte à une demande.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinAnswer {
    Accepted,
    Refused,
    Expired,
}

/// Décision de l'utilisateur de l'hôte (ou fin sans décision).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Accept,
    Refuse,
    /// L'hôte n'a plus rien à partager : la demande tombe, sans pénalité pour le demandeur.
    Cancel,
}

/// Ce que l'hôte sait de la partie en cours (appelé de façon synchrone, ne doit pas bloquer).
pub trait JoinPolicy: Send + Sync {
    /// Vrai si une partie est partagée (monde solo ouvert ou serveur de groupe hébergé).
    fn is_sharing(&self) -> bool;
    /// Vrai si ce pair a déjà accès (invité, ou déjà accepté) : la demande est acceptée sans déranger personne.
    fn is_allowed(&self, key: &PeerKey) -> bool;
    /// Accorde l'accès au pair pour la partie en cours.
    fn grant(&self, key: &PeerKey);
}

/// Cycle de vie d'une demande, pour l'interface de l'hôte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinRequestEvent {
    /// Nouvelle demande en attente d'une décision.
    Received(PeerKey),
    /// La demande n'attend plus rien (décidée, expirée, retirée par le demandeur ou annulée).
    Ended(PeerKey),
}

/// Destinataire des événements (appelé de façon synchrone, ne doit pas bloquer).
pub type JoinRequestSink = Arc<dyn Fn(JoinRequestEvent) + Send + Sync>;

#[derive(Default)]
struct Book {
    /// Numéro de la demande (pour ne retirer que la sienne) et canal de la décision.
    pending: HashMap<PeerKey, (u64, oneshot::Sender<Decision>)>,
    refused: HashMap<PeerKey, Instant>,
    next_id: u64,
}

/// Retire la demande de son carnet quand sa connexion se termine, quelle qu'en soit la raison, puis prévient l'hôte.
struct PendingGuard {
    handler: JoinRequestHandler,
    key: PeerKey,
    id: u64,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        {
            let mut book = crate::util::lock(&self.handler.book);
            if book.pending.get(&self.key).is_some_and(|(id, _)| *id == self.id) {
                book.pending.remove(&self.key);
            }
        }
        (self.handler.sink)(JoinRequestEvent::Ended(self.key));
    }
}

enum Admission {
    /// Réponse immédiate, sans événement.
    Now(JoinAnswer),
    /// Demande enregistrée : attendre la décision.
    Wait(oneshot::Receiver<Decision>, u64),
    /// Demande abusive (déjà en attente, trop de demandes) : connexion fermée.
    Reject,
}

/// Côté hôte : à enregistrer dans le `Router` sous [`JOIN_REQUEST_ALPN`] ; le même objet (cloné) sert à répondre.
#[derive(Clone)]
pub struct JoinRequestHandler {
    friends: Arc<dyn FriendLookup>,
    policy: Arc<dyn JoinPolicy>,
    sink: JoinRequestSink,
    window: Duration,
    cooldown: Duration,
    book: Arc<Mutex<Book>>,
}

impl std::fmt::Debug for JoinRequestHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("JoinRequestHandler")
    }
}

impl JoinRequestHandler {
    pub fn new(friends: Arc<dyn FriendLookup>, policy: Arc<dyn JoinPolicy>, sink: JoinRequestSink) -> Self {
        Self::with_timing(friends, policy, sink, ANSWER_WINDOW, REFUSE_COOLDOWN)
    }

    /// Comme `new`, avec des délais réglables (tests).
    pub fn with_timing(
        friends: Arc<dyn FriendLookup>,
        policy: Arc<dyn JoinPolicy>,
        sink: JoinRequestSink,
        window: Duration,
        cooldown: Duration,
    ) -> Self {
        Self { friends, policy, sink, window, cooldown, book: Arc::default() }
    }

    /// Clés des demandes en attente d'une décision.
    pub fn pending(&self) -> Vec<PeerKey> {
        crate::util::lock(&self.book).pending.keys().copied().collect()
    }

    /// Décide d'une demande en attente. `Accept` accorde l'accès (`JoinPolicy::grant`) si une partie est partagée ;
    /// sinon la demande tombe. Faux si aucune demande de ce pair n'attend (déjà décidée, expirée ou retirée).
    pub fn answer(&self, key: &PeerKey, decision: Decision) -> bool {
        let decision = if decision == Decision::Accept && !self.policy.is_sharing() { Decision::Cancel } else { decision };
        let tx = {
            let mut book = crate::util::lock(&self.book);
            let Some((_, tx)) = book.pending.remove(key) else { return false };
            if decision == Decision::Refuse {
                self.remember_refusal(&mut book, *key);
            }
            tx
        };
        if decision == Decision::Accept {
            self.policy.grant(key);
        }
        // Le demandeur a pu s'en aller entre-temps : la réponse est alors perdue, sans conséquence.
        let sent = tx.send(decision).is_ok();
        decision != Decision::Cancel && sent
    }

    /// L'hôte arrête de partager : toutes les demandes en attente tombent.
    pub fn cancel_all(&self) {
        let all: Vec<_> = crate::util::lock(&self.book).pending.drain().collect();
        for (_, (_, tx)) in all {
            let _ = tx.send(Decision::Cancel);
        }
    }

    fn remember_refusal(&self, book: &mut Book, key: PeerKey) {
        let now = Instant::now();
        book.refused.retain(|_, t| now.duration_since(*t) < self.cooldown);
        if book.refused.len() >= MAX_COOLDOWNS {
            if let Some(oldest) = book.refused.iter().min_by_key(|(_, t)| **t).map(|(k, _)| *k) {
                book.refused.remove(&oldest);
            }
        }
        book.refused.insert(key, now);
    }

    fn admit(&self, key: PeerKey) -> Admission {
        if !self.policy.is_sharing() {
            return Admission::Now(JoinAnswer::Refused);
        }
        if self.policy.is_allowed(&key) {
            return Admission::Now(JoinAnswer::Accepted);
        }
        let mut book = crate::util::lock(&self.book);
        let now = Instant::now();
        book.refused.retain(|_, t| now.duration_since(*t) < self.cooldown);
        if book.refused.contains_key(&key) {
            return Admission::Now(JoinAnswer::Refused);
        }
        if book.pending.contains_key(&key) || book.pending.len() >= MAX_PENDING {
            return Admission::Reject;
        }
        let (tx, rx) = oneshot::channel();
        book.next_id += 1;
        let id = book.next_id;
        book.pending.insert(key, (id, tx));
        Admission::Wait(rx, id)
    }

    async fn serve(&self, conn: &Connection, key: PeerKey) -> Option<()> {
        let (mut send, mut recv) = conn.accept_bi().await.ok()?;
        let bytes = recv.read_to_end(MAX_REQUEST_BYTES).await.ok()?;
        if bytes != [REQUEST] {
            return None;
        }
        let answer = match self.admit(key) {
            Admission::Reject => return None,
            Admission::Now(a) => a,
            Admission::Wait(rx, id) => {
                let _guard = PendingGuard { handler: self.clone(), key, id };
                (self.sink)(JoinRequestEvent::Received(key));
                let decided = tokio::select! {
                    d = timeout(self.window, rx) => match d {
                        Ok(Ok(Decision::Accept)) => Some(JoinAnswer::Accepted),
                        Ok(Ok(Decision::Refuse)) => Some(JoinAnswer::Refused),
                        // Annulée par l'hôte, ou délai écoulé : le demandeur apprend « expiré ».
                        Ok(Ok(Decision::Cancel)) | Ok(Err(_)) | Err(_) => Some(JoinAnswer::Expired),
                    },
                    // Le demandeur a retiré sa demande (connexion fermée).
                    _ = conn.closed() => None,
                };
                decided?
            }
        };
        let byte = match answer {
            JoinAnswer::Accepted => ACCEPTED,
            JoinAnswer::Refused => REFUSED,
            JoinAnswer::Expired => EXPIRED,
        };
        send.write_all(&[byte]).await.ok()?;
        send.finish().ok()?;
        // Laisse au demandeur le temps de lire la réponse avant de fermer.
        let _ = timeout(IO_TIMEOUT, conn.closed()).await;
        Some(())
    }
}

impl ProtocolHandler for JoinRequestHandler {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let key: PeerKey = *conn.remote_id().as_bytes();
        if !self.friends.is_friend(&key) {
            conn.close(CLOSE_DENIED.into(), b"");
            return Ok(());
        }
        // Le flux d'une demande reste ouvert au plus `window` ; la marge couvre la lecture du message et l'accusé.
        match timeout(self.window + IO_TIMEOUT * 3, self.serve(&conn, key)).await {
            Ok(Some(())) => {}
            Ok(None) | Err(_) => conn.close(CLOSE_BAD.into(), b""),
        }
        Ok(())
    }
}

/// Invité : envoie la demande à `addr` et attend la décision de l'hôte (au plus `ANSWER_WINDOW` + marge).
/// `Err` : hôte injoignable, ne nous a pas pour ami, ou demande abusive (déjà une demande en attente).
pub async fn send_join_request(endpoint: &Endpoint, addr: impl Into<EndpointAddr>) -> Result<JoinAnswer, String> {
    send_join_request_within(endpoint, addr, ANSWER_WINDOW + CLIENT_GRACE).await
}

/// Comme `send_join_request`, avec un délai d'attente de la décision réglable (tests).
pub async fn send_join_request_within(
    endpoint: &Endpoint,
    addr: impl Into<EndpointAddr>,
    wait: Duration,
) -> Result<JoinAnswer, String> {
    let addr: EndpointAddr = addr.into();
    let conn = timeout(CONNECT_TIMEOUT, endpoint.connect(addr, JOIN_REQUEST_ALPN))
        .await
        .map_err(|_| Reach::Offline.message().to_string())?
        .map_err(|_| Reach::Offline.message().to_string())?;
    let res = async {
        let (mut send, mut recv) = conn.open_bi().await.map_err(|_| "envoi de la demande impossible".to_string())?;
        send.write_all(&[REQUEST]).await.map_err(|_| "envoi de la demande impossible".to_string())?;
        send.finish().map_err(|_| "envoi de la demande impossible".to_string())?;
        let mut byte = [0u8; 1];
        match timeout(wait, recv.read_exact(&mut byte)).await {
            Err(_) => Ok(JoinAnswer::Expired),
            Ok(Ok(())) => match byte[0] {
                ACCEPTED => Ok(JoinAnswer::Accepted),
                REFUSED => Ok(JoinAnswer::Refused),
                EXPIRED => Ok(JoinAnswer::Expired),
                _ => Err("réponse inattendue de l'ami".to_string()),
            },
            Ok(Err(_)) => Err(match Reach::from_conn_settled(&conn).await {
                Some(r) => r.message().to_string(),
                None => "la demande n'a pas abouti (une demande est peut-être déjà en attente chez cet ami)".to_string(),
            }),
        }
    }
    .await;
    conn.close(0u32.into(), b"");
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    use iroh::endpoint::presets;
    use iroh::protocol::Router;
    use iroh::RelayMode;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Friends(Vec<PeerKey>);
    impl FriendLookup for Friends {
        fn friend_keys(&self) -> Vec<PeerKey> {
            self.0.clone()
        }
        fn is_friend(&self, key: &PeerKey) -> bool {
            self.0.contains(key)
        }
    }

    #[derive(Default)]
    struct Policy {
        sharing: AtomicBool,
        granted: Mutex<Vec<PeerKey>>,
    }
    impl JoinPolicy for Policy {
        fn is_sharing(&self) -> bool {
            self.sharing.load(Ordering::SeqCst)
        }
        fn is_allowed(&self, key: &PeerKey) -> bool {
            self.granted.lock().unwrap().contains(key)
        }
        fn grant(&self, key: &PeerKey) {
            self.granted.lock().unwrap().push(*key);
        }
    }

    type Events = Arc<Mutex<Vec<JoinRequestEvent>>>;

    async fn ep() -> Endpoint {
        Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled).bind().await.unwrap()
    }

    struct Rig {
        host: Endpoint,
        router: Router,
        handler: JoinRequestHandler,
        policy: Arc<Policy>,
        events: Events,
    }

    async fn rig(friends: Vec<PeerKey>, sharing: bool, window: Duration, cooldown: Duration) -> Rig {
        let host = ep().await;
        let policy = Arc::new(Policy::default());
        policy.sharing.store(sharing, Ordering::SeqCst);
        let events: Events = Arc::default();
        let e2 = Arc::clone(&events);
        let sink: JoinRequestSink = Arc::new(move |e| e2.lock().unwrap().push(e));
        let handler = JoinRequestHandler::with_timing(Arc::new(Friends(friends)), policy.clone(), sink, window, cooldown);
        let router = Router::builder(host.clone()).accept(JOIN_REQUEST_ALPN, handler.clone()).spawn();
        Rig { host, router, handler, policy, events }
    }

    async fn wait_pending(h: &JoinRequestHandler, n: usize) {
        for _ in 0..100 {
            if h.pending().len() == n {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("demandes en attente : {} au lieu de {n}", h.pending().len());
    }

    #[tokio::test]
    async fn acceptee_accorde_l_acces_avant_la_reponse() {
        let guest = ep().await;
        let gk = *guest.id().as_bytes();
        let r = rig(vec![gk], true, Duration::from_secs(10), REFUSE_COOLDOWN).await;
        let addr = r.host.addr();
        let g2 = guest.clone();
        let task = tokio::spawn(async move { send_join_request(&g2, addr).await });
        wait_pending(&r.handler, 1).await;
        assert_eq!(r.handler.pending(), vec![gk]);
        assert!(r.handler.answer(&gk, Decision::Accept));
        assert_eq!(task.await.unwrap().unwrap(), JoinAnswer::Accepted);
        assert_eq!(*r.policy.granted.lock().unwrap(), vec![gk]);
        let ev = r.events.lock().unwrap().clone();
        assert_eq!(ev, vec![JoinRequestEvent::Received(gk), JoinRequestEvent::Ended(gk)]);
        // Déjà accepté : une nouvelle demande est acceptée sans déranger l'hôte.
        assert_eq!(send_join_request(&guest, r.host.addr()).await.unwrap(), JoinAnswer::Accepted);
        assert_eq!(r.events.lock().unwrap().len(), 2);
        r.router.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn refusee_puis_limitee() {
        let guest = ep().await;
        let gk = *guest.id().as_bytes();
        let r = rig(vec![gk], true, Duration::from_secs(10), Duration::from_secs(60)).await;
        let addr = r.host.addr();
        let g2 = guest.clone();
        let task = tokio::spawn(async move { send_join_request(&g2, addr).await });
        wait_pending(&r.handler, 1).await;
        assert!(r.handler.answer(&gk, Decision::Refuse));
        assert_eq!(task.await.unwrap().unwrap(), JoinAnswer::Refused);
        assert!(r.policy.granted.lock().unwrap().is_empty());
        // Redemande trop tôt : refus immédiat, aucun nouvel événement.
        assert_eq!(send_join_request(&guest, r.host.addr()).await.unwrap(), JoinAnswer::Refused);
        assert_eq!(r.events.lock().unwrap().len(), 2);
        assert!(r.handler.pending().is_empty());
        r.router.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn redemande_possible_apres_le_delai() {
        let guest = ep().await;
        let gk = *guest.id().as_bytes();
        let r = rig(vec![gk], true, Duration::from_secs(10), Duration::from_millis(300)).await;
        let (g2, addr) = (guest.clone(), r.host.addr());
        let task = tokio::spawn(async move { send_join_request(&g2, addr).await });
        wait_pending(&r.handler, 1).await;
        assert!(r.handler.answer(&gk, Decision::Refuse));
        assert_eq!(task.await.unwrap().unwrap(), JoinAnswer::Refused);
        tokio::time::sleep(Duration::from_millis(400)).await;
        let (g2, addr) = (guest.clone(), r.host.addr());
        let task = tokio::spawn(async move { send_join_request(&g2, addr).await });
        wait_pending(&r.handler, 1).await;
        assert!(r.handler.answer(&gk, Decision::Accept));
        assert_eq!(task.await.unwrap().unwrap(), JoinAnswer::Accepted);
        r.router.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn non_ami_ferme_sans_reponse_et_sans_evenement() {
        let stranger = ep().await;
        let r = rig(vec![], true, Duration::from_secs(10), REFUSE_COOLDOWN).await;
        let err = send_join_request(&stranger, r.host.addr()).await.unwrap_err();
        assert!(err.contains("ami"), "{err}");
        assert!(r.events.lock().unwrap().is_empty());
        assert!(r.handler.pending().is_empty());
        r.router.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn rien_de_partage_refus_immediat() {
        let guest = ep().await;
        let r = rig(vec![*guest.id().as_bytes()], false, Duration::from_secs(10), REFUSE_COOLDOWN).await;
        assert_eq!(send_join_request(&guest, r.host.addr()).await.unwrap(), JoinAnswer::Refused);
        assert!(r.events.lock().unwrap().is_empty());
        assert!(r.policy.granted.lock().unwrap().is_empty());
        r.router.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn expire_apres_la_fenetre_sans_penalite() {
        let guest = ep().await;
        let gk = *guest.id().as_bytes();
        let r = rig(vec![gk], true, Duration::from_millis(300), REFUSE_COOLDOWN).await;
        assert_eq!(send_join_request(&guest, r.host.addr()).await.unwrap(), JoinAnswer::Expired);
        assert!(r.handler.pending().is_empty());
        assert_eq!(r.events.lock().unwrap().last(), Some(&JoinRequestEvent::Ended(gk)));
        // Pas de pénalité : le demandeur peut redemander et l'hôte voit la demande.
        let (g2, addr) = (guest.clone(), r.host.addr());
        let task = tokio::spawn(async move { send_join_request(&g2, addr).await });
        wait_pending(&r.handler, 1).await;
        assert!(r.handler.answer(&gk, Decision::Accept));
        assert_eq!(task.await.unwrap().unwrap(), JoinAnswer::Accepted);
        r.router.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn une_seule_demande_en_attente_par_pair() {
        let guest = ep().await;
        let gk = *guest.id().as_bytes();
        let r = rig(vec![gk], true, Duration::from_secs(10), REFUSE_COOLDOWN).await;
        let (g2, addr) = (guest.clone(), r.host.addr());
        let first = tokio::spawn(async move { send_join_request(&g2, addr).await });
        wait_pending(&r.handler, 1).await;
        let second = send_join_request(&guest, r.host.addr()).await;
        assert!(second.is_err(), "deuxième demande en attente refusée : {second:?}");
        assert_eq!(r.handler.pending().len(), 1, "la première reste en attente");
        assert!(r.handler.answer(&gk, Decision::Accept));
        assert_eq!(first.await.unwrap().unwrap(), JoinAnswer::Accepted);
        r.router.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn demandeur_parti_retire_la_demande() {
        let guest = ep().await;
        let gk = *guest.id().as_bytes();
        let r = rig(vec![gk], true, Duration::from_secs(10), REFUSE_COOLDOWN).await;
        let (g2, addr) = (guest.clone(), r.host.addr());
        let task = tokio::spawn(async move { send_join_request(&g2, addr).await });
        wait_pending(&r.handler, 1).await;
        task.abort();
        let _ = task.await;
        wait_pending(&r.handler, 0).await;
        for _ in 0..50 {
            if r.events.lock().unwrap().last() == Some(&JoinRequestEvent::Ended(gk)) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(r.events.lock().unwrap().last(), Some(&JoinRequestEvent::Ended(gk)));
        assert!(!r.handler.answer(&gk, Decision::Accept), "plus rien à décider");
        assert!(r.policy.granted.lock().unwrap().is_empty());
        r.router.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn partage_arrete_annule_les_demandes_et_accepter_sans_partage_ne_donne_rien() {
        let guest = ep().await;
        let gk = *guest.id().as_bytes();
        let r = rig(vec![gk], true, Duration::from_secs(10), REFUSE_COOLDOWN).await;
        let (g2, addr) = (guest.clone(), r.host.addr());
        let task = tokio::spawn(async move { send_join_request(&g2, addr).await });
        wait_pending(&r.handler, 1).await;
        r.handler.cancel_all();
        assert_eq!(task.await.unwrap().unwrap(), JoinAnswer::Expired);
        // Le partage s'arrête pendant l'attente : accepter ne donne aucun accès.
        let (g2, addr) = (guest.clone(), r.host.addr());
        let task = tokio::spawn(async move { send_join_request(&g2, addr).await });
        wait_pending(&r.handler, 1).await;
        r.policy.sharing.store(false, Ordering::SeqCst);
        assert!(!r.handler.answer(&gk, Decision::Accept));
        assert_eq!(task.await.unwrap().unwrap(), JoinAnswer::Expired);
        assert!(r.policy.granted.lock().unwrap().is_empty());
        r.router.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn message_invalide_ferme_sans_reponse() {
        let guest = ep().await;
        let r = rig(vec![*guest.id().as_bytes()], true, Duration::from_secs(10), REFUSE_COOLDOWN).await;
        for payload in [vec![9u8], vec![REQUEST, REQUEST], vec![0u8; MAX_REQUEST_BYTES + 50], vec![]] {
            let conn = guest.connect(r.host.addr(), JOIN_REQUEST_ALPN).await.unwrap();
            let (mut s, mut rv) = conn.open_bi().await.unwrap();
            let _ = s.write_all(&payload).await;
            let _ = s.finish();
            let mut b = [0u8; 1];
            assert!(rv.read_exact(&mut b).await.is_err(), "aucun octet pour un message refusé");
            conn.close(0u32.into(), b"");
        }
        assert!(r.events.lock().unwrap().is_empty());
        assert!(r.handler.pending().is_empty());
        r.router.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn trop_de_demandes_en_attente_refusees() {
        let r = rig(vec![], true, Duration::from_secs(10), REFUSE_COOLDOWN).await;
        // Remplit le carnet directement (une clé par ami fictif) : la 33e demande est fermée.
        let mut friends = Vec::new();
        for i in 0..=MAX_PENDING {
            let mut k = [0u8; 32];
            k[0] = i as u8 + 1;
            friends.push(k);
        }
        for k in friends.iter().take(MAX_PENDING) {
            assert!(matches!(r.handler.admit(*k), Admission::Wait(..)));
        }
        assert!(matches!(r.handler.admit(friends[MAX_PENDING]), Admission::Reject));
        r.handler.cancel_all();
        assert!(r.handler.pending().is_empty());
        r.router.shutdown().await.unwrap();
        let _ = &r.host;
    }
}
