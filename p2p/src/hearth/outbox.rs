//! Hearth : file d'envoi persistante des versions (voir `docs/hearth.md`, § 7 « Envoi des versions »).
//!
//! Chaque version produite ici (point de reprise, fin de partie) entre dans l'`Outbox`, un dossier de fichiers
//! `<id de version>.json` écrits de façon atomique : la file **survit au redémarrage**. [`Outbox::drain`] pousse
//! chaque entrée vers les cibles joignables ([`Replica`]) :
//!
//! * une cible **membre** accepte quand elle a l'en-tête *et* tout le contenu (`Stored`) : l'entrée quitte la file
//!   quand `min(2, autres membres)` membres ont accepté (les accusés s'accumulent d'un passage à l'autre, sur
//!   disque) ; les autres rattrapent auprès d'eux ou de nous ;
//! * les cibles sont **indépendantes** : un membre lent ou en panne ne retarde ni les autres entrées ni la
//!   confirmation d'un autre membre, et ses remises encore en cours sont abandonnées dès que le quorum est atteint ;
//! * un membre peut **refuser** définitivement (`PushError::refused`, par exemple une version trop grosse
//!   d'un coup) : il n'est plus retenté, et si le quorum devient impossible l'entrée passe en `failed` : elle reste en
//!   file et épinglée (le destinataire pourra la tirer chez nous) mais n'est plus poussée ;
//! * à l'ajout, un point de reprise remplace ceux de la même lignée encore en file ([`Outbox::enqueue`]) ;
//! * une cible **gardien** (réseau de relais, plus tard) reçoit l'entrée mais ne la retire pas : seule la remise à
//!   un membre libère la file. On retient à quels gardiens l'entrée est déjà partie pour ne pas la renvoyer ;
//! * aucune cible joignable : l'entrée reste, et part dès qu'un membre apparaît (l'application rappelle `drain`, ou
//!   laisse tourner [`Outbox::spawn_drain_loop`], et appelle [`Outbox::wake`] quand la présence change).
//!
//! L'application garde les blobs des entrées en file : [`Outbox::pinned_collections`] liste les collections à ne
//! jamais libérer (`WorldStore::release`) tant qu'elles sont en file.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::future::{join_all, BoxFuture};
use futures_util::stream::{FuturesUnordered, StreamExt};
use iroh::{Endpoint, EndpointAddr};
use rand::Rng;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{watch, Notify, Semaphore};
use tokio::task::JoinHandle;

use super::model::GroupId;
use super::proto::{HearthConn, NetError};
use super::version::{BlobHash, SignedVersion, VersionId, VersionKind};
use crate::util::{lock, now_secs};

/// Entrées au plus dans la file (au-delà, `enqueue` refuse : mieux vaut le dire que perdre une version).
pub const MAX_ENTRIES: usize = 1_000;
/// Taille maximale d'un fichier d'entrée relu.
const MAX_ENTRY_BYTES: u64 = 64 * 1024;
/// Délai maximal d'une remise à une cible (l'entrée reste en file et sera reproposée au tour suivant).
pub const PUSH_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Délai par défaut pour qu'un membre rapatrie une version entière.
pub const CONFIRM_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Intervalle entre deux questions « as-tu tout ? ».
const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// Entrées poussées en même temps (au-delà, elles attendent : la bande passante du membre est partagée).
const MAX_PARALLEL_ENTRIES: usize = 4;
/// Préfixe de la raison d'un refus définitif, dans `Response::Rejected` à une question `HaveVersion`.
pub const REFUSED_PREFIX: &str = "refused:";

/// Erreurs de la file (messages affichables).
#[derive(Debug, Error)]
pub enum OutboxError {
    #[error("erreur disque ({context}) : {message}")]
    Io { context: String, message: String },
    #[error("la file d'envoi est pleine")]
    Full,
    #[error("version invalide : {0}")]
    Invalid(String),
}

fn io_err(context: impl Into<String>) -> impl FnOnce(std::io::Error) -> OutboxError {
    let context = context.into();
    move |e| OutboxError::Io { context, message: e.to_string() }
}

/// Une version à pousser, et où elle en est.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboxEntry {
    pub version: SignedVersion,
    /// Secondes depuis l'epoch Unix.
    pub queued_at: u64,
    /// Gardiens déjà servis (identifiants de [`Replica::id`]) : ils ne reçoivent pas deux fois la même entrée.
    #[serde(default)]
    pub delivered: BTreeSet<String>,
    /// Membres qui ont répondu `Stored` (collection entière) ; l'entrée part quand il y en a assez.
    #[serde(default)]
    pub stored: BTreeSet<String>,
    /// Membres qui ont refusé définitivement : identifiant, raison. Ils ne sont plus retentés.
    #[serde(default)]
    pub refused: BTreeMap<String, String>,
    /// Remise abandonnée (trop de refus pour atteindre le quorum) : raison. L'entrée reste en file, épinglée.
    #[serde(default)]
    pub failed: Option<String>,
    /// Tentatives de remise échouées.
    #[serde(default)]
    pub attempts: u32,
    /// Dernière erreur (texte borné).
    #[serde(default)]
    pub last_error: Option<String>,
}

impl OutboxEntry {
    pub fn id(&self) -> VersionId {
        self.version.id()
    }

    pub fn group_id(&self) -> GroupId {
        self.version.header.group_id
    }

    /// Collection de blobs de la version.
    pub fn collection(&self) -> BlobHash {
        self.version.header.collection
    }
}

/// Nature d'une cible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplicaKind {
    /// Un membre du groupe : sa remise libère l'entrée.
    Member,
    /// Un gardien (réseau de relais) : il garde la version chiffrée, sans libérer l'entrée.
    Guardian,
}

/// Échec d'une remise.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct PushError {
    pub message: String,
    /// Refus **définitif** de la cible (elle a répondu qu'elle ne prendrait pas cette version) : inutile de réessayer.
    pub refused: bool,
}

