//! Tâche N7 : protocole d'ajout d'ami à double sens, ALPN `cubelink/friend/1`.
//!
//! Un flux bidirectionnel QUIC par demande, messages à longueur préfixée (u32 BE, 4 Kio max).
//! Chaque message est signé (ed25519) sur `domaine | corps` et vérifié avant tout effet de bord.
//!
//! ```text
//! A -> B  REQUEST (A, B, ts, nom, H(A|nonce_A))      engagement de A
//! B -> A  COMMIT  (B, A, ts, H_A, H(B|nonce_B))      engagement de B (session = H(H_A|H_B))
//! A -> B  REVEAL  (nonce_A)                          seulement après l'engagement de B
//! B -> A  REVEAL  (nonce_B)                          seulement après vérification de H_A
//! ```
//! Les deux côtés calculent `verify_code(...)`. B range alors la demande (`upsert_request`, sens
//! `In`, aucun ami) et prévient l'UI ; le flux reste ouvert jusqu'à la décision de B :
//! `DECISION` signée (acceptation ou refus). A crée l'ami à la réception d'une acceptation valide ;
//! B le crée dans `accept`. Fermeture du flux par A, ou expiration : la demande est retirée.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use core_lib::types::{Error, Friend, FriendRequest, Result, TrustLevel, VerifyCode};
use ed25519_dalek::Signature;
use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayUrl};
use rand::RngCore;
use sha2::{Digest, Sha256};
use tokio::sync::{oneshot, OwnedSemaphorePermit, Semaphore};

use crate::friend_code;
use crate::friends_db::{Direction, FriendsDb};
use crate::identity::Identity;
use crate::presence::{observed_addr, AddrSink};
use crate::verify_code::verify_code;

/// ALPN du protocole d'ajout d'ami.
pub const ALPN: &[u8] = b"cubelink/friend/1";
/// Taille maximale d'un message (corps + signature), en octets.
pub const MAX_FRAME: usize = 4096;
/// Écart d'horodatage toléré (anti-rejeu), en secondes.
pub const MAX_SKEW_SECS: u64 = 300;

const SIG_LEN: usize = 64;
const HEADER_LEN: usize = 1 + 32 + 32 + 8;
const MAX_NAME_CHARS: usize = 64;
const SIGN_DOMAIN: &[u8] = b"cubelink/friend/sig/v1";
const COMMIT_DOMAIN: &[u8] = b"cubelink/friend/commit/v1";
const SESSION_DOMAIN: &[u8] = b"cubelink/friend/session/v1";
const MAX_SEEN: usize = 4096;

const K_REQUEST: u8 = 1;
const K_COMMIT: u8 = 2;
const K_REVEAL: u8 = 3;
const K_DECISION: u8 = 4;

/// Événements pour l'UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FriendEvent {
    /// Demande reçue et vérifiée (code de vérification calculé), en attente de décision.
    IncomingRequest(FriendRequest),
    /// Notre demande a été acceptée : l'ami est créé.
    Accepted(Friend),
    /// Notre demande a été refusée (clé publique hex du pair).
    Refused(String),
}

pub type EventSink = Arc<dyn Fn(FriendEvent) + Send + Sync>;

/// Limites et délais.
#[derive(Debug, Clone)]
pub struct FriendConfig {
    /// Demandes entrantes simultanées (en cours ou en attente de décision), tous pairs confondus.
    pub max_pending_total: usize,
    /// Demandes acceptées par pair et par fenêtre.
    pub max_requests_per_peer: usize,
    pub rate_window: Duration,
    /// Plafond de demandes stockées dans la base (au-delà : rejet silencieux).
    pub max_stored_requests: usize,
    /// Poignées de main simultanées (CPU).
    pub max_concurrent_handshakes: usize,
    /// Délai par message.
    pub io_timeout: Duration,
    /// Durée d'attente de la décision de l'utilisateur.
    pub decision_ttl: Duration,
}

impl Default for FriendConfig {
    fn default() -> Self {
        Self {
            max_pending_total: 64,
            max_requests_per_peer: 3,
            rate_window: Duration::from_secs(3600),
            max_stored_requests: 1000,
            max_concurrent_handshakes: 32,
            io_timeout: Duration::from_secs(15),
            decision_ttl: Duration::from_secs(600),
        }
    }
}

fn net(e: impl fmt::Debug) -> Error {
    Error::Network(format!("{e:?}"))
}

use crate::util::{lock, now_secs};

fn check_name(name: &str) -> Result<()> {
    if name.trim().is_empty() || name.chars().count() > MAX_NAME_CHARS || name.chars().any(char::is_control) {
        return Err(Error::Invalid("nom Minecraft invalide".into()));
    }
    Ok(())
}

// ---------- messages ----------

