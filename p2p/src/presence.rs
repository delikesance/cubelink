//! Tâche N8 : présence des amis par ping direct sur l'ALPN `cubelink/presence/1`.
//!
//! Protocole minuscule et sans donnée personnelle : le client ouvre un flux bidirectionnel et envoie
//! `PING` (4 octets) ; l'ami répond `PONG` + version (5 octets). Le serveur ne répond qu'aux amis
//! acceptés ; pour les autres il ferme la connexion aussitôt, sans un octet de réponse.
//!
//! Limite iroh : la poignée de main QUIC a lieu avant notre code, donc un inconnu qui sait dialer la
//! clé voit l'établissement de la connexion ; il n'apprend rien de plus (aucun message, aucune donnée).
//!
//! Intégration : le serveur est un `ProtocolHandler` (`PresenceService::handler`) à enregistrer dans
//! le `Router` de l'application sous `PRESENCE_ALPN` (un seul routeur par endpoint). Le client
//! (sonde périodique) utilise l'endpoint passé à `start`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use core_lib::types::Presence;
use futures_util::future::BoxFuture;
use futures_util::stream::{self, StreamExt};
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::{Endpoint, EndpointAddr, EndpointId, TransportAddr};
use rand::Rng;
use tokio::sync::{watch, Semaphore};
use tokio::task::JoinHandle;

use crate::friends_db::FriendsDb;
use crate::reach::Reach;

/// ALPN du protocole de présence.
pub const PRESENCE_ALPN: &[u8] = b"cubelink/presence/1";

const PING: &[u8; 4] = b"PING";
const PONG: &[u8; 5] = b"PONG\x01";
/// Délai maximal laissé à un pair pour envoyer son ping / fermer la connexion côté serveur.
const SERVER_IO_TIMEOUT: Duration = Duration::from_secs(5);

/// Clé publique brute (ed25519, 32 octets).
pub type PeerKey = [u8; 32];

/// Source de la liste d'amis acceptés.
pub trait FriendLookup: Send + Sync {
    fn friend_keys(&self) -> Vec<PeerKey>;
    fn is_friend(&self, key: &PeerKey) -> bool;
}

impl FriendLookup for FriendsDb {
    fn friend_keys(&self) -> Vec<PeerKey> {
        let Ok(list) = self.list_friends() else { return Vec::new() };
        list.iter().filter_map(|f| decode_key(&f.public_key)).collect()
    }

    fn is_friend(&self, key: &PeerKey) -> bool {
        FriendsDb::is_friend(self, &hex::encode(key)).unwrap_or(false)
    }
}

fn decode_key(hex_key: &str) -> Option<PeerKey> {
    hex::decode(hex_key).ok()?.try_into().ok()
}

/// Changement d'état de présence d'un ami.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresenceEvent {
    pub public_key: PeerKey,
    pub presence: Presence,
}

/// Destinataire des événements (appelé de façon synchrone, ne doit pas bloquer).
pub type EventSink = Arc<dyn Fn(PresenceEvent) + Send + Sync>;

/// Réglages de la sonde.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresenceConfig {
    /// Intervalle moyen entre deux tours (gigue de +/- 20 %).
    pub interval: Duration,
    /// Délai d'attente d'une sonde.
    pub timeout: Duration,
    /// Échecs consécutifs avant de passer « hors ligne ».
    pub failures_to_offline: u32,
    /// Sondes simultanées maximales.
    pub max_concurrent: usize,
}

impl Default for PresenceConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(30),
            timeout: Duration::from_secs(5),
            failures_to_offline: 2,
            max_concurrent: 8,
        }
    }
}

/// Reçoit l'adresse observée d'un ami (relais + IP des chemins de la connexion) après une sonde réussie
/// ou une connexion entrante d'un ami. Appelé de façon synchrone : ne doit pas bloquer.
pub type AddrSink = Arc<dyn Fn(EndpointAddr) + Send + Sync>;

type SharedSink = Arc<Mutex<Option<AddrSink>>>;

fn notify(slot: &SharedSink, addr: EndpointAddr) {
    let sink = crate::util::lock(slot).clone();
    if let Some(s) = sink {
        s(addr);
    }
}