impl PushError {
    /// Échec ordinaire : la cible sera retentée au passage suivant.
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into(), refused: false }
    }

    /// Refus définitif, `reason` étant un code court (`oversize`).
    pub fn refused(reason: impl Into<String>) -> Self {
        Self { message: reason.into(), refused: true }
    }
}

/// Une cible où pousser les versions. `push` rend `Ok` seulement quand la cible a **tout** (en-tête et contenu).
pub trait Replica: Send + Sync {
    /// Identifiant stable de la cible (clé du membre, identifiant du gardien).
    fn id(&self) -> String;
    fn kind(&self) -> ReplicaKind;
    /// Cette cible concerne-t-elle ce groupe ?
    fn serves(&self, group: &GroupId) -> bool;
    fn push<'a>(&'a self, entry: &'a OutboxEntry) -> BoxFuture<'a, Result<(), PushError>>;
}

/// Bilan d'un passage de [`Outbox::drain`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DrainReport {
    /// Versions remises à assez de membres (retirées de la file).
    pub delivered: Vec<VersionId>,
    /// Entrées encore en file après le passage.
    pub pending: usize,
    /// Remises échouées : version, cible, raison.
    pub failures: Vec<(VersionId, String, String)>,
    /// Un autre passage était en cours : rien n'a été fait.
    pub busy: bool,
}

/// Donne le nombre d'**autres** membres d'un groupe (le quorum en découle).
pub type OthersFn = Arc<dyn Fn(&GroupId) -> usize + Send + Sync>;

/// Ce qu'un passage a fait d'une entrée.
#[derive(Default)]
struct EntryOutcome {
    delivered: bool,
    failures: Vec<(VersionId, String, String)>,
}

/// File d'envoi persistante.
pub struct Outbox {
    dir: PathBuf,
    entries: Mutex<BTreeMap<VersionId, OutboxEntry>>,
    others: Mutex<Option<OthersFn>>,
    drain_lock: tokio::sync::Mutex<()>,
    wake: Notify,
    skipped: Vec<String>,
}

impl std::fmt::Debug for Outbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Outbox({} entrées)", lock(&self.entries).len())
    }
}

fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("entree");
    let tmp = path.with_file_name(format!(".{name}.tmp-{:016x}", rand::rand_core::UnwrapErr(rand::rngs::SysRng).next_u64()));
    let run = || -> std::io::Result<()> {
        let mut f = fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        fs::rename(&tmp, path)
    };
    let r = run();
    if r.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    r
}

