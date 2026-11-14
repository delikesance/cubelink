//! Hearth : versions du monde et graphe des versions (voir `docs/hearth.md`, § 3.2).
//!
//! Logique pure. Une version est un instantané signé du monde : un en-tête ([`VersionHeader`]) qui désigne une
//! collection de blobs (empreinte BLAKE3, hexadécimal) et son parent. Ce module ne dépend pas d'iroh-blobs : la
//! collection n'est qu'une empreinte de 32 octets.
//!
//! # Identité et signature
//!
//! * encodage canonique de l'en-tête : JSON compact de la structure (ordre de champs fixe, hexadécimal minuscule),
//!   recalculé à partir de la valeur typée ;
//! * `VersionId = BLAKE3(encodage canonique de l'en-tête)` ;
//! * signature = ed25519 de [`VERSION_DOMAIN`] `||` encodage canonique de l'en-tête, par la clé `author`.
//!
//! # Règles du graphe ([`VersionGraph::insert`])
//!
//! * l'en-tête est bien formé, du bon groupe, signé par son auteur, et l'auteur est membre ;
//! * `epoch` ne dépasse pas l'époque courante du groupe ;
//! * sans parent : `number == 1` et `epoch == 0` (première version du groupe) ;
//! * avec parent : `number == parent.number + 1`, et soit `epoch == parent.epoch`, soit `epoch` est celle d'un
//!   `ContinueWithout` dont la base est ce parent ;
//! * parent inconnu : refusé (`ParentUnknown`, la synchro doit d'abord l'obtenir), sauf s'il s'agit de la base de
//!   l'époque (on la connaît par la chaîne d'opérations avant d'en avoir le contenu). Si la base arrive ensuite,
//!   les enfants dont le numéro ou l'époque ne collent pas sont retirés du graphe.
//!
//! # Têtes, fourches, branches mises de côté
//!
//! La **tête** est l'ensemble des versions de l'époque courante sans enfant (à défaut, la base de l'époque si on
//! la connaît). Une seule tête est l'état normal ; plusieurs : **fourche**. Les versions des époques précédentes
//! (par exemple celles de l'absent d'un `ContinueWithout`, postérieures à la base) restent stockées et
//! restaurables ([`VersionGraph::side_branches`]) mais ne sont jamais tête.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use super::model::{
    decode_array, hex_bytes, is_hash_hex, sign_domain, verify_domain, GroupId, GroupState, HearthSigner, Sig,
};
use crate::presence::PeerKey;

/// Séparation de domaine des signatures de versions.
pub const VERSION_DOMAIN: &[u8] = b"cubelink/hearth/version/1";
/// Nombre maximal de versions gardées dans un graphe (borne la mémoire face à un membre bavard).
pub const MAX_VERSIONS: usize = 50_000;
/// Taille totale maximale annoncée d'une version (1 Pio : au-delà, l'en-tête est faux).
pub const MAX_TOTAL_SIZE: u64 = 1 << 50;

/// Empreinte BLAKE3 d'un blob ou d'une collection iroh-blobs.
pub type BlobHash = [u8; 32];

/// Identifiant d'une version : BLAKE3 de l'encodage canonique de son en-tête.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VersionId(pub [u8; 32]);

impl VersionId {
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// Décode 64 caractères hexadécimaux.
    pub fn from_hex(s: &str) -> Option<Self> {
        decode_array::<32>(s).ok().map(Self)
    }
}

impl fmt::Display for VersionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for VersionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "VersionId({})", &self.to_hex()[..12])
    }
}

impl Serialize for VersionId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for VersionId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let s = String::deserialize(d)?;
        decode_array::<32>(&s).map(Self).map_err(D::Error::custom)
    }
}

/// Nature d'une version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VersionKind {
    /// Fin d'une partie.
    Session,
    /// Point de reprise pendant une partie.
    Checkpoint,
    /// Nouvelle version dont le contenu est celui d'une ancienne (`from`) ; rien n'est supprimé.
    Restore { from: VersionId },
}

/// Position d'une version dans l'histoire du groupe, pour comparer « qui a le plus récent ».
/// L'ordre est celui de `(epoch, number)` : une époque plus récente l'emporte toujours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct VersionPoint {
    pub epoch: u64,
    pub number: u64,
}

/// En-tête d'une version du monde.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionHeader {
    pub group_id: GroupId,
    /// 1 pour la première version ; `parent.number + 1` ensuite.
    pub number: u64,
    pub parent: Option<VersionId>,
    pub epoch: u64,
    #[serde(with = "hex_bytes")]
    pub author: PeerKey,
    /// Empreinte SHA-256 du manifeste du modpack avec lequel le monde a été joué (64 hexadécimaux minuscules).
    pub pack_hash: String,
    /// Secondes depuis l'epoch Unix (déclaratif : l'horloge de l'auteur).
    pub created_at: u64,
    /// Temps de jeu de la session, en secondes.
    pub played_secs: u64,
    pub kind: VersionKind,
    /// Collection de blobs du monde (`chemin relatif → blob`).
    #[serde(with = "hex_bytes")]
    pub collection: BlobHash,
    pub total_size: u64,
}

impl VersionHeader {
    /// Encodage canonique (voir l'en-tête du module).
    pub fn canonical(&self) -> Vec<u8> {
        // Chaînes, entiers et énumérations : la sérialisation ne peut pas échouer.
        serde_json::to_vec(self).expect("sérialisation JSON de types simples")
    }