/// Adresse de l'ami telle que vue par la connexion : relais et IP de tous les chemins connus.
pub fn observed_addr(conn: &Connection) -> EndpointAddr {
    let mut addr = EndpointAddr::new(conn.remote_id());
    for p in conn.paths().iter() {
        match p.remote_addr() {
            TransportAddr::Relay(u) => addr = addr.with_relay_url(u.clone()),
            TransportAddr::Ip(sa) => addr = addr.with_ip_addr(*sa),
            _ => {}
        }
    }
    addr
}

type PingFn = Arc<dyn Fn(PeerKey) -> BoxFuture<'static, bool> + Send + Sync>;

#[derive(Debug, Clone, Copy)]
struct PeerState {
    presence: Presence,
    failures: u32,
}

struct Inner {
    pinger: PingFn,
    friends: Arc<dyn FriendLookup>,
    sink: EventSink,
    cfg: PresenceConfig,
    permits: Semaphore,
    states: Mutex<HashMap<PeerKey, PeerState>>,
}

impl Inner {
    /// Sonde un ami (concurrence bornée) et met à jour son état.
    async fn probe(&self, key: PeerKey) -> Presence {
        let ok = {
            let Ok(_permit) = self.permits.acquire().await else { return self.current(&key) };
            matches!(tokio::time::timeout(self.cfg.timeout, (self.pinger)(key)).await, Ok(true))
        };
        self.record(key, ok)
    }

    fn current(&self, key: &PeerKey) -> Presence {
        self.lock().get(key).map_or(Presence::Offline, |s| s.presence)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<PeerKey, PeerState>> {
        crate::util::lock(&self.states)
    }

    /// Applique un résultat de sonde ; émet un événement uniquement si l'état change.
    fn record(&self, key: PeerKey, ok: bool) -> Presence {
        let (now, changed) = {
            let mut states = self.lock();
            let st = states.entry(key).or_insert(PeerState { presence: Presence::Offline, failures: 0 });
            let before = st.presence;
            if ok {
                st.failures = 0;
                st.presence = Presence::Online;
            } else {
                st.failures = st.failures.saturating_add(1);
                if st.failures >= self.cfg.failures_to_offline {
                    st.presence = Presence::Offline;
                }
            }
            (st.presence, st.presence != before)
        };
        if changed {
            (self.sink)(PresenceEvent { public_key: key, presence: now });
        }
        now
    }

    /// Un tour : sonde tous les amis (au plus `max_concurrent` à la fois) et oublie les non-amis.
    async fn round(&self) {
        let keys = self.friends.friend_keys();
        self.lock().retain(|k, _| keys.contains(k));
        stream::iter(keys)
            .map(|k| self.probe(k))
            .buffer_unordered(self.cfg.max_concurrent.max(1))
            .for_each(|_| async {})
            .await;
    }
}

/// Service de présence : sonde périodique des amis + état courant.
pub struct PresenceService {
    inner: Arc<Inner>,
    addrs: Arc<Mutex<HashMap<PeerKey, EndpointAddr>>>,
    sink: SharedSink,
    stop: watch::Sender<bool>,
    task: Mutex<Option<JoinHandle<()>>>,
    /// Utilisé par `Drop` : évite d'attendre une tâche déjà arrêtée.
    stopped: AtomicBool,
}

impl PresenceService {
    /// Démarre la sonde avec les réglages par défaut.
    pub fn start(endpoint: Endpoint, friends: Arc<dyn FriendLookup>, sink: EventSink) -> Self {
        Self::start_with_config(endpoint, friends, sink, PresenceConfig::default())
    }

    pub fn start_with_config(
        endpoint: Endpoint,
        friends: Arc<dyn FriendLookup>,
        sink: EventSink,
        cfg: PresenceConfig,
    ) -> Self {
        Self::start_with_addrs(endpoint, friends, sink, cfg, Vec::new())
    }

