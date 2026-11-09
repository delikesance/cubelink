//! Tâche I3 : lancement et suivi du processus Java.
//!
//! * pas de shell : `tokio::process::Command` directement sur `cmd.program` ;
//! * stdout et stderr fusionnés, écrits dans `<log_dir>/latest.log` (taille bornée, rotation
//!   simple vers `latest.log.old`) ; le jeton d'accès n'apparaît jamais (remplacé par `***`) ;
//! * flux d'événements [`LaunchEvent`] (sorties limitées en débit) ;
//! * aucun processus orphelin : si le [`RunningGame`] est abandonné, le jeu est arrêté.
//!
//! Attention : `events()` retire le récepteur du `RunningGame`, qui doit rester vivant
//! tant que le jeu doit tourner (sa destruction arrête le jeu).

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{ExitStatus, Stdio};
use std::time::{Duration, Instant, SystemTime};

use futures_util::stream::{self, Stream};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, watch};

use crate::launch_args::LaunchCommand;
use crate::types::{Error, Result};

const MASK: &str = "***";
/// Places réservées dans le canal pour les événements d'état (jamais perdus).
const RESERVED_SLOTS: usize = 8;
const EVENT_CAPACITY: usize = 1024;
/// Longueur maximale d'une ligne conservée (le reste est tronqué).
const MAX_LINE_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchEvent {
    Started { pid: u32 },
    Output(String),
    Exited { code: i32 },
    Failed { message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LaunchPhase {
    Preparing,
    Running,
    Exited,
    Failed,
}

/// Forme du contrat : `{ id, state, message? }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchStateEvent {
    pub id: String,
    pub state: LaunchPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RunOptions {
    /// Un code de sortie non nul dans ce délai est signalé comme un échec au démarrage.
    pub startup_grace: Duration,
    /// Délai entre la demande de terminaison et l'arrêt forcé.
    pub kill_grace: Duration,
    /// Taille maximale de `latest.log` avant rotation.
    pub max_log_bytes: u64,
    /// Nombre maximal d'événements `Output` par seconde (le journal reçoit tout).
    pub output_lines_per_sec: u32,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            startup_grace: Duration::from_secs(10),
            kill_grace: Duration::from_secs(5),
            max_log_bytes: 2 * 1024 * 1024,
            output_lines_per_sec: 200,
        }
    }
}

pub struct RunningGame {
    pub pid: u32,
    events: Option<mpsc::Receiver<LaunchEvent>>,
    kill_tx: mpsc::Sender<()>,
    done: watch::Receiver<bool>,
}

impl RunningGame {
    /// Récupère le flux d'événements (une seule fois).
    pub fn events(&mut self) -> Option<mpsc::Receiver<LaunchEvent>> {
        self.events.take()
    }

    /// Termine proprement le jeu, puis le tue après `kill_grace`. Rend la main à la fin du processus.
    pub async fn kill(&self) -> Result<()> {
        let _ = self.kill_tx.send(()).await;
        self.wait().await;
        Ok(())
    }

    /// Attend la fin du processus.
    pub async fn wait(&self) {
        let mut done = self.done.clone();
        let _ = done.wait_for(|d| *d).await;
    }
}

/// Lance le jeu avec les options par défaut.
pub async fn spawn(cmd: &LaunchCommand, log_dir: &Path) -> Result<RunningGame> {
    spawn_with(cmd, log_dir, RunOptions::default()).await
}

pub async fn spawn_with(cmd: &LaunchCommand, log_dir: &Path, opts: RunOptions) -> Result<RunningGame> {
    // Les valeurs masquées par `redacted_args` sont les secrets à filtrer.
    let redacted = cmd.redacted_args();
    let secrets = cmd
        .args
        .iter()
        .zip(redacted.iter())
        .filter(|(a, r)| a != r)
        .map(|(a, _)| a.clone())
        .collect();
    start(Plan {
        program: cmd.program.clone(),
        args: cmd.args.clone(),
        shown_args: redacted,
        cwd: cmd.cwd.clone(),
        secrets,
        log_dir: log_dir.to_path_buf(),
        opts,
    })
    .await
}

