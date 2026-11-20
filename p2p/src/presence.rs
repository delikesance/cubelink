//! Tâche N8 : présence des amis par ping direct sur l'ALPN `cubelink/presence/1`.
//!
//! Protocole minuscule et sans donnée personnelle : le client ouvre un flux bidirectionnel et envoie
//! `PING` (4 octets) ; l'ami répond `PONG` + version (5 octets), puis UN octet d'indicateurs facultatif (bit 0 : « un jeu
//! tourne chez lui »). Compatibilité : une ancienne version lit exactement 5 octets et ignore le suivant ; une nouvelle version
//! qui ne reçoit pas cet octet (ancien ami) le lit comme « pas en jeu ». Le serveur ne répond qu'aux amis acceptés ; pour les
//! autres il ferme la connexion aussitôt, sans un octet de réponse.
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
use tokio::sync::{watch, Semaphore};
use tokio::task::JoinHandle;

use crate::friends_db::FriendsDb;
use crate::reach::Reach;

/// ALPN du protocole de présence.
pub const PRESENCE_ALPN: &[u8] = b"cubelink/presence/1";

const PING: &[u8; 4] = b"PING";
const PONG: &[u8; 5] = b"PONG\x01";
/// Bit 0 de l'octet d'indicateurs qui suit `PONG` : un jeu tourne chez l'ami.
const FLAG_IN_GAME: u8 = 1;
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
    /// Un jeu tourne chez l'ami (toujours faux hors ligne, et pour un ami d'une ancienne version).
    pub in_game: bool,
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

/// Sonde d'un ami : `None` = pas de réponse, `Some(en_jeu)` = il a répondu.
type PingFn = Arc<dyn Fn(PeerKey) -> BoxFuture<'static, Option<bool>> + Send + Sync>;

/// Dit au serveur si un jeu tourne ici (relu à chaque `PING`).
pub type InGameSource = Arc<dyn Fn() -> bool + Send + Sync>;
type InGameSlot = Arc<Mutex<Option<InGameSource>>>;

