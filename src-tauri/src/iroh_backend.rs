//! Backend P2P réel (tâches INT / INT2) : UN `Endpoint` iroh, UN `Router`.
//!
//! Décisions :
//! - Endpoint créé par `relay::build_endpoint` avec la clé de `Identity::secret_bytes()` et les réglages
//!   `allow_relay` / `relay_only` (lus au démarrage ; un changement demande un redémarrage).
//! - Le Router enregistre, tous fournis par p2p : ami, présence, contrôle de transfert + blobs
//!   (`IrohTransport::handlers_dynamic`, hôte courant lu à chaque requête), manifeste (`ManifestServer`)
//!   et tunnel (`HostTunnel::attach/handler`).
//! - `unknown_mods` : le manifeste porte le sha1 des jars (revendication de l'hôte, vérifiée à la réception par
//!   p2p). `JoinPlan.unknown_mods` est calculé AVANT téléchargement (`find_unknown_from_manifest`) ; les jars
//!   non vérifiables vont dans `unknown_unchecked`. `P2pBackend::find_unknown_mods` (sha1 locaux des jars
//!   reçus) reste la re-vérification locale après réception.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use core_lib::instance::InstanceKind;
use core_lib::manifest::{build_manifest, ManifestOptions};
use core_lib::providers::modrinth_api::ModrinthClient;
use core_lib::unknown_mods::UnknownReport;
use core_lib::types::{Error, Friend, ManifestFile, Presence, Result as CoreResult, Settings, TransferProgress, VerifyCode};
use iroh::endpoint::Connection;
use iroh::protocol::Router;
use iroh::{Endpoint, EndpointAddr, EndpointId, SecretKey};
use p2p_lib::friend_code::{self, FriendCodePayload};
use p2p_lib::friend_proto::{self, FriendConfig, FriendEvent, FriendService};
use p2p_lib::manifest_channel::{fetch_manifest as p2p_fetch_manifest, ManifestServer};
use p2p_lib::presence::{PresenceConfig, PresenceService, PRESENCE_ALPN};
use p2p_lib::relay::{self, PathKind, PathObserver, RelayChoice, RelaySettings};
use p2p_lib::transfer::plan::Plan;
use p2p_lib::transfer::recv::RecvOptions;
use p2p_lib::transfer::resume::{ResumableReceive, ResumeConfig};
use p2p_lib::transfer::send::{BlobTransport, FetchRequest, Host, IrohTransport, ServeIndex, ServeLimits, CONTROL_ALPN};
use p2p_lib::tunnel::{GuestTunnel, HostTunnel, TunnelAuthorizer, TunnelConfig, TUNNEL_ALPN};

use crate::authz::FriendsAuthz;
use crate::error::{sanitize, user_msg};
use crate::p2p_backend::{BoxFuture, Cmd, HostManifest, P2pBackend, ProgressFn, TransferControl};
use crate::state::{lock, AppState, EventSink};

pub use p2p_lib::manifest_channel::MANIFEST_ALPN;
const NET_TIMEOUT: Duration = Duration::from_secs(15);
/// Sans aucun octet reçu pendant ce délai, le transfert passe en `failed` (message français).
pub const DEFAULT_INACTIVITY: Duration = Duration::from_secs(60);
/// Un téléchargement muet depuis ce délai est interrompu : l'adresse de l'hôte est re-résolue puis on réessaie.
pub const DEFAULT_STALL_RETRY: Duration = Duration::from_secs(15);
/// Délai laissé au transfert pour s'arrêter proprement après l'annulation d'inactivité.
const WATCHDOG_GRACE: Duration = Duration::from_secs(5);

/// Calcule les mods hors Modrinth d'un dossier de jeu (injectable pour les tests).
pub type UnknownFn = Arc<dyn Fn(Vec<ManifestFile>, PathBuf) -> BoxFuture<'static, Vec<String>> + Send + Sync>;

fn default_unknown() -> UnknownFn {
    Arc::new(|files, dir| {
        Box::pin(async move {
            let ua = concat!("Cubelink/", env!("CARGO_PKG_VERSION"));
            let client = match crate::test_hooks::hooks().modrinth_base() {
                Some(u) => ModrinthClient::with_base_url(u, ua),
                None => ModrinthClient::new(ua),
            };
            core_lib::unknown_mods::find_unknown(&files, &dir, &client).await.map(|r| r.unknown).unwrap_or_default()
        })
    })
}