    pub fn id(&self) -> VersionId {
        VersionId(*blake3::hash(&self.canonical()).as_bytes())
    }

    pub fn point(&self) -> VersionPoint {
        VersionPoint { epoch: self.epoch, number: self.number }
    }

    /// Cohérence interne de l'en-tête (le contenu vient d'un pair).
    pub fn validate(&self) -> Result<(), VersionError> {
        let bad = |m: &'static str| Err(VersionError::Invalid(m));
        match self.parent {
            None if self.number != 1 => return bad("une version sans parent porte le numéro 1"),
            None if self.epoch != 0 => return bad("une version sans parent est de l'époque 0"),
            Some(_) if self.number < 2 => return bad("numéro de version trop petit pour avoir un parent"),
            _ => {}
        }
        if !is_hash_hex(&self.pack_hash) {
            return bad("empreinte du modpack");
        }
        if self.total_size > MAX_TOTAL_SIZE {
            return bad("taille totale");
        }
        if let (VersionKind::Restore { from }, Some(parent)) = (self.kind, self.parent) {
            // Restaurer la version parent ne change rien : ce n'est pas une restauration.
            if from == parent {
                return bad("restauration de la version parente");
            }
        }
        if matches!(self.kind, VersionKind::Restore { .. }) && self.parent.is_none() {
            return bad("une restauration a un parent");
        }
        Ok(())
    }
}

/// En-tête signé par son auteur.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedVersion {
    pub header: VersionHeader,
    pub sig: Sig,
}

impl SignedVersion {
    /// Signe un en-tête. L'auteur déclaré doit être le signataire.
    pub fn sign(signer: &dyn HearthSigner, header: VersionHeader) -> Result<Self, VersionError> {
        if header.author != signer.peer_key() {
            return Err(VersionError::BadSignature);
        }
        header.validate()?;
        let sig = sign_domain(signer, VERSION_DOMAIN, &header.canonical());
        Ok(Self { header, sig })
    }

    pub fn id(&self) -> VersionId {
        self.header.id()
    }

    /// La signature est celle de `header.author` sur cet en-tête.
    pub fn verify(&self) -> bool {
        verify_domain(&self.header.author, VERSION_DOMAIN, &self.header.canonical(), &self.sig)
    }
}

/// Erreurs d'insertion dans le graphe (messages affichables).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum VersionError {
    #[error("en-tête de version invalide : {0}")]
    Invalid(&'static str),
    #[error("signature de version invalide")]
    BadSignature,
    #[error("version d'un autre groupe")]
    WrongGroup,
    #[error("l'auteur de la version n'est pas membre du groupe")]
    NotMember,
    #[error("version d'une époque inconnue (époque {version}, groupe à l'époque {current})")]
    FutureEpoch { version: u64, current: u64 },
    #[error("version parente inconnue : {0}")]
    ParentUnknown(VersionId),
    #[error("numéro de version {got} au lieu de {expected}")]
    BadNumber { expected: u64, got: u64 },
    #[error("l'époque de la version ne suit pas celle de son parent")]
    BadEpoch,
    #[error("le contenu d'une restauration ne correspond pas à la version restaurée")]
    RestoreMismatch,
    #[error("trop de versions dans le graphe")]
    TooManyVersions,
}

/// Résultat d'une insertion réussie.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inserted {
    /// Nouvelle version.
    New,
    /// Déjà dans le graphe (même identifiant) : rien n'a changé.
    AlreadyKnown,
}

/// Branche mise de côté : une pointe d'une époque révolue qui n'est pas la tête.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SideBranch {
    pub tip: VersionId,
    pub author: PeerKey,
    pub number: u64,
    pub epoch: u64,
    /// Dernier ancêtre commun avec la tête courante (`None` s'il n'y a pas de tête unique ou de point commun).
    pub fork_point: Option<VersionId>,
}

/// Graphe des versions connues d'un groupe, sans doublon, validé à l'insertion.
#[derive(Debug, Default, Clone)]
pub struct VersionGraph {
    versions: BTreeMap<VersionId, SignedVersion>,
    /// Enfants par parent, y compris pour un parent encore inconnu (la base d'une époque).
    children: BTreeMap<VersionId, BTreeSet<VersionId>>,
}

