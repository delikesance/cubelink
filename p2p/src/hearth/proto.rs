//! Hearth : protocole réseau `cubelink/hearth/1` (voir `docs/hearth.md`, § 4).
//!
//! Même discipline que `invite.rs` et `presence.rs` :
//!
//! * l'émetteur est **toujours** `conn.remote_id()`, jamais un champ du message ;
//! * messages JSON bornés à [`MAX_MESSAGE_BYTES`] (64 Kio), un flux bidirectionnel par requête, délai de
//!   [`IO_TIMEOUT`] (10 s) ;
//! * un pair qui n'est pas membre du groupe visé (ou un groupe inconnu : même réponse, pour ne rien révéler) est
//!   **refusé sans un octet de réponse** : la connexion est fermée. Seule exception, `GroupInvite`, accepté d'un
//!   ami ;
//! * un membre dont la demande est invalide reçoit `Rejected { reason }` (texte borné) ;
//! * tout ce qui vient du pair est validé avant d'être confié au [`HearthDelegate`] (signatures, appartenance,
//!   tailles, nombre d'éléments).
//!
//! # Requêtes
//!
//! | Requête | Réponse | Usage |
//! |---|---|---|
//! | `Hello { group_id, op_seq, heads }` | `Status { op_seq, heads, lease? }` | état d'un membre (sonde, 60 s) |
//! | `GetOps { group_id, after }` | `Ops { ops, more }` | rattraper la chaîne (par pages) |
//! | `GetVersions { group_id, after_number }` | `Versions { versions, more }` | rattraper les en-têtes (par pages) |
//! | `LeaseClaim { claim }` | `ClaimAck` ou `ClaimConflict { conflict }` | première phase de la prise du bail |
//! | `Lease { lease }` (signé) | `Ack` | annonce ou renouvellement du bail |
//! | `LeaseRelease { group_id, lease_id, last_seq, reason, version? }` | `Ack` | fin de partie (pierre tombale) |
//! | `GroupInvite { ops, total_ops, group_key }` | `Ack` | invitation (d'un ami) |
//! | `Propose { group_id, proposal }` / `Vote { group_id, proposal_id, sig }` | `Ack` | gouvernance asynchrone |
//! | `OfferVersion { version }` | `Ack` | « une nouvelle version existe » (file d'envoi) |
//! | `HaveVersion { group_id, version }` | `Have { complete }` | le membre a-t-il l'en-tête **et** tous les blobs ? |
//!
//! Écarts par rapport au tableau de la spec : `Propose` porte le `group_id` (une proposition n'en contient pas) ;
//! `Ops` et `Versions` portent `more` (une page tient dans 64 Kio) ; `GroupInvite` porte `total_ops` (voir
//! ci-dessous) ; `OfferVersion` et `HaveVersion` sont les deux messages de la file d'envoi (`outbox.rs`) : l'offre
//! répond tout de suite, le membre rapatrie les blobs de son côté, puis l'émetteur sonde `HaveVersion` et ne retire
//! l'entrée de sa file qu'une fois le contenu **complet** chez le membre. Les blobs voyagent sur
//! [`HEARTH_BLOBS_ALPN`] (voir `sync.rs`), pas ici.
//!
//! # Invitation
//!
//! La chaîne d'opérations d'un vieux groupe peut dépasser 64 Kio : `GroupInvite` en porte un **préfixe** (de
//! l'opération 1 aussi loin que ça tient) et `total_ops`. L'invité rejoue le préfixe, puis rattrape la suite par
//! `GetOps` auprès de l'invitant (qui le reconnaît comme membre), avant de confier la chaîne complète au délégué.
//! Elle est acceptée d'un ami, ou d'un membre d'un groupe qu'on connaît déjà (renouvellement de la clé de groupe
//! après un `RemoveMember` : l'invitant n'est pas forcément notre ami). La clé de groupe voyage uniquement dans ce
//! message, sur la connexion QUIC authentifiée.

use std::sync::Arc;
use std::time::Duration;

use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::{Endpoint, EndpointAddr};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::Semaphore;
use tokio::time::timeout;

use super::lease::{Claim, ClaimConflict, ClaimReply, Lease, LeaseId, ReleaseReason};
use super::model::{
    hex_bytes, GroupId, GroupState, OpHash, Proposal, Sig, SignedOp, MAX_OPS,
};
use super::store::GroupKey;
use super::version::{SignedVersion, VersionId};
use crate::presence::{observed_addr, PeerKey};

