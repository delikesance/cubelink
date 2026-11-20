//! Tâche N14 : tunnel de jeu P2P (TCP local <-> flux QUIC bidirectionnel Iroh).
//!
//! L'hôte expose son monde LAN (`127.0.0.1:<port>`) ; l'invité ouvre un port local éphémère
//! (`127.0.0.1` uniquement) et chaque connexion TCP est relayée dans un flux QUIC bidirectionnel.
//!
//! Sécurité : pair non autorisé = connexion fermée avant toute donnée ; l'hôte ne se connecte qu'à
//! `127.0.0.1:<port du monde>` (jamais une adresse du pair) ; connexions simultanées bornées ;
//! délais de connexion et d'inactivité ; tout est fermé au `stop()`/`drop` (pas de tâche orpheline).
//!
//! L'endpoint de l'hôte doit avoir [`TUNNEL_ALPN`] dans ses ALPN et lui être dédié (il y accepte
//! les connexions entrantes). Le protocole attend que le client TCP parle en premier (Minecraft).

use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU16, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use crate::reach::Reach;
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::{Endpoint, EndpointAddr};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{watch, Semaphore};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::timeout;

/// Attente du refus éventuel de l'hôte après l'établissement de la connexion.
const SETTLE: Duration = Duration::from_millis(350);

/// Période de revérification de l'autorisation d'une connexion déjà établie (révocation d'accès). Chaque revérification
/// coûte une requête en base sous le mutex partagé : 2 s borne la coupure d'une connexion inactive sans la payer
/// toutes les 500 ms par connexion. Chaque nouveau flux est de toute façon revérifié (`accept_bi`) : aucun accès
/// n'est possible après révocation.
const AUTHZ_RECHECK: Duration = Duration::from_secs(2);

/// ALPN dédié au tunnel de jeu.
pub const TUNNEL_ALPN: &[u8] = b"cubelink/tunnel/1";

/// Décide si un pair (clé publique ed25519) peut ouvrir un tunnel vers le monde de l'hôte.
pub trait TunnelAuthorizer: Send + Sync {
    fn is_allowed(&self, public_key: &[u8; 32]) -> bool;

    /// Raison du refus quand `is_allowed` est faux (annoncée à l'invité, y compris à la révocation) : par défaut « pas ami ».
    fn denial(&self, _public_key: &[u8; 32]) -> Reach {
        Reach::NotFriend
    }
}

/// Autorise un ensemble fixe de clés (tests, usage simple).
#[derive(Debug, Clone, Default)]
pub struct AllowKeys(pub Vec<[u8; 32]>);

impl TunnelAuthorizer for AllowKeys {
    fn is_allowed(&self, public_key: &[u8; 32]) -> bool {
        self.0.contains(public_key)
    }
}

/// Réglages du tunnel.
#[derive(Debug, Clone, Copy)]
pub struct TunnelConfig {
    /// Connexions de jeu simultanées maximales, tous pairs confondus (un groupe Hearth peut dépasser 8 membres).
    pub max_connections: usize,
    /// Connexions simultanées maximales d'un même pair : une partie, un ping de la liste des serveurs et une
    /// reconnexion tiennent dedans, mais un client bogué ou malveillant ne peut pas bloquer les autres.
    pub max_per_peer: usize,
    /// Délai pour établir une connexion (QUIC ou TCP local).
    pub connect_timeout: Duration,
    /// Inactivité maximale d'une connexion relayée (dans un sens).
    pub idle_timeout: Duration,
    /// Durée maximale pendant laquelle l'invité retente de se reconnecter à l'hôte après une coupure.
    pub reconnect_budget: Duration,
}

impl Default for TunnelConfig {
    fn default() -> Self {
        Self {
            max_connections: 32,
            max_per_peer: 4,
            connect_timeout: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(120),
            reconnect_budget: Duration::from_secs(10),
        }
    }
}

/// Chemin réseau utilisé (pour l'UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    Direct,
    Relay,
    Unknown,
}

/// Statistiques instantanées.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TunnelStats {
    pub active_connections: usize,
    pub total_connections: u64,
    pub rejected_connections: u64,
    /// Côté invité : flux refusés par l'hôte parce qu'il est plein ou que ce pair a trop de connexions ouvertes
    /// (code [`crate::reach::CLOSE_FULL`]). Toujours 0 côté hôte.
    pub refused_full: u64,
    /// Octets TCP -> QUIC (côté local vers le pair).
    pub bytes_to_peer: u64,
    /// Octets QUIC -> TCP.
    pub bytes_from_peer: u64,
}

#[derive(Default)]
struct Counters {
    total: AtomicU64,
    rejected: AtomicU64,
    full: AtomicU64,
    up: AtomicU64,
    down: AtomicU64,
}

impl Counters {
    fn snapshot(&self, sem: &Semaphore, max: usize) -> TunnelStats {
        TunnelStats {
            active_connections: max.saturating_sub(sem.available_permits()),
            total_connections: self.total.load(Ordering::Relaxed),
            rejected_connections: self.rejected.load(Ordering::Relaxed),
            refused_full: self.full.load(Ordering::Relaxed),
            bytes_to_peer: self.up.load(Ordering::Relaxed),
            bytes_from_peer: self.down.load(Ordering::Relaxed),
        }
    }
}

/// Ferme la connexion QUIC quand la tâche qui la possède est terminée ou abandonnée.
struct ConnGuard(Connection);

impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.0.close(0u32.into(), b"stop");
    }
}