/// Analyse avant téléchargement sur les sha1 du manifeste (injectable pour les tests).
pub type ManifestUnknownFn = Arc<dyn Fn(Vec<ManifestFile>) -> BoxFuture<'static, UnknownReport> + Send + Sync>;

fn modrinth_client() -> ModrinthClient {
    let ua = concat!("Cubelink/", env!("CARGO_PKG_VERSION"));
    match crate::test_hooks::hooks().modrinth_base() {
        Some(u) => ModrinthClient::with_base_url(u, ua),
        None => ModrinthClient::new(ua),
    }
}

fn default_manifest_unknown() -> ManifestUnknownFn {
    Arc::new(|files| {
        Box::pin(async move { core_lib::unknown_mods::find_unknown_from_manifest(&files, &modrinth_client()).await })
    })
}

#[derive(Clone)]
pub struct IrohOptions {
    pub relay: RelayChoice,
    /// Adresses directes dans le code ami. `None` : seulement si le relais est désactivé.
    pub advertise_addrs: Option<bool>,
    pub unknown_finder: UnknownFn,
    pub manifest_unknown_finder: ManifestUnknownFn,
    pub resume: ResumeConfig,
    pub friend: FriendConfig,
    pub presence: PresenceConfig,
    /// Délai sans octet reçu avant l'échec `failed` d'un transfert d'invité.
    pub inactivity: Duration,
    /// Délai sans octet reçu avant d'interrompre le fichier en cours pour re-résoudre l'hôte et reprendre.
    pub stall_retry: Duration,
}

impl Default for IrohOptions {
    fn default() -> Self {
        Self {
            relay: RelayChoice::Default,
            advertise_addrs: None,
            unknown_finder: default_unknown(),
            manifest_unknown_finder: default_manifest_unknown(),
            resume: ResumeConfig::default(),
            friend: FriendConfig::default(),
            presence: PresenceConfig::default(),
            inactivity: DEFAULT_INACTIVITY,
            stall_retry: DEFAULT_STALL_RETRY,
        }
    }
}

// ---------- état de partage (hôte) ----------

/// Hôte courant (instance partagée) lu par les handlers de p2p à chaque requête ; `None` = rien de partagé.
#[derive(Default)]
struct ShareState {
    cur: RwLock<Option<Arc<Host>>>,
}