impl Outbox {
    /// Ouvre (et crée au besoin) la file dans `dir` (`hearth/<groupe>/outbox/`) et relit les entrées. Les fichiers
    /// illisibles ou invalides sont écartés ([`Self::skipped`]) sans bloquer les autres.
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, OutboxError> {
        let dir = dir.into();
        fs::create_dir_all(&dir).map_err(io_err(format!("création de {}", dir.display())))?;
        let mut entries = BTreeMap::new();
        let mut skipped = Vec::new();
        let rd = fs::read_dir(&dir).map_err(io_err(format!("lecture de {}", dir.display())))?;
        for e in rd.filter_map(Result::ok) {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                // Temporaire d'une écriture interrompue.
                let _ = fs::remove_file(e.path());
                continue;
            }
            if !name.ends_with(".json") || !e.file_type().is_ok_and(|t| t.is_file()) {
                continue;
            }
            if entries.len() >= MAX_ENTRIES {
                skipped.push(format!("{name} : file pleine"));
                continue;
            }
            match Self::read_entry(&e.path()) {
                Ok(entry) if format!("{}.json", entry.id()) == name => {
                    entries.insert(entry.id(), entry);
                }
                Ok(_) => skipped.push(format!("{name} : nom de fichier différent de l'identifiant")),
                Err(why) => skipped.push(format!("{name} : {why}")),
            }
        }
        Ok(Self { dir, entries: Mutex::new(entries), others: Mutex::new(None), drain_lock: tokio::sync::Mutex::new(()), wake: Notify::new(), skipped })
    }

    fn read_entry(path: &Path) -> Result<OutboxEntry, String> {
        let meta = fs::metadata(path).map_err(|e| e.to_string())?;
        if meta.len() > MAX_ENTRY_BYTES {
            return Err("fichier trop gros".into());
        }
        let bytes = fs::read(path).map_err(|e| e.to_string())?;
        let entry: OutboxEntry = serde_json::from_slice(&bytes).map_err(|e| format!("JSON invalide : {e}"))?;
        check_version(&entry.version)?;
        Ok(entry)
    }

    /// Entrées écartées à l'ouverture (diagnostic).
    pub fn skipped(&self) -> &[String] {
        &self.skipped
    }

    fn path_of(&self, id: &VersionId) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    fn persist(&self, entry: &OutboxEntry) -> Result<(), OutboxError> {
        let data = serde_json::to_vec(entry).expect("sérialisation JSON de types simples");
        let path = self.path_of(&entry.id());
        write_atomic(&path, &data).map_err(io_err(format!("écriture de {}", path.display())))
    }

    /// Met une version en file. `Ok(false)` si elle y était déjà (rien ne change). Réveille la boucle d'envoi.
    ///
    /// **Regroupement** : un point de reprise remplace les points de reprise de sa lignée encore en file (on remonte
    /// les parents à travers les entrées du même groupe et on retire les `Checkpoint` rencontrés ; on s'arrête à
    /// la première `Session`, `Restore` ou version absente de la file, qui ne sont jamais retirées). Les membres
    /// rattrapent les en-têtes manquants auprès de nous (`unknown_parent`), et les blobs communs ne voyagent qu'une
    /// fois. Les collections retirées ne sont plus épinglées : le prochain ménage du magasin les libère.
    pub fn enqueue(&self, version: SignedVersion) -> Result<bool, OutboxError> {
        check_version(&version).map_err(OutboxError::Invalid)?;
        let entry = OutboxEntry {
            version,
            queued_at: now_secs(),
            delivered: BTreeSet::new(),
            stored: BTreeSet::new(),
            refused: BTreeMap::new(),
            failed: None,
            attempts: 0,
            last_error: None,
        };
        let id = entry.id();
        {
            let map = lock(&self.entries);
            if map.contains_key(&id) {
                return Ok(false);
            }
            if map.len() >= MAX_ENTRIES {
                return Err(OutboxError::Full);
            }
        }
        // Le fichier d'abord : une entrée qui n'est pas sur le disque n'existe pas.
        self.persist(&entry)?;
        let superseded = {
            let mut map = lock(&self.entries);
            let old = if entry.version.header.kind == VersionKind::Checkpoint { superseded_checkpoints(&map, &entry) } else { Vec::new() };
            for o in &old {
                map.remove(o);
            }
            map.insert(id, entry);
            old
        };
        for o in superseded {
            let _ = fs::remove_file(self.path_of(&o));
        }
        self.wake.notify_one();
        Ok(true)
    }

    /// Entrées en file, de la plus ancienne à la plus récente.
    pub fn pending(&self) -> Vec<OutboxEntry> {
        let mut v: Vec<OutboxEntry> = lock(&self.entries).values().cloned().collect();
        v.sort_by_key(|e| (e.queued_at, e.version.header.number, e.id()));
        v
    }

    pub fn len(&self) -> usize {
        lock(&self.entries).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn contains(&self, id: &VersionId) -> bool {
        lock(&self.entries).contains_key(id)
    }

    /// Retire une entrée sans l'avoir remise (version abandonnée par l'utilisateur).
    pub fn remove(&self, id: &VersionId) -> Result<bool, OutboxError> {
        let existed = lock(&self.entries).remove(id).is_some();
        if existed {
            let path = self.path_of(id);
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(io_err(format!("suppression de {}", path.display()))(e)),
            }
        }
        Ok(existed)
    }

    /// Collections des entrées en file : à ne pas libérer du magasin de blobs tant qu'elles y sont.
    pub fn pinned_collections(&self) -> Vec<(GroupId, BlobHash)> {
        lock(&self.entries).values().map(|e| (e.group_id(), e.collection())).collect()
    }

    /// Demande un passage immédiat à la boucle d'envoi (présence d'un membre changée, nouvelle entrée).
    pub fn wake(&self) {
        self.wake.notify_one();
    }

    /// Donne à la file le moyen de connaître le nombre d'autres membres d'un groupe (pour le quorum).
    pub fn set_others(&self, f: OthersFn) {
        *lock(&self.others) = Some(f);
    }

    /// `(autres membres, accusés `Stored` requis)` : `min(2, autres)`, au moins 1. Sans fonction donnée par
    /// [`Self::set_others`], un seul autre membre est supposé.
    fn counts(&self, group: &GroupId) -> (usize, usize) {
        let f = lock(&self.others).clone();
        let others = f.map_or(1, |f| f(group));
        (others, others.clamp(1, 2))
    }

    /// Remet à l'essai les entrées abandonnées (`failed`) et les refus : un destinataire a pu confirmer la version
    /// entre-temps (« Jouer »). À appeler au démarrage seulement, sinon une version refusée serait reproposée sans fin.
    pub fn retry_failed(&self) {
        let mut map = lock(&self.entries);
        for e in map.values_mut() {
            if e.failed.is_none() && e.refused.is_empty() {
                continue;
            }
            let before = e.clone();
            e.failed = None;
            e.refused.clear();
            if self.persist(e).is_err() {
                *e = before;
            }
        }
    }

    /// Un passage : pousse chaque entrée vers les cibles joignables de `reachable`. Un seul passage à la fois
    /// (un deuxième appel simultané rend `busy` sans rien faire). Les entrées avancent en parallèle (au plus
    /// [`MAX_PARALLEL_ENTRIES`], la plus ancienne en premier) et, dans une entrée, chaque cible est indépendante.
    pub async fn drain(&self, reachable: &[Arc<dyn Replica>]) -> DrainReport {
        let Ok(_guard) = self.drain_lock.try_lock() else { return DrainReport { busy: true, pending: self.len(), ..Default::default() } };
        let permits = Arc::new(Semaphore::new(MAX_PARALLEL_ENTRIES));
        let todo: Vec<OutboxEntry> = self.pending().into_iter().filter(|e| e.failed.is_none()).collect();
        let outcomes = join_all(todo.into_iter().map(|entry| {
            let permits = permits.clone();
            async move {
                let _permit = permits.acquire_owned().await.ok();
                let id = entry.id();
                (id, self.drain_entry(entry, reachable).await)
            }
        }))
        .await;
        let mut report = DrainReport::default();
        for (id, out) in outcomes {
            if out.delivered {
                report.delivered.push(id);
            }
            report.failures.extend(out.failures);
        }
        report.pending = self.len();
        report
    }

    /// Pousse une entrée vers toutes ses cibles à la fois, et note chaque résultat dès qu'il arrive.
    async fn drain_entry(&self, entry: OutboxEntry, reachable: &[Arc<dyn Replica>]) -> EntryOutcome {
        let (id, group) = (entry.id(), entry.group_id());
        let (others, quorum) = self.counts(&group);
        let mut out = EntryOutcome::default();
        if entry.stored.len() >= quorum {
            // Le groupe a rétréci depuis les accusés : ils suffisent maintenant.
            out.delivered = self.remove(&id).is_ok();
            return out;
        }
        let targets = reachable.iter().filter(|r| {
            r.serves(&group)
                && match r.kind() {
                    ReplicaKind::Guardian => !entry.delivered.contains(&r.id()),
                    ReplicaKind::Member => !entry.stored.contains(&r.id()) && !entry.refused.contains_key(&r.id()),
                }
        });
        let mut pushes = FuturesUnordered::new();
        for r in targets {
            let entry = &entry;
            pushes.push(async move {
                let res = match tokio::time::timeout(PUSH_TIMEOUT, r.push(entry)).await {
                    Ok(res) => res,
                    Err(_) => Err(PushError::new("délai dépassé")),
                };
                (r.id(), r.kind(), res)
            });
        }
        while let Some((rid, kind, res)) = pushes.next().await {
            let mut map = lock(&self.entries);
            // Retirée entre-temps (regroupée, groupe oublié) : on ne recrée rien sur le disque.
            let Some(cur) = map.get_mut(&id) else { break };
            let before = cur.clone();
            match res {
                Ok(()) => {
                    if kind == ReplicaKind::Member {
                        cur.stored.insert(rid);
                    } else {
                        cur.delivered.insert(rid);
                    }
                }
                Err(e) => {
                    let text: String = e.message.chars().filter(|c| !c.is_control()).take(200).collect();
                    cur.attempts = cur.attempts.saturating_add(1);
                    if e.refused && kind == ReplicaKind::Member {
                        cur.last_error = Some(format!("refusée : {text}"));
                        cur.refused.insert(rid.clone(), text.clone());
                        // Même avec tous les autres membres, le quorum n'est plus atteignable.
                        let possible = cur.stored.len() + others.saturating_sub(cur.stored.len() + cur.refused.len());
                        if possible < quorum {
                            cur.failed = Some(text.clone());
                        }
                    } else {
                        cur.last_error = Some(text.clone());
                    }
                    out.failures.push((id, rid, text));
                }
            }
            if cur.stored.len() >= quorum {
                drop(map);
                out.delivered = self.remove(&id).is_ok();
                // Les remises encore en cours sont abandonnées : le membre lent rattrape de son côté.
                break;
            }
            if *cur != before && self.persist(cur).is_err() {
                // Le disque refuse : l'entrée reste telle quelle en file.
                *cur = before;
            }
            if cur.failed.is_some() {
                break;
            }
        }
        out
    }

    /// Lance la boucle d'envoi : un passage toutes les `interval`, ou tout de suite sur [`Self::wake`] /
    /// nouvelle entrée. `reachable` donne les cibles joignables à cet instant. S'arrête quand le handle est
    /// abandonné ou que [`DrainTask::stop`] est appelé. Doit être appelé depuis un runtime tokio.
    pub fn spawn_drain_loop(
        self: &Arc<Self>,
        reachable: Arc<dyn Fn() -> Vec<Arc<dyn Replica>> + Send + Sync>,
        interval: Duration,
    ) -> DrainTask {
        let (stop, mut stopped) = watch::channel(false);
        let this = Arc::clone(self);
        let handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval.max(Duration::from_millis(50)));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = ticker.tick() => {}
                    () = this.wake.notified() => {}
                    r = stopped.changed() => {
                        if r.is_err() || *stopped.borrow() {
                            return;
                        }
                    }
                }
                if !this.is_empty() {
                    let targets = reachable();
                    this.drain(&targets).await;
                }
            }
        });
        DrainTask { stop, handle: Some(handle) }
    }
}

