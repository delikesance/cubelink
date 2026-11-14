//! Hearth : bail d'hôte et décision du bouton Jouer (voir `docs/hearth.md`, § 3.3).
//!
//! Logique pure. Un seul serveur à la fois par groupe : l'hôte annonce un [`Lease`] toutes les
//! [`LEASE_RENEW_INTERVAL`] aux membres en ligne ; chaque récepteur le garde [`LEASE_TTL_SECS`] **à partir de sa
//! réception** (aucune horloge partagée). Le temps est un `std::time::Instant` passé en paramètre : les tests le
//! font avancer sans dormir.
//!
//! Précisions par rapport à la spec :
//! * le bail est **signé** ([`SignedLease`] = [`Lease`], domaine [`LEASE_DOMAIN`]) : `epoch`, `device_id` (la clé du
//!   nœud, pour l'instant), `lease_id` (16 octets aléatoires par partie), `seq`, base et phase. Un bail reçu est
//!   accepté si l'émetteur de la connexion est le titulaire **et** son appareil, si le titulaire est membre, si
//!   l'époque est l'époque courante, si la signature est bonne et si la base n'est pas contredite par l'en-tête local ;
//! * `base_version` est une `Option` (le premier hébergement d'un groupe n'a aucune version) et le bail porte le
//!   numéro de cette base (`base_number`, 0 sans base), à titre d'affichage seulement : un bail dont la base est
//!   connue de nous et dont le numéro la contredit est rejeté, et le départage ne s'en sert jamais (voir
//!   [`rank_leases`] : phase, puis ascendance dans le graphe local, puis plus petite clé) ;
//! * un bail annoncé en phase [`LeasePhase::Starting`] dit « je prépare le serveur » (installation, démarrage d'un gros
//!   pack : 2 à 6 minutes) ; il est renouvelé comme les autres et devient `Ready` quand le serveur répond ;
//! * `LeaseRelease` porte `lease_id` et `last_seq` : le récepteur garde une **pierre tombale** `lease_id -> last_seq`
//!   [`TOMBSTONE_TTL`] et ignore tout bail de ce `lease_id` ; un bail remplacé par un autre `lease_id` du même titulaire
//!   est tombalisé de même (`Observed::Superseded`). Hors pierre tombale, un `seq` inférieur ou égal au dernier vu
//!   (même `lease_id`) est ignoré ;
//! * prise du bail en deux phases : [`Claim`] envoyé aux membres joignables, chaque témoin répond [`ClaimReply`]
//!   (verrou sur le **premier** claim, voir [`LeaseTable::claim`]), puis [`claim_outcome`] : majorité stricte des
//!   membres (le candidat compte pour lui) = bail **sûr**, sinon **au mieux** ;
//! * [`ProbeRound`] : la ronde `Hello` complète (réponses et injoignables) que reçoit [`decide`] ;
//! * auto-exclusion après veille : [`HostFence`] compare l'écart d'horloge murale et l'écart monotone (horloge
//!   injectable, [`Clock`]) ;
//! * un titulaire dont le tunnel échoue deux fois en trois minutes est ignoré quinze minutes (preuve de service,
//!   [`LeaseTable::report_unreachable`]) : un bail annoncé sans serveur derrière ne bloque pas le groupe.
//!
//! Un bail qui expire sans `LeaseRelease` est une partie **mal terminée** ([`LeaseTable::unclean`]) : elle reste
//! visible jusqu'à ce qu'elle soit résolue (un autre bail est accepté, `LeaseRelease` tardif, `revoke`).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::model::{hex_bytes, sign_domain, verify_domain, GroupId, GroupState, HearthSigner, Sig};
use super::version::{VersionGraph, VersionId, VersionPoint};
use crate::presence::PeerKey;

/// Durée de vie d'un bail chez le récepteur.
pub const LEASE_TTL_SECS: u32 = 90;
/// Durée de vie maximale acceptée d'un bail reçu.
pub const MAX_LEASE_TTL_SECS: u32 = 90;
/// Intervalle de renouvellement par l'hôte.
pub const LEASE_RENEW_INTERVAL: Duration = Duration::from_secs(30);
/// Attente d'une contre-annonce après avoir annoncé son propre bail.
pub const CONFLICT_WAIT: Duration = Duration::from_secs(2);
/// Nombre maximal de groupes suivis par la table.
pub const MAX_GROUPS: usize = 256;
/// Échecs de connexion au tunnel d'un titulaire, dans [`UNREACHABLE_WINDOW`], avant d'ignorer son bail.
pub const UNREACHABLE_FAILS: usize = 2;
/// Fenêtre de comptage des échecs de connexion.
pub const UNREACHABLE_WINDOW: Duration = Duration::from_secs(3 * 60);
/// Durée pendant laquelle le bail d'un titulaire injoignable est ignoré.
pub const UNREACHABLE_IGNORE: Duration = Duration::from_secs(15 * 60);
/// Nombre maximal de titulaires suivis pour la preuve de service (borne mémoire).
const MAX_UNREACHABLE: usize = 1024;
/// Domaine de signature d'un bail.
pub const LEASE_DOMAIN: &[u8] = b"cubelink/hearth/lease/1";
/// Format du bail signé.
pub const LEASE_FORMAT: u32 = 1;
/// Délai pour joindre un membre lors de la prise du bail (connexion, relais compris).
pub const CLAIM_TIMEOUT: Duration = Duration::from_secs(5);
/// Durée du verrou d'un témoin sur le premier claim, à défaut de premier renouvellement `Starting`.
pub const CLAIM_LOCK_TTL: Duration = Duration::from_secs(90);
/// Durée d'une pierre tombale `lease_id -> last_seq`.
pub const TOMBSTONE_TTL: Duration = Duration::from_secs(10 * 60);
/// Nombre maximal de pierres tombales gardées (borne mémoire).
const MAX_TOMBSTONES: usize = 4096;
/// Pierres tombales gardées par titulaire.
const MAX_TOMBSTONES_PER_HOLDER: usize = 64;
/// Écart entre horloge murale et horloge monotone au-delà duquel un hôte ne renouvelle pas (veille, horloge changée).
pub const FENCE_SKEW: Duration = Duration::from_secs(60);

/// Identifiant d'une partie : 16 octets aléatoires.
pub type LeaseId = [u8; 16];

/// Nouvel identifiant de bail.
pub fn new_lease_id() -> LeaseId {
    use rand::RngCore;
    let mut b = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut b);
    b
}

/// Où en est le titulaire : il prépare son serveur, ou il le sert.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LeasePhase {
    /// Installation et démarrage du serveur en cours : rien à rejoindre encore.
    Starting,
    /// Le serveur répond : on peut s'y connecter.
    #[default]
    Ready,
}

/// Bail d'hôte d'un groupe, signé par son titulaire (voir [`Lease::signed`] et [`Lease::verify`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lease {
    pub format: u32,
    pub group_id: GroupId,
    /// Époque du groupe au moment de l'annonce.
    pub epoch: u64,
    #[serde(with = "hex_bytes")]
    pub holder: PeerKey,
    /// Appareil du titulaire (la clé du nœud, égale à `holder` pour l'instant).
    #[serde(with = "hex_bytes")]
    pub device_id: PeerKey,
    /// Identifiant de la partie (un nonce par partie).
    #[serde(with = "hex_bytes")]
    pub lease_id: LeaseId,
    /// Version dont part la partie (`None` : premier hébergement du groupe).
    pub base_version: Option<VersionId>,
    /// Numéro de `base_version` (0 sans base).
    pub base_number: u64,
    /// Compteur croissant de l'hôte pour cette partie (1, 2, …).
    pub seq: u64,
    pub ttl_secs: u32,
    /// Phase de la partie.
    #[serde(default)]
    pub phase: LeasePhase,
    /// Signature du titulaire sur [`Lease::canonical`] (domaine [`LEASE_DOMAIN`]).
    pub sig: Sig,
}

/// Le bail tel qu'il circule : signé.
pub type SignedLease = Lease;

impl Lease {
    /// Premier bail d'une partie (`seq = 1`, durée de vie standard, nouveau `lease_id`, époque 0), **non signé** :
    /// voir [`Lease::at_epoch`] et [`Lease::signed`].
    pub fn new(group_id: GroupId, holder: PeerKey, base: Option<(VersionId, u64)>) -> Self {
        Self {
            format: LEASE_FORMAT,
            group_id,
            epoch: 0,
            holder,
            device_id: holder,
            lease_id: new_lease_id(),
            base_version: base.map(|b| b.0),
            base_number: base.map_or(0, |b| b.1),
            seq: 1,
            ttl_secs: LEASE_TTL_SECS,
            phase: LeasePhase::Ready,
            sig: Sig([0; 64]),
        }
    }

    /// Premier bail d'une partie dont le serveur n'est pas encore prêt.
    pub fn starting(group_id: GroupId, holder: PeerKey, base: Option<(VersionId, u64)>) -> Self {
        Self { phase: LeasePhase::Starting, ..Self::new(group_id, holder, base) }
    }

    /// Le même bail pour une autre époque.
    pub fn at_epoch(self, epoch: u64) -> Self {
        Self { epoch, ..self }
    }

    /// Le même bail pour une autre partie (`lease_id` imposé).
    pub fn with_lease_id(self, lease_id: LeaseId) -> Self {
        Self { lease_id, ..self }
    }

    /// Le même bail une fois le serveur prêt (`seq` continue). À signer de nouveau.
    pub fn ready(&self) -> Self {
        Self { phase: LeasePhase::Ready, seq: self.seq.saturating_add(1), ..self.clone() }
    }

    /// Bail suivant (même partie, `seq + 1`) à envoyer au prochain renouvellement. À signer de nouveau.
    pub fn renewed(&self) -> Self {
        Self { seq: self.seq.saturating_add(1), ..self.clone() }
    }

    /// Octets signés : tous les champs, dans un ordre et une largeur fixes (la signature ne dépend pas du JSON).
    pub fn canonical(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(256);
        b.extend_from_slice(&self.format.to_be_bytes());
        b.extend_from_slice(&self.group_id.0);
        b.extend_from_slice(&self.epoch.to_be_bytes());
        b.extend_from_slice(&self.holder);
        b.extend_from_slice(&self.device_id);
        b.extend_from_slice(&self.lease_id);
        match &self.base_version {
            Some(v) => {
                b.push(1);
                b.extend_from_slice(&v.0);
            }
            None => b.push(0),
        }
        b.extend_from_slice(&self.base_number.to_be_bytes());
        b.extend_from_slice(&self.seq.to_be_bytes());
        b.extend_from_slice(&self.ttl_secs.to_be_bytes());
        b.push(match self.phase {
            LeasePhase::Starting => 0,
            LeasePhase::Ready => 1,
        });
        b
    }

    /// Signe le bail (le signataire doit être le titulaire).
    pub fn signed(mut self, signer: &dyn HearthSigner) -> Self {
        self.sig = sign_domain(signer, LEASE_DOMAIN, &self.canonical());
        self
    }

    /// La signature est celle du titulaire sur ce contenu.
    pub fn verify(&self) -> bool {
        verify_domain(&self.holder, LEASE_DOMAIN, &self.canonical(), &self.sig)
    }

    /// Contrôle complet d'un bail reçu de `from` (identité de la connexion) : contenu (`validate`), émetteur =
    /// titulaire = appareil, signature.
    pub fn check_received(&self, state: &GroupState, from: &PeerKey) -> Result<(), LeaseError> {
        self.validate(state)?;
        if self.holder != *from || self.device_id != *from {
            return Err(LeaseError::SenderMismatch);
        }
        if !self.verify() {
            return Err(LeaseError::BadSignature);
        }
        Ok(())
    }

    /// Contrôle d'un bail reçu (le contenu vient d'un pair), sans le graphe des versions.
    pub fn validate(&self, state: &GroupState) -> Result<(), LeaseError> {
        if self.group_id != state.id {
            return Err(LeaseError::WrongGroup);
        }
        if self.format != LEASE_FORMAT {
            return Err(LeaseError::BadFormat);
        }
        if !state.is_member(&self.holder) {
            return Err(LeaseError::NotMember);
        }
        if self.epoch != state.epoch {
            return Err(LeaseError::WrongEpoch);
        }
        if self.ttl_secs == 0 || self.ttl_secs > MAX_LEASE_TTL_SECS {
            return Err(LeaseError::BadTtl);
        }
        if self.base_version.is_none() != (self.base_number == 0) {
            return Err(LeaseError::BadBase);
        }
        Ok(())
    }