    /// Comme `start_with_config`, avec des adresses déjà connues (rechargées depuis la base) disponibles
    /// dès le premier tour de sonde : sans elles, un ami n'est joignable que par sa clé nue.
    pub fn start_with_addrs(
        endpoint: Endpoint,
        friends: Arc<dyn FriendLookup>,
        sink: EventSink,
        cfg: PresenceConfig,
        initial: Vec<EndpointAddr>,
    ) -> Self {
        let addrs: Arc<Mutex<HashMap<PeerKey, EndpointAddr>>> =
            Arc::new(Mutex::new(initial.into_iter().map(|a| (*a.id.as_bytes(), a)).collect()));
        let addrs2 = Arc::clone(&addrs);
        let slot: SharedSink = Arc::default();
        let slot2 = Arc::clone(&slot);
        let pinger: PingFn = Arc::new(move |key| {
            let endpoint = endpoint.clone();
            let hint = crate::util::lock(&addrs2).get(&key).cloned();
            let slot = Arc::clone(&slot2);
            Box::pin(async move { iroh_ping(&endpoint, key, hint, &slot).await })
        });
        Self::start_with(pinger, friends, sink, cfg, addrs, slot)
    }

    fn start_with(
        pinger: PingFn,
        friends: Arc<dyn FriendLookup>,
        sink: EventSink,
        cfg: PresenceConfig,
        addrs: Arc<Mutex<HashMap<PeerKey, EndpointAddr>>>,
        sink_slot: SharedSink,
    ) -> Self {
        let inner = Arc::new(Inner {
            pinger,
            friends,
            sink,
            cfg,
            permits: Semaphore::new(cfg.max_concurrent.max(1)),
            states: Mutex::new(HashMap::new()),
        });
        let (stop, mut stop_rx) = watch::channel(false);
        let task_inner = Arc::clone(&inner);
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = stop_rx.wait_for(|s| *s) => break,
                    _ = task_inner.round() => {}
                }
                let jitter = rand::thread_rng().gen_range(0.8..1.2);
                let wait = task_inner.cfg.interval.mul_f64(jitter);
                tokio::select! {
                    _ = stop_rx.wait_for(|s| *s) => break,
                    _ = tokio::time::sleep(wait) => {}
                }
            }
        });
        Self { inner, addrs, sink: sink_slot, stop, task: Mutex::new(Some(task)), stopped: AtomicBool::new(false) }
    }

    /// Indique comment joindre un ami (sans découverte réseau, p. ex. adresses directes).
    pub fn add_addr(&self, addr: EndpointAddr) {
        crate::util::lock(&self.addrs).insert(*addr.id.as_bytes(), addr);
    }

    /// Destinataire des adresses observées (sonde réussie, connexion entrante d'un ami) : à persister.
    pub fn set_addr_sink(&self, sink: AddrSink) {
        *crate::util::lock(&self.sink) = Some(sink);
    }

    /// Serveur : à enregistrer dans le `Router` sous `PRESENCE_ALPN`.
    pub fn handler(&self) -> PresenceHandler {
        PresenceHandler { friends: Arc::clone(&self.inner.friends), sink: Arc::clone(&self.sink) }
    }

    /// Présence de chaque ami (hors ligne tant qu'aucune sonde n'a réussi).
    pub fn snapshot(&self) -> HashMap<PeerKey, Presence> {
        let states = self.inner.lock();
        self.inner
            .friends
            .friend_keys()
            .into_iter()
            .map(|k| (k, states.get(&k).map_or(Presence::Offline, |s| s.presence)))
            .collect()
    }

    /// Sonde immédiatement un ami (soumis à la même limite de concurrence) et renvoie son état.
    pub async fn probe_now(&self, key: PeerKey) -> Presence {
        self.inner.probe(key).await
    }

    /// Sonde tous les amis maintenant (un tour complet).
    pub async fn probe_all(&self) {
        self.inner.round().await;
    }

    /// Arrêt propre : la tâche de fond est terminée à son retour.
    pub async fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        let _ = self.stop.send(true);
        let task = crate::util::lock(&self.task).take();
        if let Some(t) = task {
            let _ = t.await;
        }
    }
}

impl Drop for PresenceService {
    fn drop(&mut self) {
        if !self.stopped.load(Ordering::SeqCst) {
            if let Some(t) = crate::util::lock(&self.task).take() {
                t.abort();
            }
        }
    }
}

/// Sonde réelle : connexion iroh, `PING`, attente de `PONG`.
async fn iroh_ping(endpoint: &Endpoint, key: PeerKey, hint: Option<EndpointAddr>, slot: &SharedSink) -> bool {
    match ping_reach(endpoint, key, hint).await {
        Ok(addr) => {
            notify(slot, addr);
            true
        }
        Err(_) => false,
    }
}