#[derive(Debug, Clone, PartialEq, Eq)]
enum Body {
    Request { name: String, commit: [u8; 32] },
    Commit { commit_a: [u8; 32], commit_b: [u8; 32] },
    Reveal { session: [u8; 32], nonce: [u8; 32] },
    Decision { session: [u8; 32], accept: bool, name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Msg {
    from: [u8; 32],
    to: [u8; 32],
    ts: u64,
    body: Body,
}

fn signed_bytes(body: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(SIGN_DOMAIN.len() + body.len());
    v.extend_from_slice(SIGN_DOMAIN);
    v.extend_from_slice(body);
    v
}

fn put_str(b: &mut Vec<u8>, s: &str) {
    b.extend_from_slice(&(s.len() as u16).to_be_bytes());
    b.extend_from_slice(s.as_bytes());
}

fn encode(msg: &Msg, id: &Identity) -> Vec<u8> {
    let mut b = Vec::with_capacity(256);
    b.push(match msg.body {
        Body::Request { .. } => K_REQUEST,
        Body::Commit { .. } => K_COMMIT,
        Body::Reveal { .. } => K_REVEAL,
        Body::Decision { .. } => K_DECISION,
    });
    b.extend_from_slice(&msg.from);
    b.extend_from_slice(&msg.to);
    b.extend_from_slice(&msg.ts.to_be_bytes());
    match &msg.body {
        Body::Request { name, commit } => {
            put_str(&mut b, name);
            b.extend_from_slice(commit);
        }
        Body::Commit { commit_a, commit_b } => {
            b.extend_from_slice(commit_a);
            b.extend_from_slice(commit_b);
        }
        Body::Reveal { session, nonce } => {
            b.extend_from_slice(session);
            b.extend_from_slice(nonce);
        }
        Body::Decision { session, accept, name } => {
            b.extend_from_slice(session);
            b.push(u8::from(*accept));
            put_str(&mut b, name);
        }
    }
    let sig = id.sign(&signed_bytes(&b));
    b.extend_from_slice(&sig.to_bytes());
    b
}

struct Rd<'a>(&'a [u8]);

impl Rd<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8]> {
        if self.0.len() < n {
            return Err(Error::Invalid("message tronqué".into()));
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn arr(&mut self) -> Result<[u8; 32]> {
        Ok(self.take(32)?.try_into().expect("32 octets"))
    }
    fn string(&mut self) -> Result<String> {
        let n = usize::from(u16::from_be_bytes(self.take(2)?.try_into().expect("2 octets")));
        let s = String::from_utf8(self.take(n)?.to_vec()).map_err(|_| Error::Invalid("UTF-8 invalide".into()))?;
        check_name(&s)?;
        Ok(s)
    }
}

/// Analyse puis vérifie la signature ; aucun message non signé correctement n'est exposé.
fn decode(frame: &[u8]) -> Result<Msg> {
    if frame.len() < HEADER_LEN + SIG_LEN || frame.len() > MAX_FRAME {
        return Err(Error::Invalid("taille de message invalide".into()));
    }
    let (body, sig) = frame.split_at(frame.len() - SIG_LEN);
    let mut r = Rd(body);
    let kind = r.take(1)?[0];
    let from = r.arr()?;
    let to = r.arr()?;
    let ts = u64::from_be_bytes(r.take(8)?.try_into().expect("8 octets"));
    let parsed = match kind {
        K_REQUEST => Body::Request { name: r.string()?, commit: r.arr()? },
        K_COMMIT => Body::Commit { commit_a: r.arr()?, commit_b: r.arr()? },
        K_REVEAL => Body::Reveal { session: r.arr()?, nonce: r.arr()? },
        K_DECISION => {
            let session = r.arr()?;
            let accept = match r.take(1)?[0] {
                0 => false,
                1 => true,
                _ => return Err(Error::Invalid("décision invalide".into())),
            };
            Body::Decision { session, accept, name: r.string()? }
        }
        _ => return Err(Error::Invalid("type de message inconnu".into())),
    };
    if !r.0.is_empty() {
        return Err(Error::Invalid("octets en trop".into()));
    }
    let sig = Signature::from_bytes(sig.try_into().expect("64 octets"));
    if !Identity::verify(&hex::encode(from), &signed_bytes(body), &sig) {
        return Err(Error::Auth("signature invalide".into()));
    }
    Ok(Msg { from, to, ts, body: parsed })
}

fn commitment(pk: &[u8; 32], nonce: &[u8; 32]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(COMMIT_DOMAIN);
    h.update(pk);
    h.update(nonce);
    h.finalize().into()
}

fn session_id(commit_a: &[u8; 32], commit_b: &[u8; 32]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(SESSION_DOMAIN);
    h.update(commit_a);
    h.update(commit_b);
    h.finalize().into()
}

fn random32() -> [u8; 32] {
    let mut n = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut n);
    n
}

/// Destinataire, expéditeur attendu et horodatage borné (anti-rejeu).
fn check_header(m: &Msg, expected_from: Option<&[u8; 32]>, me: &[u8; 32]) -> Result<()> {
    if &m.to != me {
        return Err(Error::Auth("message destiné à un autre pair".into()));
    }
    if expected_from.is_some_and(|f| f != &m.from) {
        return Err(Error::Auth("expéditeur inattendu".into()));
    }
    if m.ts.abs_diff(now_secs()) > MAX_SKEW_SECS {
        return Err(Error::Auth("horodatage hors fenêtre".into()));
    }
    Ok(())
}

// ---------- framing ----------

async fn write_frame(send: &mut SendStream, data: &[u8]) -> Result<()> {
    if data.len() > MAX_FRAME {
        return Err(Error::Invalid("message trop grand".into()));
    }
    send.write_all(&(data.len() as u32).to_be_bytes()).await.map_err(net)?;
    send.write_all(data).await.map_err(net)
}

async fn read_frame(recv: &mut RecvStream, timeout: Duration) -> Result<Vec<u8>> {
    let fut = async {
        let mut len = [0u8; 4];
        recv.read_exact(&mut len).await.map_err(net)?;
        let len = u32::from_be_bytes(len) as usize;
        if len == 0 || len > MAX_FRAME {
            return Err(Error::Invalid("taille de message hors bornes".into()));
        }
        let mut buf = vec![0u8; len];
        recv.read_exact(&mut buf).await.map_err(net)?;
        Ok(buf)
    };
    tokio::time::timeout(timeout, fut).await.map_err(|_| Error::Network("délai dépassé".into()))?
}

// ---------- état partagé ----------

