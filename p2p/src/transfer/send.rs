//! Tâche N11 : côté hôte du transfert + trait `BlobTransport` (couche réseau remplaçable).
//!
//! Sécurité : seuls les fichiers du manifeste validé sont servables (`ServeIndex`), jamais un chemin
//! fourni par le pair. Les pairs doivent être autorisés (`PeerAuthorizer`). Des limites bornent la
//! taille par fichier, le volume total et les requêtes simultanées par pair.
//!
//! La condition du spike N1 est respectée : `iroh-blobs` n'est utilisé que dans `IrohTransport`,
//! derrière `BlobTransport`. `MemTransport` (tests) n'en dépend pas.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex};

use core_lib::manifest::validate_manifest;
use core_lib::types::{Error, Manifest, Result};

/// Identifiant d'un pair : sa clé publique, en texte (z-base32/hex selon le transport).
pub type PeerId = String;

/// Décide si un pair a le droit de demander des fichiers (seuls les amis acceptés).
/// `FriendsDb` en fournira l'implémentation.
pub trait PeerAuthorizer: Send + Sync {
    fn is_allowed(&self, public_key: &str) -> bool;

    /// Raison du refus quand `is_allowed` est faux (annoncée à l'invité) : par défaut « pas ami ».
    fn denial(&self, _public_key: &str) -> crate::reach::Reach {
        crate::reach::Reach::NotFriend
    }
}

/// Autorise un ensemble fixe de clés (tests, et usage simple).
#[derive(Debug, Clone, Default)]
pub struct AllowList(pub Vec<String>);

impl PeerAuthorizer for AllowList {
    fn is_allowed(&self, public_key: &str) -> bool {
        self.0.iter().any(|k| k == public_key)
    }
}

/// Limites côté hôte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServeLimits {
    /// Taille maximale d'un fichier servi.
    pub max_file_bytes: u64,
    /// Volume total maximal servi à un même pair (sur la durée de vie de l'hôte).
    pub max_total_bytes_per_peer: u64,
    /// Requêtes simultanées maximales par pair.
    pub max_concurrent_per_peer: usize,
}

impl Default for ServeLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: 2 * 1024 * 1024 * 1024,
            max_total_bytes_per_peer: 20 * 1024 * 1024 * 1024,
            // L'invité en demande 4 à la fois ; marge pour une requête arrivée juste avant la fin de la précédente
            // (connexion réutilisée, pas de poignée de main entre deux fichiers).
            max_concurrent_per_peer: 8,
        }
    }
}

/// Un fichier servable : chemin local (choisi par l'hôte) et taille du manifeste.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServedFile {
    pub path: PathBuf,
    pub size: u64,
}

/// Table sha256 -> fichier du `game_dir`, construite UNIQUEMENT depuis un manifeste validé.
#[derive(Debug, Clone, Default)]
pub struct ServeIndex {
    files: HashMap<String, ServedFile>,
}

impl ServeIndex {
    /// Valide le manifeste puis associe chaque sha256 à son fichier du `game_dir`.
    pub fn from_manifest(manifest: &Manifest, game_dir: &Path) -> Result<Self> {
        Self::from_manifest_with(manifest, |rel| game_dir.join(rel))
    }

    /// Comme [`Self::from_manifest`], avec le chemin local de chaque fichier choisi par `resolve`
    /// (ex. instantané des configs, jars lus dans le jeu).
    pub fn from_manifest_with(manifest: &Manifest, resolve: impl Fn(&str) -> PathBuf) -> Result<Self> {
        validate_manifest(manifest)?;
        let mut files = HashMap::new();
        for f in &manifest.files {
            files.entry(f.sha256.clone()).or_insert_with(|| ServedFile { path: resolve(&f.path), size: f.size });
        }
        Ok(Self { files })
    }

