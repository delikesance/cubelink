//! Hearth : stockage disque d'un groupe (voir `docs/hearth.md`, § 5).
//!
//! ```text
//! <racine>/<group_id hex>/
//!   ops.jsonl            chaîne d'opérations signées (une par ligne)
//!   versions/<id>.json   en-têtes de version signés
//!   proposals/<id>.json  propositions en attente de votes
//!   group.key            clé de groupe (32 octets aléatoires, jamais dans une opération)
//!   world/ outbox/ server/   (réservés : monde matérialisé, file d'envoi, serveur dédié)
//! ```
//!
//! * Toute écriture est **atomique** : fichier temporaire dans le même dossier, `fsync`, puis `rename`. Un arrêt
//!   brutal laisse l'ancien contenu ou le nouveau, jamais un fichier tronqué.
//! * Rien n'est cru sur parole à la relecture : la chaîne passe par [`replay`] (numéros, chaînage, signatures,
//!   gouvernance) et chaque version par [`VersionGraph::insert`]. Une chaîne invalide est une erreur
//!   ([`StoreError::Corrupt`]) ; une version ou une proposition invalide est écartée et listée
//!   ([`Group::skipped`]) sans bloquer le groupe.
//! * Les tailles lues sur le disque sont bornées, comme celles qui viennent du réseau.
//! * Tout est synchrone et rapide (quelques petits fichiers) ; un appelant asynchrone peut passer par
//!   `spawn_blocking` pour un monde de plusieurs milliers de versions.
//!
//! # Anciens membres
//!
//! Une version est valable si son auteur est membre **au moment où on l'insère** ([`VersionGraph::insert`]).
//! Quand un membre part, ses anciennes versions restent pourtant les ancêtres de la tête : sans précaution, un
//! nouvel arrivant ne pourrait plus ingérer la chaîne. [`Group::add_versions`] accepte donc la version d'un ancien
//! membre si, et seulement si, elle est l'ancêtre d'une version signée par un membre actuel (celui-ci a repris sa
//! lignée dans son propre en-tête signé). Une version d'un ancien membre qui n'a aucun descendant de ce genre reste
//! refusée : un membre retiré ne peut pas continuer à alimenter le groupe.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use rand::RngCore;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use super::model::{
    decode_array, replay, GroupId, GroupOp, GroupState, ModelError, OpHash, Proposal, SignedOp, Sig, MAX_MEMBERS, MAX_OPS,
};
use super::version::{Inserted, SignedVersion, VersionError, VersionGraph, VersionId, MAX_VERSIONS};
use crate::presence::PeerKey;

/// Taille maximale d'un fichier relu (en-tête, proposition) : au-delà, le fichier n'est pas de nous.
const MAX_FILE_BYTES: u64 = 64 * 1024;
/// Taille maximale de `ops.jsonl` (`MAX_OPS` opérations d'au plus 64 Kio chacune seraient absurdes ; 256 Mio de garde-fou).
const MAX_OPS_FILE_BYTES: u64 = 256 * 1024 * 1024;
/// Propositions en attente conservées par groupe (les plus anciennes sont évincées, en commençant par celles de l'auteur
/// qui en a le plus).
pub const MAX_PROPOSALS: usize = 10;
/// Durée de vie d'une proposition sans issue : 14 jours, comptés sur l'heure locale de réception.
pub const PROPOSAL_TTL_SECS: u64 = 14 * 24 * 3600;
/// `Propose` acceptés d'une même clé par heure.
pub const MAX_PROPOSE_PER_HOUR: usize = 5;

/// Erreurs de stockage (messages affichables).
#[derive(Debug, Error)]
pub enum StoreError {
    #[error("erreur disque ({context}) : {source}")]
    Io {
        context: String,
        #[source]
        source: io::Error,
    },
    #[error("données du groupe corrompues : {0}")]
    Corrupt(String),
    #[error("ce groupe existe déjà")]
    AlreadyExists,
    #[error("groupe introuvable")]
    NotFound,
    #[error("la chaîne d'opérations reçue diverge de la nôtre")]
    Diverged,
    #[error("trop de propositions en peu de temps de la part de ce membre")]
    TooManyProposals,
    #[error("la proposition n'est pas signée par son auteur")]
    ProposerNotSigned,
    #[error("la version de départ choisie ne convient pas : {0}")]
    BadContinueBase(&'static str),
    #[error("proposition inconnue")]
    UnknownProposal,
    #[error(transparent)]
    Model(#[from] ModelError),
    #[error(transparent)]
    Version(#[from] VersionError),
}

fn io_err(context: impl Into<String>) -> impl FnOnce(io::Error) -> StoreError {
    let context = context.into();
    move |source| StoreError::Io { context, source }
}

// ---------------------------------------------------------------------------------------------------------
// Clé de groupe
// ---------------------------------------------------------------------------------------------------------

/// Clé de groupe (32 octets) : chiffre les points de reprise confiés aux gardiens. Transmise sur la connexion QUIC
/// authentifiée (`GroupInvite`), stockée dans `group.key`. Son `Debug` ne la montre jamais.
#[derive(Clone, PartialEq, Eq)]
pub struct GroupKey([u8; 32]);

impl GroupKey {
    /// Nouvelle clé aléatoire (système).
    pub fn random() -> Self {
        let mut b = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut b);
        Self(b)
    }

    pub fn from_bytes(b: [u8; 32]) -> Self {
        Self(b)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for GroupKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("GroupKey(..)")
    }
}

impl Serialize for GroupKey {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(self.0))
    }
}

impl<'de> Deserialize<'de> for GroupKey {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let s = String::deserialize(d)?;
        decode_array::<32>(&s).map(Self).map_err(D::Error::custom)
    }
}

// ---------------------------------------------------------------------------------------------------------
// Écriture et lecture de fichiers
// ---------------------------------------------------------------------------------------------------------