struct Decision {
    accept: bool,
    reply: oneshot::Sender<Result<()>>,
}

#[derive(Default)]
struct State {
    active: HashSet<String>,
    pending: HashMap<String, oneshot::Sender<Decision>>,
    attempts: HashMap<String, VecDeque<Instant>>,
    seen: HashMap<[u8; 32], Instant>,
    outgoing: HashSet<String>,
}

struct Shared {
    identity: Arc<Identity>,
    db: Arc<FriendsDb>,
    sink: EventSink,
    cfg: FriendConfig,
    me: [u8; 32],
    name: Mutex<String>,
    state: Mutex<State>,
    handshakes: Arc<Semaphore>,
    /// Reçoit l'adresse (relais + IP) du demandeur dont la demande vient d'être acceptée.
    addr_sink: Mutex<Option<AddrSink>>,
}

/// Réservation d'un « état par pair » ; libérée à la fin (RAII).
struct Admission {
    shared: Arc<Shared>,
    peer: String,
}

impl Drop for Admission {
    fn drop(&mut self) {
        let mut st = lock(&self.shared.state);
        st.active.remove(&self.peer);
        st.pending.remove(&self.peer);
    }
}

struct OutGuard {
    shared: Arc<Shared>,
    peer: String,
}

impl Drop for OutGuard {
    fn drop(&mut self) {
        lock(&self.shared.state).outgoing.remove(&self.peer);
    }
}

impl Shared {
    fn my_name(&self) -> String {
        lock(&self.name).clone()
    }

    fn sign(&self, to: [u8; 32], body: Body) -> Vec<u8> {
        encode(&Msg { from: self.me, to, ts: now_secs(), body }, &self.identity)
    }

    /// Contrôles anti-spam, puis réservation. Appelé uniquement après vérification de signature.
    fn admit(self: &Arc<Self>, peer: &str, commit: &[u8; 32]) -> Result<Admission> {
        let stored = self.db.request_count()?;
        let mut st = lock(&self.state);
        let now = Instant::now();
        st.seen.retain(|_, t| now.duration_since(*t) < Duration::from_secs(2 * MAX_SKEW_SECS));
        if st.seen.contains_key(commit) {
            return Err(Error::Auth("rejeu".into()));
        }
        if st.seen.len() >= MAX_SEEN
            || stored >= self.cfg.max_stored_requests
            || st.active.len() >= self.cfg.max_pending_total
            || st.active.contains(peer)
        {
            return Err(Error::Other("limite atteinte".into()));
        }
        let window = self.cfg.rate_window;
        st.attempts.retain(|_, q| {
            q.retain(|t| now.duration_since(*t) < window);
            !q.is_empty()
        });
        let q = st.attempts.entry(peer.to_string()).or_default();
        if q.len() >= self.cfg.max_requests_per_peer {
            return Err(Error::Other("limite par pair atteinte".into()));
        }
        q.push_back(now);
        st.seen.insert(*commit, now);
        st.active.insert(peer.to_string());
        Ok(Admission { shared: self.clone(), peer: peer.to_string() })
    }

    /// Côté B : une connexion entrante complète.
    async fn respond(self: &Arc<Self>, conn: &Connection, send: &mut SendStream, recv: &mut RecvStream) -> Result<()> {
        let t = self.cfg.io_timeout;
        let m1 = decode(&read_frame(recv, t).await?)?;
        let Body::Request { name, commit: commit_a } = &m1.body else {
            return Err(Error::Invalid("REQUEST attendu".into()));
        };
        check_header(&m1, Some(conn.remote_id().as_bytes()), &self.me)?;
        if m1.from == self.me {
            return Err(Error::Invalid("demande à soi-même".into()));
        }
        let peer = hex::encode(m1.from);
        if self.db.is_friend(&peer)? {
            return Err(Error::Invalid("déjà ami".into()));
        }
        let adm = self.admit(&peer, commit_a)?;

        let nonce_b = random32();
        let commit_b = commitment(&self.me, &nonce_b);
        let session = session_id(commit_a, &commit_b);
        let reply = self.sign(m1.from, Body::Commit { commit_a: *commit_a, commit_b });
        write_frame(send, &reply).await?;

        let m2 = decode(&read_frame(recv, t).await?)?;
        check_header(&m2, Some(&m1.from), &self.me)?;
        let Body::Reveal { session: s2, nonce: nonce_a } = m2.body else {
            return Err(Error::Invalid("REVEAL attendu".into()));
        };
        if s2 != session {
            return Err(Error::Auth("session inconnue".into()));
        }
        if commitment(&m1.from, &nonce_a) != *commit_a {
            return Err(Error::Integrity { expected: hex::encode(commit_a), actual: hex::encode(commitment(&m1.from, &nonce_a)) });
        }
        write_frame(send, &self.sign(m1.from, Body::Reveal { session, nonce: nonce_b })).await?;

        let code = verify_code(&self.me, &m1.from, &nonce_b, &nonce_a);
        let req = FriendRequest { public_key: peer.clone(), name: name.clone(), verify_code: code, received_at: now_secs() as i64 };
        self.db.upsert_request(&req, Direction::In)?;
        let (tx, rx) = oneshot::channel();
        lock(&self.state).pending.insert(peer.clone(), tx);
        // Pair authentifié (signatures, engagement, admission) : on retient l'adresse de sa connexion,
        // utilisée seulement si la demande est acceptée.
        let observed = observed_addr(conn);
        (self.sink)(FriendEvent::IncomingRequest(req));

        let outcome = tokio::select! {
            d = rx => d.ok(),
            _ = recv.read_to_end(16) => None,
            () = tokio::time::sleep(self.cfg.decision_ttl) => None,
        };
        let Some(d) = outcome else {
            let _ = self.db.remove_request(&peer);
            drop(adm);
            return Ok(());
        };
        let msg = self.sign(m1.from, Body::Decision { session, accept: d.accept, name: self.my_name() });
        if d.accept {
            let sink = lock(&self.addr_sink).clone();
            if let Some(s) = sink {
                s(observed);
            }
        }
        let written = write_frame(send, &msg).await;
        let _ = send.finish();
        if written.is_ok() {
            // Attend que A ait lu et fermé la connexion avant de libérer la connexion.
            let _ = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await;
        } else {
            let _ = self.db.remove_request(&peer);
        }
        let _ = d.reply.send(written.map_err(|e| Error::Network(e.to_string())));
        conn.close(0u32.into(), b"done");
        Ok(())
    }