/// Une version à mettre en file doit être bien formée et signée : le contenu des fichiers vient du disque.
fn check_version(v: &SignedVersion) -> Result<(), String> {
    v.header.validate().map_err(|e| e.to_string())?;
    if !v.verify() {
        return Err("signature invalide".into());
    }
    Ok(())
}

/// Points de reprise en file que `new` remplace : ses ancêtres, à travers les entrées du même groupe, tant que ce
/// sont des `Checkpoint`.
fn superseded_checkpoints(map: &BTreeMap<VersionId, OutboxEntry>, new: &OutboxEntry) -> Vec<VersionId> {
    let mut out = Vec::new();
    let mut cur = new.version.header.parent;
    while let Some(p) = cur {
        match map.get(&p) {
            Some(e) if e.group_id() == new.group_id() && e.version.header.kind == VersionKind::Checkpoint && !out.contains(&p) => {
                out.push(p);
                cur = e.version.header.parent;
            }
            _ => break,
        }
    }
    out
}

/// Poignée de la boucle d'envoi.
pub struct DrainTask {
    stop: watch::Sender<bool>,
    handle: Option<JoinHandle<()>>,
}

impl DrainTask {
    /// Arrête la boucle et attend sa fin.
    pub async fn stop(mut self) {
        let _ = self.stop.send(true);
        if let Some(h) = self.handle.take() {
            let _ = h.await;
        }
    }
}

impl Drop for DrainTask {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
    }
}

// ---------------------------------------------------------------------------------------------------------
// Cible réseau : un membre joint par le protocole Hearth
// ---------------------------------------------------------------------------------------------------------

/// Un membre joint par `cubelink/hearth/1` : on lui annonce la version (`OfferVersion`), il rapatrie les blobs de
/// son côté, puis on l'interroge (`HaveVersion`) jusqu'à ce qu'il ait **tout**.
pub struct MemberReplica {
    endpoint: Endpoint,
    addr: EndpointAddr,
    groups: BTreeSet<GroupId>,
    confirm_timeout: Duration,
}

impl MemberReplica {
    /// `groups` : les groupes que ce membre partage avec nous.
    pub fn new(endpoint: Endpoint, addr: EndpointAddr, groups: BTreeSet<GroupId>) -> Self {
        Self { endpoint, addr, groups, confirm_timeout: CONFIRM_TIMEOUT }
    }

    /// Délai accordé au membre pour tout rapatrier (par défaut [`CONFIRM_TIMEOUT`]).
    pub fn with_confirm_timeout(mut self, t: Duration) -> Self {
        self.confirm_timeout = t;
        self
    }
}

impl Replica for MemberReplica {
    fn id(&self) -> String {
        hex::encode(self.addr.id.as_bytes())
    }

    fn kind(&self) -> ReplicaKind {
        ReplicaKind::Member
    }

    fn serves(&self, group: &GroupId) -> bool {
        self.groups.contains(group)
    }