    /// Confronte la base déclarée à notre graphe : un numéro qui contredit l'en-tête local de la base est rejeté. Une
    /// base que nous ne connaissons pas encore n'est pas rejetée (elle sera synchronisée) mais ne donne aucun avantage
    /// au départage.
    pub fn check_base(&self, graph: &VersionGraph) -> Result<(), LeaseError> {
        if let Some(v) = self.base_version.as_ref().and_then(|b| graph.get(b)) {
            if v.header.group_id != self.group_id || v.header.number != self.base_number {
                return Err(LeaseError::BadBase);
            }
        }
        Ok(())
    }

    fn ttl(&self) -> Duration {
        Duration::from_secs(u64::from(self.ttl_secs))
    }
}

/// Erreurs sur un bail reçu (messages affichables).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum LeaseError {
    #[error("bail d'un autre groupe")]
    WrongGroup,
    #[error("le titulaire du bail n'est pas membre du groupe")]
    NotMember,
    #[error("durée de vie du bail invalide")]
    BadTtl,
    #[error("version de base et numéro de base incohérents")]
    BadBase,
    #[error("le bail n'est pas annoncé par son titulaire")]
    SenderMismatch,
    #[error("signature du bail invalide")]
    BadSignature,
    #[error("format de bail inconnu")]
    BadFormat,
    #[error("le bail n'est pas de l'époque courante du groupe")]
    WrongEpoch,
    #[error("trop de groupes suivis")]
    TooManyGroups,
}

/// Effet d'un bail reçu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observed {
    /// Nouveau bail, ou renouvellement plus récent : retenu.
    Accepted,
    /// Répété ou ancien (`seq` pas plus grand pour le même `lease_id`), ou `lease_id` relâché : ignoré.
    Ignored,
    /// Le `lease_id` a été remplacé par une autre partie du même titulaire : ignoré.
    Superseded,
}

/// Ce qu'un témoin sait d'un `lease_id` fini.
#[derive(Debug, Clone, Copy)]
struct Tomb {
    last_seq: u64,
    at: Instant,
    superseded: bool,
}

/// Premier claim vu par ce témoin : verrou jusqu'au premier bail de ce `lease_id` (à défaut, [`CLAIM_LOCK_TTL`]).
#[derive(Debug, Clone)]
struct ClaimLock {
    claim: Claim,
    at: Instant,
}

#[derive(Debug, Clone)]
struct Entry {
    lease: Lease,
    received: Instant,
}

impl Entry {
    fn expired(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.received) >= self.lease.ttl()
    }
}

/// Baux connus, par groupe puis par titulaire. En mémoire seulement : après un redémarrage, les annonces des
/// membres en ligne la reconstruisent en moins de 30 s.
#[derive(Debug, Default)]
pub struct LeaseTable {
    groups: HashMap<GroupId, HashMap<PeerKey, Entry>>,
    /// Preuve de service : échecs de connexion au tunnel d'un titulaire, par groupe et titulaire.
    unreachable: HashMap<(GroupId, PeerKey), Unreachable>,
    /// Pierres tombales : `lease_id` fini, avec son dernier `seq`.
    tombstones: HashMap<(PeerKey, LeaseId), Tomb>,
    /// Verrou du premier claim, par groupe.
    locks: HashMap<GroupId, ClaimLock>,
}

#[derive(Debug, Default)]
struct Unreachable {
    fails: Vec<Instant>,
    ignored_until: Option<Instant>,
}

impl LeaseTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Enregistre un bail annoncé par `from` (identité de la connexion). Un bail qui expire chez nous `ttl_secs`
    /// après **cette** réception ; un autre titulaire dont le bail avait expiré (partie mal terminée) est considéré
    /// comme résolu : quelqu'un a pris le relais. Un `lease_id` relâché ou remplacé est ignoré ; pour un même
    /// `lease_id`, un `seq` inférieur ou égal au dernier vu est ignoré ; un nouveau `lease_id` du même titulaire
    /// remplace l'ancien, qui devient une pierre tombale.
    pub fn observe(
        &mut self,
        state: &GroupState,
        graph: &VersionGraph,
        from: &PeerKey,
        lease: Lease,
        now: Instant,
    ) -> Result<Observed, LeaseError> {
        lease.check_received(state, from)?;
        lease.check_base(graph)?;
        if !self.groups.contains_key(&lease.group_id) && self.groups.len() >= MAX_GROUPS {
            return Err(LeaseError::TooManyGroups);
        }
        self.prune_tombstones(now);
        if let Some(t) = self.tombstones.get(&(lease.holder, lease.lease_id)) {
            return Ok(if t.superseded { Observed::Superseded } else { Observed::Ignored });
        }
        let mut replaced = None;
        if let Some(known) = self.groups.get(&lease.group_id).and_then(|e| e.get(&lease.holder)) {
            if known.lease.lease_id == lease.lease_id {
                if lease.seq <= known.lease.seq {
                    return Ok(Observed::Ignored);
                }
            } else {
                replaced = Some((known.lease.lease_id, known.lease.seq));
            }
        }
        let holder = lease.holder;
        if self.locks.get(&lease.group_id).is_some_and(|l| l.claim.lease_id == lease.lease_id) {
            // Le premier bail de cette partie : il protège désormais lui-même, le verrou n'a plus lieu d'être.
            self.locks.remove(&lease.group_id);
        }
        let entries = self.groups.entry(lease.group_id).or_default();
        entries.insert(holder, Entry { lease, received: now });
        entries.retain(|h, e| *h == holder || !e.expired(now));
        if let Some((id, last_seq)) = replaced {
            self.tombstone(holder, id, last_seq, true, now);
        }
        Ok(Observed::Accepted)
    }

    fn prune_tombstones(&mut self, now: Instant) {
        self.tombstones.retain(|_, t| now.saturating_duration_since(t.at) < TOMBSTONE_TTL);
    }

    /// Pierre tombale liée au titulaire : un membre ne peut enterrer que ses propres `lease_id`. Quota par titulaire
    /// (le plus ancien part) puis plafond global.
    fn tombstone(&mut self, holder: PeerKey, id: LeaseId, last_seq: u64, superseded: bool, now: Instant) {
        self.prune_tombstones(now);
        let mine: Vec<_> = self.tombstones.iter().filter(|((h, k), _)| *h == holder && *k != id).map(|(k, t)| (*k, t.at)).collect();
        if mine.len() >= MAX_TOMBSTONES_PER_HOLDER {
            if let Some((oldest, _)) = mine.iter().min_by_key(|(_, at)| *at) {
                self.tombstones.remove(oldest);
            }
        }
        if self.tombstones.len() >= MAX_TOMBSTONES {
            if let Some(oldest) = self.tombstones.iter().min_by_key(|(_, t)| t.at).map(|(k, _)| *k) {
                self.tombstones.remove(&oldest);
            }
        }
        self.tombstones.insert((holder, id), Tomb { last_seq, at: now, superseded });
    }

    /// Dernier `seq` d'un `lease_id` fini de ce titulaire, s'il a une pierre tombale vivante.
    pub fn tombstone_of(&self, holder: &PeerKey, id: &LeaseId, now: Instant) -> Option<u64> {
        self.tombstones.get(&(*holder, *id)).filter(|t| now.saturating_duration_since(t.at) < TOMBSTONE_TTL).map(|t| t.last_seq)
    }

    /// `LeaseRelease { lease_id, last_seq }` reçu du titulaire : la partie s'est terminée proprement. Le `lease_id`
    /// devient une pierre tombale (un `Lease` tardif ne le ressuscite pas) ; le bail connu du titulaire n'est retiré que
    /// s'il porte ce `lease_id` (un relâchement périmé ne touche pas une partie plus récente) ; un verrou de claim du
    /// même `lease_id` tombe aussi. Rend vrai si un bail a été retiré.
    pub fn release_lease(&mut self, group: &GroupId, holder: &PeerKey, lease_id: &LeaseId, last_seq: u64, now: Instant) -> bool {
        self.tombstone(*holder, *lease_id, last_seq, false, now);
        if self.locks.get(group).is_some_and(|l| l.claim.lease_id == *lease_id && l.claim.holder == *holder) {
            self.locks.remove(group);
        }
        let same = self.groups.get(group).and_then(|e| e.get(holder)).is_some_and(|e| e.lease.lease_id == *lease_id);
        same && self.remove(group, holder)
    }

    /// Retire le bail d'un titulaire sans pierre tombale (il répond à une sonde sans rien héberger : un
    /// `LeaseRelease` s'est perdu).
    pub fn release(&mut self, group: &GroupId, holder: &PeerKey) -> bool {
        self.remove(group, holder)
    }

    /// Annule le bail d'un membre (`ContinueWithout`, ou partie mal terminée que le groupe a tranchée).
    pub fn revoke(&mut self, group: &GroupId, holder: &PeerKey) -> bool {
        self.remove(group, holder)
    }

    fn remove(&mut self, group: &GroupId, holder: &PeerKey) -> bool {
        let Some(entries) = self.groups.get_mut(group) else { return false };
        let removed = entries.remove(holder).is_some();
        if entries.is_empty() {
            self.groups.remove(group);
        }
        removed
    }

    /// Oublie tout ce qu'on sait d'un groupe (groupe quitté).
    pub fn forget_group(&mut self, group: &GroupId) {
        self.groups.remove(group);
        self.locks.remove(group);
        self.unreachable.retain(|(g, _), _| g != group);
    }

    /// Ce titulaire est-il ignoré (son tunnel a échoué deux fois de suite) ?
    fn ignored(&self, group: &GroupId, holder: &PeerKey, now: Instant) -> bool {
        self.unreachable.get(&(*group, *holder)).and_then(|u| u.ignored_until).is_some_and(|t| now < t)
    }

    /// Preuve de service : la connexion au tunnel de `holder` a échoué. Au deuxième échec en trois minutes, son bail est
    /// ignoré quinze minutes. Rend vrai quand le titulaire est maintenant ignoré.
    pub fn report_unreachable(&mut self, group: &GroupId, holder: &PeerKey, now: Instant) -> bool {
        if !self.unreachable.contains_key(&(*group, *holder)) && self.unreachable.len() >= MAX_UNREACHABLE {
            self.unreachable.retain(|_, u| u.ignored_until.is_some_and(|t| now < t));
            if self.unreachable.len() >= MAX_UNREACHABLE {
                return false;
            }
        }
        let u = self.unreachable.entry((*group, *holder)).or_default();
        u.fails.retain(|t| now.saturating_duration_since(*t) < UNREACHABLE_WINDOW);
        u.fails.push(now);
        if u.fails.len() >= UNREACHABLE_FAILS {
            u.fails.clear();
            u.ignored_until = Some(now + UNREACHABLE_IGNORE);
        }
        u.ignored_until.is_some_and(|t| now < t)
    }

    /// La connexion au tunnel de `holder` a réussi : son compte d'échecs repart de zéro.
    pub fn report_reachable(&mut self, group: &GroupId, holder: &PeerKey) {
        self.unreachable.remove(&(*group, *holder));
    }

    /// Baux valides (hors titulaires ignorés), triés par clé de titulaire. Pour le gagnant du départage, voir
    /// [`LeaseTable::ranked`].
    pub fn active(&self, group: &GroupId, now: Instant) -> Vec<Lease> {
        let mut v: Vec<Lease> = self
            .groups
            .get(group)
            .into_iter()
            .flat_map(HashMap::values)
            .filter(|e| !e.expired(now) && !self.ignored(group, &e.lease.holder, now))
            .map(|e| e.lease.clone())
            .collect();
        v.sort_by_key(|l| l.holder);
        v
    }

    /// Baux valides, le gagnant du départage en premier (voir [`rank_leases`]).
    pub fn ranked(&self, group: &GroupId, graph: &VersionGraph, now: Instant) -> Vec<Lease> {
        rank_leases(self.active(group, now), graph)
    }

    /// Un bail valide du groupe (le plus petit titulaire). Sans graphe : pour les cas où un seul bail compte.
    pub fn holder(&self, group: &GroupId, now: Instant) -> Option<Lease> {
        self.active(group, now).into_iter().next()
    }

    /// Le bail valide de ce titulaire, s'il y en a un.
    pub fn lease_of(&self, group: &GroupId, holder: &PeerKey, now: Instant) -> Option<Lease> {
        let e = self.groups.get(group)?.get(holder)?;
        (!e.expired(now) && !self.ignored(group, holder, now)).then(|| e.lease.clone())
    }

    /// Baux expirés sans `LeaseRelease` : parties mal terminées, triées par titulaire.
    pub fn unclean(&self, group: &GroupId, now: Instant) -> Vec<Lease> {
        let mut v: Vec<Lease> = self
            .groups
            .get(group)
            .into_iter()
            .flat_map(HashMap::values)
            .filter(|e| e.expired(now) && !self.ignored(group, &e.lease.holder, now))
            .map(|e| e.lease.clone())
            .collect();
        v.sort_by_key(|l| l.holder);
        v
    }

    /// Temps restant avant l'expiration du bail d'un titulaire (`None` s'il est inconnu ou déjà expiré).
    pub fn remaining(&self, group: &GroupId, holder: &PeerKey, now: Instant) -> Option<Duration> {
        let e = self.groups.get(group)?.get(holder)?;
        e.lease.ttl().checked_sub(now.saturating_duration_since(e.received)).filter(|d| !d.is_zero())
    }
}