async fn pump<R, W>(r: &mut R, w: &mut W, idle: Duration, counter: &AtomicU64) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = timeout(idle, r.read(&mut buf))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "inactivité"))??;
        if n == 0 {
            return Ok(());
        }
        timeout(idle, w.write_all(&buf[..n]))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "écriture bloquée"))??;
        counter.fetch_add(n as u64, Ordering::Relaxed);
    }
}

/// Relaie TCP <-> flux QUIC avec propagation de la demi-fermeture dans les deux sens.
async fn relay(
    tcp: TcpStream,
    mut send: iroh::endpoint::SendStream,
    mut recv: iroh::endpoint::RecvStream,
    cfg: TunnelConfig,
    c: &Counters,
) {
    let _ = tcp.set_nodelay(true);
    let (mut tr, mut tw) = tcp.into_split();
    let res = {
        let up = async {
            pump(&mut tr, &mut send, cfg.idle_timeout, &c.up).await?;
            send.finish().map_err(io::Error::other)
        };
        let down = async {
            pump(&mut recv, &mut tw, cfg.idle_timeout, &c.down).await?;
            let _ = tw.shutdown().await;
            io::Result::Ok(())
        };
        tokio::try_join!(up, down)
    };
    if res.is_err() {
        // L'hôte plein (ou trop de connexions de ce pair) réinitialise le flux avec CLOSE_FULL : on le compte
        // pour que l'interface puisse l'expliquer. La lecture est déjà en erreur : la réponse est immédiate.
        if let Ok(Ok(Some(code))) = timeout(Duration::from_millis(50), recv.received_reset()).await {
            if code.into_inner() == u64::from(crate::reach::CLOSE_FULL) {
                c.full.fetch_add(1, Ordering::Relaxed);
            }
        }
        let _ = send.reset(0u32.into());
        let _ = recv.stop(0u32.into());
    }
}

/// Connexions ouvertes par pair (clé publique) : le compteur monte à l'acceptation d'un flux et redescend quand
/// le [`PeerPermit`] est abandonné (fin du relais, ou tâche annulée à la fermeture de la connexion).
type PerPeer = Arc<std::sync::Mutex<std::collections::HashMap<[u8; 32], usize>>>;

/// Place d'un pair parmi ses `max_per_peer` connexions.
struct PeerPermit {
    map: PerPeer,
    peer: [u8; 32],
}

impl PeerPermit {
    fn acquire(map: &PerPeer, peer: [u8; 32], max: usize) -> Option<PeerPermit> {
        let mut m = crate::util::lock(map);
        let n = m.entry(peer).or_insert(0);
        if *n >= max {
            if *n == 0 {
                m.remove(&peer);
            }
            return None;
        }
        *n += 1;
        Some(PeerPermit { map: map.clone(), peer })
    }
}

impl Drop for PeerPermit {
    fn drop(&mut self) {
        let mut m = crate::util::lock(&self.map);
        if let Some(n) = m.get_mut(&self.peer) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                m.remove(&self.peer);
            }
        }
    }
}

// ---------------------------------------------------------------- hôte

/// État partagé de l'hôte (boucle d'acceptation propre ou handler de `Router`).
struct HostCore {
    /// Port du monde LAN ; 0 = rien de partagé (connexions refusées).
    port: AtomicU16,
    authorizer: Arc<dyn TunnelAuthorizer>,
    cfg: TunnelConfig,
    sem: Arc<Semaphore>,
    per_peer: PerPeer,
    counters: Arc<Counters>,
    /// Passe à `true` à l'arrêt : les connexions servies par un `Router` sont alors fermées.
    stop: watch::Sender<bool>,
}

/// Côté hôte : accepte les pairs autorisés et relaie leurs flux vers le monde local.
///
/// Deux modes : [`HostTunnel::start`] consomme `endpoint.accept()` (endpoint dédié) ;
/// [`HostTunnel::attach`] + [`HostTunnel::handler`] s'enregistre dans un `Router` partagé.
pub struct HostTunnel {
    task: Option<JoinHandle<()>>,
    core: Arc<HostCore>,
}

impl HostTunnel {
    pub fn start(
        endpoint: Endpoint,
        world_port: u16,
        authorizer: Arc<dyn TunnelAuthorizer>,
    ) -> Result<Self> {
        Self::start_with_config(endpoint, world_port, authorizer, TunnelConfig::default())
    }

    pub fn start_with_config(
        endpoint: Endpoint,
        world_port: u16,
        authorizer: Arc<dyn TunnelAuthorizer>,
        cfg: TunnelConfig,
    ) -> Result<Self> {
        if world_port == 0 {
            return Err(anyhow!("port du monde invalide"));
        }
        let mut me = Self::attach(authorizer, cfg);
        me.set_world_port(world_port);
        let core = me.core.clone();
        me.task = Some(tokio::spawn(async move {
            let mut conns = JoinSet::new();
            loop {
                tokio::select! {
                    inc = endpoint.accept() => {
                        let Some(inc) = inc else { break };
                        conns.spawn(host_conn(inc, core.clone()));
                    }
                    Some(_) = conns.join_next(), if !conns.is_empty() => {}
                }
            }
        }));
        Ok(me)
    }

    /// Sans boucle d'acceptation : l'application enregistre [`HostTunnel::handler`] dans son
    /// `Router` sous [`TUNNEL_ALPN`]. Tant que [`HostTunnel::set_world_port`] n'a pas fourni un
    /// port, toute connexion est refusée (« rien n'est partagé »).
    pub fn attach(authorizer: Arc<dyn TunnelAuthorizer>, cfg: TunnelConfig) -> Self {
        let (stop, _) = watch::channel(false);
        let core = Arc::new(HostCore {
            port: AtomicU16::new(0),
            authorizer,
            cfg,
            sem: Arc::new(Semaphore::new(cfg.max_connections)),
            per_peer: Arc::default(),
            counters: Arc::new(Counters::default()),
            stop,
        });
        Self { task: None, core }
    }