#[derive(Debug, Clone, Copy)]
struct PeerState {
    presence: Presence,
    in_game: bool,
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
        let reply = {
            let Ok(_permit) = self.permits.acquire().await else { return self.current(&key) };
            tokio::time::timeout(self.cfg.timeout, (self.pinger)(key)).await.ok().flatten()
        };
        self.record(key, reply)
    }

    fn current(&self, key: &PeerKey) -> Presence {
        self.lock().get(key).map_or(Presence::Offline, |s| s.presence)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<PeerKey, PeerState>> {
        crate::util::lock(&self.states)
    }

    fn in_game(&self, key: &PeerKey) -> bool {
        self.lock().get(key).is_some_and(|s| s.in_game)
    }

    /// Applique un résultat de sonde (`None` : pas de réponse) ; émet un événement uniquement si la présence ou
    /// « en jeu » change.
    fn record(&self, key: PeerKey, reply: Option<bool>) -> Presence {
        let (now, in_game, changed) = {
            let mut states = self.lock();
            let st = states.entry(key).or_insert(PeerState { presence: Presence::Offline, in_game: false, failures: 0 });
            let (before, game_before) = (st.presence, st.in_game);
            match reply {
                Some(in_game) => {
                    st.failures = 0;
                    st.presence = Presence::Online;
                    st.in_game = in_game;
                }
                None => {
                    st.failures = st.failures.saturating_add(1);
                    if st.failures >= self.cfg.failures_to_offline {
                        st.presence = Presence::Offline;
                        st.in_game = false;
                    }
                }
            }
            (st.presence, st.in_game, st.presence != before || st.in_game != game_before)
        };
        if changed {
            (self.sink)(PresenceEvent { public_key: key, presence: now, in_game });
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
    in_game: InGameSlot,
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
                let jitter = rand::random_range(0.8..1.2);
                let wait = task_inner.cfg.interval.mul_f64(jitter);
                tokio::select! {
                    _ = stop_rx.wait_for(|s| *s) => break,
                    _ = tokio::time::sleep(wait) => {}
                }
            }
        });
        Self { inner, addrs, sink: sink_slot, in_game: Arc::default(), stop, task: Mutex::new(Some(task)), stopped: AtomicBool::new(false) }
    }

    /// Indique comment joindre un ami (sans découverte réseau, p. ex. adresses directes).
    pub fn add_addr(&self, addr: EndpointAddr) {
        crate::util::lock(&self.addrs).insert(*addr.id.as_bytes(), addr);
    }

    /// Destinataire des adresses observées (sonde réussie, connexion entrante d'un ami) : à persister.
    pub fn set_addr_sink(&self, sink: AddrSink) {
        *crate::util::lock(&self.sink) = Some(sink);
    }

    /// Dit si un jeu tourne ici : annoncé aux amis dans la réponse à chaque `PING` (sans source : « pas en jeu »).
    pub fn set_in_game_source(&self, source: InGameSource) {
        *crate::util::lock(&self.in_game) = Some(source);
    }

    /// Un jeu tourne-t-il chez cet ami (d'après sa dernière réponse) ?
    pub fn in_game(&self, key: &PeerKey) -> bool {
        self.inner.in_game(key)
    }

    /// Serveur : à enregistrer dans le `Router` sous `PRESENCE_ALPN`.
    pub fn handler(&self) -> PresenceHandler {
        PresenceHandler { friends: Arc::clone(&self.inner.friends), sink: Arc::clone(&self.sink), in_game: Arc::clone(&self.in_game) }
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
async fn iroh_ping(endpoint: &Endpoint, key: PeerKey, hint: Option<EndpointAddr>, slot: &SharedSink) -> Option<bool> {
    match ping_status(endpoint, key, hint).await {
        Ok((addr, in_game)) => {
            notify(slot, addr);
            Some(in_game)
        }
        Err(_) => None,
    }
}

/// Sonde un ami et dit POURQUOI il ne répond pas : `Offline` (injoignable) ou `NotFriend` (il nous a refusé).
/// En cas de succès, renvoie l'adresse observée de l'ami (à mémoriser). Sans délai propre : l'appelant borne.
pub async fn ping_reach(endpoint: &Endpoint, key: PeerKey, hint: Option<EndpointAddr>) -> Result<EndpointAddr, Reach> {
    ping_status(endpoint, key, hint).await.map(|(addr, _)| addr)
}

/// Comme [`ping_reach`], avec en plus « un jeu tourne chez l'ami » (faux pour une ancienne version, qui n'envoie pas l'octet).
pub async fn ping_status(endpoint: &Endpoint, key: PeerKey, hint: Option<EndpointAddr>) -> Result<(EndpointAddr, bool), Reach> {
    let addr = match hint {
        Some(a) => a,
        None => match EndpointId::from_bytes(&key) {
            Ok(id) => id.into(),
            Err(_) => return Err(Reach::Offline),
        },
    };
    let Ok(conn) = endpoint.connect(addr, PRESENCE_ALPN).await else { return Err(Reach::Offline) };
    let reply = async {
        let (mut send, mut recv) = conn.open_bi().await.ok()?;
        send.write_all(PING).await.ok()?;
        send.finish().ok()?;
        let mut buf = [0u8; 5];
        recv.read_exact(&mut buf).await.ok()?;
        if &buf != PONG {
            return None;
        }
        // Octet d'indicateurs : absent chez une ancienne version (le flux se termine), lu alors comme « pas en jeu ».
        let mut flags = [0u8; 1];
        let in_game = recv.read_exact(&mut flags).await.is_ok() && flags[0] & FLAG_IN_GAME != 0;
        Some(in_game)
    }
    .await;
    let ok = reply.is_some();
    let observed = observed_addr(&conn);
    let reason = if ok { None } else { Reach::from_conn_settled(&conn).await };
    conn.close(0u32.into(), b"");
    if let Some(in_game) = reply {
        Ok((observed, in_game))
    } else {
        Err(reason.unwrap_or(Reach::Offline))
    }
}

/// Côté serveur : répond `PONG` aux seuls amis acceptés.
#[derive(Clone)]
pub struct PresenceHandler {
    friends: Arc<dyn FriendLookup>,
    sink: SharedSink,
    in_game: InGameSlot,
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
            let in_game = crate::util::lock(&self.in_game).as_ref().is_some_and(|f| f());
            send.write_all(PONG).await.ok()?;
            send.write_all(&[if in_game { FLAG_IN_GAME } else { 0 }]).await.ok()?;
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
            Box::pin(async move { a.then_some(false) })
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
    async fn en_jeu_retombe_a_faux_quand_l_ami_passe_hors_ligne() {
        let answers = Arc::new(Mutex::new(vec![Some(true), None, None]));
        let a2 = Arc::clone(&answers);
        let pinger: PingFn = Arc::new(move |_| {
            let a = a2.lock().unwrap().remove(0);
            Box::pin(async move { a })
        });
        let (sink, events) = collector();
        let svc = fake(pinger, Friends::new(vec![]), sink, fast_cfg());
        svc.probe_now(key(1)).await;
        assert!(svc.in_game(&key(1)));
        // Un seul échec : encore en ligne, donc encore en jeu (même seuil que la présence).
        assert_eq!(svc.probe_now(key(1)).await, Presence::Online);
        assert!(svc.in_game(&key(1)));
        assert_eq!(svc.probe_now(key(1)).await, Presence::Offline);
        assert!(!svc.in_game(&key(1)));
        let last = *events.lock().unwrap().last().unwrap();
        assert!(last.presence == Presence::Offline && !last.in_game);
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
                Some(false)
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
            Box::pin(async { Some(false) })
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
    async fn en_jeu_annonce_par_le_pong_et_evenement_au_changement() {
        let (sink_a, _) = collector();
        let (sink_b, events_b) = collector();
        let ep_a = Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled).bind().await.unwrap();
        let (ep_b, _) = node(None).await;
        let key_a = *ep_a.id().as_bytes();
        let key_b = *ep_b.id().as_bytes();
        let svc_a = PresenceService::start_with_config(ep_a.clone(), Friends::new(vec![key_b]), sink_a, fast_cfg());
        let router_a = Router::builder(ep_a.clone()).accept(PRESENCE_ALPN, svc_a.handler()).spawn();
        let svc_b = PresenceService::start_with_config(ep_b.clone(), Friends::new(vec![key_a]), sink_b, fast_cfg());
        svc_b.add_addr(ep_a.addr());

        // Sans source : pas en jeu.
        assert_eq!(svc_b.probe_now(key_a).await, Presence::Online);
        assert!(!svc_b.in_game(&key_a));

        // Un jeu démarre chez A : B le voit à la sonde suivante, avec un événement (la présence, elle, ne change pas).
        let playing = Arc::new(AtomicBool::new(true));
        let p2 = Arc::clone(&playing);
        svc_a.set_in_game_source(Arc::new(move || p2.load(Ordering::SeqCst)));
        assert_eq!(svc_b.probe_now(key_a).await, Presence::Online);
        assert!(svc_b.in_game(&key_a));
        {
            let ev = events_b.lock().unwrap();
            assert_eq!(ev.len(), 2, "en ligne, puis en jeu");
            assert!(!ev[0].in_game && ev[1].in_game && ev[1].presence == Presence::Online);
        }

        // Le jeu se ferme : événement « plus en jeu ».
        playing.store(false, Ordering::SeqCst);
        svc_b.probe_now(key_a).await;
        assert!(!svc_b.in_game(&key_a));
        assert_eq!(events_b.lock().unwrap().len(), 3);

        // Rien de nouveau : pas d'événement de plus.
        svc_b.probe_now(key_a).await;
        assert_eq!(events_b.lock().unwrap().len(), 3);

        router_a.shutdown().await.unwrap();
        svc_a.stop().await;
        svc_b.stop().await;
    }

    /// Ancien ami (serveur d'avant l'octet d'indicateurs) : `PONG` seul puis fin du flux. Il reste « en ligne », pas « en jeu ».
    #[tokio::test]
    async fn ancien_serveur_sans_octet_d_indicateurs_reste_en_ligne() {
        #[derive(Debug, Clone)]
        struct OldServer;
        impl ProtocolHandler for OldServer {
            async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
                let served = async {
                    let (mut send, mut recv) = conn.accept_bi().await.ok()?;
                    let mut buf = [0u8; 4];
                    recv.read_exact(&mut buf).await.ok()?;
                    send.write_all(PONG).await.ok()?;
                    send.finish().ok()?;
                    let _ = tokio::time::timeout(Duration::from_secs(2), conn.closed()).await;
                    Some(())
                };
                let _ = served.await;
                Ok(())
            }
        }
        let ep_a = Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled).bind().await.unwrap();
        let router_a = Router::builder(ep_a.clone()).accept(PRESENCE_ALPN, OldServer).spawn();
        let (ep_b, _) = node(None).await;
        let key_a = *ep_a.id().as_bytes();
        let (sink, _) = collector();
        let svc_b = PresenceService::start_with_config(ep_b.clone(), Friends::new(vec![key_a]), sink, fast_cfg());
        svc_b.add_addr(ep_a.addr());
        assert_eq!(svc_b.probe_now(key_a).await, Presence::Online);
        assert!(!svc_b.in_game(&key_a));
        router_a.shutdown().await.unwrap();
        svc_b.stop().await;
    }

    /// Ancien client (lit exactement 5 octets) face à un nouveau serveur : il accepte le `PONG` et ignore l'octet en plus.
    #[tokio::test]
    async fn ancien_client_ignore_l_octet_d_indicateurs() {
        let (sink, _) = collector();
        let ep_a = Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled).bind().await.unwrap();
        let (ep_c, _) = node(None).await;
        let key_c = *ep_c.id().as_bytes();
        let svc_a = PresenceService::start_with_config(ep_a.clone(), Friends::new(vec![key_c]), sink, fast_cfg());
        svc_a.set_in_game_source(Arc::new(|| true));
        let router_a = Router::builder(ep_a.clone()).accept(PRESENCE_ALPN, svc_a.handler()).spawn();
        let conn = ep_c.connect(ep_a.addr(), PRESENCE_ALPN).await.unwrap();
        let got = async {
            let (mut s, mut r) = conn.open_bi().await.ok()?;
            s.write_all(PING).await.ok()?;
            s.finish().ok()?;
            let mut buf = [0u8; 5];
            r.read_exact(&mut buf).await.ok()?;
            Some(buf)
        }
        .await;
        assert_eq!(got.as_ref(), Some(PONG));
        conn.close(0u32.into(), b"");
        router_a.shutdown().await.unwrap();
        svc_a.stop().await;
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

    /// Retrait réel (#54) : vrai QUIC en boucle locale, vraie base SQLite sur disque. Tant que B est ami de A il reçoit
    /// un PONG ; dès que A le retire de sa base, A ne lui répond plus.
    #[tokio::test]
    async fn ami_retire_ne_recoit_plus_de_pong() {
        use core_lib::types::{Friend, TrustLevel};
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(FriendsDb::open(dir.path().join("friends.db")).unwrap());
        let (sink_a, _) = collector();
        let (sink_b, _) = collector();
        let ep_a = Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled).bind().await.unwrap();
        let (ep_b, _) = node(None).await;
        let (key_a, key_b) = (*ep_a.id().as_bytes(), *ep_b.id().as_bytes());
        db.add_friend(&Friend { public_key: hex::encode(key_b), name: "B".into(), trust: TrustLevel::Ask, added_at: 1 }).unwrap();

        let svc_a = PresenceService::start_with_config(
            ep_a.clone(),
            db.clone(),
            sink_a,
            PresenceConfig { interval: Duration::from_secs(3600), ..fast_cfg() },
        );
        let router_a = Router::builder(ep_a.clone()).accept(PRESENCE_ALPN, svc_a.handler()).spawn();
        let svc_b = PresenceService::start_with_config(ep_b.clone(), Friends::new(vec![key_a]), sink_b, fast_cfg());
        svc_b.add_addr(ep_a.addr());

        assert_eq!(svc_b.probe_now(key_a).await, Presence::Online, "ami : PONG");

        assert!(db.remove_friend(&hex::encode(key_b)).unwrap());
        assert_eq!(svc_b.probe_now(key_a).await, Presence::Online, "un échec isolé ne suffit pas (hystérésis)");
        assert_eq!(svc_b.probe_now(key_a).await, Presence::Offline, "retiré : plus de PONG");

        // Bas niveau : A ferme sans un octet.
        let conn = ep_b.connect(ep_a.addr(), PRESENCE_ALPN).await.unwrap();
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
        svc_b.stop().await;
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
