//! Journal permanent de la Scene (`logs/stage.log`) : ce que dit l'hote (adoption, repli, etapes cles) et ce que decide le
//! launcher. C'est la seule trace disponible quand un joueur signale « le jeu ne s'affiche pas » : elle est donc toujours
//! ecrite, pas seulement en mode diagnostic.
//!
//! L'ecriture passe par un fil dedie (`Outbox`) : les appelants sont le fil de l'interface et les fils de l'hote, qui ne
//! doivent jamais attendre le disque ni tenir un verrou pendant une ecriture.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TryRecvError, TrySendError};
use std::time::{SystemTime, UNIX_EPOCH};

pub const FILE_NAME: &str = "stage.log";
/// Au-dela, le fichier devient `stage.log.1` (un seul ancien fichier garde : au plus ~2 Mo sur le disque).
pub const MAX_BYTES: u64 = 1024 * 1024;
/// Une ligne plus longue est coupee (une sortie anormale ne doit pas remplir le journal a elle seule).
const MAX_LINE_CHARS: usize = 2000;
/// File d'attente bornee : si le disque ne suit plus, on perd des lignes plutot que de la memoire.
const QUEUE: usize = 512;

/// Pur : une ligne du journal, horodatee en UTC (`2026-11-12 14:03:22.123Z [source] message`). Le message tient toujours
/// sur une ligne (caracteres de controle remplaces par des espaces).
pub fn format_line(unix_ms: u64, source: &str, msg: &str) -> String {
    let secs = (unix_ms / 1000) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Algorithme de Howard Hinnant (jours depuis 1970 -> date civile).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    let clean: String = msg.chars().take(MAX_LINE_CHARS).map(|c| if c.is_control() { ' ' } else { c }).collect();
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}.{:03}Z [{source}] {}\n",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        unix_ms % 1000,
        clean.trim_end()
    )
}

/// Pur : faut-il tourner le fichier avant d'y ajouter `incoming` octets ? Un fichier vide n'est jamais tourne (sinon un seul
/// bloc plus gros que la limite tournerait a l'infini).
pub fn needs_rotation(current_len: u64, incoming: u64, max: u64) -> bool {
    current_len > 0 && current_len.saturating_add(incoming) > max
}

/// Pur : nom de l'ancien fichier (`stage.log` -> `stage.log.1`).
pub fn rotated_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".1");
    path.with_file_name(name)
}