/// Écrit `data` dans `path` de façon atomique (fichier temporaire voisin, `fsync`, `rename`).
fn write_atomic(path: &Path, data: &[u8], private: bool) -> io::Result<()> {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("fichier");
    let tmp = path.with_file_name(format!(".{name}.tmp-{:016x}", rand::rngs::OsRng.next_u64()));
    let write = || -> io::Result<()> {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        if private {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        #[cfg(not(unix))]
        let _ = private;
        let mut f = opts.open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        fs::rename(&tmp, path)
    };
    let r = write();
    if r.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    r
}

/// Lit un fichier d'au plus `max` octets.
fn read_bounded(path: &Path, max: u64) -> Result<Vec<u8>, StoreError> {
    let meta = fs::metadata(path).map_err(io_err(format!("lecture de {}", path.display())))?;
    if meta.len() > max {
        return Err(StoreError::Corrupt(format!("{} est trop gros ({} octets)", path.display(), meta.len())));
    }
    fs::read(path).map_err(io_err(format!("lecture de {}", path.display())))
}

fn json_line(op: &SignedOp) -> Vec<u8> {
    // Types simples (chaînes, entiers, énumérations) : la sérialisation ne peut pas échouer.
    let mut v = serde_json::to_vec(op).expect("sérialisation JSON de types simples");
    v.push(b'\n');
    v
}

fn parse_ops(bytes: &[u8]) -> Result<Vec<SignedOp>, StoreError> {
    let mut ops = Vec::new();
    for (i, line) in bytes.split(|b| *b == b'\n').enumerate() {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        if ops.len() >= MAX_OPS {
            return Err(StoreError::Model(ModelError::TooManyOps));
        }
        let op: SignedOp =
            serde_json::from_slice(line).map_err(|e| StoreError::Corrupt(format!("ops.jsonl, ligne {} : {e}", i + 1)))?;
        ops.push(op);
    }
    Ok(ops)
}

/// Ensemble des membres que le groupe a eus un jour (pour relire les versions d'anciens membres).
fn ever_members(ops: &[SignedOp]) -> Result<BTreeSet<PeerKey>, ModelError> {
    let (first, rest) = ops.split_first().ok_or(ModelError::EmptyChain)?;
    let mut state = GroupState::from_create(first)?;
    let mut all = state.members.clone();
    for op in rest {
        state.apply(op)?;
        all.extend(state.members.iter().copied());
    }
    Ok(all)
}

// ---------------------------------------------------------------------------------------------------------
// Rapport d'ingestion de versions
// ---------------------------------------------------------------------------------------------------------

/// Résultat de [`Group::add_versions`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct VersionsReport {
    /// Versions nouvelles, dans l'ordre d'insertion.
    pub added: Vec<VersionId>,
    /// Versions déjà connues.
    pub known: usize,
    /// Versions refusées, avec la raison (affichable).
    pub rejected: Vec<(VersionId, String)>,
}

/// Insère `batch` dans `graph` : `strict` est l'état du groupe, `loose` celui qui admet aussi les anciens membres
/// (voir l'en-tête du module). Rend les versions réellement ajoutées (dans l'ordre) et le rapport.
fn insert_batch(
    graph: &mut VersionGraph,
    strict: &GroupState,
    loose: &GroupState,
    batch: Vec<SignedVersion>,
) -> (Vec<SignedVersion>, VersionsReport) {
    let mut report = VersionsReport::default();
    let mut accepted: Vec<SignedVersion> = Vec::new();
    // Dédoublonne et trie : les parents d'abord (numéro croissant).
    let mut pending: BTreeMap<VersionId, SignedVersion> = BTreeMap::new();
    for v in batch {
        pending.entry(v.id()).or_insert(v);
    }
    let mut order: Vec<VersionId> = pending.keys().copied().collect();
    order.sort_by_key(|id| (pending[id].header.number, *id));
    let mut pending: Vec<(VersionId, SignedVersion)> = order.into_iter().filter_map(|id| pending.remove(&id).map(|v| (id, v))).collect();

    let mut ex_member: BTreeSet<VersionId> = BTreeSet::new();
    loop {
        // Passes sur les versions en attente, jusqu'à ce qu'aucune n'avance.
        let mut progress = true;
        while progress {
            progress = false;
            let mut still = Vec::new();
            for (id, v) in std::mem::take(&mut pending) {
                match graph.insert(strict, v.clone()) {
                    Ok(Inserted::New) => {
                        report.added.push(id);
                        accepted.push(v);
                        progress = true;
                    }
                    Ok(Inserted::AlreadyKnown) => {
                        report.known += 1;
                        progress = true;
                    }
                    Err(VersionError::ParentUnknown(_)) => still.push((id, v)),
                    Err(VersionError::NotMember) if loose.is_member(&v.header.author) => {
                        ex_member.insert(id);
                        still.push((id, v));
                    }
                    Err(e) => {
                        report.rejected.push((id, e.to_string()));
                        progress = true;
                    }
                }
            }
            pending = still;
        }
        if ex_member.is_empty() {
            break;
        }
        // Versions d'anciens membres : admises si une version d'un membre actuel les a pour ancêtre.
        let by_id: BTreeMap<VersionId, &SignedVersion> = pending.iter().map(|(id, v)| (*id, v)).collect();
        let mut endorsed: BTreeSet<VersionId> = BTreeSet::new();
        for (id, v) in &pending {
            if ex_member.contains(id) || !strict.is_member(&v.header.author) || !v.verify() {
                continue;
            }
            let mut cur = v.header.parent;
            while let Some(p) = cur {
                if !by_id.contains_key(&p) || !endorsed.insert(p) {
                    break;
                }
                cur = by_id[&p].header.parent;
            }
        }
        let mut next = Vec::new();
        let mut admitted = false;
        let mut rest = Vec::new();
        for (id, v) in std::mem::take(&mut pending) {
            if ex_member.contains(&id) && endorsed.contains(&id) {
                next.push((id, v));
            } else {
                rest.push((id, v));
            }
        }
        for (id, v) in next {
            match graph.insert(loose, v.clone()) {
                Ok(Inserted::New) => {
                    report.added.push(id);
                    accepted.push(v);
                    admitted = true;
                }
                Ok(Inserted::AlreadyKnown) => {
                    report.known += 1;
                    admitted = true;
                }
                Err(VersionError::ParentUnknown(_)) => rest.push((id, v)),
                Err(e) => {
                    report.rejected.push((id, e.to_string()));
                    admitted = true;
                }
            }
            ex_member.remove(&id);
        }
        pending = rest;
        if !admitted {
            break;
        }
    }
    for (id, v) in pending {
        let why = if ex_member.contains(&id) {
            VersionError::NotMember.to_string()
        } else {
            VersionError::ParentUnknown(v.header.parent.unwrap_or(id)).to_string()
        };
        report.rejected.push((id, why));
    }
    (accepted, report)
}

// ---------------------------------------------------------------------------------------------------------
// Groupe
// ---------------------------------------------------------------------------------------------------------

/// Statut d'une proposition après un ajout ou un vote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProposalStatus {
    pub id: OpHash,
    /// Votes éligibles reçus.
    pub approvals: usize,
    /// Votes nécessaires.
    pub needed: usize,
    /// Le seuil est atteint : [`Group::finalize`] peut la transformer en opération.
    pub ready: bool,
}