/// Résultat du départage de deux hôtes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arbitration {
    /// Mon bail gagne : je garde la main.
    Keep,
    /// Un autre bail gagne : je rejoins sa partie.
    Yield { to: PeerKey },
}

/// Comparaison de deux versions de départ dans le graphe **local**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseOrder {
    /// La base de `a` descend de celle de `b` (ou `b` n'a pas de base et `a` en a une connue) : `a` gagne.
    AWins,
    /// Symétrique : `b` gagne.
    BWins,
    /// Deux branches : c'est une fourche (plus petite clé, fourche marquée).
    Fork,
    /// Bases égales, ou inconnues de nous : aucun avantage (plus petite clé).
    Equal,
}

/// Compare deux bases (jamais par le seul `number`).
pub fn compare_bases(a: &Option<VersionId>, b: &Option<VersionId>, graph: &VersionGraph) -> BaseOrder {
    match (a, b) {
        (Some(x), None) if graph.contains(x) => BaseOrder::AWins,
        (None, Some(y)) if graph.contains(y) => BaseOrder::BWins,
        (Some(x), Some(y)) if x != y && graph.contains(x) && graph.contains(y) => {
            if graph.is_ancestor(y, x) {
                BaseOrder::AWins
            } else if graph.is_ancestor(x, y) {
                BaseOrder::BWins
            } else {
                BaseOrder::Fork
            }
        }
        _ => BaseOrder::Equal,
    }
}

/// Départage de deux candidats : la base descendante gagne, sinon (fourche, ou bases égales) la plus petite clé.
/// Rend `(a gagne, fourche)`.
pub fn duel(a: (&PeerKey, &Option<VersionId>), b: (&PeerKey, &Option<VersionId>), graph: &VersionGraph) -> (bool, bool) {
    match compare_bases(a.1, b.1, graph) {
        BaseOrder::AWins => (true, false),
        BaseOrder::BWins => (false, false),
        BaseOrder::Fork => (a.0 < b.0, true),
        BaseOrder::Equal => (a.0 < b.0, false),
    }
}

/// Le bail `a` bat-il `b` ? D'abord la phase (un serveur prêt l'emporte sur un serveur en préparation), puis
/// [`duel`] (ascendance dans le graphe local, puis plus petite clé).
fn beats(a: &Lease, b: &Lease, graph: &VersionGraph) -> bool {
    if a.phase != b.phase {
        return a.phase == LeasePhase::Ready;
    }
    duel((&a.holder, &a.base_version), (&b.holder, &b.base_version), graph).0
}

/// Classe des baux d'un même groupe, le gagnant du départage en premier. Le résultat ne dépend pas de l'ordre
/// d'entrée (la liste est d'abord triée par clé ; à chaque rang, le meilleur reste).
pub fn rank_leases(mut leases: Vec<Lease>, graph: &VersionGraph) -> Vec<Lease> {
    leases.sort_by_key(|l| l.holder);
    leases.dedup_by_key(|l| l.holder);
    let mut out = Vec::with_capacity(leases.len());
    while !leases.is_empty() {
        let mut best = 0;
        for i in 1..leases.len() {
            if beats(&leases[i], &leases[best], graph) {
                best = i;
            }
        }
        out.push(leases.remove(best));
    }
    out
}

/// Départage mon bail et ceux annoncés par d'autres pour le même groupe (voir [`rank_leases`]). Un hôte dont le
/// serveur n'est pas encore prêt cède à tout serveur prêt, quelle que soit sa clé. Résultat indépendant de l'ordre
/// de `others`.
pub fn arbitrate(mine: &Lease, others: &[Lease], graph: &VersionGraph) -> Arbitration {
    arbitrate_marked(mine, others, graph).0
}

/// Comme [`arbitrate`], avec la **marque de fourche** : vrai si mon bail est sur une autre branche que l'un des
/// autres (à base égale, ou descendante, il n'y a pas de fourche). La session qui se termine dans ce cas est une
/// branche visible.
pub fn arbitrate_marked(mine: &Lease, others: &[Lease], graph: &VersionGraph) -> (Arbitration, bool) {
    let others: Vec<&Lease> = others.iter().filter(|l| l.group_id == mine.group_id && l.holder != mine.holder).collect();
    let fork = others.iter().any(|o| compare_bases(&mine.base_version, &o.base_version, graph) == BaseOrder::Fork);
    let mut all: Vec<Lease> = others.into_iter().cloned().collect();
    all.push(mine.clone());
    let verdict = match rank_leases(all, graph).first() {
        Some(w) if w.holder != mine.holder => Arbitration::Yield { to: w.holder },
        _ => Arbitration::Keep,
    };
    (verdict, fork)
}

// ---------------------------------------------------------------------------------------------------------
// Prise du bail en deux phases
// ---------------------------------------------------------------------------------------------------------

/// `LeaseClaim` : « je veux héberger ».
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Claim {
    pub group_id: GroupId,
    #[serde(with = "hex_bytes")]
    pub lease_id: LeaseId,
    #[serde(with = "hex_bytes")]
    pub holder: PeerKey,
    #[serde(with = "hex_bytes")]
    pub device_id: PeerKey,
    pub base_version: Option<VersionId>,
    /// Numéro de la base (0 sans base) : redondant, contrôlé contre l'en-tête local.
    pub base_number: u64,
    pub epoch: u64,
}

impl Claim {
    /// Le claim qui précède le bail `lease` (même `lease_id`, même base, même époque).
    pub fn of(lease: &Lease) -> Self {
        Self {
            group_id: lease.group_id,
            lease_id: lease.lease_id,
            holder: lease.holder,
            device_id: lease.device_id,
            base_version: lease.base_version,
            base_number: lease.base_number,
            epoch: lease.epoch,
        }
    }

    /// Contrôle d'un claim reçu de `from` : groupe, émetteur = titulaire = appareil, membre, époque courante, base
    /// cohérente avec notre graphe.
    pub fn check_received(&self, state: &GroupState, graph: &VersionGraph, from: &PeerKey) -> Result<(), LeaseError> {
        if self.group_id != state.id {
            return Err(LeaseError::WrongGroup);
        }
        if !state.is_member(&self.holder) {
            return Err(LeaseError::NotMember);
        }
        if self.holder != *from || self.device_id != *from {
            return Err(LeaseError::SenderMismatch);
        }
        if self.epoch != state.epoch {
            return Err(LeaseError::WrongEpoch);
        }
        if self.base_version.is_none() != (self.base_number == 0) {
            return Err(LeaseError::BadBase);
        }
        if let Some(v) = self.base_version.as_ref().and_then(|b| graph.get(b)) {
            if v.header.group_id != self.group_id || v.header.number != self.base_number {
                return Err(LeaseError::BadBase);
            }
        }
        Ok(())
    }
}

/// Raison d'une fin de partie (`LeaseRelease`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReleaseReason {
    /// Fin normale : le monde est publié.
    Stopped,
    /// L'hôte cède la main à un autre membre (ou perd un départage) sans rien publier.
    Handover,
    /// Arrêt de la machine : la version est publiée au démarrage suivant.
    Shutdown,
    /// Veille, ou auto-exclusion après veille.
    Suspend,
    /// Le serveur est mort.
    Crashed { mod_hint: Option<String> },
}

/// Ce que le témoin a déjà vu : un bail, ou le claim qu'il garde comme verrou.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    Lease,
    Claim,
}

/// `ClaimConflict` : le bail ou le claim déjà vu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimConflict {
    pub kind: ConflictKind,
    #[serde(with = "hex_bytes")]
    pub holder: PeerKey,
    #[serde(with = "hex_bytes")]
    pub lease_id: LeaseId,
    pub base_version: Option<VersionId>,
    pub base_number: u64,
}

/// Réponse d'un témoin à un [`Claim`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimReply {
    Ack,
    Conflict(ClaimConflict),
}

impl LeaseTable {
    /// Un témoin reçoit un [`Claim`] de `from`. Réponse :
    /// * `Conflict(Lease)` si un bail valide d'un autre titulaire est déjà connu (il faut le rejoindre) ;
    /// * `Conflict(Claim)` si ce témoin garde déjà le claim d'une autre partie comme verrou (le **premier** claim
    ///   garde le verrou jusqu'au premier bail de sa partie, à défaut [`CLAIM_LOCK_TTL`]) ;
    /// * sinon il prend le verrou et répond `Ack`.
    ///
    /// Le candidat prend aussi le verrou pour lui-même ([`LeaseTable::lock_own_claim`]) : il ne peut pas acquitter un
    /// concurrent après avoir lancé son propre claim.
    pub fn claim(
        &mut self,
        state: &GroupState,
        graph: &VersionGraph,
        from: &PeerKey,
        claim: &Claim,
        now: Instant,
    ) -> Result<ClaimReply, LeaseError> {
        claim.check_received(state, graph, from)?;
        Ok(self.lock_claim(graph, claim, now).map_or(ClaimReply::Ack, ClaimReply::Conflict))
    }

    /// Le candidat verrouille son propre claim avant de l'envoyer. `Some(conflit)` : un autre bail ou claim a déjà la
    /// main sur ce PC.
    pub fn lock_own_claim(&mut self, graph: &VersionGraph, claim: &Claim, now: Instant) -> Option<ClaimConflict> {
        self.lock_claim(graph, claim, now)
    }

    fn lock_claim(&mut self, graph: &VersionGraph, claim: &Claim, now: Instant) -> Option<ClaimConflict> {
        if let Some(l) = rank_leases(self.active(&claim.group_id, now), graph).into_iter().find(|l| l.holder != claim.holder) {
            return Some(ClaimConflict {
                kind: ConflictKind::Lease,
                holder: l.holder,
                lease_id: l.lease_id,
                base_version: l.base_version,
                base_number: l.base_number,
            });
        }
        if let Some(lock) = self.locks.get(&claim.group_id) {
            let fresh = now.saturating_duration_since(lock.at) < CLAIM_LOCK_TTL;
            if fresh && lock.claim.lease_id != claim.lease_id {
                let c = &lock.claim;
                return Some(ClaimConflict {
                    kind: ConflictKind::Claim,
                    holder: c.holder,
                    lease_id: c.lease_id,
                    base_version: c.base_version,
                    base_number: c.base_number,
                });
            }
            if fresh {
                return None;
            }
        }
        self.locks.insert(claim.group_id, ClaimLock { claim: claim.clone(), at: now });
        None
    }

    /// Le candidat renonce (conflit, échec) : son verrou tombe ici.
    pub fn unlock_claim(&mut self, group: &GroupId, lease_id: &LeaseId) {
        if self.locks.get(group).is_some_and(|l| l.claim.lease_id == *lease_id) {
            self.locks.remove(group);
        }
    }

    /// Claim gardé comme verrou par ce témoin (pour les tests et le diagnostic).
    pub fn locked_claim(&self, group: &GroupId, now: Instant) -> Option<Claim> {
        self.locks.get(group).filter(|l| now.saturating_duration_since(l.at) < CLAIM_LOCK_TTL).map(|l| l.claim.clone())
    }
}

/// Majorité stricte de `members` membres.
pub fn majority(members: usize) -> usize {
    members / 2 + 1
}

/// Issue de la première phase, vue du candidat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimOutcome {
    /// Les acquittements (le candidat compte pour lui-même) couvrent la majorité stricte des membres : bail **sûr**.
    Safe { acks: usize },
    /// Pas de majorité : bail **au mieux**. L'interface le dit et demande une confirmation explicite.
    BestEffort { acks: usize, needed: usize, members: usize },
    /// Un autre bail ou un claim qui l'emporte : le candidat cède (rejoindre, ou attendre).
    Conflict(ClaimConflict),
}