/// ALPN du protocole Hearth.
pub const HEARTH_ALPN: &[u8] = b"cubelink/hearth/1";
/// ALPN des blobs du monde (iroh-blobs, magasin dédié à Hearth ; voir `sync.rs`).
pub const HEARTH_BLOBS_ALPN: &[u8] = b"cubelink/hearth/blobs/1";
/// Taille maximale d'un message, dans les deux sens.
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;
/// Délai pour lire une requête, la traiter et répondre.
pub const IO_TIMEOUT: Duration = Duration::from_secs(10);
/// Délai pour traiter une invitation (rattrapage de la chaîne compris).
pub const INVITE_TIMEOUT: Duration = Duration::from_secs(30);
/// Délai de connexion côté client.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Nombre maximal de têtes dans un `Hello` ou un `Status`.
pub const MAX_HEADS: usize = 64;
/// Éléments au plus dans une page d'opérations ou de versions.
pub const MAX_PAGE_ITEMS: usize = 256;
/// Part d'un message réservée aux éléments d'une page (le reste : enveloppe).
const PAGE_BUDGET: usize = 48 * 1024;
/// Requêtes traitées au plus par connexion.
const MAX_REQUESTS_PER_CONN: usize = 64;
/// Connexions Hearth servies en même temps.
const MAX_CONNECTIONS: usize = 64;
/// Texte d'une raison de rejet, en caractères.
const MAX_REASON_CHARS: usize = 200;

const CLOSE_DENIED: u32 = 1;
const CLOSE_BUSY: u32 = 3;

// ---------------------------------------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------------------------------------

/// Requête.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Hello { group_id: GroupId, op_seq: u64, heads: Vec<VersionId> },
    GetOps { group_id: GroupId, after: u64 },
    GetVersions { group_id: GroupId, after_number: u64 },
    Lease { lease: Lease },
    /// Première phase de la prise du bail : « je veux héberger ». Réponse `ClaimAck` ou `ClaimConflict`.
    LeaseClaim { claim: Claim },
    /// Fin de partie : le `lease_id` et le dernier `seq` annoncés (pierre tombale chez le récepteur), la raison, et la
    /// version publiée s'il y en a une.
    LeaseRelease {
        group_id: GroupId,
        #[serde(with = "hex_bytes")]
        lease_id: LeaseId,
        last_seq: u64,
        reason: ReleaseReason,
        version: Option<VersionId>,
    },
    GroupInvite { ops: Vec<SignedOp>, total_ops: u64, group_key: GroupKey },
    Propose { group_id: GroupId, proposal: Proposal },
    Vote {
        group_id: GroupId,
        #[serde(with = "hex_bytes")]
        proposal_id: OpHash,
        sig: Sig,
    },
    OfferVersion { version: SignedVersion },
    HaveVersion { group_id: GroupId, version: VersionId },
}

/// Réponse.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Status { op_seq: u64, heads: Vec<VersionId>, lease: Option<Lease> },
    Ops { ops: Vec<SignedOp>, more: bool },
    Versions { versions: Vec<SignedVersion>, more: bool },
    Ack,
    ClaimAck,
    ClaimConflict { conflict: ClaimConflict },
    Have { complete: bool },
    Rejected { reason: String },
}

/// État d'un membre, réponse à `Hello`.
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub op_seq: u64,
    pub heads: Vec<VersionId>,
    pub lease: Option<Lease>,
}

/// Une page d'opérations ou de versions.
#[derive(Debug, Clone, PartialEq)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// D'autres éléments suivent : redemander à partir du dernier reçu.
    pub more: bool,
}

// ---------------------------------------------------------------------------------------------------------
// Délégué : ce que l'application branche
// ---------------------------------------------------------------------------------------------------------

/// Logique de l'application derrière le protocole. Toutes les méthodes sont **synchrones et rapides** (elles
/// lisent ou modifient des structures en mémoire ; un travail long se lance avec `tokio::spawn`). Le protocole a
/// déjà vérifié l'identité et l'appartenance de l'émetteur et la forme de la demande ; le délégué garde la
/// décision métier.
pub trait HearthDelegate: Send + Sync + 'static {
    /// `key` est-elle un ami accepté ?
    fn is_friend(&self, key: &PeerKey) -> bool;
    /// État courant du groupe, s'il est connu de ce nœud.
    fn group_state(&self, group: &GroupId) -> Option<GroupState>;
    /// Au plus `limit` opérations de numéro supérieur à `after`.
    fn ops_after(&self, group: &GroupId, after: u64, limit: usize) -> Vec<SignedOp>;
    /// Au plus `limit` versions de numéro supérieur à `after_number` (numéro croissant).
    fn versions_after(&self, group: &GroupId, after_number: u64, limit: usize) -> Vec<SignedVersion>;
    /// Têtes du groupe.
    fn heads(&self, group: &GroupId) -> Vec<VersionId>;
    /// Notre propre bail signé (partie hébergée ou en préparation), tel qu'annoncé : il est signé, donc inchangé. Le
    /// bail d'un tiers n'est pas relayé (le récepteur ne croit que le bail annoncé par son titulaire).
    fn lease(&self, group: &GroupId) -> Option<Lease>;
    /// Ce nœud a-t-il l'en-tête de cette version **et** tout son contenu ?
    fn have_version(&self, group: &GroupId, version: &VersionId) -> bool;

    /// Un membre a répondu à notre sonde ou nous a sondés (état annoncé). Sans effet par défaut.
    fn on_status(&self, _from: PeerKey, _group: &GroupId, _op_seq: u64, _heads: &[VersionId]) {}
    /// Bail annoncé par son titulaire (déjà contrôlé : groupe, titulaire = émetteur, membre, durée).
    fn on_lease(&self, from: PeerKey, lease: Lease);
    /// Un membre veut prendre le bail (première phase) : le témoin répond `Ack` ou `Conflict` (voir
    /// `LeaseTable::claim`). Le claim est déjà contrôlé : groupe, émetteur = titulaire, membre, époque courante.
    fn on_claim(&self, from: PeerKey, claim: Claim) -> Result<ClaimReply, String>;
    /// Fin de partie du titulaire (`lease_id`, dernier `seq`, raison), avec la version publiée s'il y en a une.
    fn on_lease_release(
        &self,
        from: PeerKey,
        group: &GroupId,
        lease_id: LeaseId,
        last_seq: u64,
        reason: ReleaseReason,
        version: Option<VersionId>,
    );
    /// Invitation (chaîne complète rejouée, nous et l'invitant en sommes membres). À l'application d'en faire
    /// une carte à accepter, ou de refuser ; elle ne stocke la clé qu'avec l'accord de l'utilisateur.
    fn on_invite(&self, from: PeerKey, ops: Vec<SignedOp>, key: GroupKey) -> Result<(), String>;
    /// Proposition reçue (déjà validée contre l'état du groupe).
    fn on_proposal(&self, from: PeerKey, group: &GroupId, proposal: Proposal) -> Result<(), String>;
    /// Vote de `from` (sa signature n'est pas encore vérifiée : `Group::add_vote` la contrôle).
    fn on_vote(&self, from: PeerKey, group: &GroupId, proposal_id: OpHash, sig: Sig) -> Result<(), String>;
    /// Une version existe (signature et appartenance de l'auteur vérifiées). À l'application d'ingérer
    /// l'en-tête (en rattrapant au besoin les versions parentes auprès de `from`) et de rapatrier les blobs.
    fn on_version_offer(&self, from: PeerKey, version: SignedVersion) -> Result<(), String>;
}