    pub fn get(&self, sha256: &str) -> Option<&ServedFile> {
        self.files.get(sha256)
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

#[derive(Debug, Default)]
struct PeerState {
    active: usize,
    sent: u64,
}

/// Côté hôte : décide de chaque requête (autorisation, manifeste, limites).
pub struct Host {
    index: ServeIndex,
    authorizer: Arc<dyn PeerAuthorizer>,
    limits: ServeLimits,
    peers: Mutex<HashMap<PeerId, PeerState>>,
    on_drift: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl Host {
    pub fn new(index: ServeIndex, authorizer: Arc<dyn PeerAuthorizer>, limits: ServeLimits) -> Arc<Self> {
        Arc::new(Self { index, authorizer, limits, peers: Mutex::new(HashMap::new()), on_drift: Mutex::new(None) })
    }

    /// Appelé quand un fichier du jeu ne correspond plus à son empreinte annoncée (le manifeste est périmé) :
    /// l'application recalcule alors le manifeste pour que le « Réessayer » de l'invité reparte d'un état frais.
    pub fn set_on_drift(&self, f: Arc<dyn Fn() + Send + Sync>) {
        *self.on_drift.lock().unwrap_or_else(|p| p.into_inner()) = Some(f);
    }

    fn drift(&self) {
        let f = self.on_drift.lock().unwrap_or_else(|p| p.into_inner()).clone();
        if let Some(f) = f {
            f();
        }
    }

    /// Contrôle d'accès seul (utilisé aussi à l'ouverture d'une connexion).
    pub fn check_peer(&self, peer: &str) -> Result<()> {
        if self.authorizer.is_allowed(peer) {
            Ok(())
        } else {
            Err(Error::Auth("pair non autorisé".into()))
        }
    }

    /// Autorise (ou refuse) la requête d'un pair pour le contenu `sha256`.
    /// Le `Permit` renvoyé réserve un créneau de concurrence jusqu'à sa libération.
    pub fn authorize(self: &Arc<Self>, peer: &str, sha256: &str) -> Result<Permit> {
        self.check_peer(peer)?;
        let file = self
            .index
            .get(sha256)
            .ok_or_else(|| Error::NotFound("contenu absent du manifeste".into()))?
            .clone();
        if file.size > self.limits.max_file_bytes {
            return Err(Error::Invalid(format!(
                "fichier trop volumineux : {} octets (maximum : {})",
                file.size, self.limits.max_file_bytes
            )));
        }
        let mut peers = self.peers.lock().unwrap_or_else(|p| p.into_inner());
        let st = peers.entry(peer.to_string()).or_default();
        if st.active >= self.limits.max_concurrent_per_peer {
            return Err(Error::Network("trop de requêtes simultanées pour ce pair".into()));
        }
        if st.sent.saturating_add(file.size) > self.limits.max_total_bytes_per_peer {
            return Err(Error::Invalid("volume total maximal atteint pour ce pair".into()));
        }
        st.active += 1;
        st.sent += file.size;
        Ok(Permit { host: Arc::clone(self), peer: peer.to_string(), file })
    }

    pub fn limits(&self) -> &ServeLimits {
        &self.limits
    }
}

/// Créneau de service accordé. Libéré à la destruction.
pub struct Permit {
    host: Arc<Host>,
    peer: PeerId,
    file: ServedFile,
}

impl Permit {
    pub fn file(&self) -> &ServedFile {
        &self.file
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut peers = self.host.peers.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(st) = peers.get_mut(&self.peer) {
            st.active = st.active.saturating_sub(1);
        }
    }
}

/// Une demande de réception d'un contenu.
#[derive(Debug, Clone)]
pub struct FetchRequest {
    pub sha256: String,
    /// Taille attendue (manifeste) : le transport doit refuser de dépasser.
    pub size: u64,
    /// Fichier temporaire de destination (créé/écrasé par le transport).
    pub dest: PathBuf,
    /// Reprise (N13) : octets déjà présents dans `dest`. Le transport ne reprend que si `dest` a
    /// exactement cette taille (sinon il repart de 0) ; un offset > taille du fichier est `Invalid`.
    /// L'offset n'influe jamais sur l'autorisation côté hôte.
    pub resume_from: u64,
}

/// Couche réseau remplaçable. Le transport n'est PAS de confiance : l'appelant (`recv.rs`) vérifie
/// toujours sha256 et taille.
pub trait BlobTransport: Send + Sync {
    /// Côté hôte : commence à répondre aux requêtes en consultant `host` pour chaque demande.
    fn serve(&self, host: Arc<Host>) -> impl Future<Output = Result<()>> + Send;

    /// Côté invité : récupère un contenu auprès de `peer` vers `req.dest`.
    /// `on_bytes` reçoit les octets écrits au fur et à mesure ; `cancel` interrompt le transfert.
    fn fetch(
        &self,
        peer: &str,
        req: &FetchRequest,
        on_bytes: &(dyn Fn(u64) + Send + Sync),
        cancel: &AtomicBool,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Vrai si le chemin vers `peer` passe par un relais (inconnu : faux).
    fn via_relay(&self, _peer: &str) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------------------------
// MemTransport : réseau en mémoire pour les tests déterministes
// ---------------------------------------------------------------------------------------------

/// « Réseau » en mémoire : annuaire des hôtes par identifiant.
#[derive(Default)]
pub struct MemNetwork {
    hosts: Mutex<HashMap<PeerId, Arc<Host>>>,
}

impl MemNetwork {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

/// Un nœud du réseau en mémoire, identifié par `id` (sa « clé publique »).
pub struct MemTransport {
    net: Arc<MemNetwork>,
    id: PeerId,
    /// Coupure simulée : nombre d'octets servis dans un `fetch` avant `Error::Network`.
    cut_after: Mutex<Option<u64>>,
    /// Vrai : la coupure ne se produit qu'une fois (puis le transport redevient normal).
    cut_once: bool,
    /// Octets réellement transférés (hors préfixe déjà présent), tous `fetch` confondus.
    transferred: AtomicU64,
}

impl MemTransport {
    pub fn new(net: Arc<MemNetwork>, id: impl Into<PeerId>) -> Self {
        Self { net, id: id.into(), cut_after: Mutex::new(None), cut_once: true, transferred: AtomicU64::new(0) }
    }

    /// Mode test : le prochain `fetch` échoue en `Error::Network` après `n` octets (une seule fois).
    pub fn with_cut_after(mut self, n: u64) -> Self {
        *self.cut_after.get_mut().unwrap_or_else(|p| p.into_inner()) = Some(n);
        self.cut_once = true;
        self
    }

    /// Comme `with_cut_after`, mais chaque `fetch` est coupé après `n` octets.
    pub fn with_cut_after_always(mut self, n: u64) -> Self {
        *self.cut_after.get_mut().unwrap_or_else(|p| p.into_inner()) = Some(n);
        self.cut_once = false;
        self
    }

    /// Octets réellement transférés depuis la création.
    pub fn bytes_transferred(&self) -> u64 {
        self.transferred.load(std::sync::atomic::Ordering::Relaxed)
    }
}

const MEM_CHUNK: usize = 64 * 1024;

impl BlobTransport for MemTransport {
    async fn serve(&self, host: Arc<Host>) -> Result<()> {
        self.net.hosts.lock().unwrap_or_else(|p| p.into_inner()).insert(self.id.clone(), host);
        Ok(())
    }

    async fn fetch(
        &self,
        peer: &str,
        req: &FetchRequest,
        on_bytes: &(dyn Fn(u64) + Send + Sync),
        cancel: &AtomicBool,
    ) -> Result<()> {
        use std::io::SeekFrom;
        use std::sync::atomic::Ordering;
        use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

        let host = self
            .net
            .hosts
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(peer)
            .cloned()
            .ok_or_else(|| Error::Network(format!("pair injoignable : {peer}")))?;
        // Autorisation indépendante de l'offset : mêmes contrôles que pour une requête à 0.
        let permit = host.authorize(&self.id, &req.sha256)?;
        let file_size = permit.file().size;
        if req.resume_from > file_size || req.resume_from > req.size {
            return Err(Error::Invalid(format!(
                "reprise hors bornes : offset {} > taille {}",
                req.resume_from,
                file_size.min(req.size)
            )));
        }
        // Reprise seulement si `dest` contient exactement `resume_from` octets, sinon on repart de 0.
        let mut offset = req.resume_from;
        if offset > 0 {
            let cur = tokio::fs::metadata(&req.dest).await.map(|m| m.len()).ok();
            if cur != Some(offset) {
                offset = 0;
            }
        }
        let mut src = tokio::fs::File::open(&permit.file().path).await?;
        let mut out = if offset > 0 {
            src.seek(SeekFrom::Start(offset)).await?;
            tokio::fs::OpenOptions::new().append(true).open(&req.dest).await?
        } else {
            tokio::fs::File::create(&req.dest).await?
        };
        let limit = {
            let mut g = self.cut_after.lock().unwrap_or_else(|p| p.into_inner());
            if self.cut_once { g.take() } else { *g }
        };
        let mut buf = vec![0u8; MEM_CHUNK];
        let mut total = offset;
        let mut sent = 0u64;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            let max = match limit {
                Some(l) => {
                    if sent >= l {
                        out.flush().await?;
                        return Err(Error::Network("coupure simulée".into()));
                    }
                    ((l - sent) as usize).min(buf.len())
                }
                None => buf.len(),
            };
            let n = src.read(&mut buf[..max]).await?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > req.size {
                return Err(Error::Invalid("le pair envoie plus que la taille du manifeste".into()));
            }
            out.write_all(&buf[..n]).await?;
            sent += n as u64;
            self.transferred.fetch_add(n as u64, Ordering::Relaxed);
            on_bytes(n as u64);
        }
        out.flush().await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// IrohTransport : iroh (QUIC) + iroh-blobs (BLAKE3 côté réseau)
// ---------------------------------------------------------------------------------------------

/// ALPN du canal de contrôle : l'invité envoie un sha256 (64 octets ASCII), l'hôte répond
/// `[0, blake3 (32 octets)]` si accepté ou `[1]` si refusé.
pub const CONTROL_ALPN: &[u8] = b"cubelink/xfer/0";

mod iroh_impl {
    use super::*;
    use std::sync::atomic::Ordering;

    use iroh::endpoint::Connection;
    use iroh::protocol::{AcceptError, ProtocolHandler, Router};
    use iroh::{Endpoint, EndpointAddr};
    use iroh_blobs::api::blobs::{AddPathOptions, ImportMode};
    use iroh_blobs::store::fs::FsStore;
    use iroh_blobs::{BlobFormat, BlobsProtocol, Hash, HashAndFormat};
    use n0_future::StreamExt;

    /// Statut de contrôle : le fichier du jeu a changé depuis le manifeste (à réessayer avec un manifeste frais).
    const STATUS_CHANGED: u8 = 2;

    fn net(e: impl std::fmt::Debug) -> Error {
        Error::Network(format!("{e:?}"))
    }

    /// Transport iroh. Un `Endpoint` et un `FsStore` (dossier propre à ce nœud) par instance.
    /// L'endpoint doit avoir été créé avec les ALPN `CONTROL_ALPN` et `iroh_blobs::ALPN`
    /// uniquement s'il sert ; un nœud purement invité peut n'en déclarer aucun.
    pub struct IrohTransport {
        endpoint: Endpoint,
        store: FsStore,
        peers: Mutex<HashMap<PeerId, EndpointAddr>>,
        relayed: Mutex<HashMap<PeerId, bool>>,
        router: Mutex<Option<Router>>,
        /// Connexions QUIC réutilisées d'un fichier à l'autre, par pair et par ALPN (contrôle, blobs) : une
        /// poignée de main par transfert au lieu de deux par fichier, et UN contrôle de congestion partagé par
        /// tous les fichiers en vol (débit équitable pour l'hôte, pas de rafale de connexions).
        conns: Mutex<HashMap<(PeerId, &'static [u8]), Connection>>,
        /// Sérialise l'ouverture des connexions : les fetchs simultanés du début attendent la même connexion.
        connecting: tokio::sync::Mutex<()>,
        /// Connexions ouvertes depuis la création (diagnostic, tests).
        opened: AtomicU64,
    }

    impl IrohTransport {
        pub async fn new(endpoint: Endpoint, store_dir: impl AsRef<Path>) -> Result<Self> {
            let store = FsStore::load(store_dir.as_ref()).await.map_err(net)?;
            Ok(Self {
                endpoint,
                store,
                peers: Mutex::new(HashMap::new()),
                relayed: Mutex::new(HashMap::new()),
                router: Mutex::new(None),
                conns: Mutex::new(HashMap::new()),
                connecting: tokio::sync::Mutex::new(()),
                opened: AtomicU64::new(0),
            })
        }

        fn cached(&self, peer: &str, alpn: &'static [u8]) -> Option<Connection> {
            let conns = self.conns.lock().unwrap_or_else(|p| p.into_inner());
            conns.get(&(peer.to_string(), alpn)).filter(|c| c.close_reason().is_none()).cloned()
        }

        /// Connexion vivante vers `peer` pour `alpn`, réutilisée si possible.
        async fn conn(&self, peer: &str, addr: &EndpointAddr, alpn: &'static [u8]) -> Result<Connection> {
            if let Some(c) = self.cached(peer, alpn) {
                return Ok(c);
            }
            let _g = self.connecting.lock().await;
            if let Some(c) = self.cached(peer, alpn) {
                return Ok(c);
            }
            let c = self.endpoint.connect(addr.clone(), alpn).await.map_err(net)?;
            self.opened.fetch_add(1, Ordering::Relaxed);
            self.conns.lock().unwrap_or_else(|p| p.into_inner()).insert((peer.to_string(), alpn), c.clone());
            Ok(c)
        }

        /// Oublie une connexion en échec (la suivante sera rouverte), si c'est bien celle en cache.
        fn forget(&self, peer: &str, alpn: &'static [u8], c: &Connection) {
            let mut conns = self.conns.lock().unwrap_or_else(|p| p.into_inner());
            let key = (peer.to_string(), alpn);
            if conns.get(&key).is_some_and(|x| x.stable_id() == c.stable_id()) {
                conns.remove(&key);
            }
        }

        /// Un fichier sur les connexions partagées : contrôle (sha256 -> BLAKE3) puis blobs.
        async fn fetch_on(
            &self,
            peer: &str,
            addr: &EndpointAddr,
            ctl: &Connection,
            req: &FetchRequest,
            on_bytes: &(dyn Fn(u64) + Send + Sync),
            cancel: &AtomicBool,
        ) -> Result<()> {
            let (mut send, mut recv) = ctl.open_bi().await.map_err(net)?;
            send.write_all(req.sha256.as_bytes()).await.map_err(net)?;
            let mut status = [0u8; 1];
            recv.read_exact(&mut status).await.map_err(net)?;
            if status[0] == STATUS_CHANGED {
                // Pas `Network` : inutile de réessayer automatiquement avec le même manifeste périmé ; l'invité
                // relance « Réessayer » et obtient un manifeste frais.
                return Err(Error::Other(
                    "Un fichier a changé chez ton ami pendant le transfert : réessaie dans un instant".into(),
                ));
            }
            if status[0] != 0 {
                return Err(Error::Auth("requête refusée par l'hôte".into()));
            }
            let mut h = [0u8; 32];
            recv.read_exact(&mut h).await.map_err(net)?;
            let hash = Hash::from_bytes(h);

            // 2) transfert iroh-blobs (BLAKE3 vérifié par le protocole) sur la connexion blobs partagée, puis
            // export vers `dest`. Le flux de contrôle `send` reste ouvert jusqu'à la fin : l'hôte tient le créneau
            // de concurrence de ce fichier jusque-là (lâché sur erreur, le flux se termine tout seul).
            let conn = self.conn(peer, addr, iroh_blobs::ALPN).await?;
            let relayed = conn.paths().iter().any(|p| p.is_selected() && p.is_relay());
            self.relayed.lock().unwrap_or_else(|p| p.into_inner()).insert(peer.to_string(), relayed);
            let r = self.get_blob(&conn, hash, req, on_bytes, cancel).await;
            if matches!(r, Err(Error::Network(_))) || conn.close_reason().is_some() {
                self.forget(peer, iroh_blobs::ALPN, &conn);
            }
            r?;
            let _ = send.finish();
            Ok(())
        }

        async fn get_blob(
            &self,
            conn: &Connection,
            hash: Hash,
            req: &FetchRequest,
            on_bytes: &(dyn Fn(u64) + Send + Sync),
            cancel: &AtomicBool,
        ) -> Result<()> {
            let mut stream = self.store.remote().fetch(conn.clone(), HashAndFormat::raw(hash)).stream();
            // Reprise : iroh-blobs rapatrie le blob entier dans le store (pas de plage exposée ici) ;
            // `dest` est réécrit en entier par l'export. On ne compte en progression que le reste
            // (taille - resume_from) ; le gain réseau n'est pas réalisé côté iroh.
            let skip = req.resume_from;
            let mut last = 0u64;
            let mut reported = 0u64;
            let mut done = false;
            while let Some(item) = stream.next().await {
                if cancel.load(Ordering::Relaxed) {
                    // Connexion partagée : on abandonne seulement cette requête (flux lâché), sans fermer.
                    return Err(Error::Cancelled);
                }
                match item {
                    iroh_blobs::api::remote::GetProgressItem::Progress(n) => {
                        if n > last {
                            last = n;
                            let eff = last.saturating_sub(skip);
                            if eff > reported {
                                on_bytes(eff - reported);
                                reported = eff;
                            }
                        }
                    }
                    iroh_blobs::api::remote::GetProgressItem::Done(_) => {
                        done = true;
                        break;
                    }
                    iroh_blobs::api::remote::GetProgressItem::Error(e) => return Err(net(e)),
                }
            }
            if !done {
                return Err(Error::Network("transfert interrompu".into()));
            }
            let want = req.size.saturating_sub(skip);
            if reported < want {
                on_bytes(want - reported);
            }
            self.store.blobs().export(hash, &req.dest).await.map_err(net)?;
            Ok(())
        }

        /// Oublie (sans les fermer) les connexions en cache vers `peer` : les prochains fichiers en rouvrent de
        /// neuves, ceux en vol finissent sur les anciennes. Après un silence, la connexion est peut-être morte.
        pub fn forget_peer(&self, peer: &str) {
            self.conns.lock().unwrap_or_else(|p| p.into_inner()).retain(|(p, _), _| p != peer);
        }

        /// Nombre de connexions sortantes ouvertes pour des transferts depuis la création.
        pub fn connections_opened(&self) -> u64 {
            self.opened.load(Ordering::Relaxed)
        }

        /// Fin d'un transfert : ferme les connexions réutilisées vers `peer`.
        pub fn close_peer(&self, peer: &str) {
            let mut conns = self.conns.lock().unwrap_or_else(|p| p.into_inner());
            conns.retain(|(p, _), c| {
                if p == peer {
                    c.close(0u32.into(), b"done");
                }
                p != peer
            });
        }

        /// Adresse à communiquer aux pairs.
        pub fn addr(&self) -> EndpointAddr {
            self.endpoint.addr()
        }

        /// Identifiant (clé publique) de ce nœud, comme vu par les pairs.
        pub fn id(&self) -> PeerId {
            self.endpoint.id().to_string()
        }

        /// Enregistre comment joindre un pair ; renvoie son `PeerId`.
        pub fn add_peer(&self, addr: EndpointAddr) -> PeerId {
            let id = addr.id.to_string();
            let old = self.peers.lock().unwrap_or_else(|p| p.into_inner()).insert(id.clone(), addr.clone());
            // Adresse changée (hôte redémarré, re-résolution après un silence) : les connexions en cache sont
            // peut-être mortes sans le savoir ; les suivantes seront rouvertes vers la nouvelle adresse.
            if old.is_some_and(|o| o != addr) {
                self.conns.lock().unwrap_or_else(|p| p.into_inner()).retain(|(p, _), _| *p != id);
            }
            id
        }

        /// Handlers hôte à enregistrer dans un `Router` externe (un seul `Router` pour tous les
        /// protocoles) : `CONTROL_ALPN` -> contrôle, `iroh_blobs::ALPN` -> blobs gardés.
        /// L'endpoint doit déclarer ces deux ALPN. `serve` s'appuie sur cette méthode.
        pub fn handlers(&self, host: Arc<Host>) -> (ControlHandler, GatedBlobsHandler) {
            self.handlers_dynamic(Arc::new(move || Some(host.clone())))
        }

        /// Comme [`Self::handlers`], mais l'hôte est demandé à chaque connexion/requête : `None`
        /// refuse tout (rien de partagé). Permet de changer d'instance partagée à chaud.
        pub fn handlers_dynamic(&self, host: HostProvider) -> (ControlHandler, GatedBlobsHandler) {
            let control = ControlHandler { host: host.clone(), store: self.store.clone(), known: Arc::default() };
            let blobs = GatedBlobsHandler { host, inner: BlobsProtocol::new(&self.store, None) };
            (control, blobs)
        }

        /// Arrête le routeur (hôte).
        pub async fn shutdown(&self) {
            let r = self.router.lock().unwrap_or_else(|p| p.into_inner()).take();
            if let Some(r) = r {
                let _ = r.shutdown().await;
            }
        }
    }

    /// Fournit l'hôte courant (`None` : rien n'est partagé, tout est refusé). Permet à une application
    /// de changer d'instance partagée sans recréer le `Router`.
    pub type HostProvider = Arc<dyn Fn() -> Option<Arc<Host>> + Send + Sync>;

    /// Canal de contrôle (ALPN [`CONTROL_ALPN`]) : traduit sha256 -> BLAKE3 pour les seuls fichiers
    /// du manifeste. À enregistrer dans un `Router` (voir [`IrohTransport::handlers`]).
    #[derive(Clone)]
    pub struct ControlHandler {
        host: HostProvider,
        store: FsStore,
        /// sha256 -> (BLAKE3, empreinte du fichier au moment de l'import), rempli paresseusement.
        /// L'entrée n'est valable que si le fichier vivant n'a pas changé (taille + date de modification) :
        /// sinon un fichier altéré puis réparé resterait servi sous son ancien BLAKE3.
        known: Arc<Mutex<HashMap<String, (Hash, FileStamp)>>>,
    }

    impl std::fmt::Debug for ControlHandler {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("ControlHandler")
        }
    }

    /// Taille et date de modification d'un fichier : détecte qu'il a changé depuis l'import.
    type FileStamp = (u64, Option<std::time::SystemTime>);

    fn stamp(path: &Path) -> Option<FileStamp> {
        let m = std::fs::metadata(path).ok()?;
        Some((m.len(), m.modified().ok()))
    }

    /// Échec d'une requête de contrôle : refus (autorisation…) ou fichier modifié depuis le manifeste.
    #[derive(Debug)]
    enum AnswerErr {
        Refused(#[allow(dead_code)] Error),
        Changed,
    }

    impl From<Error> for AnswerErr {
        fn from(e: Error) -> Self {
            match e {
                // Contenu absent du manifeste de l'hôte : l'hôte l'a recalculé depuis (ou le manifeste de l'invité est périmé).
                Error::NotFound(_) => AnswerErr::Changed,
                e => AnswerErr::Refused(e),
            }
        }
    }

    impl ControlHandler {
        async fn answer(&self, peer: &str, sha: &str) -> std::result::Result<(Permit, Hash), AnswerErr> {
            self.answer_inner(peer, sha).await
        }

        async fn answer_inner(&self, peer: &str, sha: &str) -> std::result::Result<(Permit, Hash), AnswerErr> {
            let host = (self.host)().ok_or_else(|| Error::Auth("rien n'est partagé".into()))?;
            let permit = host.authorize(peer, sha)?;
            // Empreinte prise AVANT l'import : si le fichier change pendant, l'entrée sera périmée au prochain appel.
            let now = stamp(&permit.file().path);
            let cached = self.known.lock().unwrap_or_else(|p| p.into_inner()).get(sha).copied();
            let hash = match cached {
                Some((h, st)) if now.is_some() && Some(st) == now => h,
                _ => {
                    // Jamais d'import d'un fichier qui ne correspond pas à l'empreinte annoncée (le jeu l'a réécrit
                    // depuis le manifeste) : on refuse proprement et l'hôte recalcule son manifeste.
                    let path = permit.file().path.clone();
                    let live = tokio::task::spawn_blocking(move || crate::snapshot::sha256_of(&path)).await;
                    if !matches!(&live, Ok(Ok(h)) if h == sha) {
                        host.drift();
                        return Err(AnswerErr::Changed);
                    }
                    let tag = self
                        .store
                        .blobs()
                        .add_path_with_opts(AddPathOptions {
                            path: permit.file().path.clone(),
                            format: BlobFormat::Raw,
                            mode: ImportMode::Copy,
                        })
                        .await
                        .map_err(net)?;
                    let mut known = self.known.lock().unwrap_or_else(|p| p.into_inner());
                    match now {
                        Some(st) => known.insert(sha.to_string(), (tag.hash, st)),
                        None => known.remove(sha),
                    };
                    tag.hash
                }
            };
            Ok((permit, hash))
        }
    }

    impl ProtocolHandler for ControlHandler {
        async fn accept(&self, conn: Connection) -> std::result::Result<(), AcceptError> {
            let peer = conn.remote_id().to_string();
            // Chaque requête = un flux bidi ; le créneau est tenu jusqu'à la fermeture côté invité.
            while let Ok((mut send, mut recv)) = conn.accept_bi().await {
                let this = self.clone();
                let peer = peer.clone();
                tokio::spawn(async move {
                    let mut sha = [0u8; 64];
                    if recv.read_exact(&mut sha).await.is_err() {
                        return;
                    }
                    let sha = String::from_utf8_lossy(&sha).into_owned();
                    match this.answer(&peer, &sha).await {
                        Ok((_permit, hash)) => {
                            let mut msg = vec![0u8];
                            msg.extend_from_slice(hash.as_bytes());
                            if send.write_all(&msg).await.is_err() {
                                return;
                            }
                            let _ = send.finish();
                            // Attend que l'invité ait terminé (fin de son flux) avant de libérer le créneau.
                            let _ = recv.read_to_end(16).await;
                        }
                        Err(e) => {
                            let code = if matches!(e, AnswerErr::Changed) { STATUS_CHANGED } else { 1u8 };
                            let _ = send.write_all(&[code]).await;
                            let _ = send.finish();
                        }
                    }
                });
            }
            Ok(())
        }
    }

    /// Protège le protocole iroh-blobs (ALPN `iroh_blobs::ALPN`) : connexions refusées si le pair
    /// n'est pas autorisé ou si rien n'est partagé.
    pub struct GatedBlobsHandler {
        host: HostProvider,
        inner: BlobsProtocol,
    }

    impl std::fmt::Debug for GatedBlobsHandler {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("GatedBlobsHandler")
        }
    }

    impl ProtocolHandler for GatedBlobsHandler {
        async fn accept(&self, conn: Connection) -> std::result::Result<(), AcceptError> {
            let ok = (self.host)().is_some_and(|h| h.check_peer(&conn.remote_id().to_string()).is_ok());
            if !ok {
                conn.close(1u32.into(), b"unauthorized");
                return Ok(());
            }
            self.inner.accept(conn).await
        }
    }

    impl BlobTransport for IrohTransport {
        async fn serve(&self, host: Arc<Host>) -> Result<()> {
            let (control, blobs) = self.handlers(host);
            let router = Router::builder(self.endpoint.clone())
                .accept(CONTROL_ALPN, control)
                .accept(iroh_blobs::ALPN, blobs)
                .spawn();
            *self.router.lock().unwrap_or_else(|p| p.into_inner()) = Some(router);
            Ok(())
        }

        async fn fetch(
            &self,
            peer: &str,
            req: &FetchRequest,
            on_bytes: &(dyn Fn(u64) + Send + Sync),
            cancel: &AtomicBool,
        ) -> Result<()> {
            let addr = self
                .peers
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(peer)
                .cloned()
                .ok_or_else(|| Error::Network(format!("pair inconnu : {peer}")))?;
            if req.resume_from > req.size {
                return Err(Error::Invalid("reprise hors bornes".into()));
            }
            // 1) contrôle : un flux par fichier sur la connexion partagée.
            let ctl = self.conn(peer, &addr, CONTROL_ALPN).await?;
            let r = self.fetch_on(peer, &addr, &ctl, req, on_bytes, cancel).await;
            if matches!(r, Err(Error::Network(_))) || ctl.close_reason().is_some() {
                // La prochaine tentative rouvre une connexion neuve.
                self.forget(peer, CONTROL_ALPN, &ctl);
            }
            r
        }

        fn via_relay(&self, peer: &str) -> bool {
            self.relayed.lock().unwrap_or_else(|p| p.into_inner()).get(peer).copied().unwrap_or(false)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use core_lib::types::{Loader, LoaderSpec, ManifestFile};
        use sha2::{Digest, Sha256};
        use std::time::Duration;

        /// Un fichier qui ne correspond plus à l'empreinte annoncée n'est JAMAIS importé ni servi (le manifeste
        /// est périmé : l'hôte en est averti) ; une fois réparé, il est servi sous son vrai BLAKE3.
        #[tokio::test]
        async fn repaired_file_is_not_served_from_stale_cache() {
            let dir = tempfile::tempdir().unwrap();
            let good: Vec<u8> = (0..5000u32).map(|i| (i * 7) as u8).collect();
            let mut evil = good.clone();
            evil[0] ^= 1;
            let path = dir.path().join("mods").join("a.jar");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let sha = hex::encode(Sha256::digest(&good));
            let m = Manifest {
                name: "M".into(),
                minecraft: "1.21.1".into(),
                loader: LoaderSpec { loader: Loader::Fabric, version: "0.16.0".into() },
                java_major: 21,
                files: vec![ManifestFile { path: "mods/a.jar".into(), size: good.len() as u64, sha256: sha.clone(), sha1: None }],
            };
            let host = Host::new(
                ServeIndex::from_manifest(&m, dir.path()).unwrap(),
                Arc::new(AllowList(vec!["p".into()])),
                ServeLimits::default(),
            );
            let store = FsStore::load(dir.path().join("blobs")).await.unwrap();
            let drifts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let d2 = drifts.clone();
            host.set_on_drift(Arc::new(move || {
                d2.fetch_add(1, Ordering::SeqCst);
            }));
            let ctl = ControlHandler { host: Arc::new(move || Some(host.clone())), store, known: Arc::default() };

            std::fs::write(&path, &evil).unwrap();
            assert!(matches!(ctl.answer("p", &sha).await, Err(AnswerErr::Changed)), "fichier altéré : refusé");
            assert_eq!(drifts.load(Ordering::SeqCst), 1, "l'hôte est averti que son manifeste est périmé");

            // L'hôte répare (même taille ; date de modification forcée différente, la résolution du système de fichiers variant).
            std::fs::write(&path, &good).unwrap();
            let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            f.set_modified(std::time::SystemTime::now() + Duration::from_secs(5)).unwrap();
            drop(f);
            let (_, h_good) = ctl.answer("p", &sha).await.unwrap();
            assert_eq!(h_good, Hash::new(&good), "le fichier réparé est servi sous son vrai BLAKE3");
        }
    }
}

pub use iroh_impl::{ControlHandler, GatedBlobsHandler, HostProvider, IrohTransport};

#[cfg(test)]
mod tests {
    use super::*;
    use core_lib::types::{Loader, LoaderSpec, ManifestFile};
    use sha2::{Digest, Sha256};

    pub fn sha(data: &[u8]) -> String {
        hex::encode(Sha256::digest(data))
    }

    fn manifest(files: Vec<ManifestFile>) -> Manifest {
        Manifest {
            name: "M".into(),
            minecraft: "1.21.1".into(),
            loader: LoaderSpec { loader: Loader::Fabric, version: "0.16.0".into() },
            java_major: 21,
            files,
        }
    }

    #[test]
    fn index_only_contains_manifest_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("mods")).unwrap();
        std::fs::write(dir.path().join("mods/a.jar"), b"aaa").unwrap();
        std::fs::write(dir.path().join("secret.txt"), b"secret").unwrap();
        let m = manifest(vec![ManifestFile { path: "mods/a.jar".into(), size: 3, sha256: sha(b"aaa"), sha1: None }]);
        let idx = ServeIndex::from_manifest(&m, dir.path()).unwrap();
        assert!(idx.get(&sha(b"aaa")).is_some());
        assert!(idx.get(&sha(b"secret")).is_none());
        let bad = manifest(vec![ManifestFile { path: "../x".into(), size: 1, sha256: sha(b"x"), sha1: None }]);
        assert!(ServeIndex::from_manifest(&bad, dir.path()).is_err());
    }

    #[test]
    fn concurrency_and_total_limits() {
        let dir = tempfile::tempdir().unwrap();
        let m = manifest(vec![
            ManifestFile { path: "mods/a.jar".into(), size: 10, sha256: sha(b"a"), sha1: None },
            ManifestFile { path: "mods/b.jar".into(), size: 10, sha256: sha(b"b"), sha1: None },
        ]);
        let idx = ServeIndex::from_manifest(&m, dir.path()).unwrap();
        let limits = ServeLimits { max_file_bytes: 100, max_total_bytes_per_peer: 15, max_concurrent_per_peer: 1 };
        let host = Host::new(idx, Arc::new(AllowList(vec!["p".into()])), limits);
        let p1 = host.authorize("p", &sha(b"a")).unwrap();
        assert!(matches!(host.authorize("p", &sha(b"b")), Err(Error::Network(_))));
        drop(p1);
        // concurrence libérée, mais le total (10 + 10 > 15) est dépassé
        assert!(matches!(host.authorize("p", &sha(b"b")), Err(Error::Invalid(_))));
        assert!(matches!(host.authorize("intrus", &sha(b"a")), Err(Error::Auth(_))));
    }

    fn mem_setup(data: &[u8]) -> (tempfile::TempDir, Arc<Host>, Arc<MemNetwork>) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("mods")).unwrap();
        std::fs::write(dir.path().join("mods/a.jar"), data).unwrap();
        let m = manifest(vec![ManifestFile { path: "mods/a.jar".into(), size: data.len() as u64, sha256: sha(data), sha1: None }]);
        let idx = ServeIndex::from_manifest(&m, dir.path()).unwrap();
        let host = Host::new(idx, Arc::new(AllowList(vec!["guest".into()])), ServeLimits::default());
        (dir, host, MemNetwork::new())
    }

    fn payload() -> Vec<u8> {
        (0..300_000u32).map(|i| (i % 251) as u8).collect()
    }

    fn req(data: &[u8], dest: PathBuf, resume_from: u64) -> FetchRequest {
        FetchRequest { sha256: sha(data), size: data.len() as u64, dest, resume_from }
    }

    #[tokio::test]
    async fn mem_resume_transfers_only_the_rest() {
        let data = payload();
        let (dir, host, net) = mem_setup(&data);
        MemTransport::new(net.clone(), "host").serve(host).await.unwrap();
        let guest = MemTransport::new(net, "guest").with_cut_after(100_000);
        let dest = dir.path().join("out.part");
        let cancel = AtomicBool::new(false);
        let err = guest.fetch("host", &req(&data, dest.clone(), 0), &|_| {}, &cancel).await.unwrap_err();
        assert!(matches!(err, Error::Network(_)));
        let have = std::fs::metadata(&dest).unwrap().len();
        assert_eq!(have, 100_000);
        assert_eq!(guest.bytes_transferred(), 100_000);
        guest.fetch("host", &req(&data, dest.clone(), have), &|_| {}, &cancel).await.unwrap();
        assert_eq!(guest.bytes_transferred(), data.len() as u64);
        let got = std::fs::read(&dest).unwrap();
        assert_eq!(sha(&got), sha(&data));
    }

    #[tokio::test]
    async fn mem_inconsistent_offset_restarts_from_zero() {
        let data = payload();
        let (dir, host, net) = mem_setup(&data);
        MemTransport::new(net.clone(), "host").serve(host).await.unwrap();
        let guest = MemTransport::new(net, "guest");
        let dest = dir.path().join("out.part");
        std::fs::write(&dest, b"garbage").unwrap();
        let cancel = AtomicBool::new(false);
        guest.fetch("host", &req(&data, dest.clone(), 1000), &|_| {}, &cancel).await.unwrap();
        assert_eq!(guest.bytes_transferred(), data.len() as u64);
        assert_eq!(std::fs::read(&dest).unwrap(), data);
    }

    #[tokio::test]
    async fn mem_resume_out_of_bounds_rejected() {
        let data = payload();
        let (dir, host, net) = mem_setup(&data);
        MemTransport::new(net.clone(), "host").serve(host).await.unwrap();
        let guest = MemTransport::new(net, "guest");
        let dest = dir.path().join("out.part");
        let cancel = AtomicBool::new(false);
        let r = req(&data, dest, data.len() as u64 + 1);
        let err = guest.fetch("host", &r, &|_| {}, &cancel).await.unwrap_err();
        assert!(matches!(err, Error::Invalid(_)));
        assert_eq!(guest.bytes_transferred(), 0);
    }

    #[tokio::test]
    async fn mem_resume_does_not_bypass_authorization() {
        let data = payload();
        let (dir, host, net) = mem_setup(&data);
        MemTransport::new(net.clone(), "host").serve(host).await.unwrap();
        let intrus = MemTransport::new(net.clone(), "intrus");
        let cancel = AtomicBool::new(false);
        let dest = dir.path().join("o");
        let err = intrus.fetch("host", &req(&data, dest.clone(), 10), &|_| {}, &cancel).await.unwrap_err();
        assert!(matches!(err, Error::Auth(_)));
        let guest = MemTransport::new(net, "guest");
        let mut r = req(&data, dest, 10);
        r.sha256 = sha(b"inconnu");
        assert!(matches!(guest.fetch("host", &r, &|_| {}, &cancel).await, Err(Error::NotFound(_))));
    }
}