    /// Change le port du monde exposé (0 : ne plus rien exposer pour les NOUVELLES connexions).
    pub fn set_world_port(&self, port: u16) {
        self.core.port.store(port, Ordering::SeqCst);
    }

    /// Handler pour un `Router` externe (`.accept(TUNNEL_ALPN, tunnel.handler())`) : mêmes
    /// contrôles et mêmes compteurs que le mode `start`.
    pub fn handler(&self) -> TunnelHandler {
        TunnelHandler { core: self.core.clone() }
    }

    pub fn stats(&self) -> TunnelStats {
        self.core.counters.snapshot(&self.core.sem, self.core.cfg.max_connections)
    }

    /// Arrête tout : ferme les connexions des pairs et abandonne les relais.
    pub async fn stop(mut self) {
        let _ = self.core.stop.send(true);
        if let Some(t) = self.task.as_mut() {
            t.abort();
            let _ = t.await;
        }
    }
}

impl Drop for HostTunnel {
    fn drop(&mut self) {
        let _ = self.core.stop.send(true);
        if let Some(t) = &self.task {
            t.abort();
        }
    }
}

/// `ProtocolHandler` du tunnel hôte, à enregistrer sous [`TUNNEL_ALPN`] dans un `Router`.
#[derive(Clone)]
pub struct TunnelHandler {
    core: Arc<HostCore>,
}

impl std::fmt::Debug for TunnelHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TunnelHandler")
    }
}

impl ProtocolHandler for TunnelHandler {
    async fn accept(&self, conn: Connection) -> std::result::Result<(), AcceptError> {
        serve_conn(self.core.clone(), conn).await;
        Ok(())
    }
}

async fn host_conn(incoming: iroh::endpoint::Incoming, core: Arc<HostCore>) {
    let Ok(accepting) = incoming.accept() else { return };
    let Ok(Ok(conn)) = timeout(core.cfg.connect_timeout, accepting).await else { return };
    if conn.alpn() != TUNNEL_ALPN {
        conn.close(1u32.into(), b"denied");
        core.counters.rejected.fetch_add(1, Ordering::Relaxed);
        return;
    }
    serve_conn(core, conn).await;
}

/// Code de fermeture annoncé à un pair refusé : la raison donnée par l'autorisation (pas ami, pas invité...), et
/// `CLOSE_DENIED` pour un pair que l'autorisation laisse passer (hôte arrêté).
fn denial_code(core: &HostCore, peer: &[u8; 32]) -> u32 {
    if core.authorizer.is_allowed(peer) {
        return crate::reach::CLOSE_DENIED;
    }
    core.authorizer.denial(peer).close_code().unwrap_or(crate::reach::CLOSE_DENIED)
}

/// Sert une connexion déjà établie sur `TUNNEL_ALPN` : autorisation, puis un flux = un TCP local.
async fn serve_conn(core: Arc<HostCore>, conn: Connection) {
    let _guard = ConnGuard(conn.clone());
    let port = core.port.load(Ordering::SeqCst);
    if !*core.stop.borrow() && port == 0 && core.authorizer.is_allowed(conn.remote_id().as_bytes()) {
        // Ami autorisé mais aucun monde ouvert : raison distincte (annoncée à l'invité).
        conn.close(crate::reach::CLOSE_NOT_SHARED.into(), b"not shared");
        core.counters.rejected.fetch_add(1, Ordering::Relaxed);
        return;
    }
    if port == 0 || *core.stop.borrow() || !core.authorizer.is_allowed(conn.remote_id().as_bytes()) {
        conn.close(denial_code(&core, conn.remote_id().as_bytes()).into(), b"denied");
        core.counters.rejected.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let (cfg, sem, counters) = (core.cfg, core.sem.clone(), core.counters.clone());
    let mut stop = core.stop.subscribe();
    let world = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let peer = *conn.remote_id().as_bytes();
    let mut streams = JoinSet::new();
    let mut recheck = tokio::time::interval(AUTHZ_RECHECK);
    recheck.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = stop.wait_for(|s| *s) => break,
            // Révocation : un pair retiré (membre exclu, invité annulé, ami supprimé) perd sa connexion et ses flux
            // en cours, sans attendre son prochain flux. Fermer la connexion abandonne aussi les relais TCP.
            _ = recheck.tick() => {
                if !core.authorizer.is_allowed(&peer) {
                    conn.close(denial_code(&core, &peer).into(), b"revoked");
                    core.counters.rejected.fetch_add(1, Ordering::Relaxed);
                    break;
                }
            }
            s = conn.accept_bi() => {
                let Ok((mut send, mut recv)) = s else { break };
                // Chaque nouveau flux (un TCP vers le serveur) est revérifié : jamais d'accès après révocation.
                if !core.authorizer.is_allowed(&peer) {
                    let code = denial_code(&core, &peer);
                    let _ = send.reset(code.into());
                    let _ = recv.stop(code.into());
                    conn.close(code.into(), b"revoked");
                    core.counters.rejected.fetch_add(1, Ordering::Relaxed);
                    break;
                }
                // Limite du pair d'abord (il ne consomme alors aucun permis global), puis limite globale. Un refus ne
                // touche que ce flux : la connexion et les autres flux du pair continuent. Le code CLOSE_FULL
                // permet à l'invité de dire « serveur plein » plutôt que d'afficher une déconnexion muette.
                let full = |send: &mut iroh::endpoint::SendStream, recv: &mut iroh::endpoint::RecvStream| {
                    counters.rejected.fetch_add(1, Ordering::Relaxed);
                    let _ = send.reset(crate::reach::CLOSE_FULL.into());
                    let _ = recv.stop(crate::reach::CLOSE_FULL.into());
                };
                let Some(peer_permit) = PeerPermit::acquire(&core.per_peer, peer, cfg.max_per_peer) else {
                    full(&mut send, &mut recv);
                    continue;
                };
                let Ok(permit) = sem.clone().try_acquire_owned() else {
                    full(&mut send, &mut recv);
                    continue;
                };
                counters.total.fetch_add(1, Ordering::Relaxed);
                let counters = counters.clone();
                streams.spawn(async move {
                    let (_permit, _peer_permit) = (permit, peer_permit);
                    match timeout(cfg.connect_timeout, TcpStream::connect(world)).await {
                        Ok(Ok(tcp)) => relay(tcp, send, recv, cfg, &counters).await,
                        _ => {
                            let _ = send.reset(3u32.into());
                            let _ = recv.stop(3u32.into());
                        }
                    }
                });
            }
            Some(_) = streams.join_next(), if !streams.is_empty() => {}
        }
    }
}