// ---------------------------------------------------------------------------------------------------------
// Serveur
// ---------------------------------------------------------------------------------------------------------

/// Côté serveur : à enregistrer dans le `Router` sous [`HEARTH_ALPN`].
#[derive(Clone)]
pub struct HearthHandler {
    inner: Arc<HandlerInner>,
}

struct HandlerInner {
    endpoint: Endpoint,
    delegate: Arc<dyn HearthDelegate>,
    permits: Semaphore,
}

impl std::fmt::Debug for HearthHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HearthHandler")
    }
}

/// Issue du traitement d'une requête.
#[allow(clippy::large_enum_variant)] // un `Status` porte un bail signé ; la valeur ne vit que le temps d'une réponse
enum Served {
    Reply(Response),
    /// Non membre : fermeture sans réponse.
    Deny,
}

fn rejected(reason: impl std::fmt::Display) -> Served {
    let mut r: String = reason.to_string().chars().filter(|c| !c.is_control()).take(MAX_REASON_CHARS).collect();
    if r.is_empty() {
        r = "demande refusée".into();
    }
    Served::Reply(Response::Rejected { reason: r })
}

impl HearthHandler {
    pub fn new(endpoint: Endpoint, delegate: Arc<dyn HearthDelegate>) -> Self {
        Self { inner: Arc::new(HandlerInner { endpoint, delegate, permits: Semaphore::new(MAX_CONNECTIONS) }) }
    }

    /// État du groupe si `key` en est membre.
    fn member_state(&self, group: &GroupId, key: &PeerKey) -> Option<GroupState> {
        self.inner.delegate.group_state(group).filter(|s| s.is_member(key))
    }