impl ShareState {
    fn current(&self) -> Option<Arc<Host>> {
        self.cur.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
    fn set(&self, s: Option<Arc<Host>>) {
        *self.cur.write().unwrap_or_else(|e| e.into_inner()) = s;
    }
}

// ---------- invité : transport enveloppé ----------

struct Fetcher {
    inner: Arc<IrohTransport>,
    /// Test : fait échouer le n-ième `fetch` (1 = premier), une fois, par une coupure réseau.
    cut_at: Arc<Mutex<Option<usize>>>,
    calls: AtomicUsize,
    observer: Mutex<Option<PathObserver>>,
    /// Dernière activité (octets reçus) : alimente le délai d'inactivité.
    activity: Arc<Activity>,
    /// Silence maximal d'un téléchargement avant de le relancer.
    stall_retry: Duration,
    /// Re-résout l'adresse de l'hôte (appelé avant chaque reprise après erreur ou silence).
    refresh: Arc<dyn Fn() + Send + Sync>,
    /// Vrai après une tentative en échec : la prochaine re-résout l'hôte.
    dirty: std::sync::atomic::AtomicBool,
}

/// Instant de la dernière activité, en millisecondes depuis la création.
pub struct Activity {
    start: std::time::Instant,
    last_ms: std::sync::atomic::AtomicU64,
}

impl Activity {
    pub fn new() -> Self {
        Self { start: std::time::Instant::now(), last_ms: std::sync::atomic::AtomicU64::new(0) }
    }
    pub fn touch(&self) {
        self.last_ms.store(self.start.elapsed().as_millis() as u64, Ordering::Relaxed);
    }
    pub fn idle(&self) -> Duration {
        Duration::from_millis((self.start.elapsed().as_millis() as u64).saturating_sub(self.last_ms.load(Ordering::Relaxed)))
    }
}

impl Default for Activity {
    fn default() -> Self {
        Self::new()
    }
}

/// Message d'échec d'un transfert figé.
pub fn inactivity_message(d: Duration) -> String {
    format!(
        "le transfert est figé : aucune donnée reçue depuis {} s. L'ami est peut-être déconnecté ou a redémarré son partage : vérifiez qu'il est en ligne, puis réessayez",
        d.as_secs()
    )
}

/// Exécute `fut` en surveillant l'inactivité. Après `inactivity` sans activité (hors pause), appelle `cancel`
/// puis laisse `grace` pour s'arrêter ; renvoie alors `Error::Other(inactivity_message)`.
pub async fn run_with_watchdog<T>(
    fut: impl std::future::Future<Output = CoreResult<T>>,
    activity: &Activity,
    paused: impl Fn() -> bool,
    cancel: impl Fn(),
    inactivity: Duration,
    poll: Duration,
    grace: Duration,
) -> CoreResult<T> {
    tokio::pin!(fut);
    let mut tick = tokio::time::interval(poll);
    let mut fired: Option<std::time::Instant> = None;
    loop {
        tokio::select! {
            r = &mut fut => {
                return match (r, fired) {
                    (Err(_), Some(_)) => Err(Error::Other(inactivity_message(inactivity))),
                    (r, _) => r,
                };
            }
            _ = tick.tick() => {
                if let Some(t) = fired {
                    if t.elapsed() >= grace {
                        return Err(Error::Other(inactivity_message(inactivity)));
                    }
                } else if paused() {
                    activity.touch();
                } else if activity.idle() >= inactivity {
                    fired = Some(std::time::Instant::now());
                    cancel();
                }
            }
        }
    }
}

/// Adresse à réessayer pour un hôte : l'adresse connue sans ses adresses IP directes si un relais est
/// connu (elles changent quand l'hôte redémarre ; le relais le retrouve par sa clé), sinon inchangée.
pub fn refreshed_addr(stored: &EndpointAddr) -> EndpointAddr {
    let mut a = EndpointAddr::new(stored.id);
    let mut has_relay = false;
    for u in stored.relay_urls() {
        a = a.with_relay_url(u.clone());
        has_relay = true;
    }
    if has_relay {
        a
    } else {
        stored.clone()
    }
}

impl BlobTransport for Fetcher {
    async fn serve(&self, _host: Arc<Host>) -> CoreResult<()> {
        Ok(())
    }

    async fn fetch(
        &self,
        peer: &str,
        req: &FetchRequest,
        on_bytes: &(dyn Fn(u64) + Send + Sync),
        cancel: &AtomicBool,
    ) -> CoreResult<()> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        {
            let mut c = lock(&self.cut_at);
            if *c == Some(n) {
                *c = None;
                return Err(Error::Network("coupure simulée".into()));
            }
        }
        // Après un échec ou un silence, l'hôte a pu redémarrer : on re-résout son adresse avant de réessayer.
        if self.dirty.swap(false, Ordering::SeqCst) {
            (self.refresh)();
        }
        let (act, local) = (self.activity.clone(), Arc::new(Activity::new()));
        let l2 = local.clone();
        let counted = move |n: u64| {
            act.touch();
            l2.touch();
            on_bytes(n);
        };
        let fut = self.inner.fetch(peer, req, &counted, cancel);
        tokio::pin!(fut);
        let res = loop {
            tokio::select! {
                r = &mut fut => break r,
                _ = tokio::time::sleep(Duration::from_millis(250).min(self.stall_retry)) => {
                    if local.idle() >= self.stall_retry {
                        // Muet : on abandonne ce fichier ; la reprise (backoff p2p) re-résout l'hôte.
                        break Err(Error::Network("l'ami ne répond plus".into()));
                    }
                }
            }
        };
        if matches!(res, Err(ref e) if !matches!(e, Error::Cancelled)) {
            self.dirty.store(true, Ordering::SeqCst);
        }
        res
    }

    fn via_relay(&self, peer: &str) -> bool {
        let seen = lock(&self.observer).as_ref().map(|o| o.current() == PathKind::Relay);
        seen.unwrap_or_else(|| self.inner.via_relay(peer))
    }
}

// ---------- le backend ----------