/// Issue d'un claim : `members` membres actifs du groupe, `remote_acks` acquittements reçus (moi exclu), `conflicts`
/// les `ClaimConflict` reçus. Un bail déjà connu d'un autre titulaire → conflit. Sinon la majorité → **sûr** (aucun
/// concurrent ne peut aussi l'avoir : les témoins n'acquittent que le premier claim). Sinon un claim concurrent qui
/// me bat par [`duel`] → conflit ; sinon **au mieux**.
pub fn claim_outcome(mine: &Claim, members: usize, remote_acks: usize, conflicts: &[ClaimConflict], graph: &VersionGraph) -> ClaimOutcome {
    if let Some(c) = conflicts.iter().filter(|c| c.kind == ConflictKind::Lease).min_by_key(|c| c.holder) {
        return ClaimOutcome::Conflict(c.clone());
    }
    let acks = remote_acks + 1;
    let needed = majority(members);
    if acks >= needed {
        return ClaimOutcome::Safe { acks };
    }
    let winner = conflicts
        .iter()
        .filter(|c| c.holder != mine.holder)
        .filter(|c| !duel((&mine.holder, &mine.base_version), (&c.holder, &c.base_version), graph).0)
        .min_by_key(|c| c.holder);
    match winner {
        Some(c) => ClaimOutcome::Conflict(c.clone()),
        None => ClaimOutcome::BestEffort { acks, needed, members },
    }
}

// ---------------------------------------------------------------------------------------------------------
// Auto-exclusion après veille
// ---------------------------------------------------------------------------------------------------------

/// Horloges injectables (tests purs de l'auto-exclusion).
pub trait Clock: Send + Sync {
    /// Horloge murale.
    fn wall(&self) -> SystemTime;
    /// Horloge monotone.
    fn mono(&self) -> Instant;
}

/// Les vraies horloges.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn wall(&self) -> SystemTime {
        SystemTime::now()
    }
    fn mono(&self) -> Instant {
        Instant::now()
    }
}

/// Horloges que l'on fait avancer à la main : `advance` avance les deux, `advance_wall` seulement la murale (veille :
/// l'horloge monotone ne compte pas le sommeil, la murale si).
#[derive(Debug)]
pub struct ManualClock {
    state: Mutex<(SystemTime, Instant)>,
}

impl Default for ManualClock {
    fn default() -> Self {
        Self { state: Mutex::new((SystemTime::now(), Instant::now())) }
    }
}

impl ManualClock {
    pub fn advance(&self, by: Duration) {
        let mut s = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        s.0 += by;
        s.1 += by;
    }
    pub fn advance_wall(&self, by: Duration) {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).0 += by;
    }
    pub fn rewind_wall(&self, by: Duration) {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).0 -= by;
    }
}

impl Clock for ManualClock {
    fn wall(&self) -> SystemTime {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).0
    }
    fn mono(&self) -> Instant {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).1
    }
}

/// Pourquoi un hôte ne doit pas renouveler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FenceReason {
    /// L'horloge murale et l'horloge monotone ne comptent pas le même temps (veille, horloge changée) : écart en secondes.
    ClockSkew { skew_secs: u64 },
    /// Plus de `ttl` s depuis le dernier renouvellement : notre bail a expiré chez les autres.
    Stale { elapsed_secs: u64 },
}

/// Verdict avant un renouvellement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FenceVerdict {
    Renew,
    /// Ne pas renouveler : une ronde `Hello` décide de la suite.
    Excluded(FenceReason),
}

/// Mémoire du dernier renouvellement d'un hôte (horloges murale et monotone).
#[derive(Debug, Clone, Copy)]
pub struct HostFence {
    wall: SystemTime,
    mono: Instant,
}

impl HostFence {
    pub fn new(clock: &dyn Clock) -> Self {
        Self { wall: clock.wall(), mono: clock.mono() }
    }

    /// Note un renouvellement.
    pub fn mark(&mut self, clock: &dyn Clock) {
        *self = Self::new(clock);
    }

    /// Écarts depuis le dernier renouvellement, en secondes : `(murale signée, monotone)`.
    pub fn gaps(&self, clock: &dyn Clock) -> (i64, u64) {
        let wall = match clock.wall().duration_since(self.wall) {
            Ok(d) => i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
            Err(e) => -i64::try_from(e.duration().as_secs()).unwrap_or(i64::MAX),
        };
        (wall, clock.mono().saturating_duration_since(self.mono).as_secs())
    }

    /// Faut-il renouveler ? Non si l'écart entre les deux horloges dépasse [`FENCE_SKEW`], ou si plus de
    /// [`LEASE_TTL_SECS`] s se sont écoulées (mesure monotone **ou** murale : un sommeil que la monotone ne compte pas
    /// est vu par la murale).
    pub fn check(&self, clock: &dyn Clock) -> FenceVerdict {
        let (wall, mono) = self.gaps(clock);
        let skew = wall.abs_diff(i64::try_from(mono).unwrap_or(i64::MAX));
        if skew > FENCE_SKEW.as_secs() {
            return FenceVerdict::Excluded(FenceReason::ClockSkew { skew_secs: skew });
        }
        let elapsed = mono.max(u64::try_from(wall).unwrap_or(0));
        if elapsed > u64::from(LEASE_TTL_SECS) {
            return FenceVerdict::Excluded(FenceReason::Stale { elapsed_secs: elapsed });
        }
        FenceVerdict::Renew
    }
}

/// Que faire après la ronde `Hello` d'un hôte exclu : un autre bail ou une version plus récente existe (arrêter, publier
/// la session comme branche sans bail), sinon reprendre avec un nouveau `lease_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FenceAction {
    StopAndFork,
    Resume,
}

/// Décision d'un hôte exclu d'après la ronde : un autre bail valide, ou un membre dont la version est plus avancée que
/// la pointe de ma partie (`tip`).
pub fn fence_action(other_leases: &[Lease], round: &ProbeRound, tip: Option<VersionPoint>) -> FenceAction {
    let newer = round.answers.iter().any(|m| m.latest > tip);
    if !other_leases.is_empty() || newer {
        FenceAction::StopAndFork
    } else {
        FenceAction::Resume
    }
}

/// Un membre joignable et la version la plus avancée qu'il dit avoir.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OnlineMember {
    pub key: PeerKey,
    pub latest: Option<VersionPoint>,
}

/// Ronde `Hello` complète avant une décision : qui a répondu (avec sa version la plus avancée), qui est injoignable.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProbeRound {
    pub answers: Vec<OnlineMember>,
    pub unreachable: Vec<PeerKey>,
    /// Durée de la ronde.
    pub elapsed: Duration,
}

/// Partie mal terminée d'un membre, avec son dernier point de reprise connu de nous.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UncleanLease {
    pub holder: PeerKey,
    pub last_checkpoint: Option<VersionId>,
}

/// Entrées de la décision du bouton Jouer.
#[derive(Debug, Clone)]
pub struct DecideInput {
    pub me: PeerKey,
    /// Ma tête locale (`None` : je n'ai aucune version).
    pub my_latest: Option<VersionPoint>,
    /// Baux valides connus (le mien est ignoré), le gagnant du départage en premier (voir [`rank_leases`]).
    pub leases: Vec<Lease>,
    /// Parties mal terminées connues.
    pub unclean: Vec<UncleanLease>,
    /// Membres qui ont répondu à la ronde (moi exclu ; le cas échéant ignoré).
    pub online: Vec<OnlineMember>,
    /// Membres qui n'ont pas répondu à la ronde.
    pub unreachable: Vec<PeerKey>,
    /// Une autre source joignable (gardien du réseau de relais, plus tard).
    pub other_source: bool,
    /// Je suis le seul membre du groupe : rien à attendre d'un autre.
    pub solo: bool,
}

/// Ce que fait le bouton Jouer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Une partie est en cours chez `holder` : s'y connecter.
    Join(PeerKey),
    /// Un membre a une version plus récente : la récupérer d'abord, puis redécider.
    Sync(PeerKey),
    /// Démarrer le serveur sur ce PC (annoncer le bail, attendre `CONFLICT_WAIT`, voir [`arbitrate`]).
    Host,
    /// Aucun membre ni source joignable : jeu hors ligne interdit.
    Blocked,
    /// La partie de `holder` ne s'est pas terminée proprement : le groupe choisit (jouer depuis son dernier
    /// point de reprise, attendre, ou reprendre depuis la dernière version connue).
    Unclean { holder: PeerKey, last_checkpoint: Option<VersionId> },
}

/// Décision au clic sur Jouer, dans cet ordre :
/// 1. un bail valide d'un autre membre : **Rejoindre** (en cas de baux concurrents, le premier de la liste, donc le
///    gagnant du départage) ;
/// 2. un membre joignable a une version plus récente que la mienne : **Synchroniser** (le plus avancé, puis la
///    plus petite clé) ;
/// 3. aucun membre ni source joignable (et groupe non solo) : **Bloqué** ;
/// 4. une partie mal terminée : **Unclean** (le plus petit titulaire s'il y en a plusieurs) ;
/// 5. sinon **Héberger**.
pub fn decide(input: &DecideInput) -> Decision {
    let me = input.me;
    if let Some(l) = input.leases.iter().find(|l| l.holder != me) {
        return Decision::Join(l.holder);
    }
    let online: Vec<&OnlineMember> = input.online.iter().filter(|m| m.key != me).collect();
    let newer = online
        .iter()
        .filter(|m| m.latest > input.my_latest)
        .max_by(|a, b| a.latest.cmp(&b.latest).then(b.key.cmp(&a.key)));
    if let Some(m) = newer {
        return Decision::Sync(m.key);
    }
    if online.is_empty() && !input.other_source && !input.solo {
        return Decision::Blocked;
    }
    if let Some(u) = input.unclean.iter().filter(|u| u.holder != me).min_by_key(|u| u.holder) {
        return Decision::Unclean { holder: u.holder, last_checkpoint: u.last_checkpoint };
    }
    Decision::Host
}

/// Construit les entrées de [`decide`] depuis l'état du groupe, la table des baux et le graphe des versions,
/// puis décide. `round` : la ronde `Hello` qui vient d'être faite (réponses avec la version la plus avancée annoncée, et
/// injoignables), jamais le seul cache.
pub fn decide_from(
    state: &GroupState,
    graph: &VersionGraph,
    table: &LeaseTable,
    me: &PeerKey,
    round: &ProbeRound,
    other_source: bool,
    now: Instant,
) -> Decision {
    let input = DecideInput {
        me: *me,
        my_latest: graph.latest_by_number(state).map(|v| v.header.point()),
        leases: table.ranked(&state.id, graph, now),
        unclean: table
            .unclean(&state.id, now)
            .into_iter()
            .map(|l| UncleanLease {
                holder: l.holder,
                last_checkpoint: graph.latest_checkpoint_by(state, &l.holder).map(|v| v.id()),
            })
            .collect(),
        online: round.answers.iter().filter(|m| state.is_member(&m.key)).copied().collect(),
        unreachable: round.unreachable.iter().filter(|k| state.is_member(k)).copied().collect(),
        other_source,
        solo: state.members.len() <= 1,
    };
    decide(&input)
}

#[cfg(test)]
mod tests {
    use super::super::model::testutil::{group, key};
    use super::super::model::HearthSigner;
    use super::super::version::{SignedVersion, VersionHeader, VersionKind};
    use super::*;
    use proptest::prelude::*;

    fn pk(seed: u8) -> PeerKey {
        key(seed).peer_key()
    }

    fn lease(st: &GroupState, holder: u8, base: Option<(u8, u64)>) -> Lease {
        Lease::new(st.id, pk(holder), base.map(|(b, n)| (VersionId([b; 32]), n))).signed(&key(holder))
    }

    /// Le bail suivant de la même partie, signé de nouveau par son titulaire.
    fn next(l: &Lease, holder: u8) -> Lease {
        l.renewed().signed(&key(holder))
    }

    fn obs(t: &mut LeaseTable, st: &GroupState, from: &PeerKey, l: Lease, now: Instant) -> Result<Observed, LeaseError> {
        t.observe(st, &VersionGraph::new(), from, l, now)
    }

