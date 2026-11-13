//! Tâche N13 : reprise après coupure, couche autonome par-dessus `BlobTransport` (send.rs) et le magasin.
//!
//! Journal `<store_root>/.transfers/<id>.json` (écriture atomique), fichiers partiels
//! `<store_root>/.tmp/<sha256>.resume.part`. Le journal n'est JAMAIS cru sur parole : à chaque
//! tentative, le préfixe déjà présent est re-haché et comparé au hash du journal ; en cas de doute le
//! partiel est supprimé. Le sha256 final et la taille du manifeste sont toujours vérifiés.
//! Intégrité : jamais de nouvelle tentative (`Error::Integrity`), ni pour `Error::Cancelled`.

use std::collections::HashSet;
use std::future::Future;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use core_lib::store::hash::{sha256_reader, verify_file};
use core_lib::store::index::{object_rel_path, StoreIndex};
use core_lib::types::{Error, ManifestFile, Result, TransferPhase, TransferProgress};
use futures_util::stream::{FuturesUnordered, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use super::plan::Plan;
use super::recv::{RecvOptions, RecvSummary};
use super::send::{BlobTransport, FetchRequest};

/// Version du format de journal ; un autre numéro est ignoré.
pub const JOURNAL_VERSION: u32 = 1;
/// Âge par défaut au-delà duquel un journal orphelin est purgé : 7 jours.
pub const DEFAULT_ORPHAN_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);

/// Backoff exponentiel plafonné avec gigue.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RetryPolicy {
    /// Nombre maximal de nouvelles tentatives consécutives sans progrès.
    pub max_retries: u32,
    pub base: Duration,
    pub cap: Duration,
    /// Gigue relative, 0.0 à 1.0 : le délai varie de ±`jitter`.
    pub jitter: f64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self { max_retries: 8, base: Duration::from_millis(500), cap: Duration::from_secs(30), jitter: 0.25 }
    }
}

impl RetryPolicy {
    /// Délai avant la nouvelle tentative `retry` (0 = première) ; `unit` dans [0, 1] pilote la gigue.
    pub fn delay_for(&self, retry: u32, unit: f64) -> Duration {
        let raw = self.base.as_secs_f64() * 2f64.powi(retry.min(30) as i32);
        let j = self.jitter.clamp(0.0, 1.0);
        let factor = 1.0 + j * (2.0 * unit.clamp(0.0, 1.0) - 1.0);
        Duration::from_secs_f64((raw * factor).min(self.cap.as_secs_f64()).max(0.0))
    }
}

/// Attente injectable (temps simulé en test).
pub type SleepFn = Arc<dyn Fn(Duration) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Configuration d'un transfert reprenable.
#[derive(Clone)]
pub struct ResumeConfig {
    pub opts: RecvOptions,
    pub retry: RetryPolicy,
    pub orphan_max_age: Duration,
    pub sleeper: SleepFn,
}

impl Default for ResumeConfig {
    fn default() -> Self {
        Self {
            opts: RecvOptions::default(),
            retry: RetryPolicy::default(),
            orphan_max_age: DEFAULT_ORPHAN_MAX_AGE,
            sleeper: Arc::new(|d| Box::pin(tokio::time::sleep(d))),
        }
    }
}