// ---------------------------------------------------------------- invité

/// État de la liaison invité -> hôte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelState {
    /// Connexion établie.
    Ok,
    /// Connexion perdue : reconnexion en cours (le port local reste le même).
    Reconnecting,
    /// Reconnexion impossible dans le délai (hôte parti ou refus) : le tunnel est inutilisable.
    Lost,
}

impl TunnelState {
    /// Nom stable pour l'événement `tunnel://state`.
    pub fn as_str(self) -> &'static str {
        match self {
            TunnelState::Ok => "ok",
            TunnelState::Reconnecting => "reconnecting",
            TunnelState::Lost => "lost",
        }
    }
}

/// Connexion à l'hôte (une seule tentative) : « hors ligne » si injoignable, raison précise s'il refuse.
async fn dial(endpoint: &Endpoint, host: &EndpointAddr, wait: Duration) -> Result<Connection> {
    let conn = match timeout(wait, endpoint.connect(host.clone(), TUNNEL_ALPN)).await {
        Ok(Ok(c)) => c,
        _ => return Err(anyhow::Error::new(Reach::Offline)),
    };
    // L'hôte qui refuse ferme tout de suite avec un code : on l'attend brièvement pour renvoyer la vraie raison.
    if timeout(SETTLE, conn.closed()).await.is_ok() {
        return Err(match Reach::from_conn(&conn) {
            Some(r) => anyhow::Error::new(r),
            None => anyhow!("l'hôte a fermé la connexion"),
        });
    }
    Ok(conn)
}

struct GuestShared {
    conn: std::sync::Mutex<Connection>,
    state: tokio::sync::watch::Sender<TunnelState>,
}

impl GuestShared {
    fn conn(&self) -> Connection {
        crate::util::lock(&self.conn).clone()
    }
}

/// Reconnexion avec attente croissante (0 s, 0,5 s, 1 s, 2 s, ...) dans la limite de `reconnect_budget`.
/// Abandonne tout de suite si l'hôte REFUSE (raison explicite) plutôt que d'être injoignable.
async fn redial(endpoint: &Endpoint, host: &EndpointAddr, cfg: &TunnelConfig) -> Option<Connection> {
    let deadline = tokio::time::Instant::now() + cfg.reconnect_budget;
    let mut pause = Duration::ZERO;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return None;
        }
        tokio::time::sleep(pause.min(left)).await;
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return None;
        }
        match dial(endpoint, host, left.min(cfg.connect_timeout)).await {
            Ok(c) => return Some(c),
            Err(e) if e.downcast_ref::<Reach>().is_some_and(|r| *r != Reach::Offline) => return None,
            Err(_) => {}
        }
        pause = if pause.is_zero() { Duration::from_millis(500) } else { (pause * 2).min(Duration::from_secs(3)) };
    }
}

/// Côté invité : port local `127.0.0.1:local_port` à donner à Minecraft. Si la connexion à l'hôte tombe
/// (réseau, relais), elle est rétablie automatiquement ; le port local ne change pas.
pub struct GuestTunnel {
    pub local_port: u16,
    shared: Arc<GuestShared>,
    task: JoinHandle<()>,
    supervisor: JoinHandle<()>,
    sem: Arc<Semaphore>,
    counters: Arc<Counters>,
    max: usize,
}

impl GuestTunnel {
    pub async fn connect(endpoint: Endpoint, host: impl Into<EndpointAddr>) -> Result<Self> {
        Self::connect_with_config(endpoint, host, TunnelConfig::default()).await
    }