    async fn decide(&self, public_key: &str, accept: bool) -> Result<()> {
        let tx = lock(&self.state)
            .pending
            .remove(public_key)
            .ok_or_else(|| Error::NotFound(format!("demande {public_key}")))?;
        let (reply, rx) = oneshot::channel();
        tx.send(Decision { accept, reply }).map_err(|_| Error::NotFound(format!("demande {public_key} expirée")))?;
        tokio::time::timeout(self.cfg.io_timeout * 2, rx)
            .await
            .map_err(|_| Error::Network("délai dépassé".into()))?
            .map_err(|_| Error::Network("connexion perdue".into()))?
    }
}

// ---------- handler (côté B) ----------

/// Gestionnaire du protocole, à enregistrer sur un `Router` (déjà fait par `FriendService::start`).
#[derive(Clone)]
pub struct FriendHandler {
    shared: Arc<Shared>,
}

impl fmt::Debug for FriendHandler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FriendHandler")
    }
}

impl ProtocolHandler for FriendHandler {
    async fn accept(&self, conn: Connection) -> std::result::Result<(), AcceptError> {
        let Ok(permit): std::result::Result<OwnedSemaphorePermit, _> = self.shared.handshakes.clone().try_acquire_owned()
        else {
            conn.close(0u32.into(), b"");
            return Ok(());
        };
        let _permit = permit;
        let io = self.shared.cfg.io_timeout;
        let Ok(Ok((mut send, mut recv))) = tokio::time::timeout(io, conn.accept_bi()).await else {
            return Ok(());
        };
        if let Err(e) = self.shared.respond(&conn, &mut send, &mut recv).await {
            // Rejet silencieux : aucune réponse, simple fermeture.
            tracing::debug!("demande d'ami rejetée : {e}");
            conn.close(0u32.into(), b"");
        }
        Ok(())
    }
}

// ---------- service ----------

/// Service d'ajout d'ami : accepte les demandes en tâche de fond et en envoie.
pub struct FriendService {
    shared: Arc<Shared>,
    endpoint: Endpoint,
    router: Option<Router>,
}

impl FriendService {
    /// Démarre avec les limites par défaut. Crée un `Router` propre à l'endpoint ; si l'application
    /// a déjà un `Router`, utiliser [`FriendService::attach`] et enregistrer [`FriendService::handler`].
    pub fn start(endpoint: Endpoint, identity: Arc<Identity>, db: Arc<FriendsDb>, event_sink: EventSink) -> Result<Self> {
        Self::start_with_config(endpoint, identity, db, event_sink, FriendConfig::default())
    }

    pub fn start_with_config(
        endpoint: Endpoint,
        identity: Arc<Identity>,
        db: Arc<FriendsDb>,
        event_sink: EventSink,
        cfg: FriendConfig,
    ) -> Result<Self> {
        let mut svc = Self::attach(endpoint, identity, db, event_sink, cfg)?;
        let router = Router::builder(svc.endpoint.clone()).accept(ALPN, svc.handler()).spawn();
        svc.router = Some(router);
        Ok(svc)
    }

    /// Sans `Router` : l'appelant enregistre `handler()` sous `ALPN`.
    pub fn attach(
        endpoint: Endpoint,
        identity: Arc<Identity>,
        db: Arc<FriendsDb>,
        event_sink: EventSink,
        cfg: FriendConfig,
    ) -> Result<Self> {
        let me = identity.public_key().to_bytes();
        let name = format!("Joueur-{}", &hex::encode(me)[..8]);
        let shared = Arc::new(Shared {
            identity,
            db,
            sink: event_sink,
            handshakes: Arc::new(Semaphore::new(cfg.max_concurrent_handshakes.max(1))),
            cfg,
            me,
            name: Mutex::new(name),
            state: Mutex::new(State::default()),
            addr_sink: Mutex::new(None),
        });
        Ok(Self { shared, endpoint, router: None })
    }

    /// Destinataire de l'adresse d'un demandeur accepté (à persister) : appelé avant la réponse
    /// d'acceptation, donc avant le retour de [`FriendService::accept`].
    pub fn set_addr_sink(&self, sink: AddrSink) {
        *lock(&self.shared.addr_sink) = Some(sink);
    }

    pub fn handler(&self) -> FriendHandler {
        FriendHandler { shared: self.shared.clone() }
    }

    /// Nom Minecraft déclaré dans les demandes et acceptations.
    pub fn set_name(&self, name: &str) -> Result<()> {
        check_name(name)?;
        *lock(&self.shared.name) = name.to_string();
        Ok(())
    }

    pub async fn shutdown(&self) {
        if let Some(r) = &self.router {
            let _ = r.shutdown().await;
        }
    }