    /// Une chaîne de `n` versions (numéros 1 à n) signées par le membre 2.
    fn chain(st: &GroupState, n: u64) -> (VersionGraph, Vec<VersionId>) {
        let mut g = VersionGraph::new();
        let mut ids: Vec<VersionId> = Vec::new();
        for number in 1..=n {
            let h = VersionHeader {
                group_id: st.id,
                number,
                parent: ids.last().copied(),
                epoch: 0,
                author: pk(2),
                pack_hash: "ab".repeat(32),
                created_at: number,
                played_secs: 0,
                kind: VersionKind::Checkpoint,
                collection: [number as u8; 32],
                total_size: 1,
            };
            let v = SignedVersion::sign(&key(2), h).unwrap();
            ids.push(v.id());
            g.insert(st, v).unwrap();
        }
        (g, ids)
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn input(me: u8) -> DecideInput {
        DecideInput {
            me: pk(me),
            my_latest: None,
            leases: vec![],
            unclean: vec![],
            online: vec![],
            unreachable: vec![],
            other_source: false,
            solo: false,
        }
    }

    fn pt(epoch: u64, number: u64) -> Option<VersionPoint> {
        Some(VersionPoint { epoch, number })
    }

    fn online(seed: u8, latest: Option<VersionPoint>) -> OnlineMember {
        OnlineMember { key: pk(seed), latest }
    }

    #[test]
    fn lease_validation() {
        let st = group(&[1, 2, 3]);
        assert!(lease(&st, 2, None).validate(&st).is_ok());
        assert!(lease(&st, 2, Some((5, 4))).validate(&st).is_ok());
        assert_eq!(lease(&st, 9, None).validate(&st), Err(LeaseError::NotMember));
        let mut l = lease(&st, 2, None);
        l.group_id = GroupId([9; 16]);
        assert_eq!(l.validate(&st), Err(LeaseError::WrongGroup));
        let mut l = lease(&st, 2, None);
        l.ttl_secs = 0;
        assert_eq!(l.validate(&st), Err(LeaseError::BadTtl));
        l.ttl_secs = MAX_LEASE_TTL_SECS + 1;
        assert_eq!(l.validate(&st), Err(LeaseError::BadTtl));
        l.ttl_secs = MAX_LEASE_TTL_SECS;
        assert!(l.validate(&st).is_ok());
        // Base sans numéro, numéro sans base.
        let mut l = lease(&st, 2, Some((5, 4)));
        l.base_number = 0;
        assert_eq!(l.validate(&st), Err(LeaseError::BadBase));
        let mut l = lease(&st, 2, None);
        l.base_number = 3;
        assert_eq!(l.validate(&st), Err(LeaseError::BadBase));
    }

    #[test]
    fn lease_json_roundtrip_and_renewal() {
        let st = group(&[1, 2]);
        let l = lease(&st, 2, Some((7, 3)));
        let json = serde_json::to_string(&l).unwrap();
        assert_eq!(serde_json::from_str::<Lease>(&json).unwrap(), l);
        assert!(serde_json::from_str::<Lease>(&json.replace("\"seq\"", "\"zz\"")).is_err());
        let r = l.renewed();
        assert_eq!((r.seq, r.holder, r.base_number), (2, l.holder, 3));
        assert_eq!(l.ttl_secs, 90);
        assert_eq!(LEASE_RENEW_INTERVAL, secs(30));
        assert_eq!(CONFLICT_WAIT, secs(2));
    }

    #[test]
    fn observe_checks_sender_and_membership() {
        let st = group(&[1, 2, 3]);
        let mut t = LeaseTable::new();
        let now = Instant::now();
        let l = lease(&st, 2, None);
        assert_eq!(obs(&mut t, &st, &pk(3), l.clone(), now), Err(LeaseError::SenderMismatch));
        assert_eq!(obs(&mut t, &st, &pk(9), lease(&st, 9, None), now), Err(LeaseError::NotMember));
        assert!(t.holder(&st.id, now).is_none());
        assert_eq!(obs(&mut t, &st, &pk(2), l, now), Ok(Observed::Accepted));
        assert_eq!(t.holder(&st.id, now).unwrap().holder, pk(2));
    }

    #[test]
    fn lease_expires_relative_to_reception() {
        let st = group(&[1, 2]);
        let mut t = LeaseTable::new();
        let t0 = Instant::now();
        let l1 = lease(&st, 2, None);
        obs(&mut t, &st, &pk(2), l1.clone(), t0).unwrap();
        assert!(t.holder(&st.id, t0 + secs(89)).is_some());
        assert_eq!(t.remaining(&st.id, &pk(2), t0 + secs(60)), Some(secs(30)));
        assert!(t.holder(&st.id, t0 + secs(90)).is_none());
        assert_eq!(t.remaining(&st.id, &pk(2), t0 + secs(90)), None);
        // Un renouvellement reçu à 60 s repousse l'expiration de 90 s à partir de SA réception.
        let l2 = next(&l1, 2);
        assert_eq!(obs(&mut t, &st, &pk(2), l2, t0 + secs(60)), Ok(Observed::Accepted));
        assert!(t.holder(&st.id, t0 + secs(149)).is_some());
        assert!(t.holder(&st.id, t0 + secs(150)).is_none());
    }

    #[test]
    fn seq_rules() {
        let st = group(&[1, 2]);
        let mut t = LeaseTable::new();
        let t0 = Instant::now();
        let mut l = lease(&st, 2, None);
        l.seq = 5;
        let l = l.signed(&key(2));
        obs(&mut t, &st, &pk(2), l.clone(), t0).unwrap();
        // Même seq ou plus ancien, même partie : ignoré, sans prolonger la durée de vie.
        assert_eq!(obs(&mut t, &st, &pk(2), l.clone(), t0 + secs(30)), Ok(Observed::Ignored));
        let mut old = l.clone();
        old.seq = 2;
        let old = old.signed(&key(2));
        assert_eq!(obs(&mut t, &st, &pk(2), old.clone(), t0 + secs(30)), Ok(Observed::Ignored));
        assert!(t.holder(&st.id, t0 + secs(90)).is_none());
        // Même expiré, un seq inférieur ou égal du même lease_id reste ignoré (rejeu)...
        assert_eq!(obs(&mut t, &st, &pk(2), old, t0 + secs(100)), Ok(Observed::Ignored));
        // ... et un seq supérieur est accepté.
        assert_eq!(obs(&mut t, &st, &pk(2), next(&l, 2), t0 + secs(100)), Ok(Observed::Accepted));
        // Un hôte qui redémarre prend un nouveau lease_id : sa seq repart de 1 et il est accepté.
        let restart = lease(&st, 2, None);
        assert_ne!(restart.lease_id, l.lease_id);
        assert_eq!(obs(&mut t, &st, &pk(2), restart.clone(), t0 + secs(110)), Ok(Observed::Accepted));
        assert!(t.holder(&st.id, t0 + secs(111)).is_some());
        // L'ancienne partie est une pierre tombale : son renouvellement est refusé (Superseded).
        assert_eq!(obs(&mut t, &st, &pk(2), next(&next(&l, 2), 2), t0 + secs(120)), Ok(Observed::Superseded));
        assert_eq!(t.tombstone_of(&l.holder, &l.lease_id, t0 + secs(120)), Some(6));
        assert_eq!(t.holder(&st.id, t0 + secs(121)).unwrap().lease_id, restart.lease_id);
    }

    #[test]
    fn release_and_revoke() {
        let st = group(&[1, 2, 3]);
        let mut t = LeaseTable::new();
        let t0 = Instant::now();
        obs(&mut t, &st, &pk(2), lease(&st, 2, None), t0).unwrap();
        assert!(t.release(&st.id, &pk(2)));
        assert!(!t.release(&st.id, &pk(2)));
        assert!(t.holder(&st.id, t0).is_none());
        assert!(t.unclean(&st.id, t0 + secs(500)).is_empty(), "une fin propre n'est pas une partie mal terminée");
        obs(&mut t, &st, &pk(3), lease(&st, 3, None), t0).unwrap();
        assert!(t.revoke(&st.id, &pk(3)));
        assert!(t.active(&st.id, t0).is_empty());
        obs(&mut t, &st, &pk(3), lease(&st, 3, None), t0).unwrap();
        t.forget_group(&st.id);
        assert!(t.active(&st.id, t0).is_empty());
    }

    #[test]
    fn expired_lease_is_unclean_until_resolved() {
        let st = group(&[1, 2, 3]);
        let mut t = LeaseTable::new();
        let t0 = Instant::now();
        obs(&mut t, &st, &pk(2), lease(&st, 2, Some((4, 2))), t0).unwrap();
        assert!(t.unclean(&st.id, t0 + secs(89)).is_empty());
        let u = t.unclean(&st.id, t0 + secs(91));
        assert_eq!(u.len(), 1);
        assert_eq!(u[0].holder, pk(2));
        // Elle reste visible tant que personne ne l'a résolue.
        assert_eq!(t.unclean(&st.id, t0 + secs(100_000)).len(), 1);
        // Un autre membre prend le relais : la partie mal terminée est résolue.
        obs(&mut t, &st, &pk(3), lease(&st, 3, Some((4, 2))), t0 + secs(200)).unwrap();
        assert!(t.unclean(&st.id, t0 + secs(201)).is_empty());
        assert_eq!(t.active(&st.id, t0 + secs(201)).len(), 1);
        // Un LeaseRelease tardif du titulaire efface aussi.
        let mut t2 = LeaseTable::new();
        obs(&mut t2, &st, &pk(2), lease(&st, 2, None), t0).unwrap();
        t2.release(&st.id, &pk(2));
        assert!(t2.unclean(&st.id, t0 + secs(200)).is_empty());
    }

    #[test]
    fn concurrent_leases_order_by_tiebreak() {
        let st = group(&[1, 2, 3]);
        let mut t = LeaseTable::new();
        let t0 = Instant::now();
        obs(&mut t, &st, &pk(3), lease(&st, 3, Some((1, 5))), t0).unwrap();
        obs(&mut t, &st, &pk(2), lease(&st, 2, Some((1, 7))), t0).unwrap();
        obs(&mut t, &st, &pk(1), lease(&st, 1, Some((1, 5))), t0).unwrap();
        let order: Vec<PeerKey> = t.active(&st.id, t0).iter().map(|l| l.holder).collect();
        let mut keys = vec![pk(1), pk(2), pk(3)];
        keys.sort();
        assert_eq!(order, keys, "sans graphe, la plus petite clé d'abord");
        assert_eq!(t.holder(&st.id, t0).unwrap().holder, keys[0]);
    }

    #[test]
    fn un_bail_dont_la_base_contredit_le_graphe_local_est_rejete() {
        let st = group(&[1, 2, 3]);
        let (g, ids) = chain(&st, 3);
        let mut t = LeaseTable::new();
        let t0 = Instant::now();
        // Numéro déclaré faux pour une base connue : rejeté (on ne gagne pas un départage avec un faux numéro).
        let mut faux = Lease::new(st.id, pk(2), Some((ids[2], 3)));
        faux.base_number = 1;
        let faux = faux.signed(&key(2));
        assert_eq!(t.observe(&st, &g, &pk(2), faux, t0), Err(LeaseError::BadBase));
        assert!(t.active(&st.id, t0).is_empty());
        // Le bon numéro passe ; une base que nous ne connaissons pas encore aussi (elle sera synchronisée).
        assert_eq!(t.observe(&st, &g, &pk(2), Lease::new(st.id, pk(2), Some((ids[2], 3))).signed(&key(2)), t0), Ok(Observed::Accepted));
        assert_eq!(
            t.observe(&st, &g, &pk(3), Lease::new(st.id, pk(3), Some((VersionId([0xee; 32]), 9))).signed(&key(3)), t0),
            Ok(Observed::Accepted)
        );
    }

    #[test]
    fn un_ttl_au_dela_de_90_secondes_est_refuse() {
        let st = group(&[1, 2]);
        let mut l = lease(&st, 2, None);
        l.ttl_secs = 91;
        assert_eq!(l.validate(&st), Err(LeaseError::BadTtl));
        l.ttl_secs = 90;
        assert!(l.validate(&st).is_ok());
    }

    #[test]
    fn preuve_de_service_ignore_un_titulaire_injoignable_quinze_minutes() {
        let st = group(&[1, 2, 3]);
        let mut t = LeaseTable::new();
        let t0 = Instant::now();
        obs(&mut t, &st, &pk(2), lease(&st, 2, None), t0).unwrap();
        // Un premier échec ne suffit pas.
        assert!(!t.report_unreachable(&st.id, &pk(2), t0 + secs(5)));
        assert!(t.holder(&st.id, t0 + secs(6)).is_some());
        // Le deuxième dans les trois minutes : le bail est ignoré, même renouvelé.
        assert!(t.report_unreachable(&st.id, &pk(2), t0 + secs(60)));
        assert!(t.holder(&st.id, t0 + secs(61)).is_none());
        let l1 = next(&lease(&st, 2, None), 2);
        // (l'ancien lease_id n'est pas connu de la table : la première annonce est acceptée)
        obs(&mut t, &st, &pk(2), l1.clone(), t0 + secs(70)).unwrap();
        assert!(t.holder(&st.id, t0 + secs(71)).is_none());
        assert!(t.unclean(&st.id, t0 + secs(500)).is_empty(), "un titulaire ignoré n'est pas une partie mal terminée");
        // Quinze minutes plus tard, il compte de nouveau.
        obs(&mut t, &st, &pk(2), next(&next(&l1, 2), 2), t0 + secs(60 + 15 * 60)).unwrap();
        assert!(t.holder(&st.id, t0 + secs(61 + 15 * 60)).is_some());
        // Deux échecs espacés de plus de trois minutes ne comptent pas ensemble ; une réussite remet à zéro.
        let mut u = LeaseTable::new();
        obs(&mut u, &st, &pk(3), lease(&st, 3, None), t0).unwrap();
        assert!(!u.report_unreachable(&st.id, &pk(3), t0));
        assert!(!u.report_unreachable(&st.id, &pk(3), t0 + secs(200)));
        u.report_reachable(&st.id, &pk(3));
        assert!(!u.report_unreachable(&st.id, &pk(3), t0 + secs(201)));
    }

    #[test]
    fn une_phase_starting_est_un_bail_et_devient_ready() {
        let st = group(&[1, 2]);
        let s = Lease::starting(st.id, pk(2), None);
        assert_eq!(s.phase, LeasePhase::Starting);
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Lease>(&json).unwrap(), s);
        // Un ancien message sans phase est un bail prêt.
        let legacy = json.replace(",\"phase\":\"starting\"", "");
        assert_eq!(serde_json::from_str::<Lease>(&legacy).unwrap().phase, LeasePhase::Ready);
        let r = s.renewed();
        assert_eq!((r.phase, r.seq), (LeasePhase::Starting, 2));
        let ready = r.ready();
        assert_eq!((ready.phase, ready.seq), (LeasePhase::Ready, 3));
    }