pub struct IrohBackend {
    endpoint: Endpoint,
    router: Router,
    friends_svc: FriendService,
    presence: Arc<PresenceService>,
    transport: Arc<IrohTransport>,
    share: Arc<ShareState>,
    manifest_srv: ManifestServer,
    tunnel: HostTunnel,
    peers: Arc<Mutex<HashMap<String, EndpointAddr>>>,
    guest_tunnel: tokio::sync::Mutex<Option<GuestTunnel>>,
    instances: Arc<core_lib::instance::InstanceStore>,
    store: Arc<core_lib::store::index::StoreIndex>,
    store_root: PathBuf,
    settings: Arc<Mutex<Settings>>,
    authz: Arc<FriendsAuthz>,
    opts: IrohOptions,
    advertise: bool,
    cut_at: Arc<Mutex<Option<usize>>>,
}

fn net_err(e: impl std::fmt::Display) -> String {
    sanitize(&e.to_string())
}

/// Crée l'état complet avec ce backend réel (production et tests d'intégration).
pub async fn start_app(
    root: &std::path::Path,
    emit: EventSink,
    opts: IrohOptions,
) -> std::result::Result<(Arc<AppState>, Arc<IrohBackend>), String> {
    let st = Arc::new(AppState::new(root, emit, Arc::new(crate::p2p_backend::UnavailableBackend))?);
    let be = IrohBackend::start(&st, opts).await?;
    st.set_backend(be.clone());
    Ok((st, be))
}

impl IrohBackend {
    pub async fn start(st: &Arc<AppState>, opts: IrohOptions) -> std::result::Result<Arc<Self>, String> {
        let (allow_relay, relay_only) = {
            let s = lock(&st.settings);
            (s.allow_relay, s.relay_only)
        };
        let rs = RelaySettings { mode: opts.relay.clone(), relay_only, allow_relay, address_lookup: false };
        let alpns: Vec<Vec<u8>> = [friend_proto::ALPN, PRESENCE_ALPN, CONTROL_ALPN, iroh_blobs::ALPN, MANIFEST_ALPN, TUNNEL_ALPN]
            .iter()
            .map(|a| a.to_vec())
            .collect();
        let secret = SecretKey::from_bytes(&st.identity.secret_bytes());
        let endpoint = relay::build_endpoint(secret, &rs, alpns).await.map_err(|e| user_msg(&e))?;
        let advertise = opts.advertise_addrs.unwrap_or(!allow_relay);

        let settings = st.settings.clone();
        let share_mods: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(move || lock(&settings).share_mods);
        let authz = Arc::new(FriendsAuthz::new(st.friends.clone(), share_mods));
        let share = Arc::new(ShareState::default());

        // Amis.
        let (acc_tx, mut acc_rx) = tokio::sync::mpsc::unbounded_channel::<[u8; 32]>();
        let emit = st.emit.clone();
        let sink: friend_proto::EventSink = Arc::new(move |ev| match ev {
            FriendEvent::IncomingRequest(r) => {
                if let Ok(v) = serde_json::to_value(&r) {
                    emit("friend://request", v);
                }
            }
            FriendEvent::Accepted(f) => {
                if let Some(k) = hex::decode(&f.public_key).ok().and_then(|b| <[u8; 32]>::try_from(b).ok()) {
                    let _ = acc_tx.send(k);
                }
            }
            FriendEvent::Refused(_) => {}
        });
        let friends_svc = FriendService::attach(endpoint.clone(), st.identity.clone(), st.friends.clone(), sink, opts.friend.clone())
            .map_err(|e| user_msg(&e))?;

        // Présence.
        let emit = st.emit.clone();
        let psink: p2p_lib::presence::EventSink = Arc::new(move |e| {
            emit("friend://presence", serde_json::json!({ "public_key": hex::encode(e.public_key), "presence": e.presence }));
        });
        let presence = Arc::new(PresenceService::start_with_config(endpoint.clone(), authz.clone(), psink, opts.presence));

        // Un transport (magasin de blobs partagé hôte/invité) ; ses handlers lisent l'hôte courant à chaque requête.
        let transport = Arc::new(
            IrohTransport::new(endpoint.clone(), st.paths.p2p.join("guest-blobs")).await.map_err(|e| user_msg(&e))?,
        );
        let sh = share.clone();
        let (control, blobs) = transport.handlers_dynamic(Arc::new(move || sh.current()));
        let manifest_srv = ManifestServer::new(authz.clone());
        let tunnel_authz: Arc<dyn TunnelAuthorizer> = authz.clone();
        let tunnel = HostTunnel::attach(tunnel_authz, TunnelConfig::default());
        let router = Router::builder(endpoint.clone())
            .accept(friend_proto::ALPN, friends_svc.handler())
            .accept(PRESENCE_ALPN, presence.handler())
            .accept(CONTROL_ALPN, control)
            .accept(iroh_blobs::ALPN, blobs)
            .accept(MANIFEST_ALPN, manifest_srv.handler())
            .accept(TUNNEL_ALPN, tunnel.handler())
            .spawn();

        let be = Arc::new(Self {
            endpoint,
            router,
            friends_svc,
            presence: presence.clone(),
            transport,
            share,
            manifest_srv,
            tunnel,
            peers: Arc::new(Mutex::new(HashMap::new())),
            guest_tunnel: tokio::sync::Mutex::new(None),
            instances: st.instances.clone(),
            store: st.store.clone(),
            store_root: st.paths.store.clone(),
            settings: st.settings.clone(),
            authz,
            opts,
            advertise,
            cut_at: Arc::default(),
        });
        // Une acceptation (nous étions demandeur) : sonde l'ami tout de suite.
        let weak = Arc::downgrade(&be);
        tokio::spawn(async move {
            while let Some(k) = acc_rx.recv().await {
                let Some(be) = weak.upgrade() else { break };
                let p = be.presence.clone();
                tokio::spawn(async move {
                    p.probe_now(k).await;
                });
            }
        });
        Ok(be)
    }