/// État observable du transfert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    Idle,
    Running,
    Paused,
    Done,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct DoneEntry {
    sha256: String,
    size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Partial {
    sha256: String,
    /// Longueur du préfixe à l'écriture du journal.
    offset: u64,
    /// sha256 de ces `offset` premiers octets : vérifié avant toute reprise.
    prefix_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Journal {
    version: u32,
    id: String,
    peer: String,
    updated_unix: u64,
    done: Vec<DoneEntry>,
    partials: Vec<Partial>,
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn journal_dir(store_root: &Path) -> PathBuf {
    store_root.join(".transfers")
}

fn journal_path(store_root: &Path, id: &str) -> PathBuf {
    journal_dir(store_root).join(format!("{id}.json"))
}

fn part_path(store_root: &Path, sha256: &str) -> PathBuf {
    store_root.join(".tmp").join(format!("{sha256}.resume.part"))
}

fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp{:08x}", rand::random::<u32>()));
    let res = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    Ok(res?)
}

/// Lit un journal ; corrompu, d'une autre version ou d'un autre identifiant : `None`.
fn load_journal(path: &Path, id: &str) -> Option<Journal> {
    let data = std::fs::read(path).ok()?;
    let j: Journal = serde_json::from_slice(&data).ok()?;
    (j.version == JOURNAL_VERSION && j.id == id).then_some(j)
}

/// Re-hache les `offset` premiers octets de `path` ; si le hash correspond, tronque à `offset`.
fn verify_and_truncate(path: &Path, offset: u64, expected: &str) -> Result<bool> {
    let f = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
    if f.metadata()?.len() < offset {
        return Ok(false);
    }
    if sha256_reader((&f).take(offset))? != expected {
        return Ok(false);
    }
    f.set_len(offset)?;
    Ok(true)
}

fn prefix_hash(path: &Path, len: u64) -> Result<String> {
    sha256_reader(std::fs::File::open(path)?.take(len))
}

/// Un seul transfert actif par journal (donc par identifiant et magasin).
static ACTIVE: LazyLock<Mutex<HashSet<PathBuf>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

struct ActiveGuard(PathBuf);

impl ActiveGuard {
    fn acquire(path: PathBuf) -> Result<Self> {
        if lock(&ACTIVE).insert(path.clone()) {
            Ok(Self(path))
        } else {
            Err(Error::Invalid("un transfert est déjà actif pour cet identifiant".into()))
        }
    }
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        lock(&ACTIVE).remove(&self.0);
    }
}

/// Purge les journaux (et partiels associés) plus vieux que `max_age`, hors transferts actifs, ainsi que
/// les partiels orphelins du même âge. Retourne le nombre de journaux supprimés.
pub fn purge_orphans(store_root: &Path, max_age: Duration) -> Result<usize> {
    let old = |p: &Path| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .is_some_and(|age| age > max_age)
    };
    let mut removed = 0;
    if let Ok(rd) = std::fs::read_dir(journal_dir(store_root)) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") || lock(&ACTIVE).contains(&p) || !old(&p) {
                continue;
            }
            if let Some(j) = std::fs::read(&p).ok().and_then(|d| serde_json::from_slice::<Journal>(&d).ok()) {
                for part in &j.partials {
                    if object_rel_path(&part.sha256).is_ok() {
                        let _ = std::fs::remove_file(part_path(store_root, &part.sha256));
                    }
                }
            }
            if std::fs::remove_file(&p).is_ok() {
                removed += 1;
            }
        }
    }
    if let Ok(rd) = std::fs::read_dir(store_root.join(".tmp")) {
        for e in rd.flatten() {
            let p = e.path();
            if p.to_string_lossy().ends_with(".resume.part") && old(&p) {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
    Ok(removed)
}

struct Shared {
    start: Instant,
    bytes: AtomicU64,
    files: AtomicU64,
    files_total: u32,
    bytes_total: u64,
    /// Plus haut volume déjà compté pour chaque fichier en cours : une nouvelle tentative qui repart de
    /// zéro (reprise intra-blob impossible côté iroh) ne fait pas reculer le compteur, elle ne le fait
    /// avancer qu'au-delà de ce qui était déjà compté.
    high_water: Mutex<std::collections::HashMap<String, u64>>,
}

/// Réception reprenable d'un plan.
pub struct ResumableReceive<T: BlobTransport> {
    id: String,
    peer: String,
    plan: Plan,
    transport: Arc<T>,
    store_root: PathBuf,
    index: Arc<StoreIndex>,
    cfg: ResumeConfig,
    journal: Mutex<Journal>,
    abort: AtomicBool,
    paused: AtomicBool,
    cancelled: AtomicBool,
    notify: Notify,
    state: Mutex<RunState>,
    _guard: ActiveGuard,
}

impl<T: BlobTransport> ResumableReceive<T> {
    /// Nouveau transfert : tout journal existant pour `id` est écarté (le cache reste utilisé).
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        id: &str,
        plan: Plan,
        peer: &str,
        transport: Arc<T>,
        store_root: &Path,
        index: Arc<StoreIndex>,
        cfg: ResumeConfig,
    ) -> Result<Self> {
        Self::open(id, plan, peer, transport, store_root, index, cfg, false)
    }

    /// Reprend après coupure ou arrêt de l'app : relit le journal s'il est valide, sinon repart du cache.
    #[allow(clippy::too_many_arguments)]
    pub fn recover(
        id: &str,
        plan: Plan,
        peer: &str,
        transport: Arc<T>,
        store_root: &Path,
        index: Arc<StoreIndex>,
        cfg: ResumeConfig,
    ) -> Result<Self> {
        Self::open(id, plan, peer, transport, store_root, index, cfg, true)
    }

    #[allow(clippy::too_many_arguments)]
    fn open(
        id: &str,
        plan: Plan,
        peer: &str,
        transport: Arc<T>,
        store_root: &Path,
        index: Arc<StoreIndex>,
        cfg: ResumeConfig,
        reuse: bool,
    ) -> Result<Self> {
        if !valid_id(id) {
            return Err(Error::Invalid(format!("identifiant de transfert invalide : {id:?}")));
        }
        let jpath = journal_path(store_root, id);
        let guard = ActiveGuard::acquire(jpath.clone())?;
        let _ = purge_orphans(store_root, cfg.orphan_max_age);
        let fresh = Journal {
            version: JOURNAL_VERSION,
            id: id.to_string(),
            peer: peer.to_string(),
            updated_unix: now_unix(),
            done: Vec::new(),
            partials: Vec::new(),
        };
        let journal = if reuse { load_journal(&jpath, id).unwrap_or(fresh) } else { fresh };
        if !reuse {
            for f in &plan.to_receive {
                let _ = std::fs::remove_file(part_path(store_root, &f.sha256));
            }
        }
        let me = Self {
            id: id.to_string(),
            peer: peer.to_string(),
            plan,
            transport,
            store_root: store_root.to_path_buf(),
            index,
            cfg,
            journal: Mutex::new(journal),
            abort: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            notify: Notify::new(),
            state: Mutex::new(RunState::Idle),
            _guard: guard,
        };
        me.save_journal()?;
        Ok(me)
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn state(&self) -> RunState {
        *lock(&self.state)
    }

    fn set_state(&self, s: RunState) {
        *lock(&self.state) = s;
    }

    /// Pause manuelle (join_pause) : interrompt les fichiers en cours, partiels conservés.
    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
        self.abort.store(true, Ordering::SeqCst);
    }

    /// Reprise manuelle (join_resume).
    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
        if !self.cancelled.load(Ordering::SeqCst) {
            self.abort.store(false, Ordering::SeqCst);
        }
        self.notify.notify_one();
    }

    /// Annulation : `run` retourne `Cancelled`, temporaires et journal sont supprimés.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.abort.store(true, Ordering::SeqCst);
        self.notify.notify_one();
    }

    fn save_journal(&self) -> Result<()> {
        let mut j = lock(&self.journal);
        j.updated_unix = now_unix();
        write_atomic(&journal_path(&self.store_root, &self.id), &serde_json::to_vec_pretty(&*j).map_err(|e| Error::Other(e.to_string()))?)
    }

    fn cached(&self, f: &ManifestFile) -> bool {
        let Ok(Some(e)) = self.index.get(&f.sha256) else { return false };
        e.size == f.size && object_rel_path(&f.sha256).is_ok_and(|r| self.store_root.join(r).is_file())
    }

    /// Ce qui manque réellement : plan moins cache (a) et moins `skip_sha256`.
    fn remaining(&self) -> Vec<ManifestFile> {
        self.plan
            .to_receive
            .iter()
            .filter(|f| !self.cfg.opts.skip_sha256.contains(&f.sha256) && !self.cached(f))
            .cloned()
            .collect()
    }

    fn emit(&self, sh: &Shared, progress: &(dyn Fn(TransferProgress) + Send + Sync), phase: TransferPhase, error: Option<String>) {
        let bytes = sh.bytes.load(Ordering::Relaxed).min(sh.bytes_total);
        let secs = sh.start.elapsed().as_secs_f64();
        progress(TransferProgress {
            phase,
            files_done: sh.files.load(Ordering::Relaxed) as u32,
            files_total: sh.files_total,
            bytes_done: bytes,
            bytes_total: sh.bytes_total,
            speed_bps: if secs > 0.0 { (bytes as f64 / secs) as u64 } else { 0 },
            via_relay: self.transport.via_relay(&self.peer),
            error,
        });
    }

    /// Exécute le transfert jusqu'au bout (reprises automatiques comprises).
    pub async fn run(&self, progress: &(dyn Fn(TransferProgress) + Send + Sync)) -> Result<RecvSummary> {
        let sh = Shared {
            start: Instant::now(),
            bytes: AtomicU64::new(0),
            files: AtomicU64::new(0),
            files_total: self.plan.to_receive.len() as u32,
            bytes_total: self.plan.to_receive_bytes,
            high_water: Mutex::new(Default::default()),
        };
        // Fichiers déjà en cache (reprise, nouvel essai) : comptés d'emblée, jamais retransférés.
        let todo = self.remaining().len();
        let rem_bytes: u64 = self.remaining().iter().map(|f| f.size).sum();
        sh.bytes.store(sh.bytes_total.saturating_sub(rem_bytes), Ordering::Relaxed);
        sh.files.store((self.plan.to_receive.len() - todo) as u64, Ordering::Relaxed);
        self.set_state(RunState::Running);
        let r = self.drive(&sh, progress).await;
        match &r {
            Ok(_) => {
                self.set_state(RunState::Done);
                let _ = std::fs::remove_file(journal_path(&self.store_root, &self.id));
                self.emit(&sh, progress, TransferPhase::Done, None);
            }
            Err(Error::Cancelled) => {
                self.set_state(RunState::Cancelled);
                self.cleanup_all();
                self.emit(&sh, progress, TransferPhase::Failed, Some(Error::Cancelled.to_string()));
            }
            Err(e) => {
                self.set_state(RunState::Failed);
                self.emit(&sh, progress, TransferPhase::Failed, Some(e.to_string()));
            }
        }
        r
    }

    async fn wait_if_paused(&self, sh: &Shared, progress: &(dyn Fn(TransferProgress) + Send + Sync)) -> Result<()> {
        loop {
            if self.cancelled.load(Ordering::SeqCst) {
                return Err(Error::Cancelled);
            }
            if !self.paused.load(Ordering::SeqCst) {
                self.set_state(RunState::Running);
                return Ok(());
            }
            if self.state() != RunState::Paused {
                self.set_state(RunState::Paused);
                self.emit(sh, progress, TransferPhase::Paused, None);
            }
            self.notify.notified().await;
        }
    }

    async fn drive(&self, sh: &Shared, progress: &(dyn Fn(TransferProgress) + Send + Sync)) -> Result<RecvSummary> {
        let o = &self.cfg.opts;
        if let Some(f) = self.plan.to_receive.iter().find(|f| f.size > o.max_file_bytes) {
            return Err(Error::Invalid(format!("fichier trop volumineux : {:?} ({} octets)", f.path, f.size)));
        }
        if sh.bytes_total > o.max_total_bytes {
            return Err(Error::Invalid(format!("transfert trop volumineux : {} octets", sh.bytes_total)));
        }
        tokio::fs::create_dir_all(self.store_root.join(".tmp")).await?;
        self.emit(sh, progress, TransferPhase::Preparing, None);
        let mut summary = RecvSummary::default();
        let mut retries = 0u32;
        loop {
            self.wait_if_paused(sh, progress).await?;
            let mut remaining = self.remaining();
            // Les plus gros d'abord : un gros mod parti en dernier finirait seul et laisserait le lien sous-utilisé.
            remaining.sort_by_key(|f| std::cmp::Reverse(f.size));
            if remaining.is_empty() {
                return Ok(summary);
            }
            let before = sh.bytes.load(Ordering::Relaxed);
            let res = self.attempt(&remaining, sh, progress, &mut summary).await;
            let Err(err) = res else { continue };
            if matches!(err, Error::Cancelled) && self.cancelled.load(Ordering::SeqCst) {
                return Err(err);
            }
            self.checkpoint(&remaining).await;
            match err {
                Error::Cancelled if self.paused.load(Ordering::SeqCst) => continue,
                Error::Network(_) | Error::Cancelled => {
                    if sh.bytes.load(Ordering::Relaxed) > before {
                        retries = 0;
                    }
                    if retries >= self.cfg.retry.max_retries {
                        return Err(err);
                    }
                    let d = self.cfg.retry.delay_for(retries, rand::random::<f64>());
                    retries += 1;
                    (self.cfg.sleeper)(d).await;
                }
                e => return Err(e),
            }
        }
    }

    async fn attempt(
        &self,
        remaining: &[ManifestFile],
        sh: &Shared,
        progress: &(dyn Fn(TransferProgress) + Send + Sync),
        summary: &mut RecvSummary,
    ) -> Result<()> {
        // `FuturesUnordered` + boucle explicite (et non `stream::iter(..).map(closure).buffer_unordered`) :
        // la fermeture empruntante rendait le futur de `run` non prouvé `Send` (HRTB), cf. `run_future_is_send`.
        let max = self.cfg.opts.concurrency.max(1);
        let mut pending = remaining.iter();
        let mut inflight = FuturesUnordered::new();
        loop {
            while inflight.len() < max {
                let Some(f) = pending.next() else { break };
                inflight.push(self.fetch_one(f, sh, progress));
            }
            let Some(r) = inflight.next().await else { break };
            let size = r?;
            summary.files += 1;
            summary.bytes += size;
        }
        Ok(())
    }

    /// Prépare le partiel : vérifie le préfixe contre le journal, tronque ; retourne l'offset sûr.
    async fn prepare_partial(&self, f: &ManifestFile) -> Result<u64> {
        let path = part_path(&self.store_root, &f.sha256);
        let Ok(meta) = tokio::fs::metadata(&path).await else { return Ok(0) };
        let entry = lock(&self.journal).partials.iter().find(|p| p.sha256 == f.sha256).cloned();
        if let Some(e) = entry {
            if e.offset > 0 && e.offset < f.size && e.offset <= meta.len() {
                let p = path.clone();
                let ok = tokio::task::spawn_blocking(move || verify_and_truncate(&p, e.offset, &e.prefix_sha256))
                    .await
                    .map_err(|e| Error::Other(e.to_string()))?
                    .unwrap_or(false);
                if ok {
                    return Ok(e.offset);
                }
            }
        }
        let _ = tokio::fs::remove_file(&path).await;
        Ok(0)
    }

    async fn fetch_one(
        &self,
        f: &ManifestFile,
        sh: &Shared,
        progress: &(dyn Fn(TransferProgress) + Send + Sync),
    ) -> Result<u64> {
        if self.abort.load(Ordering::SeqCst) {
            return Err(Error::Cancelled);
        }
        let rel = object_rel_path(&f.sha256)?;
        let part = part_path(&self.store_root, &f.sha256);
        let offset = self.prepare_partial(f).await?;
        // Compte `cum` octets pour ce fichier, mais seulement au-delà du plus haut volume déjà compté.
        let credit = |cum: u64| {
            let cum = cum.min(f.size);
            let mut hw = lock(&sh.high_water);
            let seen = hw.entry(f.sha256.clone()).or_insert(0);
            if cum > *seen {
                sh.bytes.fetch_add(cum - *seen, Ordering::Relaxed);
                *seen = cum;
            }
        };
        credit(offset);
        let got = AtomicU64::new(offset);
        let req = FetchRequest { sha256: f.sha256.clone(), size: f.size, dest: part.clone(), resume_from: offset };
        let on_bytes = |n: u64| {
            credit(got.fetch_add(n, Ordering::Relaxed) + n);
            self.emit(sh, progress, TransferPhase::Receiving, None);
        };
        self.transport.fetch(&self.peer, &req, &on_bytes, &self.abort).await?;

        let actual = tokio::fs::metadata(&part).await?.len();
        if actual != f.size {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(Error::Integrity {
                expected: format!("{} octets", f.size),
                actual: format!("{actual} octets"),
            });
        }
        self.emit(sh, progress, TransferPhase::Verifying, None);
        let (vp, vs, v1) = (part.clone(), f.sha256.clone(), f.sha1.clone());
        let v = tokio::task::spawn_blocking(move || {
            verify_file(&vp, &vs).and_then(|()| super::recv::verify_claimed_sha1(&vp, v1.as_deref()))
        })
            .await
            .map_err(|e| Error::Other(format!("vérification : {e}")))?;
        if let Err(e) = v {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(e);
        }
        let dest = self.store_root.join(&rel);
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::rename(&part, &dest).await?;
        self.index.insert(&f.sha256, &rel, f.size)?;
        {
            let mut j = lock(&self.journal);
            j.partials.retain(|p| p.sha256 != f.sha256);
            j.done.push(DoneEntry { sha256: f.sha256.clone(), size: f.size });
        }
        self.save_journal()?;
        credit(f.size);
        lock(&sh.high_water).remove(&f.sha256);
        sh.files.fetch_add(1, Ordering::Relaxed);
        self.emit(sh, progress, TransferPhase::Receiving, None);
        Ok(f.size)
    }

    /// Après une tentative interrompue : consigne chaque partiel (longueur réelle + hash du préfixe).
    async fn checkpoint(&self, remaining: &[ManifestFile]) {
        for f in remaining {
            let path = part_path(&self.store_root, &f.sha256);
            let len = tokio::fs::metadata(&path).await.map(|m| m.len()).unwrap_or(0);
            let hash = if len > 0 && len < f.size {
                let p = path.clone();
                tokio::task::spawn_blocking(move || prefix_hash(&p, len)).await.ok().and_then(|r| r.ok())
            } else {
                None
            };
            if len >= f.size {
                let _ = tokio::fs::remove_file(&path).await;
            }
            let mut j = lock(&self.journal);
            j.partials.retain(|p| p.sha256 != f.sha256);
            if let Some(h) = hash {
                j.partials.push(Partial { sha256: f.sha256.clone(), offset: len, prefix_sha256: h });
            }
        }
        let _ = self.save_journal();
    }

    fn cleanup_all(&self) {
        for f in &self.plan.to_receive {
            let _ = std::fs::remove_file(part_path(&self.store_root, &f.sha256));
        }
        let _ = std::fs::remove_file(journal_path(&self.store_root, &self.id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transfer::send::Host;
    use sha2::{Digest, Sha256};
    use std::collections::{HashMap, VecDeque};
    use std::sync::atomic::AtomicUsize;

    fn sha(d: &[u8]) -> String {
        hex::encode(Sha256::digest(d))
    }

    /// Transport de test : sert des contenus en mémoire, honore `resume_from`, coupe après N octets
    /// (une coupure par appel, dans l'ordre) et compte les octets réellement transférés.
    #[derive(Default)]
    struct Flaky {
        data: HashMap<String, Vec<u8>>,
        cuts: Mutex<VecDeque<u64>>,
        sent: AtomicU64,
        calls: AtomicUsize,
        /// Imite iroh : ignore `resume_from`, retransmet le fichier entier (reprise intra-blob impossible).
        no_resume: bool,
    }

    impl BlobTransport for Flaky {
        async fn serve(&self, _host: Arc<Host>) -> Result<()> {
            Ok(())
        }
        async fn fetch(
            &self,
            _peer: &str,
            req: &FetchRequest,
            on_bytes: &(dyn Fn(u64) + Send + Sync),
            cancel: &AtomicBool,
        ) -> Result<()> {
            use tokio::io::AsyncWriteExt;
            self.calls.fetch_add(1, Ordering::SeqCst);
            let data = self.data.get(&req.sha256).ok_or_else(|| Error::NotFound(req.sha256.clone()))?;
            let mut budget = self.cuts.lock().unwrap().pop_front().unwrap_or(u64::MAX);
            let resume_from = if self.no_resume { 0 } else { req.resume_from };
            let mut out = if resume_from > 0 {
                tokio::fs::OpenOptions::new().append(true).open(&req.dest).await?
            } else {
                tokio::fs::File::create(&req.dest).await?
            };
            let mut pos = resume_from as usize;
            while pos < data.len() {
                if cancel.load(Ordering::Relaxed) {
                    return Err(Error::Cancelled);
                }
                if budget == 0 {
                    out.flush().await?;
                    return Err(Error::Network("coupure".into()));
                }
                let n = 250.min(data.len() - pos).min(budget.min(usize::MAX as u64) as usize);
                out.write_all(&data[pos..pos + n]).await?;
                pos += n;
                budget -= n as u64;
                self.sent.fetch_add(n as u64, Ordering::SeqCst);
                on_bytes(n as u64);
                tokio::task::yield_now().await;
            }
            out.flush().await?;
            Ok(())
        }
    }

    struct Fx {
        store: tempfile::TempDir,
        index: Arc<StoreIndex>,
        plan: Plan,
        contents: Vec<Vec<u8>>,
    }

    fn fx() -> Fx {
        let store = tempfile::tempdir().unwrap();
        let index = Arc::new(StoreIndex::open(store.path().join("index.db")).unwrap());
        let mut contents = Vec::new();
        let mut to_receive = Vec::new();
        for (i, len) in [6000usize, 3000, 2000].iter().enumerate() {
            let d: Vec<u8> = (0..*len).map(|j| (j * 7 + i * 13) as u8).collect();
            to_receive.push(ManifestFile { path: format!("mods/m{i}.jar"), size: d.len() as u64, sha256: sha(&d), sha1: None });
            contents.push(d);
        }
        let bytes = to_receive.iter().map(|f| f.size).sum();
        let plan = Plan { to_receive_count: 3, to_receive_bytes: bytes, total_bytes: bytes, to_receive, ..Plan::default() };
        Fx { store, index, plan, contents }
    }

    fn transport(fx: &Fx, cuts: &[u64]) -> Arc<Flaky> {
        let data = fx.plan.to_receive.iter().zip(&fx.contents).map(|(f, d)| (f.sha256.clone(), d.clone())).collect();
        Arc::new(Flaky { data, cuts: Mutex::new(cuts.iter().copied().collect()), ..Flaky::default() })
    }

    fn instant_cfg(max_retries: u32, log: Arc<Mutex<Vec<Duration>>>) -> ResumeConfig {
        ResumeConfig {
            opts: RecvOptions { concurrency: 1, ..RecvOptions::default() },
            retry: RetryPolicy {
                max_retries,
                base: Duration::from_secs(1),
                cap: Duration::from_secs(5),
                jitter: 0.0,
            },
            orphan_max_age: DEFAULT_ORPHAN_MAX_AGE,
            sleeper: Arc::new(move |d| {
                log.lock().unwrap().push(d);
                Box::pin(async {})
            }),
        }
    }

    fn quiet(_: TransferProgress) {}

    fn mk(fx: &Fx, id: &str, t: &Arc<Flaky>, cfg: ResumeConfig, reuse: bool) -> Result<ResumableReceive<Flaky>> {
        let f = if reuse { ResumableReceive::recover } else { ResumableReceive::start };
        f(id, fx.plan.clone(), "hote", t.clone(), fx.store.path(), fx.index.clone(), cfg)
    }

    fn check_installed(fx: &Fx) {
        for (f, d) in fx.plan.to_receive.iter().zip(&fx.contents) {
            let e = fx.index.get(&f.sha256).unwrap().unwrap();
            assert_eq!(&std::fs::read(fx.store.path().join(e.rel_path)).unwrap(), d);
        }
    }

    #[tokio::test]
    async fn cut_then_auto_resume_transfers_only_the_rest() {
        let fx = fx();
        let t = transport(&fx, &[4000]); // coupe au milieu du 1er fichier
        let log = Arc::new(Mutex::new(vec![]));
        let rc = mk(&fx, "t1", &t, instant_cfg(3, log.clone()), false).unwrap();
        let s = rc.run(&quiet).await.unwrap();
        assert_eq!(s.files, 3);
        assert_eq!(t.sent.load(Ordering::SeqCst), fx.plan.to_receive_bytes, "aucun octet transféré deux fois");
        assert_eq!(log.lock().unwrap().len(), 1);
        check_installed(&fx);
        assert!(!journal_path(fx.store.path(), "t1").exists());
        assert_eq!(rc.state(), RunState::Done);
    }

    /// Non-régression (bogue « le compteur retombe, files_done=0, tout est retransféré ») : avec un transport qui
    /// ne reprend pas à l'intérieur d'un fichier, les fichiers COMPLETS ne sont jamais retransférés et ni les
    /// octets ni les fichiers comptés ne reculent.
    #[tokio::test]
    async fn no_intra_file_resume_keeps_counters_monotone_and_done_files() {
        let fx = fx();
        let mut t = Arc::try_unwrap(transport(&fx, &[u64::MAX, 1500, 800])).ok().unwrap();
        t.no_resume = true; // fichier 0 entier, fichier 1 coupé à 1500 puis fichier 1 recoupé à 800
        let t = Arc::new(t);
        let log = Arc::new(Mutex::new(vec![]));
        let seen = Mutex::new(Vec::<(u64, u32)>::new());
        let rec = |p: TransferProgress| seen.lock().unwrap().push((p.bytes_done, p.files_done));
        let rc = mk(&fx, "nr", &t, instant_cfg(5, log), false).unwrap();
        rc.run(&rec).await.unwrap();
        let seen = seen.lock().unwrap();
        let (mut mb, mut mf) = (0, 0);
        for &(b, f) in seen.iter() {
            assert!(b >= mb && f >= mf, "compteur en recul : ({b},{f}) après ({mb},{mf})");
            (mb, mf) = (b, f);
        }
        assert_eq!((mb, mf), (fx.plan.to_receive_bytes, 3));
        // fichier 0 (6000 o) transféré une seule fois ; seuls les fichiers coupés sont repris depuis zéro
        let f1 = 3000u64;
        assert_eq!(t.sent.load(Ordering::SeqCst), 6000 + 1500 + 800 + f1 + 2000);
        check_installed(&fx);
    }

    /// Nouvel essai (journal écarté, cache conservé) : les fichiers déjà vérifiés sont comptés dès le départ.
    #[tokio::test]
    async fn fresh_run_counts_cached_files_from_the_start() {
        let fx = fx();
        let t = transport(&fx, &[u64::MAX, 100]);
        let r = mk(&fx, "c1", &t, instant_cfg(0, Arc::default()), false).unwrap().run(&quiet).await;
        assert!(matches!(r, Err(Error::Network(_))));
        let t2 = transport(&fx, &[]);
        let first = Mutex::new(None::<(u64, u32)>);
        let rec = |p: TransferProgress| {
            first.lock().unwrap().get_or_insert((p.bytes_done, p.files_done));
        };
        mk(&fx, "c1", &t2, instant_cfg(0, Arc::default()), false).unwrap().run(&rec).await.unwrap();
        assert_eq!(first.lock().unwrap().unwrap(), (6000, 1), "fichier 0 déjà en cache");
        assert_eq!(t2.sent.load(Ordering::SeqCst), 3000 + 2000);
    }

    #[tokio::test]
    async fn restart_of_app_resumes_from_journal() {
        let fx = fx();
        let t = transport(&fx, &[4000]);
        let log = Arc::new(Mutex::new(vec![]));
        let r = mk(&fx, "t2", &t, instant_cfg(0, log.clone()), false).unwrap().run(&quiet).await;
        assert!(matches!(r, Err(Error::Network(_))));
        assert!(journal_path(fx.store.path(), "t2").exists());
        // « redémarrage » : nouvel objet, nouveau transport
        let t2 = transport(&fx, &[]);
        let rc = mk(&fx, "t2", &t2, instant_cfg(0, log), true).unwrap();
        rc.run(&quiet).await.unwrap();
        assert_eq!(t.sent.load(Ordering::SeqCst) + t2.sent.load(Ordering::SeqCst), fx.plan.to_receive_bytes);
        assert!(t2.sent.load(Ordering::SeqCst) < fx.plan.to_receive_bytes);
        check_installed(&fx);
    }

    #[tokio::test]
    async fn corrupted_journal_is_ignored_and_cache_is_used() {
        let fx = fx();
        let t = transport(&fx, &[4000]);
        let log = Arc::new(Mutex::new(vec![]));
        let _ = mk(&fx, "t3", &t, instant_cfg(0, log.clone()), false).unwrap().run(&quiet).await;
        std::fs::write(journal_path(fx.store.path(), "t3"), b"{ pas du json").unwrap();
        let t2 = transport(&fx, &[]);
        mk(&fx, "t3", &t2, instant_cfg(0, log.clone()), true).unwrap().run(&quiet).await.unwrap();
        // le partiel n est plus vérifiable : tout est redemandé
        assert_eq!(t2.sent.load(Ordering::SeqCst), 6000 + 3000 + 2000);
        check_installed(&fx);
        // journal d'une autre version : idem
        let fx2 = self::fx();
        let t = transport(&fx2, &[4000]);
        let _ = mk(&fx2, "v", &t, instant_cfg(0, log.clone()), false).unwrap().run(&quiet).await;
        let p = journal_path(fx2.store.path(), "v");
        let txt = std::fs::read_to_string(&p).unwrap().replace("\"version\": 1", "\"version\": 99");
        std::fs::write(&p, txt).unwrap();
        let t2 = transport(&fx2, &[]);
        mk(&fx2, "v", &t2, instant_cfg(0, log), true).unwrap().run(&quiet).await.unwrap();
        assert_eq!(t2.sent.load(Ordering::SeqCst), 11000);
    }

    #[tokio::test]
    async fn altered_partial_is_detected_and_refetched() {
        let fx = fx();
        let t = transport(&fx, &[4000]);
        let log = Arc::new(Mutex::new(vec![]));
        let _ = mk(&fx, "t4", &t, instant_cfg(0, log.clone()), false).unwrap().run(&quiet).await;
        let part = part_path(fx.store.path(), &fx.plan.to_receive[0].sha256);
        let mut b = std::fs::read(&part).unwrap();
        assert_eq!(b.len(), 4000);
        b[10] ^= 0xff;
        std::fs::write(&part, b).unwrap();
        let t2 = transport(&fx, &[]);
        mk(&fx, "t4", &t2, instant_cfg(0, log), true).unwrap().run(&quiet).await.unwrap();
        assert_eq!(t2.sent.load(Ordering::SeqCst), 6000 + 3000 + 2000, "fichier altéré retransféré en entier");
        check_installed(&fx);
    }

    #[tokio::test]
    async fn backoff_is_exponential_capped_and_bounded() {
        let fx = fx();
        let t = transport(&fx, &[0, 0, 0, 0, 0]);
        let log = Arc::new(Mutex::new(vec![]));
        mk(&fx, "t5", &t, instant_cfg(5, log.clone()), false).unwrap().run(&quiet).await.unwrap();
        let secs: Vec<u64> = log.lock().unwrap().iter().map(|d| d.as_secs()).collect();
        assert_eq!(secs, vec![1, 2, 4, 5, 5]);
        // épuisement des tentatives
        let fx = self::fx();
        let t = transport(&fx, &[0, 0, 0, 0]);
        let log = Arc::new(Mutex::new(vec![]));
        let r = mk(&fx, "t5b", &t, instant_cfg(2, log.clone()), false).unwrap().run(&quiet).await;
        assert!(matches!(r, Err(Error::Network(_))));
        assert_eq!(log.lock().unwrap().len(), 2);
        // gigue : bornes
        let p = RetryPolicy { max_retries: 1, base: Duration::from_secs(2), cap: Duration::from_secs(100), jitter: 0.5 };
        assert_eq!(p.delay_for(1, 0.0), Duration::from_secs(2));
        assert_eq!(p.delay_for(1, 0.5), Duration::from_secs(4));
        assert_eq!(p.delay_for(1, 1.0), Duration::from_secs(6));
        assert_eq!(p.delay_for(20, 1.0), Duration::from_secs(100));
    }

    #[tokio::test]
    async fn pause_and_resume() {
        let fx = fx();
        let t = transport(&fx, &[]);
        let log = Arc::new(Mutex::new(vec![]));
        let rc = mk(&fx, "t6", &t, instant_cfg(1, log.clone()), false).unwrap();
        let fired = AtomicBool::new(false);
        let cb = |p: TransferProgress| {
            if p.phase == TransferPhase::Receiving && p.bytes_done > 1000 && !fired.swap(true, Ordering::SeqCst) {
                rc.pause();
            }
        };
        let watcher = async {
            while rc.state() != RunState::Paused {
                tokio::task::yield_now().await;
            }
            let sent = t.sent.load(Ordering::SeqCst);
            assert!(sent < fx.plan.to_receive_bytes);
            tokio::task::yield_now().await;
            assert_eq!(t.sent.load(Ordering::SeqCst), sent, "rien ne circule en pause");
            rc.resume();
        };
        let (r, ()) = tokio::join!(rc.run(&cb), watcher);
        r.unwrap();
        assert_eq!(t.sent.load(Ordering::SeqCst), fx.plan.to_receive_bytes);
        assert!(log.lock().unwrap().is_empty(), "la pause n'est pas une erreur réseau");
        check_installed(&fx);
    }

    #[tokio::test]
    async fn cancel_cleans_temporaries_and_journal() {
        let fx = fx();
        let t = transport(&fx, &[]);
        let log = Arc::new(Mutex::new(vec![]));
        let rc = mk(&fx, "t7", &t, instant_cfg(1, log), false).unwrap();
        let cb = |p: TransferProgress| {
            if p.bytes_done > 7000 {
                rc.cancel();
            }
        };
        let r = rc.run(&cb).await;
        assert!(matches!(r, Err(Error::Cancelled)));
        let parts = std::fs::read_dir(fx.store.path().join(".tmp")).unwrap().count();
        assert_eq!(parts, 0);
        assert!(!journal_path(fx.store.path(), "t7").exists());
        assert_eq!(rc.state(), RunState::Cancelled);
        assert!(!fx.index.contains(&fx.plan.to_receive[2].sha256).unwrap());
    }

    #[tokio::test]
    async fn integrity_error_is_never_retried() {
        let fx = fx();
        let mut t = Arc::try_unwrap(transport(&fx, &[])).ok().unwrap();
        let sha0 = fx.plan.to_receive[0].sha256.clone();
        let mut bad = fx.contents[0].clone();
        bad[5] ^= 1;
        t.data.insert(sha0, bad);
        let t = Arc::new(t);
        let log = Arc::new(Mutex::new(vec![]));
        let r = mk(&fx, "t8", &t, instant_cfg(5, log.clone()), false).unwrap().run(&quiet).await;
        assert!(matches!(r, Err(Error::Integrity { .. })), "{r:?}");
        assert_eq!(t.calls.load(Ordering::SeqCst), 1);
        assert!(log.lock().unwrap().is_empty());
        assert!(!fx.index.contains(&fx.plan.to_receive[0].sha256).unwrap());
    }

    #[tokio::test]
    async fn single_active_transfer_per_id() {
        let fx = fx();
        let t = transport(&fx, &[]);
        let log = Arc::new(Mutex::new(vec![]));
        let a = mk(&fx, "t9", &t, instant_cfg(1, log.clone()), false).unwrap();
        assert!(matches!(mk(&fx, "t9", &t, instant_cfg(1, log.clone()), true), Err(Error::Invalid(_))));
        drop(a);
        assert!(mk(&fx, "t9", &t, instant_cfg(1, log), true).is_ok());
    }

    #[test]
    fn orphan_journals_are_purged_after_n_days() {
        let dir = tempfile::tempdir().unwrap();
        let jd = journal_dir(dir.path());
        std::fs::create_dir_all(&jd).unwrap();
        let old = jd.join("vieux.json");
        let new = jd.join("recent.json");
        std::fs::write(&old, b"{}").unwrap();
        std::fs::write(&new, b"{}").unwrap();
        let f = std::fs::OpenOptions::new().write(true).open(&old).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(30 * 24 * 3600)).unwrap();
        assert_eq!(purge_orphans(dir.path(), Duration::from_secs(7 * 24 * 3600)).unwrap(), 1);
        assert!(!old.exists() && new.exists());
    }

    fn assert_send<T: Send>(_: &T) {}

    /// Le futur de `run` est `Send` (génériquement et pour les transports concrets) : il peut être
    /// lancé par `tokio::spawn` sans `block_in_place`.
    #[allow(dead_code)]
    fn run_future_is_send<T: BlobTransport + 'static>(r: &ResumableReceive<T>) {
        let f = r.run(&|_| {});
        assert_send(&f);
    }

    #[allow(dead_code)]
    fn run_future_is_send_concrete(r: &ResumableReceive<Flaky>, i: &ResumableReceive<crate::transfer::send::IrohTransport>) {
        assert_send(&r.run(&|_| {}));
        assert_send(&i.run(&|_| {}));
    }
}