/// Ajoute `text` au journal, en le tournant d'abord s'il depasserait `max`.
pub fn append(path: &Path, text: &str, max: u64) -> std::io::Result<()> {
    let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if needs_rotation(len, text.len() as u64, max) {
        let old = rotated_path(path);
        // Windows : `rename` refuse d'ecraser, l'ancien `.1` est donc retire d'abord.
        let _ = std::fs::remove_file(&old);
        std::fs::rename(path, &old)?;
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::OpenOptions::new().create(true).append(true).open(path)?.write_all(text.as_bytes())
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

type Task = Box<dyn FnOnce() + Send>;

enum Job {
    Line(String),
    Run(Task),
}

/// Fil de sortie de la Scene : ecrit le journal et execute les petites taches differees (evenements envoyes a l'interface
/// depuis un fil de l'hote), dans l'ordre ou elles ont ete postees.
pub struct Outbox {
    tx: SyncSender<Job>,
    #[cfg(test)]
    worker: std::thread::JoinHandle<()>,
}

impl Outbox {
    pub fn start(path: PathBuf) -> std::io::Result<Outbox> {
        let (tx, rx) = sync_channel::<Job>(QUEUE);
        let worker = std::thread::Builder::new().name("cubelink-stage-log".into()).spawn(move || work(&rx, &path))?;
        #[cfg(not(test))]
        drop(worker);
        Ok(Outbox {
            tx,
            #[cfg(test)]
            worker,
        })
    }

    /// Poste une ligne horodatee maintenant (l'heure est celle de l'evenement, pas celle de l'ecriture). Ne bloque jamais :
    /// file pleine, la ligne est perdue.
    pub fn line(&self, source: &str, msg: &str) {
        let _ = self.tx.try_send(Job::Line(format_line(now_ms(), source, msg)));
    }

    /// Execute `task` sur le fil de sortie. Si la file est pleine ou le fil arrete, la tache est executee tout de suite par
    /// l'appelant : elle n'est jamais perdue.
    pub fn run(&self, task: Task) {
        match self.tx.try_send(Job::Run(task)) {
            Ok(()) | Err(TrySendError::Full(Job::Line(_))) | Err(TrySendError::Disconnected(Job::Line(_))) => {}
            Err(TrySendError::Full(Job::Run(task))) | Err(TrySendError::Disconnected(Job::Run(task))) => task(),
        }
    }

    /// Attend (au plus `max`) que tout ce qui a ete poste soit ecrit : avant de quitter le processus, pour que les dernieres
    /// lignes, celles qui expliquent la sortie, ne restent pas dans la file.
    pub fn flush(&self, max: std::time::Duration) {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        // La file execute les taches dans l'ordre, apres avoir ecrit les lignes qui les precedent.
        if self.tx.try_send(Job::Run(Box::new(move || {
            let _ = tx.send(());
        }))).is_ok() {
            let _ = rx.recv_timeout(max);
        }
    }

    /// Test : attend que tout ce qui a ete poste soit ecrit et execute.
    #[cfg(test)]
    pub fn finish(self) {
        drop(self.tx);
        let _ = self.worker.join();
    }
}

fn work(rx: &Receiver<Job>, path: &Path) {
    let mut pending = String::new();
    let mut warned = false;
    let mut flush = |pending: &mut String| {
        if pending.is_empty() {
            return;
        }
        if let Err(e) = append(path, pending, MAX_BYTES) {
            // Une seule fois : un dossier en lecture seule ne doit pas inonder la sortie d'erreur.
            if !warned {
                warned = true;
                eprintln!("[Cubelink] journal de la zone de jeu ({}) : {e}", path.display());
            }
        }
        pending.clear();
    };
    loop {
        // Les lignes deja en attente sont ecrites en une seule ouverture du fichier.
        let job = match rx.try_recv() {
            Ok(j) => j,
            Err(TryRecvError::Disconnected) => break,
            Err(TryRecvError::Empty) => {
                flush(&mut pending);
                match rx.recv() {
                    Ok(j) => j,
                    Err(_) => break,
                }
            }
        };
        match job {
            Job::Line(l) => pending.push_str(&l),
            Job::Run(task) => {
                flush(&mut pending);
                task();
            }
        }
    }
    flush(&mut pending);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ligne_horodatee_sur_une_seule_ligne() {
        assert_eq!(format_line(0, "launcher", "debut"), "1970-01-01 00:00:00.000Z [launcher] debut\n");
        // 2026-11-12 14:03:22.123 UTC
        assert_eq!(format_line(1_794_492_202_123, "hote", "fenetre adoptee"), "2026-11-12 14:03:22.123Z [hote] fenetre adoptee\n");
        // annee bissextile : 2024-02-29 23:59:59.999 UTC
        assert_eq!(format_line(1_709_251_199_999, "hote", "x"), "2024-02-29 23:59:59.999Z [hote] x\n");
        let l = format_line(0, "hote", "a\r\nb\tc\n");
        assert_eq!(l, "1970-01-01 00:00:00.000Z [hote] a  b c\n", "jamais de saut de ligne dans le message");
        let long = format_line(0, "hote", &"x".repeat(10_000));
        assert!(long.chars().count() < MAX_LINE_CHARS + 60 && long.ends_with('\n'));
    }

    #[test]
    fn decision_de_rotation() {
        assert!(!needs_rotation(0, 10, 100));
        assert!(!needs_rotation(90, 10, 100), "pile a la limite : on ecrit encore");
        assert!(needs_rotation(91, 10, 100));
        assert!(!needs_rotation(0, 5000, 100), "un fichier vide n'est jamais tourne");
        assert!(needs_rotation(u64::MAX, 1, 100));
        assert_eq!(rotated_path(Path::new("logs/stage.log")), Path::new("logs/stage.log.1"));
    }

    #[test]
    fn rotation_simple_un_seul_ancien_fichier() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("logs").join(FILE_NAME);
        append(&p, "aaaa\n", 12).unwrap();
        append(&p, "bbbb\n", 12).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "aaaa\nbbbb\n");
        assert!(!rotated_path(&p).exists());
        append(&p, "cccc\n", 12).unwrap();
        assert_eq!(std::fs::read_to_string(rotated_path(&p)).unwrap(), "aaaa\nbbbb\n");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "cccc\n");
        // deuxieme rotation : l'ancien `.1` est remplace, la taille totale reste bornee
        append(&p, "dddddddd\n", 12).unwrap();
        assert_eq!(std::fs::read_to_string(rotated_path(&p)).unwrap(), "cccc\n");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "dddddddd\n");
        assert_eq!(std::fs::read_dir(p.parent().unwrap()).unwrap().count(), 2);
    }

    #[test]
    fn fil_de_sortie_ecrit_dans_l_ordre_et_execute_les_taches() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(FILE_NAME);
        let out = Outbox::start(p.clone()).unwrap();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        out.line("launcher", "un");
        let (s, p2) = (seen.clone(), p.clone());
        // la tache voit les lignes postees avant elle
        out.run(Box::new(move || *s.lock().unwrap() = std::fs::read_to_string(&p2).unwrap_or_default()));
        out.line("hote", "deux");
        out.finish();
        assert!(seen.lock().unwrap().ends_with("[launcher] un\n"));
        let all = std::fs::read_to_string(&p).unwrap();
        let lines: Vec<&str> = all.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with("[launcher] un") && lines[1].ends_with("[hote] deux"), "{all}");
    }
}