    /// Envoie une demande au pair du code ami. Renvoie le code de vérification à comparer à l'oral ;
    /// la décision de l'autre côté est attendue en tâche de fond (`Accepted` / `Refused`).
    pub async fn send_request(&self, code: &str) -> Result<VerifyCode> {
        let sh = &self.shared;
        let payload = friend_code::parse(code)?;
        let peer_bytes = payload.public_key;
        if peer_bytes == sh.me {
            return Err(Error::Invalid("on ne peut pas s'ajouter soi-même".into()));
        }
        let peer = hex::encode(peer_bytes);
        if sh.db.is_friend(&peer)? {
            return Err(Error::Invalid("déjà ami".into()));
        }
        {
            let mut st = lock(&sh.state);
            if !st.outgoing.insert(peer.clone()) {
                return Err(Error::Invalid("une demande est déjà en cours pour ce pair".into()));
            }
        }
        let guard = OutGuard { shared: sh.clone(), peer: peer.clone() };

        let id = EndpointId::from_bytes(&peer_bytes).map_err(|_| Error::Invalid("clé publique invalide".into()))?;
        let mut addr = EndpointAddr::new(id);
        if let Some(r) = payload.relay.as_deref().and_then(|r| r.parse::<RelayUrl>().ok()) {
            addr = addr.with_relay_url(r);
        }
        for a in payload.addrs.iter().filter_map(|a| a.parse::<SocketAddr>().ok()) {
            addr = addr.with_ip_addr(a);
        }

        let t = sh.cfg.io_timeout;
        let conn = tokio::time::timeout(t * 2, self.endpoint.connect(addr, ALPN))
            .await
            .map_err(|_| Error::Network("connexion : délai dépassé".into()))?
            .map_err(net)?;
        let (mut send, mut recv) = tokio::time::timeout(t, conn.open_bi())
            .await
            .map_err(|_| Error::Network("délai dépassé".into()))?
            .map_err(net)?;

        let nonce_a = random32();
        let commit_a = commitment(&sh.me, &nonce_a);
        write_frame(&mut send, &sh.sign(peer_bytes, Body::Request { name: sh.my_name(), commit: commit_a })).await?;

        let m = decode(&read_frame(&mut recv, t).await?)?;
        check_header(&m, Some(&peer_bytes), &sh.me)?;
        let Body::Commit { commit_a: echoed, commit_b } = m.body else {
            return Err(Error::Invalid("COMMIT attendu".into()));
        };
        if echoed != commit_a {
            return Err(Error::Auth("engagement non reconnu".into()));
        }
        let session = session_id(&commit_a, &commit_b);
        // Les deux engagements sont échangés : on peut révéler.
        write_frame(&mut send, &sh.sign(peer_bytes, Body::Reveal { session, nonce: nonce_a })).await?;

        let m = decode(&read_frame(&mut recv, t).await?)?;
        check_header(&m, Some(&peer_bytes), &sh.me)?;
        let Body::Reveal { session: s2, nonce: nonce_b } = m.body else {
            return Err(Error::Invalid("REVEAL attendu".into()));
        };
        if s2 != session {
            return Err(Error::Auth("session inconnue".into()));
        }
        let actual = commitment(&peer_bytes, &nonce_b);
        if actual != commit_b {
            return Err(Error::Integrity { expected: hex::encode(commit_b), actual: hex::encode(actual) });
        }
        let code = verify_code(&sh.me, &peer_bytes, &nonce_a, &nonce_b);

        let shared = sh.clone();
        tokio::spawn(async move {
            let _guard = guard;
            let wait = shared.cfg.decision_ttl + shared.cfg.io_timeout;
            let result: Result<()> = async {
                let m = decode(&read_frame(&mut recv, wait).await?)?;
                check_header(&m, Some(&peer_bytes), &shared.me)?;
                let Body::Decision { session: s3, accept, name } = m.body else {
                    return Err(Error::Invalid("DECISION attendue".into()));
                };
                if s3 != session {
                    return Err(Error::Auth("session inconnue".into()));
                }
                if accept {
                    let friend = Friend { public_key: hex::encode(peer_bytes), name, trust: TrustLevel::Ask, added_at: now_secs() as i64 };
                    shared.db.add_friend(&friend)?;
                    (shared.sink)(FriendEvent::Accepted(friend));
                } else {
                    (shared.sink)(FriendEvent::Refused(hex::encode(peer_bytes)));
                }
                Ok(())
            }
            .await;
            if let Err(e) = result {
                tracing::debug!("décision d'ami non appliquée : {e}");
            }
            let _ = send.finish();
            conn.close(0u32.into(), b"ok");
        });
        Ok(code)
    }

    /// Accepte une demande reçue : envoie l'acceptation signée puis crée l'ami.
    pub async fn accept(&self, public_key: &str) -> Result<Friend> {
        self.shared.decide(public_key, true).await?;
        self.shared.db.accept_request(public_key)
    }