    async fn handle(&self, from: PeerKey, addr: &EndpointAddr, req: Request) -> Served {
        let d = &self.inner.delegate;
        match req {
            Request::GroupInvite { ops, total_ops, group_key } => self.handle_invite(from, addr, ops, total_ops, group_key).await,
            Request::Hello { group_id, op_seq, heads } => {
                let Some(state) = self.member_state(&group_id, &from) else { return Served::Deny };
                if heads.len() > MAX_HEADS {
                    return rejected("trop de têtes");
                }
                d.on_status(from, &group_id, op_seq, &heads);
                let mut mine = d.heads(&group_id);
                mine.truncate(MAX_HEADS);
                Served::Reply(Response::Status { op_seq: state.op_seq, heads: mine, lease: d.lease(&group_id) })
            }
            Request::GetOps { group_id, after } => {
                if self.member_state(&group_id, &from).is_none() {
                    return Served::Deny;
                }
                let (ops, more) = page_ops(d.ops_after(&group_id, after, MAX_PAGE_ITEMS + 1));
                Served::Reply(Response::Ops { ops, more })
            }
            Request::GetVersions { group_id, after_number } => {
                if self.member_state(&group_id, &from).is_none() {
                    return Served::Deny;
                }
                let (versions, more) = page_versions(d.versions_after(&group_id, after_number, MAX_PAGE_ITEMS + 1));
                Served::Reply(Response::Versions { versions, more })
            }
            Request::Lease { lease } => {
                let Some(state) = self.member_state(&lease.group_id, &from) else { return Served::Deny };
                if let Err(e) = lease.check_received(&state, &from) {
                    return rejected(e);
                }
                d.on_lease(from, lease);
                Served::Reply(Response::Ack)
            }
            Request::LeaseClaim { claim } => {
                let Some(state) = self.member_state(&claim.group_id, &from) else { return Served::Deny };
                if claim.holder != from || claim.device_id != from {
                    return rejected("le claim n'est pas annoncé par son titulaire");
                }
                if claim.epoch != state.epoch {
                    return rejected("le claim n'est pas de l'époque courante du groupe");
                }
                match d.on_claim(from, claim) {
                    Ok(ClaimReply::Ack) => Served::Reply(Response::ClaimAck),
                    Ok(ClaimReply::Conflict(conflict)) => Served::Reply(Response::ClaimConflict { conflict }),
                    Err(e) => rejected(e),
                }
            }
            Request::LeaseRelease { group_id, lease_id, last_seq, reason, version } => {
                if self.member_state(&group_id, &from).is_none() {
                    return Served::Deny;
                }
                d.on_lease_release(from, &group_id, lease_id, last_seq, reason, version);
                Served::Reply(Response::Ack)
            }
            Request::Propose { group_id, proposal } => {
                let Some(state) = self.member_state(&group_id, &from) else { return Served::Deny };
                if let Err(e) = proposal.validate(&state) {
                    return rejected(e);
                }
                match d.on_proposal(from, &group_id, proposal) {
                    Ok(()) => Served::Reply(Response::Ack),
                    Err(e) => rejected(e),
                }
            }
            Request::Vote { group_id, proposal_id, sig } => {
                if self.member_state(&group_id, &from).is_none() {
                    return Served::Deny;
                }
                match d.on_vote(from, &group_id, proposal_id, sig) {
                    Ok(()) => Served::Reply(Response::Ack),
                    Err(e) => rejected(e),
                }
            }
            Request::OfferVersion { version } => {
                let group = version.header.group_id;
                if self.member_state(&group, &from).is_none() {
                    return Served::Deny;
                }
                if version.header.validate().is_err() || !version.verify() {
                    return rejected("version invalide");
                }
                // L'auteur peut être un ancien membre dont un membre a repris la lignée : le délégué tranche
                // (voir `Group::add_versions`).
                match d.on_version_offer(from, version) {
                    Ok(()) => Served::Reply(Response::Ack),
                    Err(e) => rejected(e),
                }
            }
            Request::HaveVersion { group_id, version } => {
                if self.member_state(&group_id, &from).is_none() {
                    return Served::Deny;
                }
                Served::Reply(Response::Have { complete: d.have_version(&group_id, &version) })
            }
        }
    }

    async fn handle_invite(
        &self,
        from: PeerKey,
        addr: &EndpointAddr,
        ops: Vec<SignedOp>,
        total_ops: u64,
        key: GroupKey,
    ) -> Served {
        let d = &self.inner.delegate;
        let Some(first) = ops.first() else { return rejected("chaîne vide") };
        let group = match &first.op {
            super::model::GroupOp::Create { id, .. } => *id,
            _ => return rejected("la chaîne doit commencer par la création du groupe"),
        };
        // Un ami, ou un membre d'un groupe qu'on connaît déjà (renouvellement de clé).
        if !(d.is_friend(&from) || self.member_state(&group, &from).is_some()) {
            return Served::Deny;
        }
        let total = match usize::try_from(total_ops) {
            Ok(t) if (ops.len()..=MAX_OPS).contains(&t) => t,
            _ => return rejected("nombre d'opérations invalide"),
        };
        let mut state = match super::model::replay(&ops) {
            Ok(s) => s,
            Err(e) => return rejected(e),
        };
        let mut chain = ops;
        if chain.len() < total {
            // Rattrape la suite auprès de l'invitant (il nous a déjà ajoutés à sa chaîne).
            let conn = match HearthConn::connect(&self.inner.endpoint, addr.clone()).await {
                Ok(c) => c,
                Err(e) => return rejected(format!("rattrapage impossible : {e}")),
            };
            while chain.len() < total {
                let page = match conn.get_ops(&group, state.op_seq).await {
                    Ok(p) => p,
                    Err(e) => return rejected(format!("rattrapage impossible : {e}")),
                };
                if page.items.is_empty() {
                    return rejected("chaîne incomplète");
                }
                for op in page.items {
                    if chain.len() >= total {
                        break;
                    }
                    if let Err(e) = state.apply(&op) {
                        return rejected(e);
                    }
                    chain.push(op);
                }
            }
            if chain.len() != total || state.op_seq != total as u64 {
                return rejected("chaîne incohérente");
            }
        }
        let me: PeerKey = *self.inner.endpoint.id().as_bytes();
        if !state.is_member(&me) {
            return rejected("tu ne fais pas partie de ce groupe");
        }
        if !state.is_member(&from) {
            return rejected("l'invitant ne fait pas partie du groupe");
        }
        match d.on_invite(from, chain, key) {
            Ok(()) => Served::Reply(Response::Ack),
            Err(e) => rejected(e),
        }
    }