    #[test]
    fn group_cap() {
        let st = group(&[1, 2]);
        let mut t = LeaseTable::new();
        let t0 = Instant::now();
        for i in 0..MAX_GROUPS {
            t.groups.insert(GroupId([(i % 256) as u8; 16]), HashMap::new());
        }
        // `st.id` = [7; 16] est déjà dans la table (i = 7) : accepté ; un identifiant inédit ne l'est pas.
        assert!(obs(&mut t, &st, &pk(2), lease(&st, 2, None), t0).is_ok());
        let mut t = LeaseTable::new();
        for i in 0..MAX_GROUPS {
            let mut id = [0u8; 16];
            id[..8].copy_from_slice(&(i as u64 + 1000).to_be_bytes());
            t.groups.insert(GroupId(id), HashMap::new());
        }
        assert_eq!(obs(&mut t, &st, &pk(2), lease(&st, 2, None), t0), Err(LeaseError::TooManyGroups));
    }

    #[test]
    fn arbitrate_phase_then_ancestry_then_key() {
        let st = group(&[1, 2, 3, 4]);
        let (g, ids) = chain(&st, 3);
        let at = |holder: u8, n: usize| Lease::new(st.id, pk(holder), Some((ids[n - 1], n as u64)));
        let none = VersionGraph::new();
        // La partie qui part de la version la plus avancée l'emporte, quelle que soit la clé.
        let (a, b) = (at(1, 3), at(2, 2));
        let c = Lease::new(st.id, pk(3), None);
        assert_eq!(arbitrate(&b, &[a.clone(), c.clone()], &g), Arbitration::Yield { to: pk(1) });
        assert_eq!(arbitrate(&a, &[b.clone(), c.clone()], &g), Arbitration::Keep);
        let hi = at(4, 3);
        assert_eq!(arbitrate(&hi, std::slice::from_ref(&b), &g), Arbitration::Keep, "la base la plus avancée gagne malgré la plus grande clé");
        // Un bail sans base ne gagne jamais contre un bail qui en a une, même avec la plus petite clé.
        let sans_base = Lease::new(st.id, pk(1), None);
        assert_eq!(arbitrate(&sans_base, std::slice::from_ref(&b), &g), Arbitration::Yield { to: pk(2) });
        assert_eq!(arbitrate(&b, &[sans_base], &g), Arbitration::Keep);
        // À base égale, la plus petite clé.
        let (lo, hi) = if pk(1) < pk(4) { (at(1, 3), at(4, 3)) } else { (at(4, 3), at(1, 3)) };
        assert_eq!(arbitrate(&lo, std::slice::from_ref(&hi), &g), Arbitration::Keep);
        assert_eq!(arbitrate(&hi, std::slice::from_ref(&lo), &g), Arbitration::Yield { to: lo.holder });
        // Un serveur prêt l'emporte sur un serveur en préparation, même avec une plus grande clé.
        let starting = Lease::starting(st.id, pk(1), Some((ids[2], 3)));
        let ready = Lease::new(st.id, pk(4), Some((ids[2], 3)));
        assert_eq!(arbitrate(&starting, std::slice::from_ref(&ready), &g), Arbitration::Yield { to: pk(4) });
        assert_eq!(arbitrate(&ready, &[starting], &g), Arbitration::Keep);
        // Bases inconnues de nous : aucun avantage, plus petite clé.
        let (x, y) = (Lease::new(st.id, pk(2), Some((VersionId([9; 32]), 99))), Lease::new(st.id, pk(3), Some((VersionId([8; 32]), 1))));
        assert_eq!(arbitrate(&y, std::slice::from_ref(&x), &none), Arbitration::Yield { to: pk(2) });
        // Seul, son propre bail dans la liste, ou bail d'un autre groupe : je garde.
        assert_eq!(arbitrate(&a, &[], &g), Arbitration::Keep);
        assert_eq!(arbitrate(&a, std::slice::from_ref(&a), &g), Arbitration::Keep);
        let mut foreign = c;
        foreign.group_id = GroupId([1; 16]);
        assert_eq!(arbitrate(&a, &[foreign], &g), Arbitration::Keep);
    }

    #[test]
    fn decide_join_when_other_holds_lease() {
        let st = group(&[1, 2, 3]);
        let mut i = input(1);
        i.leases = vec![lease(&st, 2, Some((1, 3)))];
        i.online = vec![online(2, pt(0, 3))];
        i.my_latest = pt(0, 1); // même plus ancien : on rejoint d'abord (la synchro viendra à la fin)
        assert_eq!(decide(&i), Decision::Join(pk(2)));
        // Mon propre bail ne compte pas.
        let mut j = input(1);
        j.leases = vec![lease(&st, 1, None)];
        j.online = vec![online(2, None)];
        assert_eq!(decide(&j), Decision::Host);
        // Deux baux : le gagnant du départage.
        let mut k = input(1);
        k.leases = vec![lease(&st, 3, Some((1, 4))), lease(&st, 2, Some((1, 5)))];
        assert_eq!(decide(&k), Decision::Join(pk(3)), "le premier de la liste classée");
        // Un bail l'emporte sur une partie mal terminée et sur « bloqué ».
        let mut m = input(1);
        m.leases = vec![lease(&st, 2, None)];
        m.unclean = vec![UncleanLease { holder: pk(3), last_checkpoint: None }];
        assert_eq!(decide(&m), Decision::Join(pk(2)));
    }

    #[test]
    fn decide_sync_before_host() {
        let mut i = input(1);
        i.my_latest = pt(0, 3);
        i.online = vec![online(2, pt(0, 3)), online(3, pt(0, 5)), online(4, pt(0, 5))];
        let expect = if pk(3) < pk(4) { pk(3) } else { pk(4) };
        assert_eq!(decide(&i), Decision::Sync(expect));
        // Je n'ai rien, un membre a quelque chose.
        let mut j = input(1);
        j.online = vec![online(2, pt(0, 1))];
        assert_eq!(decide(&j), Decision::Sync(pk(2)));
        // Une époque plus récente l'emporte sur un numéro plus grand.
        let mut k = input(1);
        k.my_latest = pt(0, 9);
        k.online = vec![online(2, pt(1, 3))];
        assert_eq!(decide(&k), Decision::Sync(pk(2)));
        // Aucun n'est plus avancé : on héberge.
        let mut h = input(1);
        h.my_latest = pt(0, 5);
        h.online = vec![online(2, pt(0, 5)), online(3, pt(0, 4)), online(4, None)];
        assert_eq!(decide(&h), Decision::Host);
        // La synchro passe avant « partie mal terminée » (le point de reprise est peut-être chez lui).
        h.online = vec![online(2, pt(0, 6))];
        h.unclean = vec![UncleanLease { holder: pk(3), last_checkpoint: None }];
        assert_eq!(decide(&h), Decision::Sync(pk(2)));
    }

    #[test]
    fn decide_blocked_offline() {
        let i = input(1);
        assert_eq!(decide(&i), Decision::Blocked);
        let mut j = input(1);
        j.my_latest = pt(0, 4);
        assert_eq!(decide(&j), Decision::Blocked, "même avec la dernière version : pas de jeu hors ligne");
        // Moi seul « en ligne » ne compte pas.
        j.online = vec![online(1, pt(0, 4))];
        assert_eq!(decide(&j), Decision::Blocked);
        // Une autre source ou un groupe solo débloquent.
        j.other_source = true;
        assert_eq!(decide(&j), Decision::Host);
        let mut s = input(1);
        s.solo = true;
        assert_eq!(decide(&s), Decision::Host);
        // Bloqué passe avant « mal terminée » : personne n'est joignable pour reprendre.
        let mut u = input(1);
        u.unclean = vec![UncleanLease { holder: pk(2), last_checkpoint: None }];
        assert_eq!(decide(&u), Decision::Blocked);
    }

    #[test]
    fn decide_unclean() {
        let cp = Some(VersionId([8; 32]));
        let mut i = input(1);
        i.my_latest = pt(0, 4);
        i.online = vec![online(3, pt(0, 4))];
        i.unclean = vec![UncleanLease { holder: pk(2), last_checkpoint: cp }];
        assert_eq!(decide(&i), Decision::Unclean { holder: pk(2), last_checkpoint: cp });
        // Ma propre partie mal terminée ne bloque pas mon prochain lancement.
        i.unclean = vec![UncleanLease { holder: pk(1), last_checkpoint: cp }];
        assert_eq!(decide(&i), Decision::Host);
    }

    #[test]
    fn decide_from_uses_table_and_graph() {
        let st = group(&[1, 2, 3]);
        let mut g = VersionGraph::new();
        let mut h = VersionHeader {
            group_id: st.id,
            number: 1,
            parent: None,
            epoch: 0,
            author: pk(2),
            pack_hash: "ab".repeat(32),
            created_at: 1,
            played_secs: 0,
            kind: VersionKind::Checkpoint,
            collection: [1; 32],
            total_size: 1,
        };
        let v1 = SignedVersion::sign(&key(2), h.clone()).unwrap();
        let v1id = v1.id();
        g.insert(&st, v1).unwrap();
        h.number = 2;
        h.parent = Some(v1id);
        h.created_at = 2;
        h.collection = [2; 32];
        let v2 = SignedVersion::sign(&key(2), h).unwrap();
        let v2id = v2.id();
        g.insert(&st, v2).unwrap();

        let mut t = LeaseTable::new();
        let t0 = Instant::now();
        let on = ProbeRound {
            answers: vec![OnlineMember { key: pk(3), latest: pt(0, 2) }, OnlineMember { key: pk(9), latest: pt(5, 5) }],
            unreachable: vec![pk(2)],
            elapsed: secs(1),
        };
        // Rien en cours, tous à jour : héberger (le membre 9 n'est pas du groupe, ignoré).
        assert_eq!(g.latest_by_number(&st).map(|v| v.header.point()), pt(0, 2));
        assert_eq!(decide_from(&st, &g, &t, &pk(1), &on, false, t0), Decision::Host);
        // Partie mal terminée de 2 : son dernier point de reprise est v2.
        obs(&mut t, &st, &pk(2), lease(&st, 2, Some((1, 1))), t0).unwrap();
        assert_eq!(
            decide_from(&st, &g, &t, &pk(1), &on, false, t0 + secs(100)),
            Decision::Unclean { holder: pk(2), last_checkpoint: Some(v2id) }
        );
        // Bail valide : rejoindre.
        assert_eq!(decide_from(&st, &g, &t, &pk(1), &on, false, t0 + secs(10)), Decision::Join(pk(2)));
    }

    // --- bail signé -----------------------------------------------------------------------------------------