    fn push<'a>(&'a self, entry: &'a OutboxEntry) -> BoxFuture<'a, Result<(), PushError>> {
        Box::pin(async move {
            let conn = HearthConn::connect(&self.endpoint, self.addr.clone()).await.map_err(|e| PushError::new(e.to_string()))?;
            conn.offer_version(&entry.version).await.map_err(|e| PushError::new(e.to_string()))?;
            let group = entry.group_id();
            let id = entry.id();
            let deadline = tokio::time::Instant::now() + self.confirm_timeout;
            loop {
                match conn.have_version(&group, &id).await {
                    Ok(true) => return Ok(()),
                    Ok(false) => {}
                    // Le membre ne prendra pas cette version (trop grosse d'un coup) : inutile de l'attendre.
                    Err(NetError::Rejected(r)) if r.starts_with(REFUSED_PREFIX) => {
                        return Err(PushError::refused(r[REFUSED_PREFIX.len()..].trim()));
                    }
                    Err(e) => return Err(PushError::new(e.to_string())),
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err(PushError::new("le membre n'a pas fini de récupérer la version"));
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::model::{testutil, HearthSigner};
    use super::super::version::{VersionHeader, VersionKind};
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn version(group: GroupId, number: u64, tag: u8) -> SignedVersion {
        let k = testutil::key(1);
        let header = VersionHeader {
            group_id: group,
            number,
            parent: if number == 1 { None } else { Some(VersionId([tag; 32])) },
            epoch: 0,
            author: k.peer_key(),
            pack_hash: "a".repeat(64),
            created_at: 100 + number,
            played_secs: 5,
            kind: VersionKind::Checkpoint,
            collection: [tag; 32],
            total_size: 9,
        };
        SignedVersion::sign(&k, header).unwrap()
    }

    struct Fake {
        id: &'static str,
        kind: ReplicaKind,
        groups: Vec<GroupId>,
        fail: bool,
        calls: AtomicUsize,
    }

    impl Fake {
        fn new(id: &'static str, kind: ReplicaKind, groups: Vec<GroupId>, fail: bool) -> Arc<Self> {
            Arc::new(Self { id, kind, groups, fail, calls: AtomicUsize::new(0) })
        }
    }

    impl Replica for Fake {
        fn id(&self) -> String {
            self.id.into()
        }
        fn kind(&self) -> ReplicaKind {
            self.kind
        }
        fn serves(&self, g: &GroupId) -> bool {
            self.groups.contains(g)
        }
        fn push<'a>(&'a self, _e: &'a OutboxEntry) -> BoxFuture<'a, Result<(), PushError>> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                if self.fail {
                    Err(PushError::new("hors ligne\nbizarre"))
                } else {
                    Ok(())
                }
            })
        }
    }

    fn as_dyn(r: &Arc<Fake>) -> Arc<dyn Replica> {
        r.clone()
    }

    #[tokio::test]
    async fn keeps_entry_until_a_member_is_reachable() {
        let tmp = tempfile::tempdir().unwrap();
        let g = GroupId([1; 16]);
        let ob = Outbox::open(tmp.path().join("outbox")).unwrap();
        let v = version(g, 1, 1);
        assert!(ob.enqueue(v.clone()).unwrap());
        assert!(!ob.enqueue(v.clone()).unwrap(), "idempotent");
        // Personne de joignable.
        let r = ob.drain(&[]).await;
        assert_eq!((r.delivered.len(), r.pending), (0, 1));
        // Un membre d'un autre groupe ne compte pas.
        let other = Fake::new("autre", ReplicaKind::Member, vec![GroupId([2; 16])], false);
        let r = ob.drain(&[as_dyn(&other)]).await;
        assert_eq!((r.delivered.len(), r.pending, other.calls.load(Ordering::SeqCst)), (0, 1, 0));
        // Un membre qui échoue : l'entrée reste, l'échec est noté (texte nettoyé).
        let bad = Fake::new("bob", ReplicaKind::Member, vec![g], true);
        let r = ob.drain(&[as_dyn(&bad)]).await;
        assert_eq!((r.delivered.len(), r.pending), (0, 1));
        assert_eq!(r.failures.len(), 1);
        assert!(!r.failures[0].2.contains('\n'));
        let e = &ob.pending()[0];
        assert_eq!(e.attempts, 1);
        assert!(e.last_error.is_some());
        // Le membre apparaît.
        let good = Fake::new("carol", ReplicaKind::Member, vec![g], false);
        let r = ob.drain(&[as_dyn(&bad), as_dyn(&good)]).await;
        assert_eq!(r.delivered, vec![v.id()]);
        assert!(ob.is_empty());
        assert!(!tmp.path().join("outbox").join(format!("{}.json", v.id())).exists());
    }

    #[tokio::test]
    async fn guardian_does_not_release_and_is_not_repeated() {
        let tmp = tempfile::tempdir().unwrap();
        let g = GroupId([1; 16]);
        let dir = tmp.path().join("outbox");
        let ob = Outbox::open(&dir).unwrap();
        let v = version(g, 1, 1);
        ob.enqueue(v.clone()).unwrap();
        let guard = Fake::new("gardien-1", ReplicaKind::Guardian, vec![g], false);
        let r = ob.drain(&[as_dyn(&guard)]).await;
        assert_eq!((r.delivered.len(), r.pending), (0, 1));
        let r = ob.drain(&[as_dyn(&guard)]).await;
        assert_eq!(r.pending, 1);
        assert_eq!(guard.calls.load(Ordering::SeqCst), 1, "le gardien ne reçoit pas deux fois la même entrée");
        // Après un redémarrage, la mémoire des gardiens servis est conservée.
        drop(ob);
        let ob = Outbox::open(&dir).unwrap();
        assert_eq!(ob.pending()[0].delivered.len(), 1);
        ob.drain(&[as_dyn(&guard)]).await;
        assert_eq!(guard.calls.load(Ordering::SeqCst), 1);
        // Le membre libère l'entrée.
        let m = Fake::new("dave", ReplicaKind::Member, vec![g], false);
        let r = ob.drain(&[as_dyn(&guard), as_dyn(&m)]).await;
        assert_eq!(r.delivered.len(), 1);
        assert!(ob.is_empty());
    }

    #[tokio::test]
    async fn survives_restart_and_skips_bad_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("outbox");
        let g = GroupId([1; 16]);
        {
            let ob = Outbox::open(&dir).unwrap();
            ob.enqueue(version(g, 1, 1)).unwrap();
            ob.enqueue(version(g, 2, 2)).unwrap();
        }
        fs::write(dir.join(format!("{}.json", "f".repeat(64))), b"{pas du json").unwrap();
        fs::write(dir.join(".reste.json.tmp-1"), b"x").unwrap();
        fs::write(dir.join("autre.txt"), b"x").unwrap();
        let ob = Outbox::open(&dir).unwrap();
        assert_eq!(ob.len(), 2);
        assert_eq!(ob.skipped().len(), 1);
        assert!(!dir.join(".reste.json.tmp-1").exists());
        assert_eq!(ob.pinned_collections().len(), 2);
        assert!(ob.remove(&ob.pending()[0].id()).unwrap());
        assert_eq!(Outbox::open(&dir).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn rejects_unsigned_versions_and_overflow() {
        let tmp = tempfile::tempdir().unwrap();
        let g = GroupId([1; 16]);
        let ob = Outbox::open(tmp.path()).unwrap();
        let mut v = version(g, 1, 1);
        v.header.played_secs += 1;
        assert!(matches!(ob.enqueue(v), Err(OutboxError::Invalid(_))));
        assert!(ob.is_empty());
    }

    #[tokio::test]
    async fn concurrent_drain_is_refused() {
        struct Slow(tokio::sync::Notify, GroupId);
        impl Replica for Slow {
            fn id(&self) -> String {
                "lent".into()
            }
            fn kind(&self) -> ReplicaKind {
                ReplicaKind::Member
            }
            fn serves(&self, g: &GroupId) -> bool {
                *g == self.1
            }
            fn push<'a>(&'a self, _e: &'a OutboxEntry) -> BoxFuture<'a, Result<(), PushError>> {
                Box::pin(async move {
                    self.0.notified().await;
                    Ok(())
                })
            }
        }
        let tmp = tempfile::tempdir().unwrap();
        let g = GroupId([1; 16]);
        let ob = Arc::new(Outbox::open(tmp.path()).unwrap());
        ob.enqueue(version(g, 1, 1)).unwrap();
        let slow = Arc::new(Slow(tokio::sync::Notify::new(), g));
        let (ob2, slow2) = (ob.clone(), slow.clone());
        let first = tokio::spawn(async move { ob2.drain(&[slow2 as Arc<dyn Replica>]).await });
        tokio::time::sleep(Duration::from_millis(100)).await;
        let second = ob.drain(&[]).await;
        assert!(second.busy);
        slow.0.notify_one();
        let r = first.await.unwrap();
        assert_eq!(r.delivered.len(), 1);
    }

    #[tokio::test]
    async fn drain_loop_delivers_when_member_appears_and_stops() {
        let tmp = tempfile::tempdir().unwrap();
        let g = GroupId([1; 16]);
        let ob = Arc::new(Outbox::open(tmp.path()).unwrap());
        let online = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let member = Fake::new("erin", ReplicaKind::Member, vec![g], false);
        let (o2, m2) = (online.clone(), member.clone());
        let task = ob.spawn_drain_loop(
            Arc::new(move || if o2.load(Ordering::SeqCst) { vec![m2.clone() as Arc<dyn Replica>] } else { vec![] }),
            Duration::from_millis(100),
        );
        ob.enqueue(version(g, 1, 1)).unwrap();
        tokio::time::sleep(Duration::from_millis(350)).await;
        assert_eq!(ob.len(), 1, "personne en ligne : l'entrée attend");
        online.store(true, Ordering::SeqCst);
        ob.wake();
        for _ in 0..50 {
            if ob.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(ob.is_empty());
        task.stop().await;
    }

    // --- #99 : quorum, cibles indépendantes, refus, regroupement -------------------------------------------

    /// Une version d'une chaîne réelle (le parent est l'identifiant du parent donné).
    fn chain(group: GroupId, number: u64, parent: Option<&SignedVersion>, kind: VersionKind) -> SignedVersion {
        let k = testutil::key(1);
        let header = VersionHeader {
            group_id: group,
            number,
            parent: parent.map(SignedVersion::id),
            epoch: 0,
            author: k.peer_key(),
            pack_hash: "a".repeat(64),
            created_at: 100 + number,
            played_secs: 5,
            kind,
            collection: [number as u8; 32],
            total_size: 9,
        };
        SignedVersion::sign(&k, header).unwrap()
    }

    #[derive(Clone, Copy)]
    enum Behavior {
        Ok,
        Fail,
        Refuse,
        /// Ne rend jamais la main (membre en panne ou très lent).
        Hang,
    }

    struct Scripted {
        id: &'static str,
        group: GroupId,
        behavior: Mutex<Behavior>,
        calls: AtomicUsize,
    }

    impl Scripted {
        fn new(id: &'static str, group: GroupId, behavior: Behavior) -> Arc<Self> {
            Arc::new(Self { id, group, behavior: Mutex::new(behavior), calls: AtomicUsize::new(0) })
        }
        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
        fn dynr(self: &Arc<Self>) -> Arc<dyn Replica> {
            self.clone()
        }
    }

    impl Replica for Scripted {
        fn id(&self) -> String {
            self.id.into()
        }
        fn kind(&self) -> ReplicaKind {
            ReplicaKind::Member
        }
        fn serves(&self, g: &GroupId) -> bool {
            *g == self.group
        }
        fn push<'a>(&'a self, _e: &'a OutboxEntry) -> BoxFuture<'a, Result<(), PushError>> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                let b = *lock(&self.behavior);
                match b {
                    Behavior::Ok => Ok(()),
                    Behavior::Fail => Err(PushError::new("hors ligne")),
                    Behavior::Refuse => Err(PushError::refused("oversize")),
                    Behavior::Hang => std::future::pending().await,
                }
            })
        }
    }

    /// Un passage qui ne doit pas attendre le membre lent : sinon le test échoue au lieu de bloquer.
    async fn quick(ob: &Outbox, reachable: &[Arc<dyn Replica>]) -> DrainReport {
        tokio::time::timeout(Duration::from_secs(5), ob.drain(reachable)).await.expect("le passage attend un membre lent")
    }

    #[tokio::test]
    async fn needs_two_acks_with_three_members_and_does_not_wait_for_the_slow_one() {
        let tmp = tempfile::tempdir().unwrap();
        let g = GroupId([1; 16]);
        let ob = Outbox::open(tmp.path()).unwrap();
        ob.set_others(Arc::new(|_| 2));
        let v = version(g, 1, 1);
        ob.enqueue(v.clone()).unwrap();
        let (a, b, slow) = (Scripted::new("a", g, Behavior::Ok), Scripted::new("b", g, Behavior::Ok), Scripted::new("lent", g, Behavior::Hang));
        let r = quick(&ob, &[slow.dynr(), a.dynr(), b.dynr()]).await;
        assert_eq!(r.delivered, vec![v.id()]);
        assert!(ob.is_empty());
        assert!(!tmp.path().join(format!("{}.json", v.id())).exists());
    }

    #[tokio::test]
    async fn one_ack_is_kept_on_disk_until_a_second_member_confirms() {
        let tmp = tempfile::tempdir().unwrap();
        let g = GroupId([1; 16]);
        let ob = Outbox::open(tmp.path()).unwrap();
        ob.set_others(Arc::new(|_| 2));
        let v = version(g, 1, 1);
        ob.enqueue(v.clone()).unwrap();
        let (a, b) = (Scripted::new("a", g, Behavior::Ok), Scripted::new("b", g, Behavior::Fail));
        let r = quick(&ob, &[a.dynr(), b.dynr()]).await;
        assert_eq!((r.delivered.len(), r.pending), (0, 1), "un seul accusé ne suffit pas");
        assert_eq!(ob.pending()[0].stored, BTreeSet::from(["a".to_string()]));
        // Redémarrage : l'accusé est conservé, et `a` n'est pas redérangé.
        drop(ob);
        let ob = Outbox::open(tmp.path()).unwrap();
        ob.set_others(Arc::new(|_| 2));
        assert_eq!(ob.pending()[0].stored.len(), 1);
        *lock(&b.behavior) = Behavior::Ok;
        let r = quick(&ob, &[a.dynr(), b.dynr()]).await;
        assert_eq!(r.delivered, vec![v.id()]);
        assert_eq!(a.calls(), 1, "un membre déjà accusé n'est pas renvoyé");
    }

    #[tokio::test]
    async fn a_group_of_two_needs_a_single_ack() {
        let tmp = tempfile::tempdir().unwrap();
        let g = GroupId([1; 16]);
        let ob = Outbox::open(tmp.path()).unwrap();
        ob.set_others(Arc::new(|_| 1));
        ob.enqueue(version(g, 1, 1)).unwrap();
        let a = Scripted::new("a", g, Behavior::Ok);
        assert_eq!(quick(&ob, &[a.dynr()]).await.delivered.len(), 1);
    }

    #[tokio::test]
    async fn a_slow_target_does_not_hold_back_another_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let (g1, g2) = (GroupId([1; 16]), GroupId([2; 16]));
        let ob = Arc::new(Outbox::open(tmp.path()).unwrap());
        let (v1, v2) = (version(g1, 1, 1), version(g2, 1, 2));
        ob.enqueue(v1.clone()).unwrap();
        ob.enqueue(v2.clone()).unwrap();
        let reach = vec![Scripted::new("lent", g1, Behavior::Hang).dynr(), Scripted::new("vif", g2, Behavior::Ok).dynr()];
        let ob2 = ob.clone();
        let pass = tokio::spawn(async move { ob2.drain(&reach).await });
        for _ in 0..100 {
            if !ob.contains(&v2.id()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(!ob.contains(&v2.id()), "l'entrée du groupe rapide part sans attendre le membre lent de l'autre");
        assert!(ob.contains(&v1.id()));
        pass.abort();
    }

    #[tokio::test]
    async fn explicit_refusal_stops_retries_and_marks_the_entry_failed_but_pinned() {
        let tmp = tempfile::tempdir().unwrap();
        let g = GroupId([1; 16]);
        let ob = Outbox::open(tmp.path()).unwrap();
        ob.set_others(Arc::new(|_| 1));
        let v = version(g, 1, 1);
        ob.enqueue(v.clone()).unwrap();
        let refuser = Scripted::new("bob", g, Behavior::Refuse);
        let r = quick(&ob, &[refuser.dynr()]).await;
        assert_eq!((r.delivered.len(), r.pending, r.failures.len()), (0, 1, 1));
        let e = &ob.pending()[0];
        assert_eq!(e.failed.as_deref(), Some("oversize"));
        assert_eq!(e.refused.get("bob").map(String::as_str), Some("oversize"));
        assert_eq!(e.attempts, 1, "le refus compte une tentative : le départ ne l'attend pas");
        assert_eq!(ob.pinned_collections(), vec![(g, v.header.collection)], "la version reste épinglée");
        // Plus aucune tentative, ni au passage suivant ni après redémarrage.
        quick(&ob, &[refuser.dynr()]).await;
        drop(ob);
        let ob = Outbox::open(tmp.path()).unwrap();
        quick(&ob, &[refuser.dynr()]).await;
        assert_eq!(refuser.calls(), 1);
        assert_eq!(ob.len(), 1);
        // Au démarrage de l'application, les refus sont rejoués (le membre a pu confirmer entre-temps).
        ob.retry_failed();
        *lock(&refuser.behavior) = Behavior::Ok;
        assert_eq!(quick(&ob, &[refuser.dynr()]).await.delivered, vec![v.id()]);
    }

    #[tokio::test]
    async fn one_refusal_does_not_fail_the_entry_while_the_quorum_is_reachable() {
        let tmp = tempfile::tempdir().unwrap();
        let g = GroupId([1; 16]);
        let ob = Outbox::open(tmp.path()).unwrap();
        ob.set_others(Arc::new(|_| 3)); // quorum 2 parmi 3 autres
        let v = version(g, 1, 1);
        ob.enqueue(v.clone()).unwrap();
        let (a, b, c) = (Scripted::new("a", g, Behavior::Refuse), Scripted::new("b", g, Behavior::Ok), Scripted::new("c", g, Behavior::Fail));
        quick(&ob, &[a.dynr(), b.dynr(), c.dynr()]).await;
        assert!(ob.pending()[0].failed.is_none());
        *lock(&c.behavior) = Behavior::Ok;
        let r = quick(&ob, &[a.dynr(), b.dynr(), c.dynr()]).await;
        assert_eq!(r.delivered, vec![v.id()]);
        assert_eq!(a.calls(), 1, "le refus n'est pas retenté");
    }

    #[tokio::test]
    async fn two_refusals_out_of_two_others_make_the_quorum_impossible() {
        let tmp = tempfile::tempdir().unwrap();
        let g = GroupId([1; 16]);
        let ob = Outbox::open(tmp.path()).unwrap();
        ob.set_others(Arc::new(|_| 2));
        ob.enqueue(version(g, 1, 1)).unwrap();
        let (a, b) = (Scripted::new("a", g, Behavior::Ok), Scripted::new("b", g, Behavior::Refuse));
        quick(&ob, &[a.dynr(), b.dynr()]).await;
        assert!(ob.pending()[0].failed.is_some(), "un accusé et un refus : le quorum de 2 n'arrivera pas");
    }

    #[tokio::test]
    async fn a_new_checkpoint_replaces_the_queued_checkpoints_of_its_lineage() {
        let tmp = tempfile::tempdir().unwrap();
        let (g, other) = (GroupId([1; 16]), GroupId([2; 16]));
        let ob = Outbox::open(tmp.path()).unwrap();
        let c1 = chain(g, 1, None, VersionKind::Checkpoint);
        let c2 = chain(g, 2, Some(&c1), VersionKind::Checkpoint);
        let c3 = chain(g, 3, Some(&c2), VersionKind::Checkpoint);
        let alien = chain(other, 1, None, VersionKind::Checkpoint);
        ob.enqueue(alien.clone()).unwrap();
        ob.enqueue(c1.clone()).unwrap();
        ob.enqueue(c2.clone()).unwrap();
        assert!(!ob.contains(&c1.id()) && ob.contains(&c2.id()));
        assert!(!tmp.path().join(format!("{}.json", c1.id())).exists(), "le fichier de l'ancien point de reprise est supprimé");
        ob.enqueue(c3.clone()).unwrap();
        assert_eq!(ob.len(), 2);
        assert!(ob.contains(&c3.id()) && ob.contains(&alien.id()));
        assert_eq!(ob.pinned_collections().len(), 2, "la collection retirée n'est plus épinglée");
    }

    #[tokio::test]
    async fn sessions_and_restores_are_never_grouped() {
        let tmp = tempfile::tempdir().unwrap();
        let g = GroupId([1; 16]);
        let ob = Outbox::open(tmp.path()).unwrap();
        let c1 = chain(g, 1, None, VersionKind::Checkpoint);
        let s2 = chain(g, 2, Some(&c1), VersionKind::Session);
        let c3 = chain(g, 3, Some(&s2), VersionKind::Checkpoint);
        let r4 = chain(g, 4, Some(&c3), VersionKind::Restore { from: c1.id() });
        let c5 = chain(g, 5, Some(&r4), VersionKind::Checkpoint);
        for v in [&c1, &s2, &c3, &r4, &c5] {
            ob.enqueue(v.clone()).unwrap();
        }
        // Chaque Checkpoint s'arrête au premier Session/Restore de sa lignée : seuls les ancêtres directs partent.
        assert!(ob.contains(&c1.id()) && ob.contains(&s2.id()) && ob.contains(&r4.id()) && ob.contains(&c5.id()));
        assert!(ob.contains(&c3.id()), "c3 est parent d'un Restore, pas d'un Checkpoint");
        assert_eq!(ob.len(), 5);
    }

    #[tokio::test]
    async fn an_entry_removed_during_a_pass_is_not_recreated_on_disk() {
        struct Remover(Arc<Outbox>);
        impl Replica for Remover {
            fn id(&self) -> String {
                "a".into()
            }
            fn kind(&self) -> ReplicaKind {
                ReplicaKind::Member
            }
            fn serves(&self, _: &GroupId) -> bool {
                true
            }
            fn push<'a>(&'a self, e: &'a OutboxEntry) -> BoxFuture<'a, Result<(), PushError>> {
                Box::pin(async move {
                    self.0.remove(&e.id()).unwrap(); // regroupée ou groupe oublié pendant la remise
                    Ok(())
                })
            }
        }
        let tmp = tempfile::tempdir().unwrap();
        let g = GroupId([1; 16]);
        let ob = Arc::new(Outbox::open(tmp.path()).unwrap());
        ob.set_others(Arc::new(|_| 2)); // un accusé ne suffit pas : l'entrée serait réécrite
        let v = version(g, 1, 1);
        ob.enqueue(v.clone()).unwrap();
        let r = quick(&ob, &[Arc::new(Remover(ob.clone())) as Arc<dyn Replica>]).await;
        assert!(r.delivered.is_empty() && ob.is_empty());
        assert!(!tmp.path().join(format!("{}.json", v.id())).exists());
    }

    #[tokio::test]
    async fn entries_written_before_the_quorum_fields_still_load() {
        let tmp = tempfile::tempdir().unwrap();
        let g = GroupId([1; 16]);
        let v = version(g, 1, 1);
        let old = serde_json::json!({ "version": v, "queued_at": 5, "delivered": [], "attempts": 2, "last_error": "x" });
        fs::write(tmp.path().join(format!("{}.json", v.id())), serde_json::to_vec(&old).unwrap()).unwrap();
        let ob = Outbox::open(tmp.path()).unwrap();
        assert!(ob.skipped().is_empty());
        let e = &ob.pending()[0];
        assert!(e.stored.is_empty() && e.refused.is_empty() && e.failed.is_none());
        assert_eq!(e.attempts, 2);
    }
}