struct Plan {
    program: PathBuf,
    args: Vec<String>,
    shown_args: Vec<String>,
    cwd: PathBuf,
    secrets: Vec<String>,
    log_dir: PathBuf,
    opts: RunOptions,
}

async fn start(plan: Plan) -> Result<RunningGame> {
    if !plan.cwd.is_dir() {
        return Err(Error::NotFound(format!("dossier de l'instance introuvable : {}", plan.cwd.display())));
    }
    tokio::fs::create_dir_all(&plan.log_dir).await?;
    let log_path = plan.log_dir.join("latest.log");
    let mut sink = Sink::open(log_path, &plan).await?;

    let mut command = Command::new(&plan.program);
    command
        .args(&plan.args)
        .current_dir(&plan.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let started_at = SystemTime::now();
    let mut child = command.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Error::NotFound(format!(
                "Java introuvable : {} (installez Java ou vérifiez le chemin dans les réglages)",
                plan.program.display()
            ))
        } else {
            Error::Io(e)
        }
    })?;
    let pid = child.id().unwrap_or(0);

    let (line_tx, line_rx) = mpsc::channel::<String>(256);
    if let Some(out) = child.stdout.take() {
        tokio::spawn(read_lines(out, line_tx.clone()));
    }
    if let Some(err) = child.stderr.take() {
        tokio::spawn(read_lines(err, line_tx.clone()));
    }
    drop(line_tx);

    let (ev_tx, ev_rx) = mpsc::channel(EVENT_CAPACITY);
    let (kill_tx, kill_rx) = mpsc::channel(1);
    let (done_tx, done_rx) = watch::channel(false);
    let _ = ev_tx.try_send(LaunchEvent::Started { pid });
    sink.header(&plan.program, &plan.shown_args).await;

    let cwd = plan.cwd.clone();
    let opts = plan.opts.clone();
    tokio::spawn(async move {
        supervise(child, pid, line_rx, kill_rx, sink, ev_tx, opts, cwd, started_at).await;
        let _ = done_tx.send(true);
    });

    Ok(RunningGame { pid, events: Some(ev_rx), kill_tx, done: done_rx })
}

async fn read_lines<R: AsyncRead + Unpin>(r: R, tx: mpsc::Sender<String>) {
    let mut reader = BufReader::new(r);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                buf.truncate(MAX_LINE_BYTES);
                let line = String::from_utf8_lossy(&buf).trim_end_matches(['\r', '\n']).to_string();
                if tx.send(line).await.is_err() {
                    return;
                }
            }
        }
    }
}

/// Journal borné + filtre du jeton + limiteur de débit des événements.
struct Sink {
    path: PathBuf,
    file: Option<tokio::fs::File>,
    written: u64,
    max: u64,
    secrets: Vec<String>,
    per_sec: u32,
    window: Instant,
    in_window: u32,
    last_line: Option<String>,
    tx: Option<mpsc::Sender<LaunchEvent>>,
}

impl Sink {
    async fn open(path: PathBuf, plan: &Plan) -> Result<Self> {
        rotate(&path).await;
        let file = tokio::fs::File::create(&path).await?;
        let mut secrets: Vec<String> = plan.secrets.iter().filter(|s| s.len() >= 4).cloned().collect();
        secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
        Ok(Self {
            path,
            file: Some(file),
            written: 0,
            max: plan.opts.max_log_bytes.max(256),
            secrets,
            per_sec: plan.opts.output_lines_per_sec.max(1),
            window: Instant::now(),
            in_window: 0,
            last_line: None,
            tx: None,
        })
    }

    fn redact(&self, line: &str) -> String {
        let mut out = line.to_string();
        for s in &self.secrets {
            if out.contains(s.as_str()) {
                out = out.replace(s.as_str(), MASK);
            }
        }
        out
    }

