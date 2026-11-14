//! Hearth : bail d'hôte et décision du bouton Jouer (voir `docs/hearth.md`, § 3.3).
//!
//! Logique pure. Un seul serveur à la fois par groupe : l'hôte annonce un [`Lease`] toutes les
//! [`LEASE_RENEW_INTERVAL`] aux membres en ligne ; chaque récepteur le garde [`LEASE_TTL_SECS`] **à partir de sa
//! réception** (aucune horloge partagée). Le temps est un `std::time::Instant` passé en paramètre : les tests le
//! font avancer sans dormir.
//!
//! Précisions par rapport à la spec :
//! * `base_version` est une `Option` (le premier hébergement d'un groupe n'a aucune version) et le bail porte le
//!   numéro de cette base (`base_number`, 0 sans base), à titre d'affichage seulement : un bail dont la base est
//!   connue de nous et dont le numéro la contredit est rejeté, et le départage ne s'en sert jamais (voir
//!   [`rank_leases`] : phase, puis ascendance dans le graphe local, puis plus petite clé) ;
//! * un bail annoncé en phase [`LeasePhase::Starting`] dit « je prépare le serveur » (installation, démarrage d'un gros
//!   pack : 2 à 6 minutes) ; il est renouvelé comme les autres et devient `Ready` quand le serveur répond ;
//! * un titulaire dont le tunnel échoue deux fois en trois minutes est ignoré quinze minutes (preuve de service,
//!   [`LeaseTable::report_unreachable`]) : un bail annoncé sans serveur derrière ne bloque pas le groupe ;
//! * le bail n'est pas signé : l'émetteur est l'identité authentifiée de la connexion QUIC, que l'appelant passe
//!   à [`LeaseTable::observe`] et qui doit être le titulaire du bail.
//!
//! Un bail qui expire sans `LeaseRelease` est une partie **mal terminée** ([`LeaseTable::unclean`]) : elle reste
//! visible jusqu'à ce qu'elle soit résolue (un autre bail est accepté, `LeaseRelease` tardif, `revoke`).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::model::{hex_bytes, GroupId, GroupState};
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

/// Bail d'hôte d'un groupe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lease {
    pub group_id: GroupId,
    #[serde(with = "hex_bytes")]
    pub holder: PeerKey,
    /// Version dont part la partie (`None` : premier hébergement du groupe).
    pub base_version: Option<VersionId>,
    /// Numéro de `base_version` (0 sans base).
    pub base_number: u64,
    /// Compteur croissant de l'hôte pour cette partie (1, 2, …).
    pub seq: u64,
    pub ttl_secs: u32,
    /// Phase de la partie (absent d'un ancien message : `Ready`).
    #[serde(default)]
    pub phase: LeasePhase,
}

impl Lease {
    /// Premier bail d'une partie (`seq = 1`, durée de vie standard).
    pub fn new(group_id: GroupId, holder: PeerKey, base: Option<(VersionId, u64)>) -> Self {
        Self {
            group_id,
            holder,
            base_version: base.map(|b| b.0),
            base_number: base.map_or(0, |b| b.1),
            seq: 1,
            ttl_secs: LEASE_TTL_SECS,
            phase: LeasePhase::Ready,
        }
    }

    /// Premier bail d'une partie dont le serveur n'est pas encore prêt.
    pub fn starting(group_id: GroupId, holder: PeerKey, base: Option<(VersionId, u64)>) -> Self {
        Self { phase: LeasePhase::Starting, ..Self::new(group_id, holder, base) }
    }

    /// Le même bail une fois le serveur prêt (`seq` continue).
    pub fn ready(&self) -> Self {
        Self { phase: LeasePhase::Ready, seq: self.seq.saturating_add(1), ..self.clone() }
    }

    /// Bail suivant (même partie, `seq + 1`) à envoyer au prochain renouvellement.
    pub fn renewed(&self) -> Self {
        Self { seq: self.seq.saturating_add(1), ..self.clone() }
    }