/// Un groupe ouvert : état, chaîne, graphe des versions, clé et propositions, avec leur copie sur le disque.
/// Chaque méthode qui modifie le groupe est tout ou rien (mémoire et disque restent cohérents) ; l'appelant
/// protège le `Group` par un verrou (`Mutex`) s'il est partagé.
#[derive(Debug)]
pub struct Group {
    dir: PathBuf,
    state: GroupState,
    ops: Vec<SignedOp>,
    ever: BTreeSet<PeerKey>,
    graph: VersionGraph,
    key: GroupKey,
    proposals: BTreeMap<OpHash, Proposal>,
    /// Heure locale (secondes Unix) de réception de chaque proposition : âge, éviction. Pas persistée : au rechargement,
    /// la date du fichier fait foi.
    proposal_times: BTreeMap<OpHash, u64>,
    /// Heures locales des `Propose` reçus, par clé émettrice : au plus [`MAX_PROPOSE_PER_HOUR`] par heure.
    propose_log: BTreeMap<PeerKey, Vec<u64>>,
    skipped: Vec<String>,
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Ce que vise une proposition : deux propositions du même auteur sur la même cible se remplacent.
fn proposal_target(op: &GroupOp) -> String {
    match op {
        GroupOp::Create { .. } => "create".into(),
        GroupOp::AddMember { key } | GroupOp::RemoveMember { key } => format!("membre:{}", hex::encode(key)),
        GroupOp::ContinueWithout { absent, .. } => format!("absent:{}", hex::encode(absent)),
        GroupOp::PackUpdate { .. } => "pack".into(),
        GroupOp::SetGovernance { .. } => "gouvernance".into(),
        GroupOp::Leave => "depart".into(),
    }
}

impl Group {
    pub fn id(&self) -> GroupId {
        self.state.id
    }

    pub fn state(&self) -> &GroupState {
        &self.state
    }

    /// Chaîne d'opérations complète.
    pub fn ops(&self) -> &[SignedOp] {
        &self.ops
    }

    /// Au plus `limit` opérations de numéro supérieur à `after`.
    pub fn ops_after(&self, after: u64, limit: usize) -> Vec<SignedOp> {
        // Les opérations sont numérotées 1, 2, … sans trou : l'opération `n` est à l'indice `n - 1`.
        let start = usize::try_from(after).unwrap_or(usize::MAX).min(self.ops.len());
        self.ops[start..].iter().take(limit).cloned().collect()
    }

    pub fn graph(&self) -> &VersionGraph {
        &self.graph
    }

    /// Têtes actuelles du groupe (identifiants).
    pub fn heads(&self) -> Vec<VersionId> {
        self.graph.heads(&self.state).iter().map(|v| v.id()).collect()
    }

    /// Au plus `limit` versions de numéro supérieur à `after_number`, par numéro croissant.
    pub fn versions_after(&self, after_number: u64, limit: usize) -> Vec<SignedVersion> {
        let mut v: Vec<&SignedVersion> = self.graph.iter().map(|(_, v)| v).filter(|v| v.header.number > after_number).collect();
        v.sort_by_key(|v| (v.header.number, v.id()));
        v.into_iter().take(limit).cloned().collect()
    }

    pub fn key(&self) -> &GroupKey {
        &self.key
    }