    pub async fn connect_with_config(
        endpoint: Endpoint,
        host: impl Into<EndpointAddr>,
        cfg: TunnelConfig,
    ) -> Result<Self> {
        let host: EndpointAddr = host.into();
        let conn = dial(&endpoint, &host, cfg.connect_timeout).await?;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let local_port = listener.local_addr()?.port();
        let sem = Arc::new(Semaphore::new(cfg.max_connections));
        let counters = Arc::new(Counters::default());
        let (state, _) = tokio::sync::watch::channel(TunnelState::Ok);
        let shared = Arc::new(GuestShared { conn: std::sync::Mutex::new(conn), state });

        // Superviseur : à la chute de la connexion, la rétablit ; sinon le tunnel est perdu.
        let (sh, ep, h) = (shared.clone(), endpoint, host);
        let supervisor = tokio::spawn(async move {
            loop {
                let c = sh.conn();
                c.closed().await;
                if matches!(Reach::from_conn(&c), Some(r) if r != Reach::Offline) {
                    // L'hôte a explicitement refusé / fermé le monde : inutile d'insister.
                    sh.state.send_replace(TunnelState::Lost);
                    return;
                }
                sh.state.send_replace(TunnelState::Reconnecting);
                match redial(&ep, &h, &cfg).await {
                    Some(n) => {
                        *crate::util::lock(&sh.conn) = n;
                        sh.state.send_replace(TunnelState::Ok);
                    }
                    None => {
                        sh.state.send_replace(TunnelState::Lost);
                        return;
                    }
                }
            }
        });

        let (sh, s2, k2) = (shared.clone(), sem.clone(), counters.clone());
        let task = tokio::spawn(async move {
            let mut tasks = JoinSet::new();
            let mut st = sh.state.subscribe();
            loop {
                tokio::select! {
                    _ = st.wait_for(|s| *s == TunnelState::Lost) => break,
                    acc = listener.accept() => {
                        let Ok((tcp, _)) = acc else { break };
                        let Ok(permit) = s2.clone().try_acquire_owned() else {
                            k2.rejected.fetch_add(1, Ordering::Relaxed);
                            continue; // tcp fermé au drop
                        };
                        k2.total.fetch_add(1, Ordering::Relaxed);
                        let (sh, k) = (sh.clone(), k2.clone());
                        tasks.spawn(async move {
                            let _permit = permit;
                            if let Some((send, recv)) = open_stream(&sh, &cfg).await {
                                relay(tcp, send, recv, cfg, &k).await;
                            }
                        });
                    }
                    Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
                }
            }
        });
        Ok(Self { local_port, shared, task, supervisor, sem, counters, max: cfg.max_connections })
    }

    /// Chemin sélectionné (direct ou relais).
    pub fn path_kind(&self) -> PathKind {
        for p in self.shared.conn().paths().iter() {
            if p.is_selected() {
                return if p.is_relay() { PathKind::Relay } else { PathKind::Direct };
            }
        }
        PathKind::Unknown
    }

    pub fn stats(&self) -> TunnelStats {
        self.counters.snapshot(&self.sem, self.max)
    }

    /// État courant de la liaison.
    pub fn state(&self) -> TunnelState {
        *self.shared.state.borrow()
    }

    /// Suivi des changements d'état (événement `tunnel://state`).
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<TunnelState> {
        self.shared.state.subscribe()
    }

    /// Se résout quand le tunnel est PERDU : l'hôte est parti ou refuse, et la reconnexion a échoué.
    pub async fn closed(&self) {
        let mut st = self.shared.state.subscribe();
        let _ = st.wait_for(|s| *s == TunnelState::Lost).await;
    }

    /// Raison annoncée par l'hôte quand il a fermé la connexion (refus, révocation), si elle est connue.
    pub fn close_reason(&self) -> Option<Reach> {
        Reach::from_conn(&self.shared.conn())
    }

    /// Coupe de force la connexion à l'hôte (tests, bascule réseau simulée) : la reconnexion démarre.
    pub fn force_drop_connection(&self) {
        self.shared.conn().close(0u32.into(), b"test-drop");
    }

    pub async fn stop(mut self) {
        self.shared.conn().close(0u32.into(), b"stop");
        self.supervisor.abort();
        self.task.abort();
        let _ = (&mut self.task).await;
    }
}