/// Sonde un ami et dit POURQUOI il ne répond pas : `Offline` (injoignable) ou `NotFriend` (il nous a refusé).
/// En cas de succès, renvoie l'adresse observée de l'ami (à mémoriser). Sans délai propre : l'appelant borne.
pub async fn ping_reach(endpoint: &Endpoint, key: PeerKey, hint: Option<EndpointAddr>) -> Result<EndpointAddr, Reach> {
    let addr = match hint {
        Some(a) => a,
        None => match EndpointId::from_bytes(&key) {
            Ok(id) => id.into(),
            Err(_) => return Err(Reach::Offline),
        },
    };
    let Ok(conn) = endpoint.connect(addr, PRESENCE_ALPN).await else { return Err(Reach::Offline) };
    let ok = async {
        let (mut send, mut recv) = conn.open_bi().await.ok()?;
        send.write_all(PING).await.ok()?;
        send.finish().ok()?;
        let mut buf = [0u8; 5];
        recv.read_exact(&mut buf).await.ok()?;
        (&buf == PONG).then_some(())
    }
    .await
    .is_some();
    let observed = observed_addr(&conn);
    let reason = if ok { None } else { Reach::from_conn_settled(&conn).await };
    conn.close(0u32.into(), b"");
    if ok {
        Ok(observed)
    } else {
        Err(reason.unwrap_or(Reach::Offline))
    }
}

/// Côté serveur : répond `PONG` aux seuls amis acceptés.
#[derive(Clone)]
pub struct PresenceHandler {
    friends: Arc<dyn FriendLookup>,
    sink: SharedSink,
}

impl std::fmt::Debug for PresenceHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PresenceHandler")
    }
}