    pub fn endpoint_addr(&self) -> EndpointAddr {
        self.endpoint.addr()
    }

    /// Mémorise comment joindre un pair (code ami, découverte manuelle, tests).
    pub fn add_peer_addr(&self, addr: EndpointAddr) {
        let key = hex::encode(addr.id.as_bytes());
        self.presence.add_addr(addr.clone());
        self.transport.add_peer(addr.clone());
        lock(&self.peers).insert(key, addr);
    }

    fn addr_for(&self, key: &str) -> std::result::Result<EndpointAddr, String> {
        if let Some(a) = lock(&self.peers).get(key) {
            return Ok(a.clone());
        }
        let bytes: [u8; 32] = hex::decode(key).ok().and_then(|b| b.try_into().ok()).ok_or("clé publique invalide")?;
        let id = EndpointId::from_bytes(&bytes).map_err(|_| "clé publique invalide".to_string())?;
        Ok(EndpointAddr::new(id))
    }

    /// Test : fait échouer le n-ième téléchargement de fichier (1 = premier) par une coupure.
    pub fn inject_cut_at_fetch(&self, n: usize) {
        *lock(&self.cut_at) = Some(n);
    }

    /// Test : vrai si la coupure injectée a eu lieu.
    pub fn cut_consumed(&self) -> bool {
        lock(&self.cut_at).is_none()
    }

    pub async fn shutdown(&self) {
        self.close_guest_tunnel().await;
        self.presence.stop().await;
        let _ = self.router.shutdown().await;
    }

    // ----- hôte -----

    /// Construit le manifeste de l'instance et le sert aux amis acceptés (si `share_mods`).
    pub async fn share_instance(&self, instance_id: &str) -> std::result::Result<(), String> {
        if !lock(&self.settings).share_mods {
            return Err("le partage de mods avec vos amis est désactivé dans les paramètres".into());
        }
        let inst = self.instances.get(instance_id).map_err(|e| user_msg(&e))?;
        if self.instances.kind(instance_id).map_err(|e| user_msg(&e))? != InstanceKind::Own {
            return Err("seule une de vos instances peut être partagée".into());
        }
        let game_dir = self.instances.game_dir(instance_id).map_err(|e| user_msg(&e))?;
        let (gd, i2) = (game_dir.clone(), inst.clone());
        let opts = ManifestOptions { include_config: true, ..ManifestOptions::default() };
        let manifest = tokio::task::spawn_blocking(move || build_manifest(&i2, &gd, &opts))
            .await
            .map_err(|_| "calcul du manifeste interrompu".to_string())?
            .map_err(|e| user_msg(&e))?;
        let index = ServeIndex::from_manifest(&manifest, &game_dir).map_err(|e| user_msg(&e))?;
        let host = Host::new(index, self.authz.clone(), ServeLimits::default());
        self.manifest_srv.set(Some(&manifest)).map_err(|e| user_msg(&e))?;
        self.share.set(Some(host));
        Ok(())
    }