/// Ouvre un flux vers l'hôte pour une connexion TCP de jeu. Si la liaison est perdue, attend la
/// reconnexion (au plus `reconnect_budget`) puis réessaie UNE fois ; sinon abandonne (le TCP est fermé).
async fn open_stream(
    sh: &GuestShared,
    cfg: &TunnelConfig,
) -> Option<(iroh::endpoint::SendStream, iroh::endpoint::RecvStream)> {
    for attempt in 0..2 {
        let mut st = sh.state.subscribe();
        let ready = timeout(cfg.reconnect_budget + SETTLE, st.wait_for(|s| *s != TunnelState::Reconnecting))
            .await
            .map(|r| r.map(|s| *s));
        if !matches!(ready, Ok(Ok(TunnelState::Ok))) {
            return None;
        }
        let c = sh.conn();
        match timeout(cfg.connect_timeout, c.open_bi()).await {
            Ok(Ok(streams)) => return Some(streams),
            _ if attempt == 0 => {
                // Liaison morte sans que QUIC l'ait encore signalé : on la ferme pour déclencher la reconnexion.
                c.close(0u32.into(), b"dead");
                let _ = timeout(Duration::from_millis(200), c.closed()).await;
                // Laisse le superviseur passer en « reconnexion » avant de relire l'état.
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            _ => return None,
        }
    }
    None
}

impl Drop for GuestTunnel {
    fn drop(&mut self) {
        self.shared.conn().close(0u32.into(), b"stop");
        self.supervisor.abort();
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::endpoint::presets;
    use iroh::RelayMode;

    async fn ep(alpn: bool) -> Endpoint {
        let b = Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled);
        let b = if alpn { b.alpns(vec![TUNNEL_ALPN.to_vec()]) } else { b };
        b.bind().await.unwrap()
    }

    /// Faux monde : lit "MCHS" + 4 octets (handshake), répond "OK!" puis fait écho.
    async fn world() -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                tokio::spawn(async move {
                    let mut hs = [0u8; 8];
                    if s.read_exact(&mut hs).await.is_err() || &hs[..4] != b"MCHS" {
                        return;
                    }
                    let _ = s.write_all(b"OK!").await;
                    let (mut r, mut w) = s.into_split();
                    let _ = tokio::io::copy(&mut r, &mut w).await;
                    let _ = w.shutdown().await;
                });
            }
        });
        port
    }

    fn reach_of(e: anyhow::Error) -> Reach {
        *e.downcast_ref::<Reach>().expect("erreur de type Reach")
    }

    fn key(e: &Endpoint) -> [u8; 32] {
        *e.id().as_bytes()
    }

    async fn setup(allowed: bool, cfg: TunnelConfig) -> (HostTunnel, GuestTunnel, Endpoint, Endpoint) {
        let port = world().await;
        let (h, g) = (ep(true).await, ep(false).await);
        let auth = Arc::new(AllowKeys(if allowed { vec![key(&g)] } else { vec![] }));
        let host = HostTunnel::start_with_config(h.clone(), port, auth, cfg).unwrap();
        let guest = GuestTunnel::connect_with_config(g.clone(), h.addr(), cfg).await.unwrap();
        (host, guest, h, g)
    }

    async fn mc_session(port: u16, payload: &[u8]) -> Vec<u8> {
        let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        s.write_all(b"MCHS\x00\x01\x02\x03").await.unwrap();
        let mut ok = [0u8; 3];
        s.read_exact(&mut ok).await.unwrap();
        assert_eq!(&ok, b"OK!");
        let (mut r, mut w) = s.into_split();
        let data = payload.to_vec();
        let wr = tokio::spawn(async move {
            w.write_all(&data).await.unwrap();
            w.shutdown().await.unwrap();
        });
        let mut out = Vec::new();
        r.read_to_end(&mut out).await.unwrap();
        wr.await.unwrap();
        out
    }

    async fn open_session(port: u16) -> TcpStream {
        let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        s.write_all(b"MCHS\x00\x00\x00\x00").await.unwrap();
        let mut ok = [0u8; 3];
        s.read_exact(&mut ok).await.unwrap();
        s
    }


    #[tokio::test]
    async fn guest_reconnects_after_the_connection_is_cut_and_keeps_its_port() {
        let (host, guest, _h, _g) = setup(true, TunnelConfig::default()).await;
        let port = guest.local_port;
        assert_eq!(mc_session(port, b"avant").await, b"avant");

        // Coupure de force de la connexion iroh invité -> hôte (changement de réseau simulé).
        guest.force_drop_connection();
        // Même port local, sans rien relancer : le prochain flux de jeu aboutit.
        let t = std::time::Instant::now();
        assert_eq!(timeout(Duration::from_secs(15), mc_session(port, b"apres")).await.expect("reconnexion"), b"apres");
        assert_eq!(guest.local_port, port);
        assert_eq!(guest.state(), TunnelState::Ok);
        // La reprise est rapide en local.
        assert!(t.elapsed() < Duration::from_secs(10), "{:?}", t.elapsed());
        // Deuxième coupure : ça recommence.
        guest.force_drop_connection();
        assert_eq!(timeout(Duration::from_secs(15), mc_session(port, b"encore")).await.expect("2e reconnexion"), b"encore");
        assert!(host.stats().total_connections >= 3);
    }

    #[tokio::test]
    async fn guest_gives_up_cleanly_when_the_host_is_really_gone() {
        let cfg = TunnelConfig { reconnect_budget: Duration::from_secs(2), connect_timeout: Duration::from_secs(1), ..TunnelConfig::default() };
        let (_host, guest, h, _g) = setup(true, cfg).await;
        let port = guest.local_port;
        assert_eq!(mc_session(port, b"ok").await, b"ok");

        // L'hôte disparaît pour de bon (endpoint fermé) puis la connexion tombe.
        h.close().await;
        guest.force_drop_connection();
        let t = std::time::Instant::now();
        timeout(Duration::from_secs(20), guest.closed()).await.expect("abandon borné, pas de boucle infinie");
        assert_eq!(guest.state(), TunnelState::Lost);
        assert!(t.elapsed() < Duration::from_secs(12), "{:?}", t.elapsed());
        // Plus rien n'est servi sur le port local.
        if let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)).await {
            let _ = s.write_all(b"MCHS\0\0\0\0").await;
            let mut b = [0u8; 3];
            let r = timeout(Duration::from_secs(2), s.read(&mut b)).await;
            assert!(!matches!(r, Ok(Ok(n)) if n > 0));
        }
    }

    #[tokio::test]
    async fn tunnel_echo_and_handshake() {
        let (host, guest, _h, _g) = setup(true, TunnelConfig::default()).await;
        let data: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        assert_eq!(mc_session(guest.local_port, &data).await, data);
        assert!(host.stats().total_connections >= 1);
        assert_ne!(guest.path_kind(), PathKind::Relay);
        guest.stop().await;
        host.stop().await;
    }

    #[tokio::test]
    async fn tunnel_multiple_simultaneous() {
        // 5 flux en même temps d'un seul pair : au-dessus de la limite par pair par défaut, d'où la config.
        let cfg = TunnelConfig { max_per_peer: 8, ..TunnelConfig::default() };
        let (host, guest, _h, _g) = setup(true, cfg).await;
        let mut js = Vec::new();
        for i in 0..5u8 {
            let p = guest.local_port;
            js.push(tokio::spawn(async move {
                let d = vec![i; 20_000 + i as usize];
                assert_eq!(mc_session(p, &d).await, d);
            }));
        }
        for j in js {
            j.await.unwrap();
        }
        guest.stop().await;
        host.stop().await;
    }

    #[tokio::test]
    async fn tunnel_unauthorized_peer_refused() {
        let port = world().await;
        let (h, g) = (ep(true).await, ep(false).await);
        let host = HostTunnel::start_with_config(h.clone(), port, Arc::new(AllowKeys(vec![])), TunnelConfig::default()).unwrap();
        let err = GuestTunnel::connect(g.clone(), h.addr()).await.err().expect("refusé");
        assert_eq!(reach_of(err), Reach::NotFriend);
        assert_eq!(host.stats().total_connections, 0);
        assert!(host.stats().rejected_connections >= 1);
    }

    /// Autorisation de test : un interrupteur, et `NotInvited` comme raison du refus (ami non invité ou invitation retirée).
    struct Switch(std::sync::atomic::AtomicBool);

    impl TunnelAuthorizer for Switch {
        fn is_allowed(&self, _public_key: &[u8; 32]) -> bool {
            self.0.load(Ordering::SeqCst)
        }
        fn denial(&self, _public_key: &[u8; 32]) -> Reach {
            Reach::NotInvited
        }
    }

    #[tokio::test]
    async fn tunnel_refus_et_revocation_annoncent_la_raison_de_l_autorisation() {
        let port = world().await;
        let (h, g) = (ep(true).await, ep(false).await);
        let auth = Arc::new(Switch(std::sync::atomic::AtomicBool::new(false)));
        let host = HostTunnel::start_with_config(h.clone(), port, auth.clone(), TunnelConfig::default()).unwrap();

        // Refus initial : « pas invité », pas « pas ami ».
        let err = GuestTunnel::connect(g.clone(), h.addr()).await.err().expect("refusé");
        assert_eq!(reach_of(err), Reach::NotInvited);

        // Invité, puis révoqué en cours de partie : la connexion tombe avec la même raison.
        auth.0.store(true, Ordering::SeqCst);
        let guest = GuestTunnel::connect(g.clone(), h.addr()).await.unwrap();
        assert_eq!(mc_session(guest.local_port, b"salut").await, b"salut");
        auth.0.store(false, Ordering::SeqCst);
        timeout(Duration::from_secs(5), guest.closed()).await.expect("tunnel coupé à la révocation");
        assert_eq!(guest.close_reason(), Some(Reach::NotInvited));
        guest.stop().await;
        host.stop().await;
    }

    #[tokio::test]
    async fn tunnel_host_stop_closes_guest_cleanly() {
        // L'hôte s'arrête : l'invité tente de se reconnecter (budget court ici) puis déclare le tunnel perdu.
        let cfg = TunnelConfig { reconnect_budget: Duration::from_secs(2), connect_timeout: Duration::from_secs(1), ..TunnelConfig::default() };
        let (host, guest, _h, _g) = setup(true, cfg).await;
        let mut s = open_session(guest.local_port).await;
        host.stop().await;
        let mut rest = Vec::new();
        let _ = timeout(Duration::from_secs(5), s.read_to_end(&mut rest))
            .await
            .expect("pas de blocage");
        assert!(rest.is_empty());
        timeout(Duration::from_secs(10), guest.closed())
            .await
            .expect("invité notifié");
    }

    #[tokio::test]
    async fn tunnel_connection_limit() {
        let cfg = TunnelConfig { max_connections: 2, ..TunnelConfig::default() };
        let (host, guest, _h, _g) = setup(true, cfg).await;
        let held = vec![
            open_session(guest.local_port).await,
            open_session(guest.local_port).await,
        ];
        let mut third = TcpStream::connect(("127.0.0.1", guest.local_port)).await.unwrap();
        let _ = third.write_all(b"MCHS\x00\x00\x00\x00").await;
        let mut b = [0u8; 3];
        let r = timeout(Duration::from_secs(5), third.read(&mut b))
            .await
            .expect("fermée vite");
        assert!(!matches!(r, Ok(n) if n > 0));
        assert_eq!(host.stats().active_connections, 2);
        drop(held);
        guest.stop().await;
        host.stop().await;
    }

    /// Un hôte et `n` invités autorisés ; chaque invité a `guest_cfg` (sa propre limite locale).
    async fn setup_many(n: usize, host_cfg: TunnelConfig, guest_cfg: TunnelConfig) -> (HostTunnel, Vec<GuestTunnel>, Endpoint, Vec<Endpoint>) {
        let port = world().await;
        let h = ep(true).await;
        let mut eps = Vec::new();
        for _ in 0..n {
            eps.push(ep(false).await);
        }
        let auth = Arc::new(AllowKeys(eps.iter().map(key).collect()));
        let host = HostTunnel::start_with_config(h.clone(), port, auth, host_cfg).unwrap();
        let mut guests = Vec::new();
        for e in &eps {
            guests.push(GuestTunnel::connect_with_config(e.clone(), h.addr(), guest_cfg).await.unwrap());
        }
        (host, guests, h, eps)
    }

    /// Ouvre une session et dit si l'hôte l'a servie (`false` : flux refusé, connexion TCP fermée).
    async fn try_session(port: u16) -> Option<TcpStream> {
        let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let _ = s.write_all(b"MCHS\x00\x00\x00\x00").await;
        let mut ok = [0u8; 3];
        match timeout(Duration::from_secs(5), s.read_exact(&mut ok)).await.expect("réponse ou fermeture rapide") {
            Ok(_) => Some(s),
            Err(_) => None,
        }
    }

    #[test]
    fn default_limits_fit_a_group() {
        let c = TunnelConfig::default();
        assert!(c.max_connections >= 32);
        assert!(c.max_per_peer >= 2 && c.max_per_peer <= c.max_connections);
    }

    #[tokio::test]
    async fn tunnel_per_peer_limit_spares_other_peers() {
        let host_cfg = TunnelConfig { max_per_peer: 2, ..TunnelConfig::default() };
        let (host, guests, _h, _eps) = setup_many(2, host_cfg, TunnelConfig::default()).await;
        let (a, b) = (&guests[0], &guests[1]);
        let mut held = vec![try_session(a.local_port).await.unwrap(), try_session(a.local_port).await.unwrap()];
        // La 3e connexion du pair A est refusée, avec le code explicite « plein » vu côté invité.
        assert!(try_session(a.local_port).await.is_none());
        assert_eq!(host.stats().rejected_connections, 1);
        assert_eq!(a.stats().refused_full, 1);
        assert_eq!(host.stats().active_connections, 2);
        // Les flux déjà ouverts de A et le pair B ne sont pas touchés.
        held[0].write_all(b"x").await.unwrap();
        let mut one = [0u8; 1];
        timeout(Duration::from_secs(5), held[0].read_exact(&mut one)).await.unwrap().unwrap();
        assert_eq!(&one, b"x");
        assert!(try_session(b.local_port).await.is_some());
        assert_eq!(b.stats().refused_full, 0);
        // Libérer une place la rend aussitôt : le compteur par pair redescend, puis la table se vide.
        drop(held.pop());
        let mut again = None;
        for _ in 0..50 {
            again = try_session(a.local_port).await;
            if again.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(again.is_some(), "la place libérée doit être réutilisable");
        drop((held, again));
        for _ in 0..50 {
            if crate::util::lock(&host.core.per_peer).is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(crate::util::lock(&host.core.per_peer).is_empty(), "pas de fuite du compteur par pair");
        for g in guests {
            g.stop().await;
        }
        host.stop().await;
    }

    #[tokio::test]
    async fn tunnel_global_limit_refuses_with_full_code() {
        let host_cfg = TunnelConfig { max_connections: 2, max_per_peer: 2, ..TunnelConfig::default() };
        let (host, guests, _h, _eps) = setup_many(2, host_cfg, TunnelConfig::default()).await;
        let (a, b) = (&guests[0], &guests[1]);
        let _held = [try_session(a.local_port).await.unwrap(), try_session(b.local_port).await.unwrap()];
        // Hôte plein : la 2e connexion de A (sous sa limite par pair) est refusée par la limite globale.
        assert!(try_session(a.local_port).await.is_none());
        assert_eq!(a.stats().refused_full, 1);
        assert_eq!(host.stats().rejected_connections, 1);
        assert_eq!(host.stats().active_connections, 2);
        for g in guests {
            g.stop().await;
        }
        host.stop().await;
    }

    #[tokio::test]
    async fn tunnel_serves_ten_group_members_with_default_config() {
        // Scénario du ticket : un groupe de 10 membres, une partie chacun. Avec l'ancienne limite (8 au total),
        // le 9e était refusé sans explication.
        let (host, guests, _h, _eps) = setup_many(10, TunnelConfig::default(), TunnelConfig::default()).await;
        let mut held = Vec::new();
        for g in &guests {
            held.push(try_session(g.local_port).await.expect("chaque membre est servi"));
        }
        let st = host.stats();
        assert_eq!(st.active_connections, 10);
        assert_eq!(st.rejected_connections, 0);
        drop(held);
        for g in guests {
            g.stop().await;
        }
        host.stop().await;
    }

    #[tokio::test]
    async fn tunnel_large_transfer_10mb() {
        let (host, guest, _h, _g) = setup(true, TunnelConfig::default()).await;
        let data: Vec<u8> = (0..10 * 1024 * 1024u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8)
            .collect();
        let out = timeout(Duration::from_secs(60), mc_session(guest.local_port, &data))
            .await
            .unwrap();
        assert!(out == data, "10 Mo corrompus");
        guest.stop().await;
        host.stop().await;
    }

    #[tokio::test]
    async fn tunnel_via_shared_router_handler() {
        use iroh::protocol::Router;
        let port = world().await;
        let (h, g) = (ep(true).await, ep(false).await);
        let host = HostTunnel::attach(Arc::new(AllowKeys(vec![key(&g)])), TunnelConfig::default());
        let router = Router::builder(h.clone()).accept(TUNNEL_ALPN, host.handler()).spawn();

        // Rien d'exposé tant qu'aucun port n'est défini.
        let err = GuestTunnel::connect(g.clone(), h.addr()).await.err().expect("refusé : rien n'est partagé");
        assert_eq!(reach_of(err), Reach::NothingShared);
        assert!(host.stats().rejected_connections >= 1);

        host.set_world_port(port);
        let guest = GuestTunnel::connect(g.clone(), h.addr()).await.unwrap();
        assert_eq!(mc_session(guest.local_port, b"via router").await, b"via router");
        assert!(host.stats().total_connections >= 1);

        // stop() ferme aussi les connexions servies par le Router.
        let mut s = open_session(guest.local_port).await;
        host.stop().await;
        let mut rest = Vec::new();
        let _ = timeout(Duration::from_secs(5), s.read_to_end(&mut rest)).await.expect("pas de blocage");
        timeout(Duration::from_secs(5), guest.closed()).await.expect("invité notifié");
        let _ = router.shutdown().await;
    }
}