impl VersionGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.versions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.versions.is_empty()
    }

    pub fn contains(&self, id: &VersionId) -> bool {
        self.versions.contains_key(id)
    }

    pub fn get(&self, id: &VersionId) -> Option<&SignedVersion> {
        self.versions.get(id)
    }

    /// Toutes les versions, par identifiant croissant.
    pub fn iter(&self) -> impl Iterator<Item = (&VersionId, &SignedVersion)> {
        self.versions.iter()
    }

    /// Enfants connus d'une version (identifiants croissants).
    pub fn children_of(&self, id: &VersionId) -> Vec<VersionId> {
        self.children.get(id).map(|c| c.iter().copied().collect()).unwrap_or_default()
    }

    fn has_children(&self, id: &VersionId) -> bool {
        self.children.get(id).is_some_and(|c| !c.is_empty())
    }

    /// Insère une version après l'avoir validée contre l'état du groupe (voir les règles en tête de module).
    pub fn insert(&mut self, state: &GroupState, version: SignedVersion) -> Result<Inserted, VersionError> {
        let h = &version.header;
        h.validate()?;
        if h.group_id != state.id {
            return Err(VersionError::WrongGroup);
        }
        let id = h.id();
        if self.versions.contains_key(&id) {
            return Ok(Inserted::AlreadyKnown);
        }
        if !state.is_member(&h.author) {
            return Err(VersionError::NotMember);
        }
        if h.epoch > state.epoch {
            return Err(VersionError::FutureEpoch { version: h.epoch, current: state.epoch });
        }
        if !version.verify() {
            return Err(VersionError::BadSignature);
        }
        self.check_lineage(state, h)?;
        if let VersionKind::Restore { from } = h.kind {
            if let Some(src) = self.versions.get(&from) {
                if src.header.collection != h.collection || src.header.total_size != h.total_size {
                    return Err(VersionError::RestoreMismatch);
                }
            }
        }
        if self.versions.len() >= MAX_VERSIONS {
            return Err(VersionError::TooManyVersions);
        }

        let parent = h.parent;
        let header = h.clone();
        self.versions.insert(id, version);
        if let Some(p) = parent {
            self.children.entry(p).or_default().insert(id);
        }
        self.prune_inconsistent_children(&id, &header);
        Ok(Inserted::New)
    }

    fn check_lineage(&self, state: &GroupState, h: &VersionHeader) -> Result<(), VersionError> {
        let Some(pid) = h.parent else {
            // `validate` impose déjà numéro 1 et époque 0 sans parent.
            return Ok(());
        };
        match self.versions.get(&pid) {
            Some(p) => {
                let p = &p.header;
                if h.number != p.number + 1 {
                    return Err(VersionError::BadNumber { expected: p.number + 1, got: h.number });
                }
                let same_epoch = h.epoch == p.epoch;
                let epoch_start = h.epoch > p.epoch && state.base_for_epoch(h.epoch) == Some(pid);
                if same_epoch || epoch_start {
                    Ok(())
                } else {
                    Err(VersionError::BadEpoch)
                }
            }
            None if h.epoch > 0 && state.base_for_epoch(h.epoch) == Some(pid) => Ok(()),
            None => Err(VersionError::ParentUnknown(pid)),
        }
    }

    /// Une base arrivée après ses enfants : retire ceux qui ne collent pas (et leurs descendants).
    fn prune_inconsistent_children(&mut self, id: &VersionId, parent: &VersionHeader) {
        let bad: Vec<VersionId> = self
            .children_of(id)
            .into_iter()
            .filter(|c| {
                self.versions.get(c).is_some_and(|c| {
                    c.header.number != parent.number + 1 || c.header.epoch < parent.epoch
                })
            })
            .collect();
        for c in bad {
            tracing::warn!(version = %c, "version écartée : incohérente avec son parent arrivé après elle");
            self.remove_subtree(&c);
        }
    }

    fn remove_subtree(&mut self, root: &VersionId) {
        let mut stack = vec![*root];
        while let Some(id) = stack.pop() {
            stack.extend(self.children_of(&id));
            self.children.remove(&id);
            if let Some(v) = self.versions.remove(&id) {
                if let Some(p) = v.header.parent {
                    if let Some(set) = self.children.get_mut(&p) {
                        set.remove(&id);
                    }
                }
            }
        }
    }

    /// Têtes de l'époque courante, la plus récente d'abord (numéro décroissant, puis identifiant).
    ///
    /// Les versions des époques précédentes ne sont jamais tête ; si l'époque courante n'a encore aucune
    /// version, la tête est sa base (si on la connaît).
    pub fn heads(&self, state: &GroupState) -> Vec<&SignedVersion> {
        let mut heads: Vec<(&VersionId, &SignedVersion)> = self
            .versions
            .iter()
            .filter(|(id, v)| v.header.epoch == state.epoch && !self.has_children(id))
            .collect();
        if heads.is_empty() && state.epoch > 0 {
            if let Some(base) = state.base_for_epoch(state.epoch) {
                if let Some((id, v)) = self.versions.get_key_value(&base) {
                    heads.push((id, v));
                }
            }
        }
        heads.sort_by(|a, b| b.1.header.number.cmp(&a.1.header.number).then(a.0.cmp(b.0)));
        heads.into_iter().map(|(_, v)| v).collect()
    }

    /// Plusieurs têtes : le groupe doit choisir.
    pub fn is_fork(&self, state: &GroupState) -> bool {
        self.heads(state).len() > 1
    }

    /// Têtes qui comptent encore : les têtes brutes ([`VersionGraph::heads`]) sans celles qu'une résolution de fourche a
    /// absorbées. Une tête brute est absorbée quand l'ascendance d'une autre tête contient une `Restore` de son monde
    /// (même collection) : la résolution a posé cette `Restore` sur l'autre branche, puis la partie a continué
    /// dessus. Sans cette règle, la tête gardée et les `Restore` de la résolution redeviendraient une fourche à la
    /// première partie suivante. Même ordre que [`VersionGraph::heads`].
    pub fn open_heads(&self, state: &GroupState) -> Vec<&SignedVersion> {
        let raw = self.heads(state);
        if raw.len() < 2 {
            return raw;
        }
        let ids: Vec<VersionId> = raw.iter().map(|v| v.id()).collect();
        raw.into_iter()
            .enumerate()
            .filter(|(i, h)| {
                let hid = ids[*i];
                let h_is_restore = matches!(h.header.kind, VersionKind::Restore { .. });
                !ids.iter().enumerate().any(|(j, other)| {
                    if j == *i {
                        return false;
                    }
                    let anc = self.ancestry(other);
                    // Une `Restore` encore tête n'est absorbée que par un ancêtre strict d'une autre tête qui porte son
                    // monde (deux `Restore` jumelles ne s'absorbent pas l'une l'autre) ; une autre tête, par une
                    // `Restore` de son monde, y compris la tête elle-même.
                    let skip = usize::from(h_is_restore);
                    anc.iter()
                        .skip(skip)
                        .filter(|a| **a != hid)
                        .filter_map(|a| self.versions.get(a))
                        .any(|a| (h_is_restore || matches!(a.header.kind, VersionKind::Restore { .. })) && a.header.collection == h.header.collection)
                })
            })
            .map(|(_, h)| h)
            .collect()
    }

    /// Têtes distinctes à présenter : [`VersionGraph::open_heads`], une seule par monde (collection), la plus avancée.
    /// Plus d'une : fourche.
    pub fn effective_heads(&self, state: &GroupState) -> Vec<&SignedVersion> {
        let mut by_world: std::collections::BTreeMap<BlobHash, &SignedVersion> = std::collections::BTreeMap::new();
        for v in self.open_heads(state) {
            match by_world.get(&v.header.collection) {
                Some(cur) if (cur.header.number, std::cmp::Reverse(cur.id())) >= (v.header.number, std::cmp::Reverse(v.id())) => {}
                _ => {
                    by_world.insert(v.header.collection, v);
                }
            }
        }
        let mut out: Vec<&SignedVersion> = by_world.into_values().collect();
        out.sort_by(|a, b| b.header.number.cmp(&a.header.number).then(a.id().cmp(&b.id())));
        out
    }

    /// La tête la plus avancée (numéro le plus haut ; à égalité, l'identifiant le plus petit), si elle existe.
    pub fn latest_by_number(&self, state: &GroupState) -> Option<&SignedVersion> {
        self.heads(state).into_iter().next()
    }

    /// Ancêtres connus de `id`, `id` compris, du plus récent au plus ancien. Vide si `id` est inconnu.
    pub fn ancestry(&self, id: &VersionId) -> Vec<VersionId> {
        let mut out = Vec::new();
        let mut cur = Some(*id);
        while let Some(c) = cur {
            let Some(v) = self.versions.get(&c) else { break };
            out.push(c);
            cur = v.header.parent;
            if out.len() > self.versions.len() {
                break; // garde-fou : l'insertion garantit l'absence de cycle
            }
        }
        out
    }

    /// `ancestor` est-il `id` ou l'un de ses ancêtres connus ?
    pub fn is_ancestor(&self, ancestor: &VersionId, id: &VersionId) -> bool {
        self.ancestry(id).contains(ancestor)
    }

    /// Dernier point de reprise de l'époque courante signé par `author` (le plus haut numéro).
    pub fn latest_checkpoint_by(&self, state: &GroupState, author: &PeerKey) -> Option<&SignedVersion> {
        self.versions
            .iter()
            .filter(|(_, v)| {
                v.header.kind == VersionKind::Checkpoint
                    && v.header.author == *author
                    && v.header.epoch == state.epoch
            })
            .max_by(|a, b| a.1.header.number.cmp(&b.1.header.number).then(b.0.cmp(a.0)))
            .map(|(_, v)| v)
    }

    /// Pointes des époques révolues qui ne sont pas la tête : branches stockées, restaurables, jamais tête.
    /// Triées par numéro décroissant puis identifiant.
    pub fn side_branches(&self, state: &GroupState) -> Vec<SideBranch> {
        let heads = self.heads(state);
        let head_ids: BTreeSet<VersionId> = heads.iter().map(|v| v.id()).collect();
        let mainline: BTreeSet<VersionId> = if heads.len() == 1 {
            self.ancestry(&heads[0].id()).into_iter().collect()
        } else {
            BTreeSet::new()
        };
        let mut out: Vec<SideBranch> = self
            .versions
            .iter()
            .filter(|(id, v)| v.header.epoch < state.epoch && !self.has_children(id) && !head_ids.contains(id))
            .map(|(id, v)| SideBranch {
                tip: *id,
                author: v.header.author,
                number: v.header.number,
                epoch: v.header.epoch,
                fork_point: self.ancestry(id).into_iter().find(|a| mainline.contains(a)),
            })
            .collect();
        out.sort_by(|a, b| b.number.cmp(&a.number).then(a.tip.cmp(&b.tip)));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::super::model::testutil::{group, key};
    use super::super::model::{GroupOp, Proposal};
    use super::*;
    use proptest::prelude::*;

    fn pk(seed: u8) -> PeerKey {
        key(seed).peer_key()
    }

    fn header(state: &GroupState, author: u8, number: u64, parent: Option<VersionId>, epoch: u64, tag: u8) -> VersionHeader {
        VersionHeader {
            group_id: state.id,
            number,
            parent,
            epoch,
            author: pk(author),
            pack_hash: "ab".repeat(32),
            created_at: 1_700_000_000 + u64::from(tag),
            played_secs: u64::from(tag),
            kind: VersionKind::Session,
            collection: [tag; 32],
            total_size: 1000 + u64::from(tag),
        }
    }

    fn signed(h: VersionHeader) -> SignedVersion {
        let seed = (1..=10u8).find(|s| pk(*s) == h.author).unwrap();
        SignedVersion::sign(&key(seed), h).unwrap()
    }

    fn ins(g: &mut VersionGraph, st: &GroupState, author: u8, number: u64, parent: Option<VersionId>, tag: u8) -> VersionId {
        let v = signed(header(st, author, number, parent, st.epoch, tag));
        let id = v.id();
        g.insert(st, v).unwrap();
        id
    }

    fn continued(st: &GroupState, absent: u8, base: VersionId, voters: &[u8]) -> GroupState {
        let mut p = Proposal::new(st, GroupOp::ContinueWithout { absent: pk(absent), base_version: base, absent_since: 1_000, at: 1_000 + crate::hearth::model::MIN_ABSENCE_SECS }).unwrap();
        for v in voters {
            p.vote(st, &key(*v)).unwrap();
        }
        let op = p.into_signed(st).unwrap();
        let mut next = st.clone();
        next.apply(&op).unwrap();
        next
    }

    #[test]
    fn id_signature_and_json() {
        let st = group(&[1, 2]);
        let v = signed(header(&st, 1, 1, None, 0, 1));
        assert!(v.verify());
        assert_eq!(v.id(), VersionId(*blake3::hash(&v.header.canonical()).as_bytes()));
        let text = String::from_utf8(v.header.canonical()).unwrap();
        assert!(text.starts_with(r#"{"group_id":"07070707070707070707070707070707","number":1,"parent":null,"epoch":0,"author":""#), "{text}");
        assert!(text.contains(r#""kind":{"kind":"session"}"#), "{text}");
        let json = serde_json::to_string(&v).unwrap();
        let back: SignedVersion = serde_json::from_str(&json).unwrap();
        assert_eq!(back, v);
        assert_eq!(back.id(), v.id());
        // Un autre en-tête, un autre identifiant.
        assert_ne!(signed(header(&st, 1, 1, None, 0, 2)).id(), v.id());
    }

    #[test]
    fn signature_binds_domain_and_author() {
        let st = group(&[1, 2]);
        let v = signed(header(&st, 1, 1, None, 0, 1));
        let mut other_author = v.clone();
        other_author.header.author = pk(2);
        assert!(!other_author.verify());
        let mut tampered = v.clone();
        tampered.header.total_size += 1;
        assert!(!tampered.verify());
        let mut g = VersionGraph::new();
        assert_eq!(g.insert(&st, tampered).unwrap_err(), VersionError::BadSignature);
        // Signature produite avec un autre domaine.
        let wrong = SignedVersion { header: v.header.clone(), sig: sign_domain(&key(1), b"cubelink/hearth/op/1", &v.header.canonical()) };
        assert_eq!(g.insert(&st, wrong).unwrap_err(), VersionError::BadSignature);
        // On ne signe pas au nom d'un autre.
        assert_eq!(SignedVersion::sign(&key(2), v.header.clone()).unwrap_err(), VersionError::BadSignature);
        assert!(g.is_empty());
    }

    #[test]
    fn header_validation() {
        let st = group(&[1, 2]);
        let base = header(&st, 1, 1, None, 0, 1);
        let mut h = base.clone();
        h.number = 2;
        assert!(matches!(h.validate(), Err(VersionError::Invalid(_))), "sans parent, numéro 1 obligatoire");
        let mut h = base.clone();
        h.number = 0;
        assert!(h.validate().is_err());
        let mut h = base.clone();
        h.parent = Some(VersionId([1; 32]));
        assert!(h.validate().is_err(), "avec parent, numéro >= 2");
        let mut h = base.clone();
        h.pack_hash = "zz".into();
        assert!(h.validate().is_err());
        let mut h = base.clone();
        h.total_size = MAX_TOTAL_SIZE + 1;
        assert!(h.validate().is_err());
        let mut h = base.clone();
        h.epoch = 1;
        assert!(h.validate().is_err(), "sans parent, époque 0");
        let mut h = base.clone();
        h.kind = VersionKind::Restore { from: VersionId([5; 32]) };
        assert!(h.validate().is_err(), "restauration sans parent");
        let mut h = header(&st, 1, 2, Some(VersionId([5; 32])), 0, 1);
        h.kind = VersionKind::Restore { from: VersionId([5; 32]) };
        assert!(h.validate().is_err(), "restauration du parent");
        assert!(base.validate().is_ok());
        // JSON : champ inconnu refusé.
        let mut v = serde_json::to_value(signed(base)).unwrap();
        v["header"]["bonus"] = serde_json::json!(1);
        assert!(serde_json::from_value::<SignedVersion>(v).is_err());
    }

    #[test]
    fn insert_rules() {
        let st = group(&[1, 2, 3]);
        let mut g = VersionGraph::new();
        let a = signed(header(&st, 1, 1, None, 0, 1));
        let aid = a.id();
        assert_eq!(g.insert(&st, a.clone()).unwrap(), Inserted::New);
        assert_eq!(g.insert(&st, a).unwrap(), Inserted::AlreadyKnown);
        assert_eq!(g.len(), 1);

        // Auteur non membre, autre groupe, époque future.
        let stranger = SignedVersion::sign(&key(9), header(&st, 9, 1, None, 0, 7)).unwrap();
        assert_eq!(g.insert(&st, stranger).unwrap_err(), VersionError::NotMember);
        let mut other = header(&st, 2, 1, None, 0, 8);
        other.group_id = GroupId([1; 16]);
        assert_eq!(g.insert(&st, signed(other)).unwrap_err(), VersionError::WrongGroup);
        let mut future = header(&st, 2, 2, Some(aid), 1, 9);
        future.epoch = 1;
        assert!(matches!(g.insert(&st, signed(future)).unwrap_err(), VersionError::FutureEpoch { version: 1, current: 0 }));

        // Parent inconnu, mauvais numéro.
        let orphan = signed(header(&st, 2, 3, Some(VersionId([42; 32])), 0, 10));
        assert!(matches!(g.insert(&st, orphan).unwrap_err(), VersionError::ParentUnknown(_)));
        let skip = signed(header(&st, 2, 3, Some(aid), 0, 11));
        assert_eq!(g.insert(&st, skip).unwrap_err(), VersionError::BadNumber { expected: 2, got: 3 });
        let ok = signed(header(&st, 2, 2, Some(aid), 0, 12));
        let okid = ok.id();
        g.insert(&st, ok).unwrap();
        assert_eq!(g.ancestry(&okid), vec![okid, aid]);
        assert!(g.is_ancestor(&aid, &okid));
        assert!(!g.is_ancestor(&okid, &aid));
        assert_eq!(g.children_of(&aid), vec![okid]);
        assert_eq!(g.len(), 2);
    }

    #[test]
    fn fork_and_heads() {
        let st = group(&[1, 2, 3]);
        let mut g = VersionGraph::new();
        assert!(g.heads(&st).is_empty());
        assert!(g.latest_by_number(&st).is_none());
        let v1 = ins(&mut g, &st, 1, 1, None, 1);
        let v2 = ins(&mut g, &st, 1, 2, Some(v1), 2);
        assert_eq!(g.heads(&st).len(), 1);
        assert!(!g.is_fork(&st));
        // Deux joueurs prolongent v2 chacun de leur côté : fourche.
        let a3 = ins(&mut g, &st, 2, 3, Some(v2), 3);
        let b3 = ins(&mut g, &st, 3, 3, Some(v2), 4);
        assert!(g.is_fork(&st));
        let heads: BTreeSet<VersionId> = g.heads(&st).iter().map(|v| v.id()).collect();
        assert_eq!(heads, [a3, b3].into_iter().collect());
        // Plus avancée : v4 sur a3 ; la tête b3 reste (fourche), le plus haut numéro est v4.
        let a4 = ins(&mut g, &st, 2, 4, Some(a3), 5);
        assert_eq!(g.latest_by_number(&st).unwrap().id(), a4);
        assert!(g.is_fork(&st));
        assert_eq!(g.heads(&st).len(), 2);
        // Deux racines : fourche aussi.
        let mut g2 = VersionGraph::new();
        ins(&mut g2, &st, 1, 1, None, 1);
        ins(&mut g2, &st, 2, 1, None, 2);
        assert!(g2.is_fork(&st));
    }

    #[test]
    fn restore_must_match_source_when_known() {
        let st = group(&[1, 2]);
        let mut g = VersionGraph::new();
        let v1 = ins(&mut g, &st, 1, 1, None, 1);
        let v2 = ins(&mut g, &st, 1, 2, Some(v1), 2);
        let src = g.get(&v1).unwrap().header.clone();
        let mut r = header(&st, 2, 3, Some(v2), 0, 3);
        r.kind = VersionKind::Restore { from: v1 };
        r.collection = src.collection;
        r.total_size = src.total_size;
        let r_ok = signed(r.clone());
        g.insert(&st, r_ok).unwrap();
        let mut bad = header(&st, 2, 4, Some(g.heads(&st)[0].id()), 0, 9);
        bad.kind = VersionKind::Restore { from: v1 };
        assert_eq!(g.insert(&st, signed(bad)).unwrap_err(), VersionError::RestoreMismatch);
    }

    #[test]
    fn continue_without_sets_aside_absent_branch() {
        let st0 = group(&[1, 2, 3]);
        let mut g = VersionGraph::new();
        let v1 = ins(&mut g, &st0, 1, 1, None, 1);
        let v2 = ins(&mut g, &st0, 2, 2, Some(v1), 2);
        // L'absent (1) a joué sans que les autres le sachent : v3 sur v2, jamais reçu des autres.
        let absent_v3 = ins(&mut g, &st0, 1, 3, Some(v2), 3);
        // Les membres restants repartent de v2.
        let st1 = continued(&st0, 1, v2, &[2, 3]);
        assert_eq!(st1.epoch, 1);
        // La tête est la version de l'époque 0 sans enfant ... mais l'époque courante n'a pas encore de version : base.
        let heads = g.heads(&st1);
        assert_eq!(heads.len(), 1);
        assert_eq!(heads[0].id(), v2);
        let side = g.side_branches(&st1);
        assert_eq!(side.len(), 1);
        assert_eq!(side[0].tip, absent_v3);
        assert_eq!(side[0].fork_point, Some(v2));
        assert_eq!(side[0].author, pk(1));
        // Une nouvelle version de l'époque 1 sur la base : seule tête ; la branche de l'absent reste de côté.
        let n3 = signed(header(&st1, 2, 3, Some(v2), 1, 20));
        let n3id = n3.id();
        g.insert(&st1, n3).unwrap();
        assert_eq!(g.heads(&st1).iter().map(|v| v.id()).collect::<Vec<_>>(), vec![n3id]);
        assert!(!g.is_fork(&st1));
        let side = g.side_branches(&st1);
        assert_eq!(side.iter().map(|s| s.tip).collect::<Vec<_>>(), vec![absent_v3]);
        // Une version tardive de l'absent, époque 0, est stockée mais jamais tête.
        let late = signed(header(&st1, 1, 4, Some(absent_v3), 0, 21));
        let lid = late.id();
        g.insert(&st1, late).unwrap();
        assert_eq!(g.heads(&st1).len(), 1);
        let side = g.side_branches(&st1);
        assert_eq!(side.iter().map(|s| s.tip).collect::<Vec<_>>(), vec![lid]);
        assert_eq!(side[0].fork_point, Some(v2));
        // Une version de l'époque 1 doit partir de la base de l'époque (ou d'une version de l'époque 1).
        let bad = signed(header(&st1, 2, 4, Some(absent_v3), 1, 22));
        assert_eq!(g.insert(&st1, bad).unwrap_err(), VersionError::BadEpoch);
        let ok = signed(header(&st1, 3, 4, Some(n3id), 1, 23));
        g.insert(&st1, ok).unwrap();
        assert_eq!(g.ancestry(&g.heads(&st1)[0].id()).len(), 4);
    }

    #[test]
    fn base_unknown_then_arriving() {
        let st0 = group(&[1, 2, 3]);
        // Construit la base X en dehors du graphe pour connaître son identifiant.
        let x = signed(header(&st0, 1, 1, None, 0, 1));
        let xid = x.id();
        let st1 = continued(&st0, 3, xid, &[1, 2]);
        let mut g = VersionGraph::new();
        // Parent inconnu mais base de l'époque : accepté.
        let child_ok = signed(header(&st1, 2, 2, Some(xid), 1, 2));
        let cid = child_ok.id();
        g.insert(&st1, child_ok).unwrap();
        // Un enfant au numéro incohérent est aussi accepté (la base est inconnue)…
        let child_bad = signed(header(&st1, 1, 5, Some(xid), 1, 3));
        let bid = child_bad.id();
        g.insert(&st1, child_bad).unwrap();
        // … avec un parent quelconque inconnu, non.
        let nope = signed(header(&st1, 1, 2, Some(VersionId([9; 32])), 1, 4));
        assert!(matches!(g.insert(&st1, nope).unwrap_err(), VersionError::ParentUnknown(_)));
        assert_eq!(g.heads(&st1).len(), 2);
        // La base arrive : l'enfant incohérent disparaît, l'autre reste.
        g.insert(&st1, x).unwrap();
        assert!(g.contains(&cid));
        assert!(!g.contains(&bid));
        assert_eq!(g.heads(&st1).iter().map(|v| v.id()).collect::<Vec<_>>(), vec![cid]);
    }

    #[test]
    fn latest_checkpoint_by_author() {
        let st = group(&[1, 2]);
        let mut g = VersionGraph::new();
        let v1 = ins(&mut g, &st, 1, 1, None, 1);
        let mut c = header(&st, 2, 2, Some(v1), 0, 2);
        c.kind = VersionKind::Checkpoint;
        let c1 = signed(c);
        let c1id = c1.id();
        g.insert(&st, c1).unwrap();
        let mut c = header(&st, 2, 3, Some(c1id), 0, 3);
        c.kind = VersionKind::Checkpoint;
        let c2 = signed(c);
        let c2id = c2.id();
        g.insert(&st, c2).unwrap();
        assert_eq!(g.latest_checkpoint_by(&st, &pk(2)).unwrap().id(), c2id);
        assert!(g.latest_checkpoint_by(&st, &pk(1)).is_none());
    }

    #[test]
    fn version_cap() {
        // Le plafond est testé via un graphe artificiellement rempli, pour ne pas signer 50 000 versions.
        let st = group(&[1]);
        let mut g = VersionGraph::new();
        let v1 = signed(header(&st, 1, 1, None, 0, 1));
        for i in 0..MAX_VERSIONS {
            let mut id = [0u8; 32];
            id[..8].copy_from_slice(&(i as u64).to_be_bytes());
            id[31] = 0xAA;
            g.versions.insert(VersionId(id), v1.clone());
        }
        let v2 = signed(header(&st, 1, 1, None, 0, 2));
        assert_eq!(g.insert(&st, v2).unwrap_err(), VersionError::TooManyVersions);
    }

    // --- propriétés ---------------------------------------------------------------------------------------

    /// Graphe aléatoire : le nœud `i` a pour parent un nœud précédent, ou est une nouvelle racine.
    fn build(choices: &[usize]) -> (GroupState, Vec<SignedVersion>) {
        let st = group(&[1, 2, 3]);
        let mut out: Vec<SignedVersion> = Vec::new();
        for (i, c) in choices.iter().enumerate() {
            let pick = c % (i + 1);
            let author = 1 + (i % 3) as u8;
            let (number, parent) = if pick == i { (1, None) } else { (out[pick].header.number + 1, Some(out[pick].id())) };
            out.push(signed(header(&st, author, number, parent, 0, i as u8)));
        }
        (st, out)
    }

    #[test]
    fn une_fourche_resolue_le_reste_apres_une_partie() {
        let st = group(&[1, 2]);
        let mut g = VersionGraph::new();
        let root = ins(&mut g, &st, 1, 1, None, 1);
        // Alice (K) et Bob (D) jouent chacun de leur côté depuis la racine.
        let k = ins(&mut g, &st, 1, 2, Some(root), 10);
        let d = ins(&mut g, &st, 2, 2, Some(root), 20);
        assert_eq!(g.effective_heads(&st).len(), 2, "deux mondes : fourche");
        // Alice garde K : `Restore(K)` posée sur D (même collection que K, numéro plus haut).
        let mut r = header(&st, 1, 3, Some(d), st.epoch, 10);
        r.kind = VersionKind::Restore { from: k };
        let r_id = signed(r).id();
        g.insert(&st, signed({
            let mut h = header(&st, 1, 3, Some(d), st.epoch, 10);
            h.kind = VersionKind::Restore { from: k };
            h
        }))
        .unwrap();
        let eff: Vec<VersionId> = g.effective_heads(&st).iter().map(|v| v.id()).collect();
        assert_eq!(eff, vec![r_id], "une seule tête juste après la résolution");
        // La partie continue sur la tête effective : la version gardée K reste une tête brute, mais absorbée.
        let s1 = ins(&mut g, &st, 1, 4, Some(r_id), 30);
        assert!(g.heads(&st).iter().any(|v| v.id() == k), "K n'a toujours pas d'enfant");
        let eff: Vec<VersionId> = g.effective_heads(&st).iter().map(|v| v.id()).collect();
        assert_eq!(eff, vec![s1], "plus de fourche après la première partie suivante");
        assert!(!g.is_fork(&st) || g.effective_heads(&st).len() == 1);
    }

    #[test]
    fn une_fourche_a_trois_branches_resolue_reste_resolue() {
        let st = group(&[1, 2, 3]);
        let mut g = VersionGraph::new();
        let root = ins(&mut g, &st, 1, 1, None, 1);
        let a = ins(&mut g, &st, 1, 2, Some(root), 10);
        let b = ins(&mut g, &st, 2, 2, Some(root), 20);
        let c = ins(&mut g, &st, 3, 2, Some(root), 30);
        assert_eq!(g.effective_heads(&st).len(), 3);
        // Garde A : une `Restore` sur B (R1) et une sur C (R2).
        let restore = |g: &mut VersionGraph, parent: VersionId| {
            let mut h = header(&st, 1, 3, Some(parent), st.epoch, 10);
            h.kind = VersionKind::Restore { from: a };
            let v = signed(h);
            let id = v.id();
            g.insert(&st, v).unwrap();
            id
        };
        let r1 = restore(&mut g, b);
        let r2 = restore(&mut g, c);
        assert_eq!(g.effective_heads(&st).len(), 1);
        // Une partie sur la tête effective : ni A, ni la `Restore` restée à côté ne ramènent la fourche.
        let top = g.effective_heads(&st)[0].id();
        assert!(top == r1 || top == r2);
        let s = ins(&mut g, &st, 1, 4, Some(top), 40);
        assert_eq!(g.effective_heads(&st).iter().map(|v| v.id()).collect::<Vec<_>>(), vec![s]);
        // Une vraie nouvelle fourche, elle, reste visible.
        let e = ins(&mut g, &st, 2, 4, Some(top), 50);
        let mut eff: Vec<VersionId> = g.effective_heads(&st).iter().map(|v| v.id()).collect();
        eff.sort();
        let mut want = vec![s, e];
        want.sort();
        assert_eq!(eff, want);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]

        #[test]
        fn heads_are_exactly_the_leaves(choices in prop::collection::vec(any::<usize>(), 1..24)) {
            let (st, versions) = build(&choices);
            let mut g = VersionGraph::new();
            for v in &versions {
                prop_assert_eq!(g.insert(&st, v.clone()).unwrap(), Inserted::New);
            }
            let parents: BTreeSet<VersionId> = versions.iter().filter_map(|v| v.header.parent).collect();
            let leaves: BTreeSet<VersionId> = versions.iter().map(|v| v.id()).filter(|id| !parents.contains(id)).collect();
            let heads: BTreeSet<VersionId> = g.heads(&st).iter().map(|v| v.id()).collect();
            prop_assert_eq!(&heads, &leaves);
            prop_assert_eq!(g.is_fork(&st), leaves.len() > 1);
            let max = versions.iter().filter(|v| leaves.contains(&v.id())).map(|v| v.header.number).max().unwrap();
            prop_assert_eq!(g.latest_by_number(&st).unwrap().header.number, max);
            for v in &versions {
                prop_assert_eq!(g.ancestry(&v.id()).len() as u64, v.header.number);
            }
            prop_assert!(g.side_branches(&st).is_empty());
        }

        #[test]
        fn insertion_order_does_not_change_heads(choices in prop::collection::vec(any::<usize>(), 1..24), salt in any::<u64>()) {
            let (st, versions) = build(&choices);
            // Ordre topologique différent : par numéro croissant, puis un ordre pseudo-aléatoire.
            let mut shuffled = versions.clone();
            shuffled.sort_by_key(|v| (v.header.number, (u64::from(v.header.created_at as u32)).wrapping_mul(salt | 1)));
            let mut a = VersionGraph::new();
            for v in &versions { a.insert(&st, v.clone()).unwrap(); }
            let mut b = VersionGraph::new();
            for v in &shuffled { b.insert(&st, v.clone()).unwrap(); }
            let ids = |g: &VersionGraph| g.heads(&st).iter().map(|v| v.id()).collect::<Vec<_>>();
            prop_assert_eq!(ids(&a), ids(&b));
            prop_assert_eq!(a.len(), b.len());
        }

        #[test]
        fn children_before_parents_are_refused(choices in prop::collection::vec(any::<usize>(), 2..16)) {
            let (st, versions) = build(&choices);
            // Le premier nœud avec parent, inséré dans un graphe vide : refusé.
            if let Some(v) = versions.iter().find(|v| v.header.parent.is_some()) {
                let mut g = VersionGraph::new();
                prop_assert!(matches!(g.insert(&st, v.clone()), Err(VersionError::ParentUnknown(_))));
            }
        }
    }
}