    /// Sert une connexion : plusieurs requêtes, une par flux.
    async fn serve(&self, conn: Connection) {
        let from: PeerKey = *conn.remote_id().as_bytes();
        let addr = observed_addr(&conn);
        for _ in 0..MAX_REQUESTS_PER_CONN {
            let Ok(Ok((mut send, mut recv))) = timeout(IO_TIMEOUT, conn.accept_bi()).await else { return };
            let Ok(Ok(bytes)) = timeout(IO_TIMEOUT, recv.read_to_end(MAX_MESSAGE_BYTES)).await else {
                conn.close(CLOSE_DENIED.into(), b"");
                return;
            };
            let Ok(req) = serde_json::from_slice::<Request>(&bytes) else {
                // Message illisible : un membre honnête n'en envoie pas.
                conn.close(CLOSE_DENIED.into(), b"");
                return;
            };
            let limit = if matches!(req, Request::GroupInvite { .. }) { INVITE_TIMEOUT } else { IO_TIMEOUT };
            let served = match timeout(limit, self.handle(from, &addr, req)).await {
                Ok(s) => s,
                Err(_) => rejected("délai dépassé"),
            };
            let Served::Reply(resp) = served else {
                conn.close(CLOSE_DENIED.into(), b"");
                return;
            };
            let mut body = serde_json::to_vec(&resp).unwrap_or_default();
            if body.is_empty() || body.len() > MAX_MESSAGE_BYTES {
                body = serde_json::to_vec(&Response::Rejected { reason: "réponse trop grosse".into() }).unwrap_or_default();
            }
            let wrote = async {
                send.write_all(&body).await.ok()?;
                send.finish().ok()
            };
            if timeout(IO_TIMEOUT, wrote).await.ok().flatten().is_none() {
                return;
            }
        }
    }
}

impl ProtocolHandler for HearthHandler {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let Ok(_permit) = self.inner.permits.try_acquire() else {
            conn.close(CLOSE_BUSY.into(), b"");
            return Ok(());
        };
        self.serve(conn.clone()).await;
        // Laisse au client le temps de lire la dernière réponse avant de fermer.
        let _ = timeout(IO_TIMEOUT, conn.closed()).await;
        Ok(())
    }
}

/// Coupe une liste d'opérations (demandée avec une de plus que la page) à la taille d'une page.
fn page_ops(mut ops: Vec<SignedOp>) -> (Vec<SignedOp>, bool) {
    let mut more = ops.len() > MAX_PAGE_ITEMS;
    ops.truncate(MAX_PAGE_ITEMS);
    let mut used = 0usize;
    let mut keep = 0usize;
    for op in &ops {
        used += serde_json::to_vec(op).map_or(usize::MAX / 2, |b| b.len() + 1);
        if used > PAGE_BUDGET && keep > 0 {
            break;
        }
        keep += 1;
    }
    if keep < ops.len() {
        ops.truncate(keep);
        more = true;
    }
    (ops, more)
}

/// Idem pour les versions, en ne coupant qu'entre deux numéros (les fourches d'un même numéro restent ensemble)
/// tant que c'est possible.
fn page_versions(mut versions: Vec<SignedVersion>) -> (Vec<SignedVersion>, bool) {
    let mut more = versions.len() > MAX_PAGE_ITEMS;
    versions.truncate(MAX_PAGE_ITEMS);
    let mut used = 0usize;
    let mut keep = 0usize;
    for v in &versions {
        used += serde_json::to_vec(v).map_or(usize::MAX / 2, |b| b.len() + 1);
        if used > PAGE_BUDGET && keep > 0 {
            break;
        }
        keep += 1;
    }
    if keep < versions.len() || more {
        if keep < versions.len() {
            // Recule jusqu'à la frontière d'un numéro, sauf si un seul numéro remplit la page.
            let n = versions[keep - 1].header.number;
            let mut cut = keep;
            while cut > 0 && versions[cut - 1].header.number == n {
                cut -= 1;
            }
            if cut > 0 && versions.get(keep).is_some_and(|v| v.header.number == n) {
                keep = cut;
            }
            versions.truncate(keep);
        } else if let Some(last) = versions.last().map(|v| v.header.number) {
            // Page pleine par le nombre d'éléments : ne coupe pas une fourche en deux.
            let cut = versions.iter().rposition(|v| v.header.number != last).map_or(0, |i| i + 1);
            if cut > 0 {
                versions.truncate(cut);
            }
        }
        more = true;
    }
    (versions, more)
}

// ---------------------------------------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------------------------------------

/// Erreurs côté client (messages affichables).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum NetError {
    #[error("le membre est injoignable")]
    Unreachable,
    #[error("délai dépassé")]
    Timeout,
    #[error("le pair n'a pas répondu (demande refusée ou connexion coupée)")]
    NoReply,
    #[error("demande rejetée : {0}")]
    Rejected(String),
    #[error("réponse invalide : {0}")]
    Protocol(String),
    #[error("message trop gros")]
    TooLarge,
}

