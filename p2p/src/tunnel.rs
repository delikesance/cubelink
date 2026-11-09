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
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::{Endpoint, EndpointAddr};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{watch, Semaphore};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::timeout;

/// ALPN dédié au tunnel de jeu.
pub const TUNNEL_ALPN: &[u8] = b"cubelink/tunnel/1";

/// Décide si un pair (clé publique ed25519) peut ouvrir un tunnel vers le monde de l'hôte.
pub trait TunnelAuthorizer: Send + Sync {
    fn is_allowed(&self, public_key: &[u8; 32]) -> bool;
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
    /// Connexions de jeu simultanées maximales.
    pub max_connections: usize,
    /// Délai pour établir une connexion (QUIC ou TCP local).
    pub connect_timeout: Duration,
    /// Inactivité maximale d'une connexion relayée (dans un sens).
    pub idle_timeout: Duration,
}

impl Default for TunnelConfig {
    fn default() -> Self {
        Self {
            max_connections: 8,
            connect_timeout: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(120),
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
    /// Octets TCP -> QUIC (côté local vers le pair).
    pub bytes_to_peer: u64,
    /// Octets QUIC -> TCP.
    pub bytes_from_peer: u64,
}

#[derive(Default)]
struct Counters {
    total: AtomicU64,
    rejected: AtomicU64,
    up: AtomicU64,
    down: AtomicU64,
}

impl Counters {
    fn snapshot(&self, sem: &Semaphore, max: usize) -> TunnelStats {
        TunnelStats {
            active_connections: max.saturating_sub(sem.available_permits()),
            total_connections: self.total.load(Ordering::Relaxed),
            rejected_connections: self.rejected.load(Ordering::Relaxed),
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
        let _ = send.reset(0u32.into());
        let _ = recv.stop(0u32.into());
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

/// Sert une connexion déjà établie sur `TUNNEL_ALPN` : autorisation, puis un flux = un TCP local.
async fn serve_conn(core: Arc<HostCore>, conn: Connection) {
    let _guard = ConnGuard(conn.clone());
    let port = core.port.load(Ordering::SeqCst);
    if port == 0 || *core.stop.borrow() || !core.authorizer.is_allowed(conn.remote_id().as_bytes()) {
        conn.close(1u32.into(), b"denied");
        core.counters.rejected.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let (cfg, sem, counters) = (core.cfg, core.sem.clone(), core.counters.clone());
    let mut stop = core.stop.subscribe();
    let world = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut streams = JoinSet::new();
    loop {
        tokio::select! {
            _ = stop.wait_for(|s| *s) => break,
            s = conn.accept_bi() => {
                let Ok((mut send, mut recv)) = s else { break };
                let Ok(permit) = sem.clone().try_acquire_owned() else {
                    counters.rejected.fetch_add(1, Ordering::Relaxed);
                    let _ = send.reset(2u32.into());
                    let _ = recv.stop(2u32.into());
                    continue;
                };
                counters.total.fetch_add(1, Ordering::Relaxed);
                let counters = counters.clone();
                streams.spawn(async move {
                    let _permit = permit;
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

/// Côté invité : port local `127.0.0.1:local_port` à donner à Minecraft.
pub struct GuestTunnel {
    pub local_port: u16,
    conn: Connection,
    task: JoinHandle<()>,
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
        let conn = timeout(cfg.connect_timeout, endpoint.connect(host, TUNNEL_ALPN))
            .await
            .map_err(|_| anyhow!("délai de connexion à l'hôte dépassé"))??;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let local_port = listener.local_addr()?.port();
        let sem = Arc::new(Semaphore::new(cfg.max_connections));
        let counters = Arc::new(Counters::default());
        let (c, s2, k2) = (conn.clone(), sem.clone(), counters.clone());
        let task = tokio::spawn(async move {
            let _guard = ConnGuard(c.clone());
            let mut tasks = JoinSet::new();
            loop {
                tokio::select! {
                    _ = c.closed() => break,
                    acc = listener.accept() => {
                        let Ok((tcp, _)) = acc else { break };
                        let Ok(permit) = s2.clone().try_acquire_owned() else {
                            k2.rejected.fetch_add(1, Ordering::Relaxed);
                            continue; // tcp fermé au drop
                        };
                        k2.total.fetch_add(1, Ordering::Relaxed);
                        let (c, k) = (c.clone(), k2.clone());
                        tasks.spawn(async move {
                            let _permit = permit;
                            if let Ok(Ok((send, recv))) = timeout(cfg.connect_timeout, c.open_bi()).await {
                                relay(tcp, send, recv, cfg, &k).await;
                            }
                        });
                    }
                    Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
                }
            }
        });
        Ok(Self { local_port, conn, task, sem, counters, max: cfg.max_connections })
    }

    /// Chemin sélectionné (direct ou relais).
    pub fn path_kind(&self) -> PathKind {
        for p in self.conn.paths().iter() {
            if p.is_selected() {
                return if p.is_relay() { PathKind::Relay } else { PathKind::Direct };
            }
        }
        PathKind::Unknown
    }

    pub fn stats(&self) -> TunnelStats {
        self.counters.snapshot(&self.sem, self.max)
    }

    /// Se résout quand la connexion avec l'hôte est fermée (arrêt de l'hôte, refus, réseau).
    pub async fn closed(&self) {
        self.conn.closed().await;
    }

    pub async fn stop(mut self) {
        self.conn.close(0u32.into(), b"stop");
        self.task.abort();
        let _ = (&mut self.task).await;
    }
}

impl Drop for GuestTunnel {
    fn drop(&mut self) {
        self.conn.close(0u32.into(), b"stop");
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
        let (host, guest, _h, _g) = setup(true, TunnelConfig::default()).await;
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
        let (host, guest, _h, _g) = setup(false, TunnelConfig::default()).await;
        timeout(Duration::from_secs(5), guest.closed())
            .await
            .expect("connexion fermée par l'hôte");
        if let Ok(mut s) = TcpStream::connect(("127.0.0.1", guest.local_port)).await {
            let _ = s.write_all(b"MCHS\x00\x00\x00\x00").await;
            let mut b = [0u8; 3];
            let r = timeout(Duration::from_secs(2), s.read(&mut b)).await;
            assert!(!matches!(r, Ok(Ok(n)) if n > 0));
        }
        assert_eq!(host.stats().total_connections, 0);
        assert!(host.stats().rejected_connections >= 1);
    }

    #[tokio::test]
    async fn tunnel_host_stop_closes_guest_cleanly() {
        let (host, guest, _h, _g) = setup(true, TunnelConfig::default()).await;
        let mut s = open_session(guest.local_port).await;
        host.stop().await;
        let mut rest = Vec::new();
        let _ = timeout(Duration::from_secs(5), s.read_to_end(&mut rest))
            .await
            .expect("pas de blocage");
        assert!(rest.is_empty());
        timeout(Duration::from_secs(5), guest.closed())
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
        let closed = GuestTunnel::connect(g.clone(), h.addr()).await.unwrap();
        timeout(Duration::from_secs(5), closed.closed()).await.expect("refusé : rien n'est partagé");
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
