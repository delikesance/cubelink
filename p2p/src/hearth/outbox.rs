//! Hearth : file d'envoi persistante des versions (voir `docs/hearth.md`, § 7 « Envoi des versions »).
//!
//! Chaque version produite ici (point de reprise, fin de partie) entre dans l'`Outbox`, un dossier de fichiers
//! `<id de version>.json` écrits de façon atomique : la file **survit au redémarrage**. [`Outbox::drain`] pousse
//! chaque entrée vers les cibles joignables ([`Replica`]) :
//!
//! * une cible **membre** accepte quand elle a l'en-tête *et* tout le contenu : dès qu'**au moins un membre** a
//!   accepté, l'entrée quitte la file (les autres membres rattrapent auprès de lui ou de nous) ;
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
use iroh::{Endpoint, EndpointAddr};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{watch, Notify};
use tokio::task::JoinHandle;

use super::model::GroupId;
use super::proto::HearthConn;
use super::version::{BlobHash, SignedVersion, VersionId};
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
    /// Cibles déjà servies (identifiants de [`Replica::id`]) ; seuls les gardiens y restent, une remise à un membre
    /// retire l'entrée.
    #[serde(default)]
    pub delivered: BTreeSet<String>,
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
#[error("{0}")]
pub struct PushError(pub String);

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
    /// Versions remises à au moins un membre (retirées de la file).
    pub delivered: Vec<VersionId>,
    /// Entrées encore en file après le passage.
    pub pending: usize,
    /// Remises échouées : version, cible, raison.
    pub failures: Vec<(VersionId, String, String)>,
    /// Un autre passage était en cours : rien n'a été fait.
    pub busy: bool,
}

/// File d'envoi persistante.
pub struct Outbox {
    dir: PathBuf,
    entries: Mutex<BTreeMap<VersionId, OutboxEntry>>,
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
    let tmp = path.with_file_name(format!(".{name}.tmp-{:016x}", rand::rngs::OsRng.next_u64()));
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
        Ok(Self { dir, entries: Mutex::new(entries), drain_lock: tokio::sync::Mutex::new(()), wake: Notify::new(), skipped })
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
    pub fn enqueue(&self, version: SignedVersion) -> Result<bool, OutboxError> {
        check_version(&version).map_err(OutboxError::Invalid)?;
        let entry = OutboxEntry { version, queued_at: now_secs(), delivered: BTreeSet::new(), attempts: 0, last_error: None };
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
        lock(&self.entries).insert(id, entry);
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

    /// Un passage : pousse chaque entrée vers les cibles joignables de `reachable`. Un seul passage à la fois
    /// (un deuxième appel simultané rend `busy` sans rien faire).
    pub async fn drain(&self, reachable: &[Arc<dyn Replica>]) -> DrainReport {
        let Ok(_guard) = self.drain_lock.try_lock() else { return DrainReport { busy: true, pending: self.len(), ..Default::default() } };
        let mut report = DrainReport::default();
        for entry in self.pending() {
            let group = entry.group_id();
            // Les membres d'abord ; les gardiens déjà servis sont sautés.
            let targets: Vec<&Arc<dyn Replica>> = reachable
                .iter()
                .filter(|r| r.serves(&group) && !(r.kind() == ReplicaKind::Guardian && entry.delivered.contains(&r.id())))
                .collect();
            if targets.is_empty() {
                continue;
            }
            let results = join_all(targets.iter().map(|r| async {
                let out = match tokio::time::timeout(PUSH_TIMEOUT, r.push(&entry)).await {
                    Ok(res) => res,
                    Err(_) => Err(PushError("délai dépassé".into())),
                };
                (r.id(), r.kind(), out)
            }))
            .await;
            let mut member_ok = false;
            let mut updated = entry.clone();
            for (rid, kind, out) in results {
                match out {
                    Ok(()) => match kind {
                        ReplicaKind::Member => member_ok = true,
                        ReplicaKind::Guardian => {
                            updated.delivered.insert(rid);
                        }
                    },
                    Err(e) => {
                        let text: String = e.0.chars().filter(|c| !c.is_control()).take(200).collect();
                        updated.attempts = updated.attempts.saturating_add(1);
                        updated.last_error = Some(text.clone());
                        report.failures.push((entry.id(), rid, text));
                    }
                }
            }
            if member_ok {
                if self.remove(&entry.id()).is_ok() {
                    report.delivered.push(entry.id());
                }
            } else if updated != entry {
                // Mémorise gardiens servis et échecs ; si le disque refuse, l'entrée reste telle quelle en file.
                if self.persist(&updated).is_ok() {
                    if let Some(slot) = lock(&self.entries).get_mut(&entry.id()) {
                        *slot = updated;
                    }
                }
            }
        }
        report.pending = self.len();
        report
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
            let conn = HearthConn::connect(&self.endpoint, self.addr.clone()).await.map_err(|e| PushError(e.to_string()))?;
            conn.offer_version(&entry.version).await.map_err(|e| PushError(e.to_string()))?;
            let group = entry.group_id();
            let id = entry.id();
            let deadline = tokio::time::Instant::now() + self.confirm_timeout;
            loop {
                if conn.have_version(&group, &id).await.map_err(|e| PushError(e.to_string()))? {
                    return Ok(());
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err(PushError("le membre n'a pas fini de récupérer la version".into()));
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
                    Err(PushError("hors ligne\nbizarre".into()))
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
}