    async fn header(&mut self, program: &Path, args: &[String]) {
        let line = format!("# {} {}", program.display(), args.join(" "));
        self.write(&line).await;
    }

    async fn write(&mut self, line: &str) {
        let len = line.len() as u64 + 1;
        if self.written + len > self.max {
            self.file = None; // ferme avant de renommer (Windows)
            rotate(&self.path).await;
            self.file = tokio::fs::File::create(&self.path).await.ok();
            self.written = 0;
        }
        if let Some(f) = self.file.as_mut() {
            let mut data = Vec::with_capacity(line.len() + 1);
            data.extend_from_slice(line.as_bytes());
            data.push(b'\n');
            if f.write_all(&data).await.is_ok() {
                let _ = f.flush().await;
                self.written += len;
            }
        }
    }

    async fn push(&mut self, raw: &str) {
        let line = self.redact(raw);
        self.write(&line).await;
        if self.window.elapsed() >= Duration::from_secs(1) {
            self.window = Instant::now();
            self.in_window = 0;
        }
        if self.in_window < self.per_sec {
            self.in_window += 1;
            if let Some(tx) = &self.tx {
                // Les dernières places sont réservées aux événements d'état.
                if tx.capacity() > RESERVED_SLOTS {
                    let _ = tx.try_send(LaunchEvent::Output(line.clone()));
                }
            }
        }
        self.last_line = Some(line);
    }
}