impl ProtocolHandler for PresenceHandler {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let key: PeerKey = *conn.remote_id().as_bytes();
        if !self.friends.is_friend(&key) {
            conn.close(1u32.into(), b"");
            return Ok(());
        }
        // Un ami nous joint : son adresse du moment est la meilleure qu'on ait.
        notify(&self.sink, observed_addr(&conn));
        let served = async {
            let (mut send, mut recv) = conn.accept_bi().await.ok()?;
            let mut buf = [0u8; 4];
            recv.read_exact(&mut buf).await.ok()?;
            if &buf != PING {
                return None;
            }
            send.write_all(PONG).await.ok()?;
            send.finish().ok()?;
            // Laisse au client le temps de lire la réponse avant de fermer.
            let _ = tokio::time::timeout(SERVER_IO_TIMEOUT, conn.closed()).await;
            Some(())
        };
        if tokio::time::timeout(SERVER_IO_TIMEOUT * 2, served).await.is_err() {
            conn.close(2u32.into(), b"");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    use iroh::endpoint::presets;
    use iroh::protocol::Router;
    use iroh::RelayMode;

    struct Friends(Mutex<Vec<PeerKey>>);

    impl Friends {
        fn new(keys: Vec<PeerKey>) -> Arc<Self> {
            Arc::new(Self(Mutex::new(keys)))
        }
    }

    impl FriendLookup for Friends {
        fn friend_keys(&self) -> Vec<PeerKey> {
            self.0.lock().unwrap().clone()
        }
        fn is_friend(&self, key: &PeerKey) -> bool {
            self.0.lock().unwrap().contains(key)
        }
    }

    fn key(n: u8) -> PeerKey {
        [n; 32]
    }

    fn collector() -> (EventSink, Arc<Mutex<Vec<PresenceEvent>>>) {
        let events: Arc<Mutex<Vec<PresenceEvent>>> = Arc::default();
        let e2 = Arc::clone(&events);
        (Arc::new(move |e| e2.lock().unwrap().push(e)), events)
    }

    fn fast_cfg() -> PresenceConfig {
        PresenceConfig {
            interval: Duration::from_secs(3600),
            timeout: Duration::from_millis(500),
            failures_to_offline: 2,
            max_concurrent: 8,
        }
    }

    fn fake(pinger: PingFn, friends: Arc<dyn FriendLookup>, sink: EventSink, cfg: PresenceConfig) -> PresenceService {
        PresenceService::start_with(pinger, friends, sink, cfg, Arc::default(), Arc::default())
    }

    #[tokio::test]
    async fn events_only_on_changes() {
        let answers = Arc::new(Mutex::new(Vec::<bool>::new()));
        let a2 = Arc::clone(&answers);
        let pinger: PingFn = Arc::new(move |_| {
            let a = a2.lock().unwrap().remove(0);
            Box::pin(async move { a })
        });
        let (sink, events) = collector();
        let svc = fake(pinger, Friends::new(vec![]), sink, fast_cfg());
        // ok, ok, ko, ko, ko, ok, ko, ok : hors ligne seulement après 2 échecs consécutifs.
        *answers.lock().unwrap() = vec![true, true, false, false, false, true, false, true];
        let seq: Vec<Presence> = {
            let mut v = Vec::new();
            for _ in 0..8 {
                v.push(svc.probe_now(key(1)).await);
            }
            v
        };
        use Presence::{Offline as F, Online as N};
        assert_eq!(seq, [N, N, N, F, F, N, N, N]);
        let ev: Vec<Presence> = events.lock().unwrap().iter().map(|e| e.presence).collect();
        assert_eq!(ev, [N, F, N]);
        svc.stop().await;
    }

    #[tokio::test]
    async fn timeout_counts_as_failure() {
        let pinger: PingFn = Arc::new(|_| Box::pin(std::future::pending()));
        let (sink, events) = collector();
        let cfg = PresenceConfig { timeout: Duration::from_millis(20), ..fast_cfg() };
        let svc = fake(pinger, Friends::new(vec![]), sink, cfg);
        assert_eq!(svc.probe_now(key(1)).await, Presence::Offline);
        assert!(events.lock().unwrap().is_empty());
        svc.stop().await;
    }

    #[tokio::test]
    async fn concurrency_is_bounded() {
        let cur = Arc::new(AtomicUsize::new(0));
        let max = Arc::new(AtomicUsize::new(0));
        let (c2, m2) = (Arc::clone(&cur), Arc::clone(&max));
        let pinger: PingFn = Arc::new(move |_| {
            let (c, m) = (Arc::clone(&c2), Arc::clone(&m2));
            Box::pin(async move {
                let now = c.fetch_add(1, Ordering::SeqCst) + 1;
                m.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(20)).await;
                c.fetch_sub(1, Ordering::SeqCst);
                true
            })
        });
        let keys: Vec<PeerKey> = (0..40u8).map(key).collect();
        let (sink, events) = collector();
        let svc = fake(pinger, Friends::new(keys.clone()), sink, fast_cfg());
        svc.probe_all().await;
        // Le tour de fond a aussi pu tourner en parallèle : la limite est globale (sémaphore).
        let peak = max.load(Ordering::SeqCst);
        assert!(peak <= 8, "pic de {peak} sondes simultanées");
        assert!(peak >= 2);
        let snap = svc.snapshot();
        assert_eq!(snap.len(), 40);
        assert!(snap.values().all(|p| *p == Presence::Online));
        assert_eq!(events.lock().unwrap().len(), 40);
        svc.stop().await;
    }