    #[test]
    fn signature_valide_falsification_et_domaine() {
        let st = group(&[1, 2, 3]);
        let l = lease(&st, 2, Some((5, 4)));
        assert!(l.verify());
        assert_eq!(l.check_received(&st, &pk(2)), Ok(()));
        // Chaque champ est couvert par la signature.
        type Tamper = Box<dyn Fn(&mut Lease)>;
        let tamper: Vec<Tamper> = vec![
            Box::new(|l| l.seq += 1),
            Box::new(|l| l.epoch += 1),
            Box::new(|l| l.lease_id[0] ^= 1),
            Box::new(|l| l.base_number += 1),
            Box::new(|l| l.base_version = None),
            Box::new(|l| l.phase = LeasePhase::Starting),
            Box::new(|l| l.ttl_secs = 30),
            Box::new(|l| l.device_id = pk(3)),
            Box::new(|l| l.group_id = GroupId([1; 16])),
        ];
        for f in tamper {
            let mut m = l.clone();
            f(&mut m);
            assert!(!m.verify(), "un champ modifié doit invalider la signature");
        }
        // Signé par un autre que le titulaire : refusé.
        let forged = Lease::new(st.id, pk(2), None).signed(&key(3));
        assert_eq!(forged.check_received(&st, &pk(2)), Err(LeaseError::BadSignature));
        // Le domaine compte : la même signature sur un autre domaine ne vaut pas bail.
        let mut other = Lease::new(st.id, pk(2), None);
        other.sig = sign_domain(&key(2), b"cubelink/hearth/autre/1", &other.canonical());
        assert!(!other.verify());
        // Émetteur ou appareil différents du titulaire : refusé.
        assert_eq!(l.check_received(&st, &pk(3)), Err(LeaseError::SenderMismatch));
        let mut dev = Lease::new(st.id, pk(2), None);
        dev.device_id = pk(3);
        assert_eq!(dev.signed(&key(2)).check_received(&st, &pk(2)), Err(LeaseError::SenderMismatch));
        // JSON : aller-retour, et un champ en trop est refusé.
        let json = serde_json::to_string(&l).unwrap();
        assert_eq!(serde_json::from_str::<Lease>(&json).unwrap(), l);
        assert!(serde_json::from_str::<Lease>(&json.replace("\"seq\"", "\"zz\"")).is_err());
    }

    #[test]
    fn epoque_et_format_sont_verifies() {
        let st = group(&[1, 2]);
        let mut t = LeaseTable::new();
        let t0 = Instant::now();
        let stale = Lease::new(st.id, pk(2), None).at_epoch(st.epoch + 1).signed(&key(2));
        assert_eq!(obs(&mut t, &st, &pk(2), stale, t0), Err(LeaseError::WrongEpoch));
        let mut bad = Lease::new(st.id, pk(2), None);
        bad.format = 9;
        assert_eq!(obs(&mut t, &st, &pk(2), bad.signed(&key(2)), t0), Err(LeaseError::BadFormat));
        assert!(t.active(&st.id, t0).is_empty());
    }

    #[test]
    fn pierre_tombale_apres_release_dix_minutes() {
        let st = group(&[1, 2, 3]);
        let mut t = LeaseTable::new();
        let t0 = Instant::now();
        let l = lease(&st, 2, None);
        let l3 = next(&next(&l, 2), 2);
        obs(&mut t, &st, &pk(2), l.clone(), t0).unwrap();
        assert!(t.release_lease(&st.id, &pk(2), &l.lease_id, 3, t0 + secs(5)));
        assert!(t.holder(&st.id, t0 + secs(6)).is_none());
        assert_eq!(t.tombstone_of(&pk(2), &l.lease_id, t0 + secs(6)), Some(3));
        // Un Lease tardif de cette partie (seq plus grand compris) est ignoré pendant 10 minutes.
        assert_eq!(obs(&mut t, &st, &pk(2), l3.clone(), t0 + secs(7)), Ok(Observed::Ignored));
        assert_eq!(obs(&mut t, &st, &pk(2), l.clone(), t0 + secs(8)), Ok(Observed::Ignored));
        assert!(t.holder(&st.id, t0 + secs(9)).is_none());
        // Une autre partie du même titulaire n'est pas touchée.
        let autre = lease(&st, 2, None);
        assert_eq!(obs(&mut t, &st, &pk(2), autre.clone(), t0 + secs(10)), Ok(Observed::Accepted));
        // Un release périmé (autre lease_id) ne retire pas la partie en cours.
        assert!(!t.release_lease(&st.id, &pk(2), &l.lease_id, 3, t0 + secs(11)));
        assert!(t.holder(&st.id, t0 + secs(12)).is_some());
        // La pierre tombale tombe au bout de 10 minutes.
        assert_eq!(t.tombstone_of(&pk(2), &l.lease_id, t0 + secs(11 + 600)), None);
        assert_eq!(obs(&mut t, &st, &pk(2), l3, t0 + secs(700)), Ok(Observed::Accepted));
    }

    #[test]
    fn les_pierres_tombales_sont_bornees() {
        let st = group(&[1, 2]);
        let mut t = LeaseTable::new();
        let t0 = Instant::now();
        for i in 0..(MAX_TOMBSTONES + 10) {
            let mut id = [0u8; 16];
            id[..8].copy_from_slice(&(i as u64).to_be_bytes());
            t.release_lease(&st.id, &pk(2), &id, 1, t0 + Duration::from_millis(i as u64));
        }
        assert!(t.tombstones.len() <= MAX_TOMBSTONES);
    }

    #[test]
    fn une_pierre_tombale_ne_vaut_que_pour_son_titulaire() {
        let st = group(&[1, 2, 3]);
        let mut t = LeaseTable::new();
        let t0 = Instant::now();
        let l = lease(&st, 2, None);
        // Un autre membre enterre le lease_id d'Alice : sans effet sur elle.
        t.release_lease(&st.id, &pk(3), &l.lease_id, 0, t0);
        assert_eq!(obs(&mut t, &st, &pk(2), l.clone(), t0 + secs(1)), Ok(Observed::Accepted));
        assert_eq!(t.tombstone_of(&pk(2), &l.lease_id, t0 + secs(1)), None);
        // Quota par titulaire.
        for i in 0..(MAX_TOMBSTONES_PER_HOLDER + 10) {
            let mut id = [9u8; 16];
            id[..8].copy_from_slice(&(i as u64).to_be_bytes());
            t.release_lease(&st.id, &pk(3), &id, 1, t0 + Duration::from_millis(i as u64));
        }
        assert!(t.tombstones.len() <= MAX_TOMBSTONES_PER_HOLDER);
    }

    // --- prise en deux phases -------------------------------------------------------------------------------

    fn claim_of(st: &GroupState, holder: u8, base: Option<(VersionId, u64)>) -> Claim {
        Claim::of(&Lease::new(st.id, pk(holder), base))
    }

    #[test]
    fn le_temoin_garde_le_premier_claim_comme_verrou() {
        let st = group(&[1, 2, 3]);
        let g = VersionGraph::new();
        let mut w = LeaseTable::new();
        let t0 = Instant::now();
        let (a, b) = (claim_of(&st, 1, None), claim_of(&st, 2, None));
        assert_eq!(w.claim(&st, &g, &pk(1), &a, t0), Ok(ClaimReply::Ack));
        // Le même claim rejoué reste acquitté ; un second claim est un conflit, avec le claim gardé.
        assert_eq!(w.claim(&st, &g, &pk(1), &a, t0 + secs(1)), Ok(ClaimReply::Ack));
        let Ok(ClaimReply::Conflict(c)) = w.claim(&st, &g, &pk(2), &b, t0 + secs(1)) else { panic!("conflit attendu") };
        assert_eq!((c.kind, c.holder, c.lease_id), (ConflictKind::Claim, pk(1), a.lease_id));
        // Le verrou tombe 90 s après le claim, à défaut de bail.
        assert_eq!(w.claim(&st, &g, &pk(2), &b, t0 + secs(91)), Ok(ClaimReply::Ack));
        // Un claim d'un autre que son titulaire, d'un non-membre, d'une autre époque : refusé.
        assert_eq!(w.claim(&st, &g, &pk(3), &a, t0), Err(LeaseError::SenderMismatch));
        let mut x = claim_of(&st, 3, None);
        x.epoch = 4;
        assert_eq!(w.claim(&st, &g, &pk(3), &x, t0), Err(LeaseError::WrongEpoch));
        assert_eq!(w.claim(&st, &g, &pk(9), &claim_of(&st, 9, None), t0), Err(LeaseError::NotMember));
    }

    #[test]
    fn le_verrou_tombe_au_premier_bail_starting_ou_au_release() {
        let st = group(&[1, 2, 3]);
        let g = VersionGraph::new();
        let t0 = Instant::now();
        let mut w = LeaseTable::new();
        let l = Lease::starting(st.id, pk(1), None).signed(&key(1));
        let a = Claim::of(&l);
        w.claim(&st, &g, &pk(1), &a, t0).unwrap();
        assert!(w.locked_claim(&st.id, t0).is_some());
        // Le premier bail Starting de cette partie remplace le verrou : le bail lui-même protège.
        assert_eq!(obs(&mut w, &st, &pk(1), l, t0 + secs(2)), Ok(Observed::Accepted));
        assert!(w.locked_claim(&st.id, t0 + secs(2)).is_none());
        let Ok(ClaimReply::Conflict(c)) = w.claim(&st, &g, &pk(2), &claim_of(&st, 2, None), t0 + secs(3)) else { panic!() };
        assert_eq!((c.kind, c.holder), (ConflictKind::Lease, pk(1)));
        // Un candidat qui renonce relâche son verrou (LeaseRelease de son lease_id).
        let mut w2 = LeaseTable::new();
        w2.claim(&st, &g, &pk(1), &a, t0).unwrap();
        w2.release_lease(&st.id, &pk(1), &a.lease_id, 0, t0 + secs(1));
        assert_eq!(w2.claim(&st, &g, &pk(2), &claim_of(&st, 2, None), t0 + secs(2)), Ok(ClaimReply::Ack));
    }

    #[test]
    fn deux_candidats_simultanes_un_seul_obtient_le_quorum() {
        // 3 membres : A (1) et B (2) candidats, C (3) témoin. Le témoin n'acquitte que le premier ; chaque candidat
        // verrouille son propre claim chez lui, donc il refuse l'autre.
        let st = group(&[1, 2, 3]);
        let g = VersionGraph::new();
        let t0 = Instant::now();
        let (ca, cb) = (claim_of(&st, 1, None), claim_of(&st, 2, None));
        let (mut ta, mut tb, mut tc) = (LeaseTable::new(), LeaseTable::new(), LeaseTable::new());
        assert!(ta.lock_own_claim(&g, &ca, t0).is_none());
        assert!(tb.lock_own_claim(&g, &cb, t0).is_none());
        let reply = |t: &mut LeaseTable, from: u8, c: &Claim| t.claim(&st, &g, &pk(from), c, t0).unwrap();
        // Les claims s'entrecroisent ; C voit B en premier.
        let (b_at_c, a_at_c) = (reply(&mut tc, 2, &cb), reply(&mut tc, 1, &ca));
        let (a_at_b, b_at_a) = (reply(&mut tb, 1, &ca), reply(&mut ta, 2, &cb));
        assert_eq!(b_at_c, ClaimReply::Ack);
        let (mut acks_a, mut conf_a, mut acks_b, mut conf_b) = (0, vec![], 0, vec![]);
        for r in [a_at_c, b_at_a] {
            match r {
                ClaimReply::Ack => acks_a += 1,
                ClaimReply::Conflict(c) => conf_a.push(c),
            }
        }
        for r in [b_at_c, a_at_b] {
            match r {
                ClaimReply::Ack => acks_b += 1,
                ClaimReply::Conflict(c) => conf_b.push(c),
            }
        }
        let out_a = claim_outcome(&ca, 3, acks_a, &conf_a, &g);
        let out_b = claim_outcome(&cb, 3, acks_b, &conf_b, &g);
        assert!(matches!(out_b, ClaimOutcome::Safe { acks: 2 }), "B a C et lui-même : {out_b:?}");
        assert!(matches!(out_a, ClaimOutcome::Conflict(_)), "A n'a que lui-même et cède : {out_a:?}");
    }