    /// Partage l'instance (manifeste + fichiers) et expose le monde LAN `127.0.0.1:world_port` aux amis.
    /// `world_port == 0` : fichiers seulement.
    pub async fn host_start_sharing_impl(&self, instance_id: &str, world_port: u16) -> std::result::Result<(), String> {
        self.share_instance(instance_id).await?;
        self.tunnel.set_world_port(world_port);
        Ok(())
    }

    pub fn host_stop_sharing_impl(&self) {
        self.share.set(None);
        let _ = self.manifest_srv.set(None);
        self.tunnel.set_world_port(0);
    }

    /// Invité : tunnel local vers le monde de l'ami (port `local_port` à donner à Minecraft).
    pub async fn join_tunnel(&self, friend_public_key: &str) -> std::result::Result<GuestTunnel, String> {
        let addr = self.addr_for(friend_public_key)?;
        GuestTunnel::connect(self.endpoint.clone(), addr).await.map_err(net_err)
    }

    /// Ferme le tunnel invité ouvert, s'il existe.
    pub async fn close_guest_tunnel(&self) {
        let t = self.guest_tunnel.lock().await.take();
        if let Some(t) = t {
            t.stop().await;
        }
    }

    /// Connexion gardée vers l'ami (ALPN de contrôle, sans flux) : sert à observer le chemin (relais ou direct).
    async fn hold(&self, key: &str) -> Option<Connection> {
        let addr = self.addr_for(key).ok()?;
        tokio::time::timeout(NET_TIMEOUT, self.endpoint.connect(addr, CONTROL_ALPN)).await.ok()?.ok()
    }
}

impl P2pBackend for IrohBackend {
    fn presence(&self, public_key: &str) -> Presence {
        let Some(k) = hex::decode(public_key).ok().and_then(|b| <[u8; 32]>::try_from(b).ok()) else {
            return Presence::Offline;
        };
        self.presence.snapshot().get(&k).copied().unwrap_or(Presence::Offline)
    }

    fn set_display_name(&self, name: &str) {
        let _ = self.friends_svc.set_name(name);
    }

    fn code_hints(&self) -> (Option<String>, Vec<String>) {
        let info = relay::endpoint_addr_info(&self.endpoint.addr());
        // Adresses directes seulement si le relais est désactivé (elles révèlent nos IP à qui reçoit le code).
        let addrs = if self.advertise { info.addrs } else { vec![] };
        (info.relay, addrs)
    }