    /// Refuse une demande reçue : envoie le refus signé puis retire la demande.
    pub async fn refuse(&self, public_key: &str) -> Result<()> {
        self.shared.decide(public_key, false).await?;
        self.shared.db.remove_request(public_key)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::endpoint::presets;
    use iroh::{RelayMode, SecretKey};

    type Events = Arc<Mutex<Vec<FriendEvent>>>;

    struct Node {
        _dir: tempfile::TempDir,
        ident: Arc<Identity>,
        ep: Endpoint,
        db: Arc<FriendsDb>,
        svc: FriendService,
        events: Events,
        code: String,
    }

    impl Node {
        async fn new(name: &str) -> Node {
            Self::with_config(name, FriendConfig { io_timeout: Duration::from_secs(5), ..FriendConfig::default() }).await
        }

        async fn with_config(name: &str, cfg: FriendConfig) -> Node {
            let dir = tempfile::tempdir().unwrap();
            let ident = Arc::new(Identity::load_or_create(dir.path()).unwrap());
            let secret: [u8; 32] = std::fs::read(dir.path().join("identity.key")).unwrap().try_into().unwrap();
            let ep = Endpoint::builder(presets::Minimal)
                .relay_mode(RelayMode::Disabled)
                .secret_key(SecretKey::from_bytes(&secret))
                .bind()
                .await
                .unwrap();
            assert_eq!(ep.id().as_bytes(), &ident.public_key().to_bytes());
            let db = Arc::new(FriendsDb::open_in_memory().unwrap());
            let events: Events = Arc::default();
            let ev = events.clone();
            let sink: EventSink = Arc::new(move |e| lock(&ev).push(e));
            let svc = FriendService::start_with_config(ep.clone(), ident.clone(), db.clone(), sink, cfg).unwrap();
            svc.set_name(name).unwrap();
            let code = friend_code::encode(&friend_code::FriendCodePayload {
                public_key: ident.public_key().to_bytes(),
                epoch: 0,
                relay: None,
                addrs: ep.addr().ip_addrs().map(|a| a.to_string()).take(4).collect(),
            });
            Node { _dir: dir, ident, ep, db, svc, events, code }
        }

        fn pk(&self) -> [u8; 32] {
            self.ident.public_key().to_bytes()
        }
        fn hex(&self) -> String {
            self.ident.public_key_hex()
        }
        fn events(&self) -> Vec<FriendEvent> {
            lock(&self.events).clone()
        }
        async fn wait_event(&self, f: impl Fn(&FriendEvent) -> bool) -> FriendEvent {
            for _ in 0..200 {
                if let Some(e) = self.events().into_iter().find(|e| f(e)) {
                    return e;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            panic!("événement attendu non reçu");
        }
        async fn raw(&self, target: &Node) -> (Connection, SendStream, RecvStream) {
            let c = self.ep.connect(target.ep.addr(), ALPN).await.unwrap();
            let (s, r) = c.open_bi().await.unwrap();
            (c, s, r)
        }
        fn msg(&self, to: &[u8; 32], ts: u64, body: Body) -> Vec<u8> {
            encode(&Msg { from: self.pk(), to: *to, ts, body }, &self.ident)
        }
    }

    async fn send_raw(s: &mut SendStream, frame: &[u8]) {
        let _ = write_frame(s, frame).await;
    }

    async fn try_read(r: &mut RecvStream) -> Option<Vec<u8>> {
        read_frame(r, Duration::from_secs(2)).await.ok()
    }

    fn nothing_happened(n: &Node) {
        assert!(n.events().is_empty());
        assert_eq!(n.db.request_count().unwrap(), 0);
        assert!(n.db.list_friends().unwrap().is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn full_exchange_then_accept() {
        let a = Node::new("Alice").await;
        let b = Node::new("Bob").await;
        let code_a = a.svc.send_request(&b.code).await.unwrap();
        let ev = b.wait_event(|e| matches!(e, FriendEvent::IncomingRequest(_))).await;
        let FriendEvent::IncomingRequest(req) = ev else { unreachable!() };
        assert_eq!(req.verify_code, code_a, "même code des deux côtés");
        assert_eq!((req.name.as_str(), req.public_key.as_str()), ("Alice", a.hex().as_str()));
        // Demande rangée, aucun ami avant acceptation.
        assert_eq!(b.db.request_count().unwrap(), 1);
        assert!(b.db.list_friends().unwrap().is_empty());
        assert!(a.db.list_friends().unwrap().is_empty());

        let friend = b.svc.accept(&a.hex()).await.unwrap();
        assert_eq!(friend.name, "Alice");
        a.wait_event(|e| matches!(e, FriendEvent::Accepted(_))).await;
        assert!(a.db.is_friend(&b.hex()).unwrap());
        assert_eq!(a.db.get_friend(&b.hex()).unwrap().unwrap().name, "Bob");
        assert!(b.db.is_friend(&a.hex()).unwrap());
        assert_eq!(b.db.request_count().unwrap(), 0);
        // Une fois amis, nouvelle demande refusée localement.
        assert!(a.svc.send_request(&b.code).await.is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn accepted_requester_address_is_remembered_before_accept_returns() {
        let a = Node::new("Alice").await;
        let b = Node::new("Bob").await;
        let got: Arc<Mutex<Vec<EndpointAddr>>> = Arc::default();
        let g2 = got.clone();
        b.svc.set_addr_sink(Arc::new(move |addr| lock(&g2).push(addr)));
        a.svc.send_request(&b.code).await.unwrap();
        b.wait_event(|e| matches!(e, FriendEvent::IncomingRequest(_))).await;
        // Avant la décision : rien n'est mémorisé.
        assert!(lock(&got).is_empty());
        b.svc.accept(&a.hex()).await.unwrap();
        let got = lock(&got).clone();
        assert_eq!(got.len(), 1, "une seule adresse, dès le retour de accept (sans attendre de sonde)");
        assert_eq!(got[0].id.as_bytes(), &a.pk());
        assert!(got[0].ip_addrs().next().is_some(), "au moins une IP du demandeur");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn refused_requester_address_is_not_remembered() {
        let a = Node::new("Alice").await;
        let b = Node::new("Bob").await;
        let got: Arc<Mutex<Vec<EndpointAddr>>> = Arc::default();
        let g2 = got.clone();
        b.svc.set_addr_sink(Arc::new(move |addr| lock(&g2).push(addr)));
        let _ = a.svc.send_request(&b.code).await;
        b.wait_event(|e| matches!(e, FriendEvent::IncomingRequest(_))).await;
        b.svc.refuse(&a.hex()).await.unwrap();
        assert!(lock(&got).is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn refusal_creates_no_friend() {
        let a = Node::new("Alice").await;
        let b = Node::new("Bob").await;
        a.svc.send_request(&b.code).await.unwrap();
        b.wait_event(|e| matches!(e, FriendEvent::IncomingRequest(_))).await;
        b.svc.refuse(&a.hex()).await.unwrap();
        a.wait_event(|e| matches!(e, FriendEvent::Refused(k) if *k == b.hex())).await;
        assert!(a.db.list_friends().unwrap().is_empty());
        assert!(b.db.list_friends().unwrap().is_empty());
        assert_eq!(b.db.request_count().unwrap(), 0);
        assert!(matches!(b.svc.accept(&a.hex()).await, Err(Error::NotFound(_))));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn forged_signature_rejected() {
        let b = Node::new("Bob").await;
        let x = Node::new("Eve").await;
        let mut f = x.msg(&b.pk(), now_secs(), Body::Request { name: "Eve".into(), commit: random32() });
        let n = f.len();
        f[n - 1] ^= 1;
        let (_c, mut s, mut r) = x.raw(&b).await;
        send_raw(&mut s, &f).await;
        assert!(try_read(&mut r).await.is_none());
        // Corps modifié après signature.
        let mut g = x.msg(&b.pk(), now_secs(), Body::Request { name: "Eve".into(), commit: random32() });
        g[HEADER_LEN + 5] ^= 1;
        let (_c2, mut s2, mut r2) = x.raw(&b).await;
        send_raw(&mut s2, &g).await;
        assert!(try_read(&mut r2).await.is_none());
        nothing_happened(&b);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn oversized_message_rejected() {
        let b = Node::new("Bob").await;
        let x = Node::new("Eve").await;
        let (_c, mut s, mut r) = x.raw(&b).await;
        let _ = s.write_all(&(MAX_FRAME as u32 + 1).to_be_bytes()).await;
        let _ = s.write_all(&vec![7u8; MAX_FRAME + 1]).await;
        assert!(try_read(&mut r).await.is_none());
        assert!(write_frame(&mut s, &vec![0u8; MAX_FRAME + 1]).await.is_err());
        nothing_happened(&b);
        // Le service reste utilisable.
        x.svc.send_request(&b.code).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn replay_and_stale_rejected() {
        let b = Node::new("Bob").await;
        let x = Node::new("Eve").await;
        let f = x.msg(&b.pk(), now_secs(), Body::Request { name: "Eve".into(), commit: random32() });
        let (c1, mut s1, mut r1) = x.raw(&b).await;
        send_raw(&mut s1, &f).await;
        assert!(try_read(&mut r1).await.is_some(), "première fois : COMMIT");
        c1.close(0u32.into(), b"");
        tokio::time::sleep(Duration::from_millis(300)).await;
        let (_c2, mut s2, mut r2) = x.raw(&b).await;
        send_raw(&mut s2, &f).await;
        assert!(try_read(&mut r2).await.is_none(), "rejeu rejeté");
        // Horodatage trop ancien ou futur (signé correctement).
        for ts in [now_secs() - MAX_SKEW_SECS - 60, now_secs() + MAX_SKEW_SECS + 60] {
            let g = x.msg(&b.pk(), ts, Body::Request { name: "Eve".into(), commit: random32() });
            let (_c, mut s, mut r) = x.raw(&b).await;
            send_raw(&mut s, &g).await;
            assert!(try_read(&mut r).await.is_none(), "ts {ts}");
        }
        // Message destiné à un tiers.
        let other = [9u8; 32];
        let h = x.msg(&other, now_secs(), Body::Request { name: "Eve".into(), commit: random32() });
        let (_c, mut s, mut r) = x.raw(&b).await;
        send_raw(&mut s, &h).await;
        assert!(try_read(&mut r).await.is_none());
        nothing_happened(&b);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn nonce_not_matching_commitment_rejected_by_responder() {
        let b = Node::new("Bob").await;
        let x = Node::new("Eve").await;
        let nonce = random32();
        let (_c, mut s, mut r) = x.raw(&b).await;
        send_raw(&mut s, &x.msg(&b.pk(), now_secs(), Body::Request { name: "Eve".into(), commit: commitment(&x.pk(), &nonce) })).await;
        let m = decode(&try_read(&mut r).await.unwrap()).unwrap();
        let Body::Commit { commit_a, commit_b } = m.body else { panic!() };
        let session = session_id(&commit_a, &commit_b);
        // Révèle un autre nonce que celui engagé (grinding).
        send_raw(&mut s, &x.msg(&b.pk(), now_secs(), Body::Reveal { session, nonce: random32() })).await;
        assert!(try_read(&mut r).await.is_none());
        tokio::time::sleep(Duration::from_millis(200)).await;
        nothing_happened(&b);
    }

    #[derive(Clone)]
    struct EvilResponder {
        ident: Arc<Identity>,
    }
    impl fmt::Debug for EvilResponder {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("EvilResponder")
        }
    }
    impl ProtocolHandler for EvilResponder {
        async fn accept(&self, conn: Connection) -> std::result::Result<(), AcceptError> {
            let (mut s, mut r) = conn.accept_bi().await?;
            let me = self.ident.public_key().to_bytes();
            let m = decode(&read_frame(&mut r, Duration::from_secs(5)).await.unwrap()).unwrap();
            let Body::Request { commit, .. } = m.body else { return Ok(()) };
            let nonce_b = random32();
            let commit_b = commitment(&me, &nonce_b);
            let sign = |body| encode(&Msg { from: me, to: m.from, ts: now_secs(), body }, &self.ident);
            write_frame(&mut s, &sign(Body::Commit { commit_a: commit, commit_b })).await.unwrap();
            let _ = read_frame(&mut r, Duration::from_secs(5)).await;
            // Révèle un nonce différent de l'engagement.
            let session = session_id(&commit, &commit_b);
            let _ = write_frame(&mut s, &sign(Body::Reveal { session, nonce: random32() })).await;
            let _ = tokio::time::timeout(Duration::from_secs(3), conn.closed()).await;
            Ok(())
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn nonce_not_matching_commitment_rejected_by_initiator() {
        let a = Node::new("Alice").await;
        let dir = tempfile::tempdir().unwrap();
        let ident = Arc::new(Identity::load_or_create(dir.path()).unwrap());
        let secret: [u8; 32] = std::fs::read(dir.path().join("identity.key")).unwrap().try_into().unwrap();
        let ep = Endpoint::builder(presets::Minimal)
            .relay_mode(RelayMode::Disabled)
            .secret_key(SecretKey::from_bytes(&secret))
            .bind()
            .await
            .unwrap();
        let _router = Router::builder(ep.clone()).accept(ALPN, EvilResponder { ident: ident.clone() }).spawn();
        let code = friend_code::encode(&friend_code::FriendCodePayload {
            public_key: ident.public_key().to_bytes(),
            epoch: 0,
            relay: None,
            addrs: ep.addr().ip_addrs().map(|a| a.to_string()).take(4).collect(),
        });
        let r = a.svc.send_request(&code).await;
        assert!(matches!(r, Err(Error::Integrity { .. })), "{r:?}");
        nothing_happened(&a);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn simulated_mitm_gives_different_codes() {
        // A croit parler à B mais le code ami contient la clé de Mallory ; Mallory relaie vers B
        // avec sa propre identité : les deux victimes voient des codes différents.
        let a = Node::new("Alice").await;
        let m = Node::new("Mallory").await;
        let b = Node::new("Bob").await;
        let code_a = a.svc.send_request(&m.code).await.unwrap();
        let code_b = m.svc.send_request(&b.code).await.unwrap();
        let FriendEvent::IncomingRequest(req_b) = b.wait_event(|e| matches!(e, FriendEvent::IncomingRequest(_))).await else {
            unreachable!()
        };
        assert_eq!(req_b.verify_code, code_b);
        assert_ne!(code_a, req_b.verify_code);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn request_to_self_rejected() {
        let a = Node::new("Alice").await;
        let r = a.svc.send_request(&a.code).await;
        assert!(matches!(r, Err(Error::Invalid(_))), "{r:?}");
        assert!(a.svc.send_request("pas un code").await.is_err());
        nothing_happened(&a);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn anti_spam_limits() {
        let cfg = FriendConfig { max_requests_per_peer: 2, max_pending_total: 2, io_timeout: Duration::from_secs(5), ..FriendConfig::default() };
        let b = Node::with_config("Bob", cfg).await;
        let x = Node::new("Eve").await;
        // Par pair : 2 tentatives passent, la 3e est ignorée.
        for i in 0..3 {
            let f = x.msg(&b.pk(), now_secs(), Body::Request { name: "Eve".into(), commit: random32() });
            let (c, mut s, mut r) = x.raw(&b).await;
            send_raw(&mut s, &f).await;
            assert_eq!(try_read(&mut r).await.is_some(), i < 2, "tentative {i}");
            c.close(0u32.into(), b"");
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        // Total : deux pairs retiennent la capacité, un troisième est ignoré.
        let cfg = FriendConfig { max_pending_total: 2, io_timeout: Duration::from_secs(5), ..FriendConfig::default() };
        let b = Node::with_config("Bob", cfg).await;
        let mut held = Vec::new();
        for _ in 0..2 {
            let p = Node::new("P").await;
            let (c, mut s, mut r) = p.raw(&b).await;
            send_raw(&mut s, &p.msg(&b.pk(), now_secs(), Body::Request { name: "P".into(), commit: random32() })).await;
            assert!(try_read(&mut r).await.is_some());
            held.push((p, c, s, r));
        }
        let late = Node::new("Late").await;
        let (_c, mut s, mut r) = late.raw(&b).await;
        send_raw(&mut s, &late.msg(&b.pk(), now_secs(), Body::Request { name: "L".into(), commit: random32() })).await;
        assert!(try_read(&mut r).await.is_none());
        nothing_happened(&b);
    }

    #[test]
    fn codec_roundtrip_and_tamper() {
        let d = tempfile::tempdir().unwrap();
        let id = Identity::load_or_create(d.path()).unwrap();
        let m = Msg {
            from: id.public_key().to_bytes(),
            to: [3; 32],
            ts: 42,
            body: Body::Decision { session: [1; 32], accept: true, name: "Bob".into() },
        };
        let f = encode(&m, &id);
        assert_eq!(decode(&f).unwrap(), m);
        for i in 0..f.len() {
            let mut g = f.clone();
            g[i] ^= 0x80;
            assert!(decode(&g).is_err(), "octet {i}");
        }
        assert!(decode(&f[..f.len() - 1]).is_err());
        assert!(decode(&[]).is_err());
        assert!(decode(&vec![0u8; MAX_FRAME + 1]).is_err());
        // Commitment lié à la clé : même nonce, autre clé -> autre engagement.
        assert_ne!(commitment(&[1; 32], &[5; 32]), commitment(&[2; 32], &[5; 32]));
    }
}