/// Connexion cliente vers un membre : plusieurs requêtes successives (un flux chacune).
pub struct HearthConn {
    conn: Connection,
}

impl Drop for HearthConn {
    fn drop(&mut self) {
        self.conn.close(0u32.into(), b"");
    }
}

impl HearthConn {
    /// Ouvre une connexion vers `addr` (délai de [`CONNECT_TIMEOUT`]).
    pub async fn connect(endpoint: &Endpoint, addr: impl Into<EndpointAddr>) -> Result<Self, NetError> {
        let conn = timeout(CONNECT_TIMEOUT, endpoint.connect(addr.into(), HEARTH_ALPN))
            .await
            .map_err(|_| NetError::Unreachable)?
            .map_err(|_| NetError::Unreachable)?;
        Ok(Self { conn })
    }

    /// Clé du pair joint.
    pub fn remote(&self) -> PeerKey {
        *self.conn.remote_id().as_bytes()
    }

    async fn call(&self, req: &Request, limit: Duration) -> Result<Response, NetError> {
        let body = serde_json::to_vec(req).map_err(|e| NetError::Protocol(e.to_string()))?;
        if body.len() > MAX_MESSAGE_BYTES {
            return Err(NetError::TooLarge);
        }
        let work = async {
            let (mut send, mut recv) = self.conn.open_bi().await.map_err(|_| NetError::Unreachable)?;
            send.write_all(&body).await.map_err(|_| NetError::NoReply)?;
            send.finish().map_err(|_| NetError::NoReply)?;
            recv.read_to_end(MAX_MESSAGE_BYTES).await.map_err(|_| NetError::NoReply)
        };
        let bytes = timeout(limit, work).await.map_err(|_| NetError::Timeout)??;
        let resp: Response = serde_json::from_slice(&bytes).map_err(|_| NetError::Protocol("réponse illisible".into()))?;
        match resp {
            Response::Rejected { reason } => Err(NetError::Rejected(reason.chars().filter(|c| !c.is_control()).take(MAX_REASON_CHARS).collect())),
            r => Ok(r),
        }
    }

    async fn call_ack(&self, req: &Request) -> Result<(), NetError> {
        match self.call(req, IO_TIMEOUT).await? {
            Response::Ack => Ok(()),
            _ => Err(NetError::Protocol("réponse inattendue".into())),
        }
    }

    /// `Hello` : état du membre (au plus [`MAX_HEADS`] têtes sont envoyées).
    pub async fn hello(&self, group: &GroupId, op_seq: u64, heads: &[VersionId]) -> Result<Status, NetError> {
        let heads = heads.iter().take(MAX_HEADS).copied().collect();
        match self.call(&Request::Hello { group_id: *group, op_seq, heads }, IO_TIMEOUT).await? {
            Response::Status { op_seq, heads, lease } if heads.len() <= MAX_HEADS => Ok(Status { op_seq, heads, lease }),
            _ => Err(NetError::Protocol("réponse inattendue".into())),
        }
    }

    /// Une page d'opérations de numéro supérieur à `after` (numéros consécutifs vérifiés).
    pub async fn get_ops(&self, group: &GroupId, after: u64) -> Result<Page<SignedOp>, NetError> {
        match self.call(&Request::GetOps { group_id: *group, after }, IO_TIMEOUT).await? {
            Response::Ops { ops, more } if ops.len() <= MAX_PAGE_ITEMS => {
                let mut expect = after.saturating_add(1);
                for op in &ops {
                    if op.op_seq != expect {
                        return Err(NetError::Protocol("opérations hors séquence".into()));
                    }
                    expect = expect.saturating_add(1);
                }
                Ok(Page { items: ops, more })
            }
            _ => Err(NetError::Protocol("réponse inattendue".into())),
        }
    }

    /// Une page d'en-têtes de numéro supérieur à `after_number` (validité vérifiée par `Group::add_versions`).
    pub async fn get_versions(&self, group: &GroupId, after_number: u64) -> Result<Page<SignedVersion>, NetError> {
        match self.call(&Request::GetVersions { group_id: *group, after_number }, IO_TIMEOUT).await? {
            Response::Versions { versions, more }
                if versions.len() <= MAX_PAGE_ITEMS && versions.iter().all(|v| v.header.number > after_number) =>
            {
                Ok(Page { items: versions, more })
            }
            _ => Err(NetError::Protocol("réponse inattendue".into())),
        }
    }

    /// Toutes les opérations après `after`, page après page (au plus `MAX_OPS` au total).
    pub async fn fetch_ops(&self, group: &GroupId, after: u64) -> Result<Vec<SignedOp>, NetError> {
        let mut all: Vec<SignedOp> = Vec::new();
        let mut last = after;
        loop {
            let page = self.get_ops(group, last).await?;
            let Some(tail) = page.items.last().map(|o| o.op_seq) else { return Ok(all) };
            last = tail;
            all.extend(page.items);
            if !page.more {
                return Ok(all);
            }
            if all.len() > MAX_OPS {
                return Err(NetError::Protocol("chaîne trop longue".into()));
            }
        }
    }