    #[test]
    fn quorum_sur_ou_au_mieux() {
        let st = group(&[1, 2, 3, 4, 5]);
        let g = VersionGraph::new();
        let c = claim_of(&st, 1, None);
        assert_eq!(majority(1), 1);
        assert_eq!(majority(2), 2);
        assert_eq!(majority(4), 3);
        assert_eq!(majority(5), 3);
        // 5 membres : moi + 2 acquittements = 3 : sûr ; moi + 1 = 2 : au mieux ; seul : au mieux.
        assert_eq!(claim_outcome(&c, 5, 2, &[], &g), ClaimOutcome::Safe { acks: 3 });
        assert_eq!(claim_outcome(&c, 5, 1, &[], &g), ClaimOutcome::BestEffort { acks: 2, needed: 3, members: 5 });
        assert_eq!(claim_outcome(&c, 5, 0, &[], &g), ClaimOutcome::BestEffort { acks: 1, needed: 3, members: 5 });
        // Groupe à 2 : la majorité stricte est 2, un candidat seul reste « au mieux » ; un groupe d'un seul membre est sûr.
        assert!(matches!(claim_outcome(&c, 2, 0, &[], &g), ClaimOutcome::BestEffort { .. }));
        assert_eq!(claim_outcome(&c, 2, 1, &[], &g), ClaimOutcome::Safe { acks: 2 });
        assert_eq!(claim_outcome(&c, 1, 0, &[], &g), ClaimOutcome::Safe { acks: 1 });
        // Un bail déjà connu d'un autre l'emporte toujours, même avec la majorité des acquittements.
        let lease_conflict = ClaimConflict {
            kind: ConflictKind::Lease,
            holder: pk(2),
            lease_id: [3; 16],
            base_version: None,
            base_number: 0,
        };
        assert!(matches!(claim_outcome(&c, 5, 4, std::slice::from_ref(&lease_conflict), &g), ClaimOutcome::Conflict(_)));
        // Un claim concurrent : si je le bats au départage sans majorité, au mieux ; sinon je cède.
        let other = |holder: u8| ClaimConflict {
            kind: ConflictKind::Claim,
            holder: pk(holder),
            lease_id: [9; 16],
            base_version: None,
            base_number: 0,
        };
        let (lo, hi) = if pk(1) < pk(2) { (1, 2) } else { (2, 1) };
        let (c_lo, c_hi) = (claim_of(&st, lo, None), claim_of(&st, hi, None));
        assert!(matches!(claim_outcome(&c_lo, 5, 1, &[other(hi)], &g), ClaimOutcome::BestEffort { .. }));
        assert!(matches!(claim_outcome(&c_hi, 5, 1, &[other(lo)], &g), ClaimOutcome::Conflict(_)));
        // La majorité garde le dessus sur un claim concurrent.
        assert!(matches!(claim_outcome(&c_hi, 5, 2, &[other(lo)], &g), ClaimOutcome::Safe { .. }));
    }

    #[test]
    fn depart_par_ascendance_puis_fourche_puis_cle_jamais_par_le_numero() {
        let st = group(&[1, 2, 3, 4]);
        let (mut g, ids) = chain(&st, 3);
        // Une branche : v2' (numéro 2) fils de ids[0], donc fourche avec ids[1] et ids[2].
        let h = VersionHeader {
            group_id: st.id,
            number: 2,
            parent: Some(ids[0]),
            epoch: 0,
            author: pk(2),
            pack_hash: "ab".repeat(32),
            created_at: 50,
            played_secs: 0,
            kind: VersionKind::Checkpoint,
            collection: [0x77; 32],
            total_size: 1,
        };
        let side = SignedVersion::sign(&key(2), h).unwrap();
        let side_id = side.id();
        g.insert(&st, side).unwrap();
        let (v1, v2, v3) = (Some(ids[0]), Some(ids[1]), Some(ids[2]));
        let (p1, p4) = (pk(1), pk(4));
        // Descendante gagne, quelle que soit la clé et le numéro déclaré.
        assert_eq!(duel((&p4, &v3), (&p1, &v1), &g), (true, false));
        assert_eq!(duel((&p1, &v1), (&p4, &v3), &g), (false, false));
        assert_eq!(duel((&p4, &v3), (&p1, &v2), &g), (true, false));
        // Branches différentes : fourche, plus petite clé, drapeau levé (même si l'autre a un numéro plus grand).
        assert_eq!(compare_bases(&Some(side_id), &v3, &g), BaseOrder::Fork);
        assert_eq!(duel((&p1, &Some(side_id)), (&p4, &v3), &g), (true, true));
        assert_eq!(duel((&p4, &v3), (&p1, &Some(side_id)), &g), (false, true));
        // Bases égales : plus petite clé, pas de fourche.
        assert_eq!(duel((&p1, &v2), (&p4, &v2), &g), (true, false));
        assert_eq!(duel((&p4, &v2), (&p1, &v2), &g), (false, false));
        // Base inconnue de nous : aucun avantage.
        let unknown = Some(VersionId([0xdd; 32]));
        assert_eq!(compare_bases(&unknown, &v3, &g), BaseOrder::Equal);
        // arbitrate_marked lève le drapeau de fourche.
        let mine = Lease::new(st.id, p1, Some((side_id, 2)));
        let theirs = Lease::new(st.id, p4, Some((ids[2], 3)));
        assert_eq!(arbitrate_marked(&mine, std::slice::from_ref(&theirs), &g), (Arbitration::Keep, true));
        assert_eq!(arbitrate_marked(&theirs, std::slice::from_ref(&mine), &g), (Arbitration::Yield { to: p1 }, true));
        let same = Lease::new(st.id, p4, Some((side_id, 2)));
        assert_eq!(arbitrate_marked(&mine, &[same], &g), (Arbitration::Keep, false));
    }

    // --- auto-exclusion -------------------------------------------------------------------------------------

    #[test]
    fn auto_exclusion_selon_les_ecarts_d_horloge() {
        let clock = ManualClock::default();
        let mut f = HostFence::new(&clock);
        // Renouvellement normal : 30 s sur les deux horloges.
        clock.advance(secs(30));
        assert_eq!(f.check(&clock), FenceVerdict::Renew);
        f.mark(&clock);
        // Veille de 10 minutes : l'horloge murale avance, pas la monotone (écart > 60 s).
        clock.advance(secs(30));
        clock.advance_wall(secs(600));
        assert!(matches!(f.check(&clock), FenceVerdict::Excluded(FenceReason::ClockSkew { skew_secs: 600 })));
        f.mark(&clock);
        // Horloge murale reculée de plus de 60 s : exclu aussi.
        clock.advance(secs(30));
        clock.rewind_wall(secs(120));
        assert!(matches!(f.check(&clock), FenceVerdict::Excluded(FenceReason::ClockSkew { .. })));
        f.mark(&clock);
        // Un écart de 60 s pile ne suffit pas ; 61 s oui.
        clock.advance(secs(30));
        clock.advance_wall(secs(60));
        assert_eq!(f.check(&clock), FenceVerdict::Renew);
        clock.advance_wall(secs(1));
        assert!(matches!(f.check(&clock), FenceVerdict::Excluded(_)));
        f.mark(&clock);
        // Plus de `ttl` s depuis le dernier renouvellement, même sur les deux horloges : exclu (le bail a expiré).
        clock.advance(secs(90));
        assert_eq!(f.check(&clock), FenceVerdict::Renew);
        clock.advance(secs(1));
        assert!(matches!(f.check(&clock), FenceVerdict::Excluded(FenceReason::Stale { elapsed_secs: 91 })));
        // Un petit écart (55 s) entre les horloges, sous le seuil, ne suffit pas.
        f.mark(&clock);
        clock.advance(secs(10));
        clock.advance_wall(secs(55));
        assert_eq!(f.check(&clock), FenceVerdict::Renew);
    }

    #[test]
    fn apres_la_ronde_un_autre_bail_ou_une_version_plus_recente_arrete_l_hote() {
        let st = group(&[1, 2, 3]);
        let tip = pt(0, 5);
        let none = ProbeRound { answers: vec![online(2, pt(0, 5)), online(3, pt(0, 4))], unreachable: vec![], elapsed: secs(1) };
        assert_eq!(fence_action(&[], &none, tip), FenceAction::Resume);
        // Personne n'a répondu : rien ne contredit, l'hôte reprend (nouveau lease_id côté application).
        assert_eq!(fence_action(&[], &ProbeRound::default(), tip), FenceAction::Resume);
        // Un autre bail valide : arrêt, session publiée comme branche sans bail.
        assert_eq!(fence_action(&[lease(&st, 2, None)], &none, tip), FenceAction::StopAndFork);
        // Une version plus récente chez un membre (même époque ou époque suivante).
        let newer = ProbeRound { answers: vec![online(2, pt(0, 6))], ..Default::default() };
        assert_eq!(fence_action(&[], &newer, tip), FenceAction::StopAndFork);
        let epoch = ProbeRound { answers: vec![online(2, pt(1, 1))], ..Default::default() };
        assert_eq!(fence_action(&[], &epoch, tip), FenceAction::StopAndFork);
        // Sans pointe de partie (premier hébergement), toute version d'un membre est plus récente.
        assert_eq!(fence_action(&[], &none, None), FenceAction::StopAndFork);
    }

    #[test]
    fn decide_recoit_la_ronde_complete() {
        let st = group(&[1, 2, 3]);
        let g = VersionGraph::new();
        let t = LeaseTable::new();
        let t0 = Instant::now();
        // Personne n'a répondu, des membres sont injoignables : bloqué, pas Héberger sur la foi d'un cache.
        let round = ProbeRound { answers: vec![], unreachable: vec![pk(2), pk(3), pk(9)], elapsed: secs(4) };
        assert_eq!(decide_from(&st, &g, &t, &pk(1), &round, false, t0), Decision::Blocked);
        // Un membre a répondu : héberger est possible (les injoignables sont transmis à la décision).
        let round = ProbeRound { answers: vec![online(2, None)], unreachable: vec![pk(3)], elapsed: secs(4) };
        assert_eq!(decide_from(&st, &g, &t, &pk(1), &round, false, t0), Decision::Host);
    }

    // --- propriétés ---------------------------------------------------------------------------------------

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        #[test]
        fn exactly_one_host_keeps_the_lead(bases in prop::collection::vec((0u64..4, 1u8..40), 1..8)) {
            // Un hôte par graine (clés distinctes) : chacun arbitre contre les autres, un seul garde la main.
            let st = group(&[1]);
            let mut seen = std::collections::BTreeSet::new();
            let leases: Vec<Lease> = bases
                .iter()
                .filter(|(_, s)| seen.insert(*s))
                .map(|(n, s)| {
                    let mut l = Lease::new(st.id, pk(*s), Some((VersionId([1; 32]), 1)));
                    l.phase = if *n % 2 == 0 { LeasePhase::Ready } else { LeasePhase::Starting };
                    l
                })
                .collect();
            let keepers = leases
                .iter()
                .filter(|l| arbitrate(l, &leases, &VersionGraph::new()) == Arbitration::Keep)
                .count();
            prop_assert_eq!(keepers, 1);
            // Indépendant de l'ordre des autres.
            let mut rev = leases.clone();
            rev.reverse();
            for l in &leases {
                prop_assert_eq!(arbitrate(l, &leases, &VersionGraph::new()), arbitrate(l, &rev, &VersionGraph::new()));
            }
        }

        #[test]
        fn decide_invariants(
            my in prop::option::of((0u64..3, 1u64..6)),
            online_raw in prop::collection::vec((1u8..6, prop::option::of((0u64..3, 1u64..6))), 0..5),
            lease_holders in prop::collection::vec(1u8..6, 0..3),
            other_source in any::<bool>(),
            solo in any::<bool>(),
        ) {
            let st = group(&[1]);
            let mut i = input(1);
            i.my_latest = my.map(|(e, n)| VersionPoint { epoch: e, number: n });
            // Un membre n'apparaît qu'une fois dans la liste des membres joignables.
            let mut seen = std::collections::BTreeSet::new();
            i.online = online_raw
                .iter()
                .filter(|(s, _)| seen.insert(*s))
                .map(|(s, p)| OnlineMember { key: pk(*s), latest: p.map(|(e, n)| VersionPoint { epoch: e, number: n }) })
                .collect();
            i.leases = lease_holders.iter().map(|s| Lease::new(st.id, pk(*s), None)).collect();
            i.other_source = other_source;
            i.solo = solo;
            let others_online = i.online.iter().any(|m| m.key != pk(1));
            let other_lease = i.leases.iter().any(|l| l.holder != pk(1));
            match decide(&i) {
                Decision::Join(h) => {
                    prop_assert!(h != pk(1));
                    prop_assert!(other_lease);
                }
                Decision::Sync(f) => {
                    prop_assert!(!other_lease);
                    let theirs = i.online.iter().find(|m| m.key == f).unwrap().latest;
                    prop_assert!(theirs > i.my_latest);
                }
                Decision::Blocked => {
                    prop_assert!(!other_lease && !others_online && !other_source && !solo);
                }
                Decision::Host => {
                    prop_assert!(!other_lease);
                    prop_assert!(others_online || other_source || solo);
                }
                Decision::Unclean { .. } => prop_assert!(false, "aucune partie mal terminée en entrée"),
            }
        }
    }
}