    fn send_request(&self, target: FriendCodePayload, my_name: String) -> BoxFuture<'_, std::result::Result<VerifyCode, String>> {
        Box::pin(async move {
            let _ = self.friends_svc.set_name(&my_name);
            let id = EndpointId::from_bytes(&target.public_key).map_err(|_| "code ami invalide".to_string())?;
            let mut addr = EndpointAddr::new(id);
            if let Some(r) = relay::relay_hint(&target) {
                addr = addr.with_relay_url(r);
            }
            for a in target.addrs.iter().filter_map(|a| a.parse::<SocketAddr>().ok()) {
                addr = addr.with_ip_addr(a);
            }
            self.add_peer_addr(addr);
            self.friends_svc.send_request(&friend_code::encode(&target)).await.map_err(|e| user_msg(&e))
        })
    }

    fn accept_request(&self, public_key: String) -> BoxFuture<'_, std::result::Result<Option<Friend>, String>> {
        Box::pin(async move {
            let f = self.friends_svc.accept(&public_key).await.map_err(|e| user_msg(&e))?;
            if let Some(k) = hex::decode(&f.public_key).ok().and_then(|b| <[u8; 32]>::try_from(b).ok()) {
                let p = self.presence.clone();
                tokio::spawn(async move {
                    p.probe_now(k).await;
                });
            }
            Ok(Some(f))
        })
    }

    fn refuse_request(&self, public_key: String) -> BoxFuture<'_, std::result::Result<bool, String>> {
        Box::pin(async move {
            self.friends_svc.refuse(&public_key).await.map_err(|e| user_msg(&e))?;
            Ok(true)
        })
    }

    fn notify_accepted(&self, _f: Friend) -> BoxFuture<'_, std::result::Result<(), String>> {
        Box::pin(async { Ok(()) })
    }

    fn fetch_manifest(&self, key: String) -> BoxFuture<'_, std::result::Result<HostManifest, String>> {
        Box::pin(async move {
            let addr = self.addr_for(&key)?;
            let manifest = p2p_fetch_manifest(&self.endpoint, addr).await.map_err(|e| match e {
                Error::Auth(_) => "l'ami ne partage pas son monde (ou n'est pas votre ami)".to_string(),
                e => user_msg(&e),
            })?;
            // p2p ne transporte pas d'indication de l'hôte : voir `find_unknown_mods` (côté invité, après réception).
            Ok(HostManifest { manifest, unknown_mods: vec![] })
        })
    }

    fn find_unknown_from_manifest(&self, files: Vec<ManifestFile>) -> BoxFuture<'_, UnknownReport> {
        (self.opts.manifest_unknown_finder)(files)
    }

    fn find_unknown_mods(&self, files: Vec<ManifestFile>, game_dir: PathBuf) -> BoxFuture<'_, Vec<String>> {
        (self.opts.unknown_finder)(files, game_dir)
    }

    fn receive_files(
        &self,
        key: String,
        transfer_id: String,
        plan: Plan,
        progress: ProgressFn,
        control: Arc<TransferControl>,
    ) -> BoxFuture<'_, std::result::Result<(), Error>> {
        Box::pin(async move {
            let addr = self.addr_for(&key).map_err(Error::Invalid)?;
            let peer = self.transport.add_peer(addr);
            // Connexion gardée pendant le transfert : `PathObserver` dit si on passe par un relais.
            let keep = self.hold(&key).await;
            let observer = keep.as_ref().map(|c| PathObserver::spawn(c.clone()));
            let activity = Arc::new(Activity::new());
            activity.touch();
            let (peers, transport, pkey) = (self.peers.clone(), self.transport.clone(), key.clone());
            let refresh: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
                let stored = lock(&peers).get(&pkey).cloned();
                if let Some(a) = stored {
                    transport.add_peer(refreshed_addr(&a));
                }
            });
            let fetcher = Arc::new(Fetcher {
                inner: self.transport.clone(),
                cut_at: self.cut_at.clone(),
                calls: AtomicUsize::new(0),
                observer: Mutex::new(observer),
                activity: activity.clone(),
                stall_retry: self.opts.stall_retry,
                refresh,
                dirty: std::sync::atomic::AtomicBool::new(false),
            });
            let mut cfg = self.opts.resume.clone();
            cfg.opts = RecvOptions { ..cfg.opts };
            let rr = Arc::new(ResumableReceive::recover(
                &transfer_id,
                plan,
                &peer,
                fetcher,
                &self.store_root,
                self.store.clone(),
                cfg,
            )?);
            let r2 = rr.clone();
            control.set_hook(Box::new(move |c| match c {
                Cmd::Pause => r2.pause(),
                Cmd::Resume => r2.resume(),
                Cmd::Cancel => r2.cancel(),
            }));
            let (a2, r3, c2) = (activity.clone(), rr.clone(), control.clone());
            let touch = {
                let a = activity.clone();
                move |p: TransferProgress| {
                    a.touch();
                    progress(p)
                }
            };
            let inactivity = self.opts.inactivity;
            let poll = (inactivity / 4).clamp(Duration::from_millis(50), Duration::from_secs(1));
            let res = run_with_watchdog(
                async { rr.run(&touch).await.map(|_| ()) },
                &a2,
                || c2.is_paused(),
                || r3.cancel(),
                inactivity,
                poll,
                WATCHDOG_GRACE,
            )
            .await;
            if let Some(c) = keep {
                c.close(0u32.into(), b"done");
            }
            res
        })
    }

    fn host_start_sharing(&self, instance_id: String, world_port: u16) -> BoxFuture<'_, std::result::Result<(), String>> {
        Box::pin(async move { self.host_start_sharing_impl(&instance_id, world_port).await })
    }

    fn host_stop_sharing(&self) {
        self.host_stop_sharing_impl();
    }

    fn join_open_tunnel(&self, key: String) -> BoxFuture<'_, std::result::Result<u16, String>> {
        Box::pin(async move {
            self.close_guest_tunnel().await;
            let t = self.join_tunnel(&key).await.map_err(|e| format!("impossible de joindre le monde de l'ami : {e}"))?;
            let port = t.local_port;
            *self.guest_tunnel.lock().await = Some(t);
            Ok(port)
        })
    }

    fn join_close_tunnel(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move { self.close_guest_tunnel().await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: fn(u64) -> Duration = Duration::from_millis;

    #[tokio::test]
    async fn transfert_fige_passe_en_echec_francais() {
        let act = Activity::new();
        act.touch();
        let cancelled = AtomicBool::new(false);
        // Futur qui ne finit jamais (hôte muet) et ignore même l'annulation : la grâce prend le relais.
        let r: CoreResult<()> =
            run_with_watchdog(std::future::pending(), &act, || false, || cancelled.store(true, Ordering::SeqCst), MS(120), MS(20), MS(100)).await;
        assert!(cancelled.load(Ordering::SeqCst), "annulation demandée");
        let msg = match r {
            Err(Error::Other(m)) => m,
            other => panic!("{other:?}"),
        };
        assert!(msg.contains("figé") && msg.contains("réessayez"), "{msg}");
    }

    #[tokio::test]
    async fn annulation_d_inactivite_remplace_l_erreur_annule() {
        let act = Activity::new();
        let flag = Arc::new(AtomicBool::new(false));
        let f2 = flag.clone();
        let fut = async move {
            while !f2.load(Ordering::SeqCst) {
                tokio::time::sleep(MS(5)).await;
            }
            Err::<(), _>(Error::Cancelled)
        };
        let r = run_with_watchdog(fut, &act, || false, || flag.store(true, Ordering::SeqCst), MS(60), MS(10), MS(500)).await;
        assert!(matches!(r, Err(Error::Other(ref m)) if m.contains("figé")), "{r:?}");
    }

    #[tokio::test]
    async fn pas_d_echec_si_actif_en_pause_ou_termine() {
        // Terminé avant le délai : résultat inchangé.
        let act = Activity::new();
        let r = run_with_watchdog(async { Ok::<_, Error>(7) }, &act, || false, || {}, MS(50), MS(10), MS(50)).await;
        assert_eq!(r.unwrap(), 7);
        // En pause plus longtemps que le délai : aucun échec.
        let act = Activity::new();
        let fut = async {
            tokio::time::sleep(MS(200)).await;
            Ok::<_, Error>(1)
        };
        let r = run_with_watchdog(fut, &act, || true, || panic!("pas d'annulation en pause"), MS(60), MS(10), MS(50)).await;
        assert_eq!(r.unwrap(), 1);
        // Activité régulière : pas d'échec.
        let act = Arc::new(Activity::new());
        let a2 = act.clone();
        let fut = async move {
            for _ in 0..10 {
                tokio::time::sleep(MS(20)).await;
                a2.touch();
            }
            Ok::<_, Error>(2)
        };
        let r = run_with_watchdog(fut, &act, || false, || panic!("actif"), MS(100), MS(10), MS(50)).await;
        assert_eq!(r.unwrap(), 2);
    }

    #[test]
    fn adresse_hote_re_resolue_sans_ip_perimees() {
        let id = SecretKey::from_bytes(&[7u8; 32]).public();
        let relay: iroh::RelayUrl = "https://relay.example./".parse().unwrap();
        let ip: SocketAddr = "127.0.0.1:4000".parse().unwrap();
        let stored = EndpointAddr::new(id).with_relay_url(relay.clone()).with_ip_addr(ip);
        let r = refreshed_addr(&stored);
        assert_eq!(r.id, id);
        assert_eq!(r.ip_addrs().count(), 0, "IP directes périmées retirées");
        assert_eq!(r.relay_urls().count(), 1);
        // Sans relais : l'adresse est conservée (seul indice disponible).
        let direct = EndpointAddr::new(id).with_ip_addr(ip);
        assert_eq!(refreshed_addr(&direct), direct);
    }

    #[test]
    fn delais_par_defaut_coherents() {
        assert!(DEFAULT_STALL_RETRY < DEFAULT_INACTIVITY);
        assert!(inactivity_message(Duration::from_secs(60)).contains("60 s"));
        let o = IrohOptions::default();
        assert_eq!((o.inactivity, o.stall_retry), (DEFAULT_INACTIVITY, DEFAULT_STALL_RETRY));
    }
}