    /// Toutes les versions de numéro supérieur à `after_number`, page après page (au plus
    /// `crate::hearth::version::MAX_VERSIONS`). Une page pleine d'un seul numéro (fourche énorme) arrête la boucle.
    pub async fn fetch_versions(&self, group: &GroupId, after_number: u64) -> Result<Vec<SignedVersion>, NetError> {
        let mut all: Vec<SignedVersion> = Vec::new();
        let mut last = after_number;
        loop {
            let page = self.get_versions(group, last).await?;
            let Some(tail) = page.items.iter().map(|v| v.header.number).max() else { return Ok(all) };
            all.extend(page.items);
            if !page.more {
                return Ok(all);
            }
            if tail <= last || all.len() > super::version::MAX_VERSIONS {
                return Err(NetError::Protocol("pagination bloquée".into()));
            }
            last = tail;
        }
    }

    /// Annonce ou renouvelle le bail.
    pub async fn send_lease(&self, lease: &Lease) -> Result<(), NetError> {
        self.call_ack(&Request::Lease { lease: lease.clone() }).await
    }

    /// Première phase de la prise du bail, avec le délai [`CLAIM_TIMEOUT`](super::lease::CLAIM_TIMEOUT).
    pub async fn claim_lease(&self, claim: &Claim) -> Result<ClaimReply, NetError> {
        match self.call(&Request::LeaseClaim { claim: claim.clone() }, super::lease::CLAIM_TIMEOUT).await? {
            Response::ClaimAck => Ok(ClaimReply::Ack),
            Response::ClaimConflict { conflict } => Ok(ClaimReply::Conflict(conflict)),
            _ => Err(NetError::Protocol("réponse inattendue".into())),
        }
    }

    /// Fin de partie : `lease_id` et dernier `seq` de cette partie, raison, version publiée s'il y en a une.
    pub async fn release_lease(
        &self,
        group: &GroupId,
        lease_id: LeaseId,
        last_seq: u64,
        reason: ReleaseReason,
        version: Option<VersionId>,
    ) -> Result<(), NetError> {
        self.call_ack(&Request::LeaseRelease { group_id: *group, lease_id, last_seq, reason, version }).await
    }

    /// Invite le pair (un ami) dans le groupe. La chaîne qui ne tient pas dans un message est envoyée en préfixe :
    /// l'invité rattrape la suite par `GetOps` (voir l'en-tête du module).
    pub async fn send_invite(&self, ops: &[SignedOp], key: &GroupKey) -> Result<(), NetError> {
        if ops.is_empty() || ops.len() > MAX_OPS {
            return Err(NetError::Protocol("chaîne d'opérations invalide".into()));
        }
        // Plus grand préfixe dont le message tient dans la limite.
        let envelope = serde_json::to_vec(&Request::GroupInvite { ops: Vec::new(), total_ops: ops.len() as u64, group_key: key.clone() })
            .map_err(|e| NetError::Protocol(e.to_string()))?
            .len();
        let mut used = envelope;
        let mut take = 0usize;
        for op in ops {
            let n = serde_json::to_vec(op).map_err(|e| NetError::Protocol(e.to_string()))?.len() + 1;
            if used + n > MAX_MESSAGE_BYTES {
                break;
            }
            used += n;
            take += 1;
        }
        if take == 0 {
            return Err(NetError::TooLarge);
        }
        let req = Request::GroupInvite { ops: ops[..take].to_vec(), total_ops: ops.len() as u64, group_key: key.clone() };
        match self.call(&req, INVITE_TIMEOUT + IO_TIMEOUT).await? {
            Response::Ack => Ok(()),
            _ => Err(NetError::Protocol("réponse inattendue".into())),
        }
    }

    /// Soumet une proposition.
    pub async fn propose(&self, group: &GroupId, proposal: &Proposal) -> Result<(), NetError> {
        self.call_ack(&Request::Propose { group_id: *group, proposal: proposal.clone() }).await
    }

    /// Envoie notre vote (la signature de `Proposal::sign`).
    pub async fn vote(&self, group: &GroupId, proposal_id: OpHash, sig: Sig) -> Result<(), NetError> {
        self.call_ack(&Request::Vote { group_id: *group, proposal_id, sig }).await
    }

    /// « Une version existe » : le membre ingère l'en-tête et rapatrie les blobs de son côté.
    pub async fn offer_version(&self, version: &SignedVersion) -> Result<(), NetError> {
        self.call_ack(&Request::OfferVersion { version: version.clone() }).await
    }