    /// Fichiers ou versions écartés à la relecture (raison affichable), pour le diagnostic.
    pub fn skipped(&self) -> &[String] {
        &self.skipped
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Monde matérialisé de la tête (`world/`).
    pub fn world_dir(&self) -> PathBuf {
        self.dir.join("world")
    }

    /// File d'envoi persistante (`outbox/`).
    pub fn outbox_dir(&self) -> PathBuf {
        self.dir.join("outbox")
    }

    /// Serveur dédié installé pour ce groupe (`server/`).
    pub fn server_dir(&self) -> PathBuf {
        self.dir.join("server")
    }

    /// Remplace la clé de groupe (renouvelée après un `RemoveMember`).
    pub fn set_key(&mut self, key: GroupKey) -> Result<(), StoreError> {
        write_atomic(&self.dir.join("group.key"), key.as_bytes(), true).map_err(io_err("écriture de group.key"))?;
        self.key = key;
        Ok(())
    }

    // --- opérations ---------------------------------------------------------------------------------

    /// Applique les opérations de `ops` qui prolongent notre chaîne (celles déjà connues doivent être
    /// identiques, sinon [`StoreError::Diverged`]). Tout ou rien. Rend le nombre d'opérations ajoutées.
    pub fn append_ops(&mut self, ops: &[SignedOp]) -> Result<usize, StoreError> {
        let mut state = self.state.clone();
        let mut ever = self.ever.clone();
        let mut fresh: Vec<&SignedOp> = Vec::new();
        for op in ops {
            if op.op_seq <= state.op_seq {
                let mine = usize::try_from(op.op_seq).ok().and_then(|n| n.checked_sub(1)).and_then(|i| self.ops.get(i));
                if mine.map(SignedOp::hash) != Some(op.hash()) {
                    return Err(StoreError::Diverged);
                }
                continue;
            }
            if self.ops.len() + fresh.len() >= MAX_OPS {
                return Err(StoreError::Model(ModelError::TooManyOps));
            }
            state.apply(op)?;
            ever.extend(state.members.iter().copied());
            fresh.push(op);
        }
        if fresh.is_empty() {
            return Ok(0);
        }
        let mut all = self.ops.clone();
        all.extend(fresh.iter().map(|o| (*o).clone()));
        let mut data = Vec::new();
        for op in &all {
            data.extend(json_line(op));
        }
        write_atomic(&self.dir.join("ops.jsonl"), &data, false).map_err(io_err("écriture de ops.jsonl"))?;
        let added = fresh.len();
        self.ops = all;
        self.state = state;
        self.ever = ever;
        self.purge_stale_proposals();
        Ok(added)
    }

    /// Ajoute une opération (cas courant de [`Self::append_ops`]).
    pub fn append_op(&mut self, op: &SignedOp) -> Result<(), StoreError> {
        if op.op_seq != self.state.next_op_seq() {
            return Err(ModelError::BadSeq { expected: self.state.next_op_seq(), got: op.op_seq }.into());
        }
        self.append_ops(std::slice::from_ref(op)).map(|_| ())
    }

    // --- versions -----------------------------------------------------------------------------------

    /// Ingère des en-têtes de version reçus d'un pair (voir l'en-tête du module pour les anciens membres).
    /// Chaque version acceptée est écrite sur le disque ; une erreur disque interrompt et est renvoyée (les
    /// versions déjà écrites restent valables).
    pub fn add_versions(&mut self, batch: Vec<SignedVersion>) -> Result<VersionsReport, StoreError> {
        let mut loose = self.state.clone();
        loose.members = self.ever.clone();
        let strict = self.state.clone();
        let (accepted, report) = insert_batch(&mut self.graph, &strict, &loose, batch);
        for v in &accepted {
            self.write_version(v)?;
        }
        Ok(report)
    }

    /// Ajoute une version (la nôtre ou une reçue). `Err` si elle est refusée.
    pub fn add_version(&mut self, v: SignedVersion) -> Result<Inserted, StoreError> {
        let r = self.graph.insert(&self.state, v.clone())?;
        if r == Inserted::New {
            self.write_version(&v)?;
        }
        Ok(r)
    }

    fn write_version(&self, v: &SignedVersion) -> Result<(), StoreError> {
        let path = self.dir.join("versions").join(format!("{}.json", v.id()));
        let data = serde_json::to_vec(v).expect("sérialisation JSON de types simples");
        write_atomic(&path, &data, false).map_err(io_err(format!("écriture de {}", path.display())))
    }

    // --- propositions -------------------------------------------------------------------------------

    pub fn proposals(&self) -> impl Iterator<Item = &Proposal> {
        self.proposals.values()
    }

    pub fn proposal(&self, id: &OpHash) -> Option<&Proposal> {
        self.proposals.get(id)
    }

    fn status_of(&self, p: &Proposal) -> ProposalStatus {
        let (approvals, needed) = p.approvals(&self.state);
        ProposalStatus { id: p.id(), approvals, needed, ready: p.is_ready(&self.state) }
    }

    fn write_proposal(&self, p: &Proposal) -> Result<(), StoreError> {
        let path = self.dir.join("proposals").join(format!("{}.json", hex::encode(p.id())));
        let data = serde_json::to_vec(p).expect("sérialisation JSON de types simples");
        write_atomic(&path, &data, false).map_err(io_err(format!("écriture de {}", path.display())))
    }

    /// Enregistre une proposition de `proposer` (nous, ou le pair qui l'a envoyée) après validation, ou fusionne ses
    /// votes avec celle qu'on a déjà (même identifiant).
    ///
    /// Une nouvelle proposition doit porter la signature de son auteur en premier ; l'auteur n'en garde qu'une ouverte
    /// par cible (la nouvelle remplace l'ancienne), n'en émet pas plus de [`MAX_PROPOSE_PER_HOUR`] par heure, et le
    /// groupe en garde au plus [`MAX_PROPOSALS`] (la plus ancienne de l'auteur le plus bavard est évincée). Les
    /// propositions de plus de 14 jours sont retirées.
    pub fn put_proposal(&mut self, p: Proposal, proposer: &PeerKey) -> Result<ProposalStatus, StoreError> {
        self.put_proposal_at(p, proposer, unix_now())
    }

    /// Comme [`Group::put_proposal`], à l'heure locale `now` (secondes Unix).
    pub fn put_proposal_at(&mut self, p: Proposal, proposer: &PeerKey, now: u64) -> Result<ProposalStatus, StoreError> {
        p.validate(&self.state)?;
        self.check_continue_base(&p.op, false)?;
        self.purge_expired_proposals(now);
        let id = p.id();
        let mut merged = self.proposals.get(&id).cloned();
        match merged.as_mut() {
            Some(mine) => {
                for (k, s) in &p.sigs {
                    mine.add_vote(&self.state, *k, *s)?;
                }
            }
            None => {
                if p.sigs.first().map(|s| &s.0) != Some(proposer) {
                    return Err(StoreError::ProposerNotSigned);
                }
                let log = self.propose_log.entry(*proposer).or_default();
                log.retain(|t| now.saturating_sub(*t) < 3600);
                if log.len() >= MAX_PROPOSE_PER_HOUR {
                    return Err(StoreError::TooManyProposals);
                }
                log.push(now);
                if self.propose_log.len() > 4 * MAX_MEMBERS {
                    self.propose_log.retain(|_, v| v.last().is_some_and(|t| now.saturating_sub(*t) < 3600));
                }
                // Une seule proposition ouverte par auteur et par cible : la nouvelle remplace l'ancienne.
                let target = proposal_target(&p.op);
                let same: Vec<OpHash> = self
                    .proposals
                    .iter()
                    .filter(|(_, q)| q.sigs.first().map(|s| &s.0) == Some(proposer) && proposal_target(&q.op) == target)
                    .map(|(qid, _)| *qid)
                    .collect();
                for old in same {
                    self.drop_proposal(&old);
                }
                while self.proposals.len() >= MAX_PROPOSALS {
                    let Some(victim) = self.eviction_victim() else { break };
                    self.drop_proposal(&victim);
                }
                merged = Some(p);
            }
        }
        let merged = merged.ok_or(StoreError::UnknownProposal)?;
        self.write_proposal(&merged)?;
        let status = self.status_of(&merged);
        self.proposals.insert(id, merged);
        self.proposal_times.entry(id).or_insert(now);
        Ok(status)
    }

    /// La proposition à évincer : la plus ancienne de l'auteur qui en a le plus.
    fn eviction_victim(&self) -> Option<OpHash> {
        let mut per_author: BTreeMap<PeerKey, usize> = BTreeMap::new();
        for q in self.proposals.values() {
            if let Some((k, _)) = q.sigs.first() {
                *per_author.entry(*k).or_default() += 1;
            }
        }
        self.proposals
            .iter()
            .max_by_key(|(id, q)| {
                let count = q.sigs.first().and_then(|(k, _)| per_author.get(k)).copied().unwrap_or(0);
                let age = std::cmp::Reverse(self.proposal_times.get(*id).copied().unwrap_or(0));
                (count, age)
            })
            .map(|(id, _)| *id)
    }

    fn drop_proposal(&mut self, id: &OpHash) {
        self.proposals.remove(id);
        self.proposal_times.remove(id);
        let _ = fs::remove_file(self.dir.join("proposals").join(format!("{}.json", hex::encode(id))));
    }

    /// Contrôle la base d'un `ContinueWithout` contre notre graphe : si nous la connaissons, elle doit être de l'époque
    /// courante et n'avoir aucun enfant dans cette époque (une feuille). `require_known` : refuse aussi une base que nous
    /// n'avons pas (avant de signer un vote, jamais à la réception d'une proposition). Sans effet pour les autres opérations.
    pub fn check_continue_base(&self, op: &GroupOp, require_known: bool) -> Result<(), StoreError> {
        let GroupOp::ContinueWithout { base_version, .. } = op else { return Ok(()) };
        let Some(base) = self.graph.get(base_version) else {
            return if require_known { Err(StoreError::BadContinueBase("elle n'est pas connue de ce PC")) } else { Ok(()) };
        };
        if base.header.epoch != self.state.epoch {
            return Err(StoreError::BadContinueBase("elle ne fait pas partie de la période en cours"));
        }
        let has_child = self.graph.iter().any(|(_, v)| v.header.parent.as_ref() == Some(base_version) && v.header.epoch == self.state.epoch);
        if has_child {
            return Err(StoreError::BadContinueBase("une version plus récente en descend déjà"));
        }
        Ok(())
    }

    /// Retire les propositions reçues il y a plus de 14 jours.
    fn purge_expired_proposals(&mut self, now: u64) {
        let old: Vec<OpHash> = self
            .proposal_times
            .iter()
            .filter(|(_, t)| now.saturating_sub(**t) > PROPOSAL_TTL_SECS)
            .map(|(id, _)| *id)
            .collect();
        for id in old {
            self.drop_proposal(&id);
        }
    }

    /// Ajoute le vote de `key` à la proposition `id`.
    pub fn add_vote(&mut self, id: &OpHash, key: PeerKey, sig: Sig) -> Result<ProposalStatus, StoreError> {
        let mut p = self.proposals.get(id).cloned().ok_or(StoreError::UnknownProposal)?;
        if p.add_vote(&self.state, key, sig)? {
            self.write_proposal(&p)?;
        }
        let status = self.status_of(&p);
        self.proposals.insert(*id, p);
        Ok(status)
    }

    /// Transforme une proposition qui a atteint son seuil en opération de la chaîne.
    pub fn finalize(&mut self, id: &OpHash) -> Result<SignedOp, StoreError> {
        let p = self.proposals.get(id).cloned().ok_or(StoreError::UnknownProposal)?;
        let op = p.into_signed(&self.state)?;
        self.append_op(&op)?;
        Ok(op)
    }

    /// Retire les propositions que l'avancée de la chaîne a rendues périmées.
    fn purge_stale_proposals(&mut self) {
        let state = &self.state;
        let stale: Vec<OpHash> = self.proposals.iter().filter(|(_, p)| p.validate(state).is_err()).map(|(id, _)| *id).collect();
        for id in stale {
            self.drop_proposal(&id);
        }
    }
}

// ---------------------------------------------------------------------------------------------------------
// Magasin de groupes
// ---------------------------------------------------------------------------------------------------------

/// Racine `hearth/` : un dossier par groupe.
#[derive(Debug, Clone)]
pub struct GroupStore {
    root: PathBuf,
}

impl GroupStore {
    /// Ouvre (et crée au besoin) le dossier racine.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(io_err(format!("création de {}", root.display())))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn group_dir(&self, id: &GroupId) -> PathBuf {
        self.root.join(id.to_hex())
    }