    /// Contrôle d'un bail reçu (le contenu vient d'un pair), sans le graphe des versions.
    pub fn validate(&self, state: &GroupState) -> Result<(), LeaseError> {
        if self.group_id != state.id {
            return Err(LeaseError::WrongGroup);
        }
        if !state.is_member(&self.holder) {
            return Err(LeaseError::NotMember);
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
    #[error("trop de groupes suivis")]
    TooManyGroups,
}

/// Effet d'un bail reçu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observed {
    /// Nouveau bail, ou renouvellement plus récent : retenu.
    Accepted,
    /// Ancien ou répété (`seq` pas plus grand alors que le bail connu est encore valide) : ignoré.
    Ignored,
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
    /// comme résolu : quelqu'un a pris le relais.
    pub fn observe(
        &mut self,
        state: &GroupState,
        graph: &VersionGraph,
        from: &PeerKey,
        lease: Lease,
        now: Instant,
    ) -> Result<Observed, LeaseError> {
        lease.validate(state)?;
        lease.check_base(graph)?;
        if lease.holder != *from {
            return Err(LeaseError::SenderMismatch);
        }
        if !self.groups.contains_key(&lease.group_id) && self.groups.len() >= MAX_GROUPS {
            return Err(LeaseError::TooManyGroups);
        }
        let entries = self.groups.entry(lease.group_id).or_default();
        if let Some(known) = entries.get(&lease.holder) {
            if lease.seq <= known.lease.seq && !known.expired(now) {
                return Ok(Observed::Ignored);
            }
        }
        let holder = lease.holder;
        entries.insert(holder, Entry { lease, received: now });
        entries.retain(|h, e| *h == holder || !e.expired(now));
        Ok(Observed::Accepted)
    }

    /// `LeaseRelease` reçu du titulaire : la partie s'est terminée proprement.
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

/// Le bail `a` bat-il `b` ? D'abord la phase (un serveur prêt l'emporte sur un serveur en préparation), puis
/// l'ascendance dans le graphe **local** (une partie qui part d'une version plus avancée l'emporte ; une partie sans
/// base perd contre toute partie qui en a une ; une base inconnue de nous ne donne rien), puis la plus petite clé.
fn beats(a: &Lease, b: &Lease, graph: &VersionGraph) -> bool {
    if a.phase != b.phase {
        return a.phase == LeasePhase::Ready;
    }
    match (&a.base_version, &b.base_version) {
        (Some(x), None) if graph.contains(x) => return true,
        (None, Some(y)) if graph.contains(y) => return false,
        (Some(x), Some(y)) if x != y && graph.contains(x) && graph.contains(y) => {
            if graph.is_ancestor(y, x) {
                return true;
            }
            if graph.is_ancestor(x, y) {
                return false;
            }
        }
        _ => {}
    }
    a.holder < b.holder
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
    let mut all: Vec<Lease> = others.iter().filter(|l| l.group_id == mine.group_id && l.holder != mine.holder).cloned().collect();
    all.push(mine.clone());
    match rank_leases(all, graph).first() {
        Some(w) if w.holder != mine.holder => Arbitration::Yield { to: w.holder },
        _ => Arbitration::Keep,
    }
}

/// Un membre joignable et la version la plus avancée qu'il dit avoir.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OnlineMember {
    pub key: PeerKey,
    pub latest: Option<VersionPoint>,
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
    /// Membres joignables maintenant (moi exclu ; le cas échéant ignoré).
    pub online: Vec<OnlineMember>,
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
/// puis décide. `online` : membres joignables avec la version la plus avancée qu'ils ont annoncée.
pub fn decide_from(
    state: &GroupState,
    graph: &VersionGraph,
    table: &LeaseTable,
    me: &PeerKey,
    online: &[OnlineMember],
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
        online: online.iter().filter(|m| state.is_member(&m.key)).copied().collect(),
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
        Lease::new(st.id, pk(holder), base.map(|(b, n)| (VersionId([b; 32]), n)))
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
        obs(&mut t, &st, &pk(2), lease(&st, 2, None), t0).unwrap();
        assert!(t.holder(&st.id, t0 + secs(89)).is_some());
        assert_eq!(t.remaining(&st.id, &pk(2), t0 + secs(60)), Some(secs(30)));
        assert!(t.holder(&st.id, t0 + secs(90)).is_none());
        assert_eq!(t.remaining(&st.id, &pk(2), t0 + secs(90)), None);
        // Un renouvellement reçu à 60 s repousse l'expiration de 90 s à partir de SA réception.
        let l2 = lease(&st, 2, None).renewed();
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
        obs(&mut t, &st, &pk(2), l.clone(), t0).unwrap();
        // Même seq ou plus ancien, bail valide : ignoré, sans prolonger la durée de vie.
        assert_eq!(obs(&mut t, &st, &pk(2), l.clone(), t0 + secs(30)), Ok(Observed::Ignored));
        let mut old = l.clone();
        old.seq = 2;
        assert_eq!(obs(&mut t, &st, &pk(2), old.clone(), t0 + secs(30)), Ok(Observed::Ignored));
        assert!(t.holder(&st.id, t0 + secs(90)).is_none());
        // Une fois expiré, un hôte qui redémarre (seq repartie de 1) est accepté.
        assert_eq!(obs(&mut t, &st, &pk(2), old, t0 + secs(100)), Ok(Observed::Accepted));
        assert!(t.holder(&st.id, t0 + secs(100)).is_some());
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
        assert_eq!(t.observe(&st, &g, &pk(2), faux, t0), Err(LeaseError::BadBase));
        assert!(t.active(&st.id, t0).is_empty());
        // Le bon numéro passe ; une base que nous ne connaissons pas encore aussi (elle sera synchronisée).
        assert_eq!(t.observe(&st, &g, &pk(2), Lease::new(st.id, pk(2), Some((ids[2], 3))), t0), Ok(Observed::Accepted));
        assert_eq!(t.observe(&st, &g, &pk(3), Lease::new(st.id, pk(3), Some((VersionId([0xee; 32]), 9))), t0), Ok(Observed::Accepted));
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
        obs(&mut t, &st, &pk(2), lease(&st, 2, None).renewed(), t0 + secs(70)).unwrap();
        assert!(t.holder(&st.id, t0 + secs(71)).is_none());
        assert!(t.unclean(&st.id, t0 + secs(500)).is_empty(), "un titulaire ignoré n'est pas une partie mal terminée");
        // Quinze minutes plus tard, il compte de nouveau.
        obs(&mut t, &st, &pk(2), lease(&st, 2, None).renewed().renewed(), t0 + secs(60 + 15 * 60)).unwrap();
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
        let on = [OnlineMember { key: pk(3), latest: pt(0, 2) }, OnlineMember { key: pk(9), latest: pt(5, 5) }];
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