    /// Le membre a-t-il l'en-tête et tout le contenu de cette version ?
    pub async fn have_version(&self, group: &GroupId, version: &VersionId) -> Result<bool, NetError> {
        match self.call(&Request::HaveVersion { group_id: *group, version: *version }, IO_TIMEOUT).await? {
            Response::Have { complete } => Ok(complete),
            _ => Err(NetError::Protocol("réponse inattendue".into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::model::{testutil, GroupOp, HearthSigner};
    use super::super::version::{VersionHeader, VersionKind};
    use super::*;

    fn version(n: u64, tag: u8) -> SignedVersion {
        let k = testutil::key(1);
        let header = VersionHeader {
            group_id: GroupId([1; 16]),
            number: n,
            parent: if n == 1 { None } else { Some(VersionId([tag; 32])) },
            epoch: 0,
            author: k.peer_key(),
            pack_hash: "a".repeat(64),
            created_at: 1,
            played_secs: 1,
            kind: VersionKind::Session,
            collection: [tag; 32],
            total_size: 1,
        };
        SignedVersion::sign(&k, header).unwrap()
    }

    #[test]
    fn messages_roundtrip_in_json() {
        let g = GroupId([7; 16]);
        let reqs = vec![
            Request::Hello { group_id: g, op_seq: 3, heads: vec![VersionId([1; 32])] },
            Request::GetOps { group_id: g, after: 2 },
            Request::GetVersions { group_id: g, after_number: 0 },
            Request::LeaseRelease { group_id: g, lease_id: [4; 16], last_seq: 7, reason: ReleaseReason::Crashed { mod_hint: Some("x".into()) }, version: None },
            Request::LeaseClaim {
                claim: Claim {
                    group_id: g,
                    lease_id: [5; 16],
                    holder: [1; 32],
                    device_id: [1; 32],
                    base_version: Some(VersionId([3; 32])),
                    base_number: 2,
                    epoch: 0,
                },
            },
            Request::Vote { group_id: g, proposal_id: [9; 32], sig: Sig([5; 64]) },
            Request::HaveVersion { group_id: g, version: VersionId([2; 32]) },
            Request::OfferVersion { version: version(2, 4) },
        ];
        for r in reqs {
            let bytes = serde_json::to_vec(&r).unwrap();
            assert_eq!(serde_json::from_slice::<Request>(&bytes).unwrap(), r);
        }
        let conflict = ClaimConflict {
            kind: super::super::lease::ConflictKind::Claim,
            holder: [2; 32],
            lease_id: [6; 16],
            base_version: None,
            base_number: 0,
        };
        for r in [Response::ClaimAck, Response::ClaimConflict { conflict }] {
            assert_eq!(serde_json::from_slice::<Response>(&serde_json::to_vec(&r).unwrap()).unwrap(), r);
        }
        let resp = Response::Status { op_seq: 1, heads: vec![], lease: None };
        assert_eq!(serde_json::from_slice::<Response>(&serde_json::to_vec(&resp).unwrap()).unwrap(), resp);
        // Le type est une étiquette lisible, la clé de groupe ne sort pas en `Debug`.
        let inv = Request::GroupInvite { ops: vec![], total_ops: 0, group_key: GroupKey::random() };
        assert!(!format!("{inv:?}").contains("group_key: GroupKey(["));
        assert_eq!(format!("{:?}", GroupKey::random()), "GroupKey(..)");
    }

    #[test]
    fn unknown_or_malformed_messages_do_not_parse() {
        assert!(serde_json::from_slice::<Request>(b"{\"type\":\"inconnu\"}").is_err());
        assert!(serde_json::from_slice::<Request>(b"{\"type\":\"get_ops\",\"group_id\":\"zz\",\"after\":1}").is_err());
        assert!(serde_json::from_slice::<Request>(b"{\"type\":\"get_ops\",\"group_id\":\"00000000000000000000000000000000\",\"after\":-1}").is_err());
        assert!(serde_json::from_slice::<Request>(b"pas du json").is_err());
    }

    #[test]
    fn pages_respect_size_and_keep_forks_together() {
        // Beaucoup de versions : la page est bornée en nombre et en octets, et ne coupe pas un numéro en deux.
        let vs: Vec<SignedVersion> = (0..(MAX_PAGE_ITEMS as u64 + 1)).map(|i| version(2 + i / 2, (i % 200) as u8 + 1)).collect();
        let (page, more) = page_versions(vs.clone());
        assert!(more);
        assert!(page.len() <= MAX_PAGE_ITEMS);
        let last = page.last().unwrap().header.number;
        assert!(vs.iter().filter(|v| v.header.number == last).count() == page.iter().filter(|v| v.header.number == last).count());
        let total: usize = page.iter().map(|v| serde_json::to_vec(v).unwrap().len()).sum();
        assert!(total <= PAGE_BUDGET + 2048);
        // Une petite liste passe entière, sans `more`.
        let (page, more) = page_versions(vec![version(2, 1), version(3, 2)]);
        assert_eq!((page.len(), more), (2, false));
        // Ops : un seul élément est toujours renvoyé, même énorme.
        let k = testutil::key(1);
        let op = SignedOp::create(&k, GroupId([1; 16]), "x", testutil::pack(), &Default::default()).unwrap();
        let (ops, more) = page_ops(vec![op.clone(); 3]);
        assert_eq!((ops.len(), more), (3, false));
        let _ = GroupOp::Leave;
    }

    #[test]
    fn rejection_reasons_are_bounded() {
        let Served::Reply(Response::Rejected { reason }) = rejected("a\nb".repeat(500)) else { panic!() };
        assert!(reason.chars().count() <= MAX_REASON_CHARS);
        assert!(!reason.contains('\n'));
    }
}