    /// Groupes présents sur le disque (par identifiant croissant).
    pub fn list(&self) -> Result<Vec<GroupId>, StoreError> {
        let rd = fs::read_dir(&self.root).map_err(io_err(format!("lecture de {}", self.root.display())))?;
        let mut ids: Vec<GroupId> = rd
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| e.file_name().to_str().and_then(GroupId::from_hex))
            .collect();
        ids.sort();
        Ok(ids)
    }

    pub fn exists(&self, id: &GroupId) -> bool {
        self.group_dir(id).join("ops.jsonl").is_file()
    }

    /// Crée un groupe à partir de son opération `Create`.
    pub fn create(&self, create: &SignedOp, key: GroupKey) -> Result<Group, StoreError> {
        self.import(std::slice::from_ref(create), key)
    }

    /// Crée un groupe à partir d'une chaîne reçue (invitation) : la chaîne est rejouée et vérifiée en entier. Le
    /// dossier apparaît d'un coup (construit à côté puis renommé) : jamais de groupe à moitié écrit.
    pub fn import(&self, ops: &[SignedOp], key: GroupKey) -> Result<Group, StoreError> {
        let state = replay(ops)?;
        let ever = ever_members(ops)?;
        let dest = self.group_dir(&state.id);
        if dest.exists() {
            return Err(StoreError::AlreadyExists);
        }
        let tmp = self.root.join(format!(".creating-{:016x}", rand::rngs::OsRng.next_u64()));
        let build = || -> Result<(), StoreError> {
            fs::create_dir_all(tmp.join("versions")).map_err(io_err("création du groupe"))?;
            fs::create_dir_all(tmp.join("proposals")).map_err(io_err("création du groupe"))?;
            let mut data = Vec::new();
            for op in ops {
                data.extend(json_line(op));
            }
            write_atomic(&tmp.join("ops.jsonl"), &data, false).map_err(io_err("écriture de ops.jsonl"))?;
            write_atomic(&tmp.join("group.key"), key.as_bytes(), true).map_err(io_err("écriture de group.key"))?;
            fs::rename(&tmp, &dest).map_err(io_err("création du groupe"))
        };
        if let Err(e) = build() {
            let _ = fs::remove_dir_all(&tmp);
            return Err(e);
        }
        Ok(Group {
            dir: dest,
            state,
            ops: ops.to_vec(),
            ever,
            graph: VersionGraph::new(),
            key,
            proposals: BTreeMap::new(),
            proposal_times: BTreeMap::new(),
            propose_log: BTreeMap::new(),
            skipped: Vec::new(),
        })
    }

    /// Ouvre un groupe en revalidant tout son contenu.
    pub fn open_group(&self, id: &GroupId) -> Result<Group, StoreError> {
        let dir = self.group_dir(id);
        if !dir.join("ops.jsonl").is_file() {
            return Err(StoreError::NotFound);
        }
        let ops = parse_ops(&read_bounded(&dir.join("ops.jsonl"), MAX_OPS_FILE_BYTES)?)?;
        let state = replay(&ops).map_err(|e| StoreError::Corrupt(format!("chaîne d'opérations invalide : {e}")))?;
        if state.id != *id {
            return Err(StoreError::Corrupt("le dossier ne porte pas l'identifiant du groupe".into()));
        }
        let ever = ever_members(&ops)?;
        let key_bytes = read_bounded(&dir.join("group.key"), 32)?;
        let key = GroupKey::from_bytes(
            key_bytes.try_into().map_err(|_| StoreError::Corrupt("group.key doit faire 32 octets".into()))?,
        );

        let mut skipped = Vec::new();
        let mut batch = Vec::new();
        let versions_dir = dir.join("versions");
        for (path, name) in json_files(&versions_dir, MAX_VERSIONS)? {
            match read_json::<SignedVersion>(&path) {
                Ok(v) if format!("{}.json", v.id()) == name => batch.push(v),
                Ok(_) => skipped.push(format!("versions/{name} : nom de fichier différent de l'identifiant")),
                Err(e) => skipped.push(format!("versions/{name} : {e}")),
            }
        }
        // Ces versions ont toutes été validées à leur insertion : on admet les anciens membres sans condition.
        let mut loose = state.clone();
        loose.members = ever.clone();
        let mut graph = VersionGraph::new();
        let (_, report) = insert_batch(&mut graph, &loose, &loose, batch);
        skipped.extend(report.rejected.into_iter().map(|(id, why)| format!("version {id} : {why}")));

        let mut proposals = BTreeMap::new();
        let mut proposal_times = BTreeMap::new();
        let now = unix_now();
        for (path, name) in json_files(&dir.join("proposals"), MAX_PROPOSALS * 4)? {
            // Heure de réception : la date du fichier (une proposition illisible de date reste « reçue maintenant »).
            let received = fs::metadata(&path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(now, |d| d.as_secs());
            match read_json::<Proposal>(&path) {
                Ok(p)
                    if p.validate(&state).is_ok()
                        && format!("{}.json", hex::encode(p.id())) == name
                        && now.saturating_sub(received) <= PROPOSAL_TTL_SECS =>
                {
                    if proposals.len() < MAX_PROPOSALS {
                        proposal_times.insert(p.id(), received);
                        proposals.insert(p.id(), p);
                    }
                }
                // Périmée (la chaîne a avancé) ou illisible : retirée.
                _ => {
                    let _ = fs::remove_file(&path);
                }
            }
        }
        Ok(Group { dir, state, ops, ever, graph, key, proposals, proposal_times, propose_log: BTreeMap::new(), skipped })
    }

    /// Supprime un groupe et tout son contenu (monde, file d'envoi, serveur compris).
    pub fn delete(&self, id: &GroupId) -> Result<(), StoreError> {
        let dir = self.group_dir(id);
        if !dir.is_dir() {
            return Err(StoreError::NotFound);
        }
        fs::remove_dir_all(&dir).map_err(io_err(format!("suppression de {}", dir.display())))
    }
}

/// Fichiers `*.json` d'un dossier (au plus `max`), sans les temporaires.
fn json_files(dir: &Path, max: usize) -> Result<Vec<(PathBuf, String)>, StoreError> {
    let rd = match fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io_err(format!("lecture de {}", dir.display()))(e)),
    };
    let mut out = Vec::new();
    for entry in rd.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !name.ends_with(".json") || !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        if out.len() >= max {
            break;
        }
        out.push((entry.path(), name));
    }
    out.sort();
    Ok(out)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let bytes = read_bounded(path, MAX_FILE_BYTES).map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| format!("JSON invalide : {e}"))
}