/// `latest.log` -> `latest.log.old` (remplacé).
async fn rotate(path: &Path) {
    if tokio::fs::metadata(path).await.is_ok() {
        let mut old = path.as_os_str().to_owned();
        old.push(".old");
        let old = PathBuf::from(old);
        let _ = tokio::fs::remove_file(&old).await;
        let _ = tokio::fs::rename(path, &old).await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn supervise(
    mut child: Child,
    pid: u32,
    mut lines: mpsc::Receiver<String>,
    mut kill_rx: mpsc::Receiver<()>,
    mut sink: Sink,
    ev_tx: mpsc::Sender<LaunchEvent>,
    opts: RunOptions,
    cwd: PathBuf,
    started_at: SystemTime,
) {
    sink.tx = Some(ev_tx.clone());
    let started = Instant::now();
    let mut killed = false;
    let mut lines_open = true;
    let status: std::io::Result<ExitStatus> = loop {
        tokio::select! {
            line = lines.recv(), if lines_open => match line {
                Some(l) => sink.push(&l).await,
                None => lines_open = false,
            },
            req = kill_rx.recv() => {
                // `Some` : kill() explicite (arrêt propre) ; `None` : RunningGame abandonné.
                killed = true;
                break terminate(&mut child, pid, req.is_some(), opts.kill_grace).await;
            }
            st = child.wait() => break st,
        }
    };
    // Vide ce qui reste dans les tuyaux (sans attendre indéfiniment un petit-enfant).
    while let Ok(Some(l)) = tokio::time::timeout(Duration::from_millis(500), lines.recv()).await {
        sink.push(&l).await;
    }
    let last = sink.last_line.clone();
    sink.tx = None;

    let event = match status {
        Err(e) => LaunchEvent::Failed { message: format!("Impossible de suivre le processus Java : {e}") },
        Ok(st) => {
            let code = st.code().unwrap_or(-1);
            if code == 0 || killed {
                LaunchEvent::Exited { code }
            } else {
                LaunchEvent::Failed { message: crash_message(code, started.elapsed() < opts.startup_grace, &cwd, started_at, last).await }
            }
        }
    };
    let _ = ev_tx.send(event).await;
}

async fn terminate(child: &mut Child, pid: u32, graceful: bool, grace: Duration) -> std::io::Result<ExitStatus> {
    if graceful {
        request_stop(pid).await;
        if let Ok(st) = tokio::time::timeout(grace, child.wait()).await {
            return st;
        }
    }
    force_kill_tree(pid).await;
    let _ = child.start_kill();
    child.wait().await
}

async fn request_stop(pid: u32) {
    let mut c = Command::new(if cfg!(windows) { "taskkill" } else { "kill" });
    if cfg!(windows) {
        c.args(["/PID", &pid.to_string(), "/T"]);
    } else {
        c.args(["-TERM", &pid.to_string()]);
    }
    #[cfg(windows)]
    c.creation_flags(0x0800_0000);
    let _ = c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().await;
}

/// Sous Windows, tue aussi les processus enfants (évite les orphelins).
async fn force_kill_tree(pid: u32) {
    if cfg!(windows) {
        let mut c = Command::new("taskkill");
        c.args(["/PID", &pid.to_string(), "/T", "/F"]);
        #[cfg(windows)]
        c.creation_flags(0x0800_0000);
        let _ = c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().await;
    }
}

async fn recent_crash_report(cwd: &Path, since: SystemTime) -> Option<PathBuf> {
    let since = since.checked_sub(Duration::from_secs(2)).unwrap_or(since);
    let mut rd = tokio::fs::read_dir(cwd.join("crash-reports")).await.ok()?;
    let mut best: Option<(SystemTime, PathBuf)> = None;
    while let Ok(Some(e)) = rd.next_entry().await {
        let Ok(modified) = e.metadata().await.and_then(|m| m.modified()) else { continue };
        if modified >= since && best.as_ref().is_none_or(|(t, _)| modified > *t) {
            best = Some((modified, e.path()));
        }
    }
    best.map(|(_, p)| p)
}

async fn crash_message(code: i32, early: bool, cwd: &Path, since: SystemTime, last: Option<String>) -> String {
    let mut msg = if let Some(report) = recent_crash_report(cwd, since).await {
        format!("Le jeu a planté (code {code}). Rapport de plantage : {}", report.display())
    } else if early {
        format!("Le jeu s'est arrêté dès le démarrage (code {code}). Vérifiez la version de Java et les arguments JVM.")
    } else {
        format!("Le jeu s'est fermé de façon inattendue (code {code}).")
    };
    if let Some(l) = last.filter(|l| !l.trim().is_empty()) {
        msg.push_str(&format!(" Dernier message : {l}"));
    }
    msg
}

/// Traduit les événements en états du contrat. Le flux se termine après `exited` / `failed`.
/// Abandonner le flux abandonne le `RunningGame` et donc arrête le jeu.
pub fn launch_state_stream(id: impl Into<String>, mut game: RunningGame) -> Pin<Box<dyn Stream<Item = LaunchStateEvent> + Send>> {
    let rx = game.events();
    let state = (id.into(), game, rx, false);
    Box::pin(stream::unfold(state, |(id, game, mut rx, mut sent_prep)| async move {
        let ev = |state, message| LaunchStateEvent { id: id.clone(), state, message };
        if !sent_prep {
            sent_prep = true;
            let e = ev(LaunchPhase::Preparing, None);
            return Some((e, (id, game, rx, sent_prep)));
        }
        let rx_ref = rx.as_mut()?;
        loop {
            let out = match rx_ref.recv().await {
                Some(LaunchEvent::Started { .. }) => ev(LaunchPhase::Running, None),
                Some(LaunchEvent::Output(_)) => continue,
                Some(LaunchEvent::Exited { code }) => {
                    let m = (code != 0).then(|| format!("Code de sortie {code}"));
                    // Terminal : plus de récepteur, le flux s'arrête au prochain appel.
                    rx = None;
                    return Some((ev(LaunchPhase::Exited, m), (id, game, rx, sent_prep)));
                }
                Some(LaunchEvent::Failed { message }) => {
                    rx = None;
                    return Some((ev(LaunchPhase::Failed, Some(message)), (id, game, rx, sent_prep)));
                }
                None => return None,
            };
            return Some((out, (id, game, rx, sent_prep)));
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

    fn script(lines: &[&str]) -> (PathBuf, Vec<String>) {
        if cfg!(windows) {
            (PathBuf::from("cmd"), vec!["/C".into(), lines.join(" & ")])
        } else {
            (PathBuf::from("sh"), vec!["-c".into(), lines.join("; ")])
        }
    }

    fn plan(lines: &[&str], dir: &Path, secrets: &[&str], opts: RunOptions) -> Plan {
        let (program, args) = script(lines);
        let cwd = dir.join("game");
        std::fs::create_dir_all(&cwd).unwrap();
        Plan {
            program,
            shown_args: args.iter().map(|a| secrets.iter().fold(a.clone(), |a, s| a.replace(s, MASK))).collect(),
            args,
            cwd,
            secrets: secrets.iter().map(|s| s.to_string()).collect(),
            log_dir: dir.join("logs"),
            opts,
        }
    }

    fn fast() -> RunOptions {
        RunOptions { kill_grace: Duration::from_millis(300), ..RunOptions::default() }
    }

    async fn collect(game: &mut RunningGame) -> Vec<LaunchEvent> {
        let mut rx = game.events().unwrap();
        let mut all = vec![];
        while let Some(e) = rx.recv().await {
            let end = matches!(e, LaunchEvent::Exited { .. } | LaunchEvent::Failed { .. });
            all.push(e);
            if end {
                break;
            }
        }
        all
    }

    #[tokio::test]
    async fn exit_zero_writes_witness_in_cwd() {
        let d = tempfile::tempdir().unwrap();
        let mut g = start(plan(&["echo ok> witness.txt", "echo hello", "exit 0"], d.path(), &[], fast())).await.unwrap();
        let evs = collect(&mut g).await;
        assert!(matches!(evs.first(), Some(LaunchEvent::Started { .. })));
        assert!(evs.iter().any(|e| matches!(e, LaunchEvent::Output(l) if l.trim() == "hello")));
        assert_eq!(evs.last(), Some(&LaunchEvent::Exited { code: 0 }));
        assert!(d.path().join("game/witness.txt").exists());
        let log = std::fs::read_to_string(d.path().join("logs/latest.log")).unwrap();
        assert!(log.contains("hello"));
    }

    #[tokio::test]
    async fn exit_one_reports_failure_with_message() {
        let d = tempfile::tempdir().unwrap();
        let mut g = start(plan(&["echo boom 1>&2", "exit 1"], d.path(), &[], fast())).await.unwrap();
        let evs = collect(&mut g).await;
        match evs.last() {
            Some(LaunchEvent::Failed { message }) => {
                assert!(message.contains("code 1"), "{message}");
                assert!(message.contains("boom"), "{message}");
            }
            other => panic!("attendu Failed, obtenu {other:?}"),
        }
    }

    #[tokio::test]
    async fn crash_report_is_mentioned() {
        let d = tempfile::tempdir().unwrap();
        let p = plan(&["exit 1"], d.path(), &[], fast());
        std::fs::create_dir_all(p.cwd.join("crash-reports")).unwrap();
        std::fs::write(p.cwd.join("crash-reports/crash-test.txt"), "x").unwrap();
        let mut g = start(p).await.unwrap();
        match collect(&mut g).await.last() {
            Some(LaunchEvent::Failed { message }) => {
                assert!(message.contains("a planté") && message.contains("crash-test.txt"), "{message}")
            }
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn missing_program_is_not_found() {
        let d = tempfile::tempdir().unwrap();
        let mut p = plan(&["exit 0"], d.path(), &[], fast());
        p.program = d.path().join("pas-de-java-ici");
        match start(p).await {
            Err(Error::NotFound(m)) => assert!(m.contains("Java introuvable"), "{m}"),
            Ok(_) => panic!("devrait échouer"),
            Err(e) => panic!("{e}"),
        }
    }

    #[tokio::test]
    async fn kill_stops_the_process() {
        let d = tempfile::tempdir().unwrap();
        let sleeper = if cfg!(windows) { "ping -n 30 127.0.0.1 >nul" } else { "exec sleep 30" };
        let mut g = start(plan(&[sleeper], d.path(), &[], fast())).await.unwrap();
        let mut rx = g.events().unwrap();
        let t = Instant::now();
        g.kill().await.unwrap();
        assert!(t.elapsed() < Duration::from_secs(15));
        let mut last = None;
        while let Some(e) = rx.recv().await {
            last = Some(e);
        }
        assert!(matches!(last, Some(LaunchEvent::Exited { .. })), "{last:?}");
    }

    #[tokio::test]
    async fn drop_kills_the_process() {
        let d = tempfile::tempdir().unwrap();
        let sleeper = if cfg!(windows) { "ping -n 30 127.0.0.1 >nul" } else { "exec sleep 30" };
        let mut g = start(plan(&[sleeper], d.path(), &[], fast())).await.unwrap();
        let mut rx = g.events().unwrap();
        drop(g);
        let r = tokio::time::timeout(Duration::from_secs(15), async {
            while let Some(e) = rx.recv().await {
                if matches!(e, LaunchEvent::Exited { .. } | LaunchEvent::Failed { .. }) {
                    return true;
                }
            }
            false
        })
        .await;
        assert_eq!(r, Ok(true));
    }

    #[tokio::test]
    async fn token_is_masked_in_log_and_events() {
        let d = tempfile::tempdir().unwrap();
        let mut g = start(plan(&["echo tok=SECRET-TOKEN-123", "echo SECRET-TOKEN-123 1>&2", "exit 0"], d.path(), &["SECRET-TOKEN-123"], fast()))
            .await
            .unwrap();
        let evs = collect(&mut g).await;
        let log = std::fs::read_to_string(d.path().join("logs/latest.log")).unwrap();
        assert!(!log.contains("SECRET-TOKEN-123"), "{log}");
        assert!(log.contains("tok=***"));
        for e in evs {
            if let LaunchEvent::Output(l) = e {
                assert!(!l.contains("SECRET-TOKEN-123"));
            }
        }
    }

    #[tokio::test]
    async fn log_size_is_bounded_with_rotation() {
        let d = tempfile::tempdir().unwrap();
        let echoes: Vec<String> = (0..60).map(|i| format!("echo ligne-numero-{i:03}-remplissage-remplissage")).collect();
        let mut lines: Vec<&str> = echoes.iter().map(String::as_str).collect();
        lines.push("exit 0");
        let opts = RunOptions { max_log_bytes: 512, ..fast() };
        let mut g = start(plan(&lines, d.path(), &[], opts)).await.unwrap();
        collect(&mut g).await;
        let log = d.path().join("logs/latest.log");
        let size = std::fs::metadata(&log).unwrap().len();
        assert!(size <= 512, "{size}");
        assert!(d.path().join("logs/latest.log.old").exists());
        assert!(std::fs::read_to_string(&log).unwrap().contains("ligne-numero-059"));
    }

    #[tokio::test]
    async fn state_stream_follows_contract() {
        let d = tempfile::tempdir().unwrap();
        let g = start(plan(&["echo hi", "exit 0"], d.path(), &[], fast())).await.unwrap();
        let states: Vec<LaunchStateEvent> = launch_state_stream("inst-1", g).collect().await;
        let kinds: Vec<LaunchPhase> = states.iter().map(|s| s.state).collect();
        assert_eq!(kinds, vec![LaunchPhase::Preparing, LaunchPhase::Running, LaunchPhase::Exited]);
        assert!(states.iter().all(|s| s.id == "inst-1"));
        let json = serde_json::to_value(&states[0]).unwrap();
        assert_eq!(json, serde_json::json!({"id": "inst-1", "state": "preparing"}));
    }

    #[tokio::test]
    async fn state_stream_reports_failure() {
        let d = tempfile::tempdir().unwrap();
        let g = start(plan(&["exit 3"], d.path(), &[], fast())).await.unwrap();
        let states: Vec<LaunchStateEvent> = launch_state_stream("x", g).collect().await;
        let last = states.last().unwrap();
        assert_eq!(last.state, LaunchPhase::Failed);
        assert!(last.message.as_deref().unwrap().contains("code 3"));
    }
}