    #[tokio::test]
    async fn periodic_rounds_and_clean_stop() {
        let token = Arc::new(());
        let calls = Arc::new(AtomicUsize::new(0));
        let (t2, c2) = (Arc::clone(&token), Arc::clone(&calls));
        let pinger: PingFn = Arc::new(move |_| {
            let _keep = Arc::clone(&t2);
            c2.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { true })
        });
        let cfg = PresenceConfig { interval: Duration::from_millis(20), ..fast_cfg() };
        let (sink, events) = collector();
        let svc = fake(pinger, Friends::new(vec![key(1)]), sink, cfg);
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(calls.load(Ordering::SeqCst) >= 3);
        assert_eq!(events.lock().unwrap().len(), 1, "un seul événement malgré plusieurs tours");
        svc.stop().await;
        let after = calls.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(calls.load(Ordering::SeqCst), after, "plus aucune sonde après l'arrêt");
        drop(svc);
        assert_eq!(Arc::strong_count(&token), 1, "aucune tâche orpheline ne garde l'état");
    }

    async fn node(alpn_handler: Option<PresenceHandler>) -> (Endpoint, Option<Router>) {
        let ep = Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled).bind().await.unwrap();
        let router = alpn_handler.map(|h| Router::builder(ep.clone()).accept(PRESENCE_ALPN, h).spawn());
        (ep, router)
    }

    #[tokio::test]
    async fn iroh_friend_online_then_offline_and_stranger_ignored() {
        let (sink_a, _) = collector();
        let (sink_b, events_b) = collector();

        // A (serveur) : B est son ami. B (client) : A est son ami.
        let ep_a = Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled).bind().await.unwrap();
        let (ep_b, _) = node(None).await;
        let key_a = *ep_a.id().as_bytes();
        let key_b = *ep_b.id().as_bytes();

        let svc_a = PresenceService::start_with_config(
            ep_a.clone(),
            Friends::new(vec![key_b]),
            sink_a,
            PresenceConfig { interval: Duration::from_secs(3600), ..fast_cfg() },
        );
        let router_a = Router::builder(ep_a.clone()).accept(PRESENCE_ALPN, svc_a.handler()).spawn();

        let svc_b = PresenceService::start_with_config(ep_b.clone(), Friends::new(vec![key_a]), sink_b, fast_cfg());
        svc_b.add_addr(ep_a.addr());

        assert_eq!(svc_b.probe_now(key_a).await, Presence::Online);
        assert_eq!(svc_b.snapshot().get(&key_a), Some(&Presence::Online));
        assert_eq!(events_b.lock().unwrap().len(), 1);

        // A s'arrête : hors ligne seulement après 2 échecs.
        router_a.shutdown().await.unwrap();
        assert_eq!(svc_b.probe_now(key_a).await, Presence::Online);
        assert_eq!(svc_b.probe_now(key_a).await, Presence::Offline);
        assert_eq!(events_b.lock().unwrap().len(), 2);

        svc_a.stop().await;
        svc_b.stop().await;
    }

    #[tokio::test]
    async fn iroh_stranger_gets_no_answer() {
        let (sink, _) = collector();
        let ep_a = Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled).bind().await.unwrap();
        let (ep_c, _) = node(None).await;
        let key_a = *ep_a.id().as_bytes();
        // A ne connaît personne.
        let svc_a = PresenceService::start_with_config(ep_a.clone(), Friends::new(vec![]), sink.clone(), fast_cfg());
        let router_a = Router::builder(ep_a.clone()).accept(PRESENCE_ALPN, svc_a.handler()).spawn();

        // C se croit ami de A mais A ne l'a pas accepté : aucune réponse.
        let svc_c = PresenceService::start_with_config(ep_c.clone(), Friends::new(vec![key_a]), sink, fast_cfg());
        svc_c.add_addr(ep_a.addr());
        assert_eq!(svc_c.probe_now(key_a).await, Presence::Offline);
        assert_eq!(svc_c.probe_now(key_a).await, Presence::Offline);
        assert_eq!(svc_c.snapshot().get(&key_a), Some(&Presence::Offline));

        // Bas niveau : l'inconnu ne reçoit aucun octet.
        let conn = ep_c.connect(ep_a.addr(), PRESENCE_ALPN).await.unwrap();
        let got = async {
            let (mut s, mut r) = conn.open_bi().await.ok()?;
            s.write_all(PING).await.ok()?;
            let mut buf = [0u8; 5];
            r.read_exact(&mut buf).await.ok()
        }
        .await;
        assert!(got.is_none());

        router_a.shutdown().await.unwrap();
        svc_a.stop().await;
        svc_c.stop().await;
    }

    #[test]
    fn friends_db_adapter() {
        use core_lib::types::{Friend, TrustLevel};
        let db = FriendsDb::open_in_memory().unwrap();
        let k = key(7);
        db.add_friend(&Friend { public_key: hex::encode(k), name: "A".into(), trust: TrustLevel::Ask, added_at: 1 })
            .unwrap();
        let lookup: Arc<dyn FriendLookup> = Arc::new(db);
        assert_eq!(lookup.friend_keys(), vec![k]);
        assert!(lookup.is_friend(&k));
        assert!(!lookup.is_friend(&key(8)));
    }
}