/// Nombre maximal de membres, rappelé ici pour les appelants qui bornent des listes de clés.
pub const MAX_GROUP_MEMBERS: usize = MAX_MEMBERS;

#[cfg(test)]
mod tests {
    use super::super::model::{testutil, GroupOp, HearthSigner};
    use super::super::version::{VersionHeader, VersionKind};
    use super::*;

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn create_op(owner: &ed25519_dalek::SigningKey, others: &[PeerKey]) -> SignedOp {
        let set: BTreeSet<PeerKey> = others.iter().copied().collect();
        SignedOp::create(owner, GroupId::random(), "Les copains", testutil::pack(), &set).unwrap()
    }

    fn version(
        signer: &ed25519_dalek::SigningKey,
        group: GroupId,
        number: u64,
        parent: Option<VersionId>,
        epoch: u64,
        tag: u8,
    ) -> SignedVersion {
        let header = VersionHeader {
            group_id: group,
            number,
            parent,
            epoch,
            author: signer.peer_key(),
            pack_hash: "a".repeat(64),
            created_at: 1_700_000_000 + number,
            played_secs: 60,
            kind: VersionKind::Session,
            collection: [tag; 32],
            total_size: 10,
        };
        SignedVersion::sign(signer, header).unwrap()
    }

    #[test]
    fn create_reopen_and_revalidate() {
        let tmp = dir();
        let store = GroupStore::open(tmp.path().join("hearth")).unwrap();
        let (a, b) = (testutil::key(1), testutil::key(2));
        let op = create_op(&a, &[b.peer_key()]);
        let key = GroupKey::random();
        let g = store.create(&op, key.clone()).unwrap();
        let id = g.id();
        assert_eq!(store.list().unwrap(), vec![id]);
        assert!(matches!(store.create(&op, key.clone()), Err(StoreError::AlreadyExists)));

        let again = store.open_group(&id).unwrap();
        assert_eq!(again.state(), g.state());
        assert_eq!(again.key(), &key);
        assert_eq!(again.ops().len(), 1);

        // La clé n'apparaît jamais dans les journaux.
        assert_eq!(format!("{:?}", again.key()), "GroupKey(..)");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(g.dir().join("group.key")).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "group.key ne doit être lisible que par son propriétaire");
        }
    }

    #[test]
    fn corrupt_chain_is_refused_not_half_loaded() {
        let tmp = dir();
        let store = GroupStore::open(tmp.path()).unwrap();
        let a = testutil::key(1);
        let g = store.create(&create_op(&a, &[]), GroupKey::random()).unwrap();
        let id = g.id();
        // Une signature altérée invalide toute la chaîne.
        let path = g.dir().join("ops.jsonl");
        let text = fs::read_to_string(&path).unwrap();
        let tampered = text.replace("Les copains", "Les copaine");
        fs::write(&path, tampered).unwrap();
        assert!(matches!(store.open_group(&id), Err(StoreError::Corrupt(_))));
        // Fichier tronqué.
        fs::write(&path, &text.as_bytes()[..text.len() / 2]).unwrap();
        assert!(matches!(store.open_group(&id), Err(StoreError::Corrupt(_))));
        // Clé de mauvaise taille.
        fs::write(&path, &text).unwrap();
        assert!(store.open_group(&id).is_ok());
        fs::write(g.dir().join("group.key"), b"court").unwrap();
        assert!(matches!(store.open_group(&id), Err(StoreError::Corrupt(_))));
    }

    #[test]
    fn append_ops_is_all_or_nothing_and_persisted() {
        let tmp = dir();
        let store = GroupStore::open(tmp.path()).unwrap();
        let (a, b, c) = (testutil::key(1), testutil::key(2), testutil::key(3));
        let mut g = store.create(&create_op(&a, &[b.peer_key()]), GroupKey::random()).unwrap();
        let add_c = g.state().sign_single(GroupOp::AddMember { key: c.peer_key() }, &a).unwrap();
        // Une opération valide suivie d'une invalide : rien n'est appliqué.
        let mut bad = add_c.clone();
        bad.op_seq = 5;
        assert!(g.append_ops(&[add_c.clone(), bad]).is_err());
        assert_eq!(g.state().op_seq, 1);
        assert_eq!(store.open_group(&g.id()).unwrap().state().op_seq, 1);

        assert_eq!(g.append_ops(std::slice::from_ref(&add_c)).unwrap(), 1);
        // Rejouer une opération connue ne change rien ; une autre à la même place est une divergence.
        assert_eq!(g.append_ops(&[add_c]).unwrap(), 0);
        let other = g.state().sign_single(GroupOp::Leave, &b).unwrap();
        let mut forged = other.clone();
        forged.op_seq = 2;
        assert!(matches!(g.append_ops(&[forged]), Err(StoreError::Diverged)));
        g.append_op(&other).unwrap();

        let reopened = store.open_group(&g.id()).unwrap();
        assert_eq!(reopened.state(), g.state());
        assert!(reopened.state().is_member(&c.peer_key()));
        assert!(!reopened.state().is_member(&b.peer_key()));
        assert_eq!(g.ops_after(1, 10).len(), 2);
        assert!(g.ops_after(3, 10).is_empty());
    }

    #[test]
    fn versions_are_validated_persisted_and_reloaded() {
        let tmp = dir();
        let store = GroupStore::open(tmp.path()).unwrap();
        let (a, b) = (testutil::key(1), testutil::key(2));
        let mut g = store.create(&create_op(&a, &[b.peer_key()]), GroupKey::random()).unwrap();
        let id = g.id();
        let v1 = version(&a, id, 1, None, 0, 1);
        let v2 = version(&b, id, 2, Some(v1.id()), 0, 2);
        let v3 = version(&a, id, 3, Some(v2.id()), 0, 3);
        // Dans le désordre : tout est accepté, parents d'abord.
        let rep = g.add_versions(vec![v3.clone(), v1.clone(), v2.clone(), v1.clone()]).unwrap();
        assert_eq!(rep.added, vec![v1.id(), v2.id(), v3.id()]);
        assert!(rep.rejected.is_empty());
        assert_eq!(g.heads(), vec![v3.id()]);
        assert_eq!(g.versions_after(1, 10).len(), 2);
        // Une version d'un non-membre, ou sans parent connu, est refusée avec une raison.
        let stranger = testutil::key(9);
        let rep = g.add_versions(vec![version(&stranger, id, 1, None, 0, 7)]).unwrap();
        assert_eq!(rep.added.len(), 0);
        assert_eq!(rep.rejected.len(), 1);
        assert!(g.add_version(version(&a, id, 9, Some(VersionId([9; 32])), 0, 8)).is_err());

        let again = store.open_group(&id).unwrap();
        assert_eq!(again.heads(), vec![v3.id()]);
        assert_eq!(again.graph().len(), 3);
        assert!(again.skipped().is_empty());
    }

    #[test]
    fn invalid_version_file_is_skipped_but_group_loads() {
        let tmp = dir();
        let store = GroupStore::open(tmp.path()).unwrap();
        let a = testutil::key(1);
        let mut g = store.create(&create_op(&a, &[]), GroupKey::random()).unwrap();
        let id = g.id();
        let v1 = version(&a, id, 1, None, 0, 1);
        g.add_version(v1.clone()).unwrap();
        fs::write(g.dir().join("versions").join(format!("{}.json", "0".repeat(64))), b"{pas du json").unwrap();
        // Contenu valide mais sous le mauvais nom.
        fs::write(g.dir().join("versions").join(format!("{}.json", "1".repeat(64))), serde_json::to_vec(&v1).unwrap()).unwrap();
        // Fichier géant : refusé sans être lu.
        fs::write(g.dir().join("versions").join(format!("{}.json", "2".repeat(64))), vec![b' '; 70_000]).unwrap();
        let again = store.open_group(&id).unwrap();
        assert_eq!(again.graph().len(), 1);
        assert_eq!(again.skipped().len(), 3);
    }

    #[test]
    fn departed_member_history_is_ingested_only_when_endorsed() {
        let tmp = dir();
        let store = GroupStore::open(tmp.path()).unwrap();
        let (a, b, c) = (testutil::key(1), testutil::key(2), testutil::key(3));
        let create = create_op(&a, &[b.peer_key()]);
        let id = match &create.op {
            GroupOp::Create { id, .. } => *id,
            _ => unreachable!(),
        };
        // Historique complet : B joue, puis quitte ; A reprend sa lignée ; C arrive.
        let v1 = version(&b, id, 1, None, 0, 1);
        let v2 = version(&a, id, 2, Some(v1.id()), 0, 2);
        let v_orphan = version(&b, id, 3, Some(v2.id()), 0, 3);
        let mut full = store.create(&create, GroupKey::random()).unwrap();
        full.add_versions(vec![v1.clone(), v2.clone()]).unwrap();
        let leave = full.state().sign_single(GroupOp::Leave, &b).unwrap();
        full.append_op(&leave).unwrap();
        let add_c = full.state().sign_single(GroupOp::AddMember { key: c.peer_key() }, &a).unwrap();
        full.append_op(&add_c).unwrap();
        // Relecture : l'ancien membre B n'empêche pas de retrouver sa version.
        let reopened = store.open_group(&id).unwrap();
        assert_eq!(reopened.graph().len(), 2, "{:?}", reopened.skipped());

        // Un nouvel arrivant reçoit la chaîne puis les versions, dont celles de B.
        let tmp2 = dir();
        let store2 = GroupStore::open(tmp2.path()).unwrap();
        let mut newcomer = store2.import(full.ops(), GroupKey::random()).unwrap();
        let rep = newcomer.add_versions(vec![v2.clone(), v1.clone()]).unwrap();
        assert_eq!(rep.added.len(), 2, "{rep:?}");
        assert_eq!(newcomer.heads(), vec![v2.id()]);
        // Une version d'un ancien membre sans descendant d'un membre actuel reste refusée.
        let rep = newcomer.add_versions(vec![v_orphan.clone()]).unwrap();
        assert!(rep.added.is_empty());
        assert_eq!(rep.rejected.len(), 1);
        assert!(newcomer.add_version(v_orphan).is_err());
        // Et sa lignée seule (sans la version du membre actuel) ne passe pas non plus.
        let tmp3 = dir();
        let mut other = GroupStore::open(tmp3.path()).unwrap().import(full.ops(), GroupKey::random()).unwrap();
        let rep = other.add_versions(vec![v1]).unwrap();
        assert!(rep.added.is_empty());
    }

    #[test]
    fn proposals_persist_merge_and_finalize() {
        let tmp = dir();
        let store = GroupStore::open(tmp.path()).unwrap();
        let (a, b, c) = (testutil::key(1), testutil::key(2), testutil::key(3));
        let mut g = store.create(&create_op(&a, &[b.peer_key(), c.peer_key()]), GroupKey::random()).unwrap();
        // Passe à la majorité : le créateur part ; B et C restent.
        let leave = g.state().sign_single(GroupOp::Leave, &a).unwrap();
        g.append_op(&leave).unwrap();
        let d = testutil::key(4);
        let p = Proposal::new(g.state(), GroupOp::AddMember { key: d.peer_key() }).unwrap();
        let mut by_b = p.clone();
        by_b.vote(g.state(), &b).unwrap();
        let st = g.put_proposal(by_b, &b.peer_key()).unwrap();
        assert_eq!((st.approvals, st.needed, st.ready), (1, 2, false));
        // Les votes de deux copies se fusionnent.
        let mut by_c = p.clone();
        by_c.vote(g.state(), &c).unwrap();
        let st = g.put_proposal(by_c, &c.peer_key()).unwrap();
        assert_eq!((st.approvals, st.ready), (2, true));
        assert_eq!(store.open_group(&g.id()).unwrap().proposals().count(), 1);
        // Un vote sur une proposition inconnue est refusé.
        assert!(matches!(g.add_vote(&[7; 32], b.peer_key(), p.sign(&b)), Err(StoreError::UnknownProposal)));
        let op = g.finalize(&p.id()).unwrap();
        assert!(g.state().is_member(&d.peer_key()));
        assert_eq!(op.op_seq, g.state().op_seq);
        assert_eq!(g.proposals().count(), 0, "la proposition aboutie est retirée du disque");
        assert_eq!(store.open_group(&g.id()).unwrap().proposals().count(), 0);
    }

    #[test]
    fn un_membre_ne_peut_pas_saturer_les_propositions() {
        let tmp = dir();
        let store = GroupStore::open(tmp.path()).unwrap();
        let (a, b, c) = (testutil::key(1), testutil::key(2), testutil::key(3));
        let mut g = store.create(&create_op(&a, &[b.peer_key(), c.peer_key()]), GroupKey::random()).unwrap();
        let t0 = 1_000_000u64;
        let junk = |g: &Group, n: u8| {
            let mut p = Proposal::new(g.state(), GroupOp::AddMember { key: testutil::key(100 + n).peer_key() }).unwrap();
            p.vote(g.state(), &b).unwrap();
            p
        };
        // Cinq `Propose` par heure et par clé : le sixième est refusé.
        for n in 0..MAX_PROPOSE_PER_HOUR as u8 {
            g.put_proposal_at(junk(&g, n), &b.peer_key(), t0 + u64::from(n)).unwrap();
        }
        assert!(matches!(g.put_proposal_at(junk(&g, 9), &b.peer_key(), t0 + 10), Err(StoreError::TooManyProposals)));
        // Une proposition honnête passe, puis M recommence une heure plus tard : le plafond du groupe évince
        // les siennes (les plus anciennes), jamais celle du créateur.
        let mut honest = Proposal::new(g.state(), GroupOp::RemoveMember { key: b.peer_key() }).unwrap();
        honest.vote(g.state(), &a).unwrap();
        let honest_id = honest.id();
        g.put_proposal_at(honest, &a.peer_key(), t0 + 20).unwrap();
        for n in 10..10 + MAX_PROPOSE_PER_HOUR as u8 {
            g.put_proposal_at(junk(&g, n), &b.peer_key(), t0 + 3700 + u64::from(n)).unwrap();
        }
        assert_eq!(g.proposals().count(), MAX_PROPOSALS);
        assert!(g.proposal(&honest_id).is_some(), "la proposition du créateur n'est pas évincée par le bruit de M");
        // Le retrait de M reste donc possible.
        let st = g.put_proposal_at(g.proposal(&honest_id).unwrap().clone(), &a.peer_key(), t0 + 3800).unwrap();
        assert_eq!(st.id, honest_id);
    }

    #[test]
    fn une_proposition_sans_signature_de_son_auteur_est_refusee_et_la_nouvelle_remplace_l_ancienne() {
        let tmp = dir();
        let store = GroupStore::open(tmp.path()).unwrap();
        let (a, b) = (testutil::key(1), testutil::key(2));
        let mut g = store.create(&create_op(&a, &[b.peer_key()]), GroupKey::random()).unwrap();
        let t0 = 5_000u64;
        let bare = Proposal::new(g.state(), GroupOp::AddMember { key: testutil::key(50).peer_key() }).unwrap();
        assert!(matches!(g.put_proposal_at(bare.clone(), &b.peer_key(), t0), Err(StoreError::ProposerNotSigned)));
        // Signée par un autre que l'émetteur : refusée aussi.
        let mut by_a = bare;
        by_a.vote(g.state(), &a).unwrap();
        assert!(matches!(g.put_proposal_at(by_a, &b.peer_key(), t0), Err(StoreError::ProposerNotSigned)));
        // Même auteur, même cible (le modpack) : la seconde remplace la première.
        let pack = |mc: &str| {
            let mut pk = testutil::pack();
            pk.minecraft = mc.into();
            GroupOp::PackUpdate { pack: pk }
        };
        let mut first = Proposal::new(g.state(), pack("1.20.1")).unwrap();
        first.vote(g.state(), &b).unwrap();
        let first_id = first.id();
        g.put_proposal_at(first, &b.peer_key(), t0).unwrap();
        let mut second = Proposal::new(g.state(), pack("1.20.4")).unwrap();
        second.vote(g.state(), &b).unwrap();
        let second_id = second.id();
        g.put_proposal_at(second, &b.peer_key(), t0 + 1).unwrap();
        assert!(g.proposal(&first_id).is_none() && g.proposal(&second_id).is_some());
        assert_eq!(g.proposals().count(), 1);
        assert!(!g.dir().join("proposals").join(format!("{}.json", hex::encode(first_id))).exists());
    }

    #[test]
    fn une_proposition_expire_apres_quatorze_jours() {
        let tmp = dir();
        let store = GroupStore::open(tmp.path()).unwrap();
        let (a, b) = (testutil::key(1), testutil::key(2));
        let mut g = store.create(&create_op(&a, &[b.peer_key()]), GroupKey::random()).unwrap();
        let t0 = 10_000u64;
        let mut p = Proposal::new(g.state(), GroupOp::AddMember { key: testutil::key(60).peer_key() }).unwrap();
        p.vote(g.state(), &b).unwrap();
        let id = p.id();
        g.put_proposal_at(p, &b.peer_key(), t0).unwrap();
        // Une autre proposition, reçue juste avant l'échéance : la première est encore là.
        let mut q = Proposal::new(g.state(), GroupOp::AddMember { key: testutil::key(61).peer_key() }).unwrap();
        q.vote(g.state(), &a).unwrap();
        g.put_proposal_at(q.clone(), &a.peer_key(), t0 + PROPOSAL_TTL_SECS).unwrap();
        assert!(g.proposal(&id).is_some());
        let mut r = Proposal::new(g.state(), GroupOp::AddMember { key: testutil::key(62).peer_key() }).unwrap();
        r.vote(g.state(), &a).unwrap();
        g.put_proposal_at(r, &a.peer_key(), t0 + PROPOSAL_TTL_SECS + 1).unwrap();
        assert!(g.proposal(&id).is_none(), "plus de 14 jours : retirée");
        assert_eq!(g.proposals().count(), 2);
    }

    #[test]
    fn stale_proposal_is_dropped_when_chain_advances() {
        let tmp = dir();
        let store = GroupStore::open(tmp.path()).unwrap();
        let (a, b, c) = (testutil::key(1), testutil::key(2), testutil::key(3));
        let mut g = store.create(&create_op(&a, &[b.peer_key()]), GroupKey::random()).unwrap();
        let mut p = Proposal::new(g.state(), GroupOp::AddMember { key: c.peer_key() }).unwrap();
        p.vote(g.state(), &a).unwrap();
        let old_id = p.id();
        g.put_proposal(p, &a.peer_key()).unwrap();
        let other = g.state().sign_single(GroupOp::PackUpdate { pack: {
            let mut pk = testutil::pack();
            pk.minecraft = "1.20.1".into();
            pk
        } }, &a).unwrap();
        g.append_op(&other).unwrap();
        assert!(g.proposal(&old_id).is_none());
        assert!(!g.dir().join("proposals").join(format!("{}.json", hex::encode(old_id))).exists());
    }

    #[test]
    fn delete_and_key_rotation() {
        let tmp = dir();
        let store = GroupStore::open(tmp.path()).unwrap();
        let a = testutil::key(1);
        let mut g = store.create(&create_op(&a, &[]), GroupKey::random()).unwrap();
        let fresh = GroupKey::random();
        g.set_key(fresh.clone()).unwrap();
        assert_eq!(store.open_group(&g.id()).unwrap().key(), &fresh);
        let id = g.id();
        store.delete(&id).unwrap();
        assert!(matches!(store.open_group(&id), Err(StoreError::NotFound)));
        assert!(store.list().unwrap().is_empty());
        assert!(matches!(store.delete(&id), Err(StoreError::NotFound)));
        // Aucun résidu temporaire.
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 0);
    }
}
