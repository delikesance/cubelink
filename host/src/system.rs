//! Préférences du système qui règlent la Scène : mouvement réduit (rideau sans animation).

/// Preference systeme « afficher les animations dans Windows » desactivee : le rideau ne doit pas s'animer. Illisible :
/// animations gardees (comportement par defaut de Windows).
#[cfg(windows)]
pub fn reduced_motion() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION};
    let mut animations: i32 = 1;
    // SAFETY : `SPI_GETCLIENTAREAANIMATION` ecrit un BOOL (i32) a l'adresse fournie, valide pendant l'appel.
    let ok = unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&mut animations as *mut i32).cast(), 0) };
    ok != 0 && animations == 0
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn reduced_motion() -> bool {
    false
}

/// Linux : reglage GNOME « animations » desactive (`gsettings get org.gnome.desktop.interface enable-animations` rend
/// `false`). Illisible (pas de gsettings, autre bureau, D-Bus lent) : animations gardees.
///
/// Ne bloque jamais plus de [`WAIT`] : `gsettings` tourne dans un fil, et si D-Bus ou dconf tarde, l'appelant (fil principal
/// de `setup`, lancement) recoit la derniere valeur connue pendant que la lecture se termine en arriere-plan.
#[cfg(target_os = "linux")]
pub fn reduced_motion() -> bool {
    static PROBE: std::sync::OnceLock<std::sync::Arc<linux::Probe>> = std::sync::OnceLock::new();
    PROBE
        .get_or_init(|| std::sync::Arc::new(linux::Probe::default()))
        .read("gsettings", linux::WAIT, linux::HARD_DEADLINE)
}

#[cfg(target_os = "linux")]
mod linux {
    use super::animations_off;
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
    use std::sync::{mpsc, Arc};
    use std::time::{Duration, Instant};

    /// Attente maximale de l'appelant.
    pub const WAIT: Duration = Duration::from_millis(200);
    /// Duree de vie maximale du fil de lecture : au-dela, `gsettings` est tue.
    pub const HARD_DEADLINE: Duration = Duration::from_secs(3);

    // Le cache vaut 0 tant que rien n'a ete lu.
    const ANIMATED: u8 = 1;
    const CALM: u8 = 2;

    /// Derniere valeur lue et lecture en cours (une seule a la fois, pour ne pas empiler de processus).
    #[derive(Default)]
    pub struct Probe {
        cache: AtomicU8,
        inflight: AtomicBool,
    }

    impl Probe {
        fn cached(&self) -> bool {
            self.cache.load(Ordering::SeqCst) == CALM
        }

        /// Lit la preference avec `program`, en attendant au plus `wait`. Delai depasse ou lecture deja en cours : valeur
        /// en cache (inconnue : `false`, animations gardees).
        pub fn read(self: &Arc<Self>, program: &str, wait: Duration, hard_deadline: Duration) -> bool {
            if self.inflight.swap(true, Ordering::SeqCst) {
                return self.cached();
            }
            let (tx, rx) = mpsc::channel();
            let probe = Arc::clone(self);
            let program = program.to_owned();
            let spawned = std::thread::Builder::new().name("cubelink-reduced-motion".into()).spawn(move || {
                let calm = query(&program, hard_deadline);
                if let Some(calm) = calm {
                    probe.cache.store(if calm { CALM } else { ANIMATED }, Ordering::SeqCst);
                }
                probe.inflight.store(false, Ordering::SeqCst);
                let _ = tx.send(calm);
            });
            if spawned.is_err() {
                self.inflight.store(false, Ordering::SeqCst);
                return self.cached();
            }
            match rx.recv_timeout(wait) {
                Ok(Some(calm)) => calm,
                _ => self.cached(),
            }
        }
    }

    /// `Some(animations desactivees)`, `None` si illisible. Tue `program` au bout de `hard_deadline` (pas de processus orphelin).
    fn query(program: &str, hard_deadline: Duration) -> Option<bool> {
        let mut child = Command::new(program)
            .args(["get", "org.gnome.desktop.interface", "enable-animations"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let start = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if start.elapsed() < hard_deadline => std::thread::sleep(Duration::from_millis(10)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
            }
        };
        // La sortie tient dans le tampon du tube : lue apres la fin du processus, sans risque de blocage.
        let mut out = String::new();
        child.stdout.take()?.read_to_string(&mut out).ok()?;
        status.success().then(|| animations_off(&out))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        /// Faux `gsettings` : un script sh qui execute `body`.
        fn fake(dir: &std::path::Path, name: &str, body: &str) -> String {
            let path = dir.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path.to_string_lossy().into_owned()
        }

        fn tmp(name: &str) -> std::path::PathBuf {
            let d = std::env::temp_dir().join(format!("cubelink-sys-{name}-{}", std::process::id()));
            std::fs::create_dir_all(&d).unwrap();
            d
        }

        #[test]
        fn gsettings_lent_ne_bloque_pas_puis_le_cache_se_remplit() {
            let dir = tmp("lent");
            let prog = fake(&dir, "gsettings", "sleep 1; echo false");
            let probe = Arc::new(Probe::default());
            let t = Instant::now();
            assert!(!probe.read(&prog, Duration::from_millis(200), Duration::from_secs(5)));
            assert!(t.elapsed() < Duration::from_millis(300), "bloque {:?}", t.elapsed());
            // Une lecture est en cours : l'appel suivant rend le cache sans attendre ni lancer un second processus.
            let t = Instant::now();
            assert!(!probe.read(&prog, Duration::from_millis(200), Duration::from_secs(5)));
            assert!(t.elapsed() < Duration::from_millis(50));
            let start = Instant::now();
            while probe.inflight.load(Ordering::SeqCst) && start.elapsed() < Duration::from_secs(5) {
                std::thread::sleep(Duration::from_millis(20));
            }
            assert!(probe.cached(), "le cache doit refleter la lecture terminee en arriere-plan");
            assert!(probe.read(&prog, Duration::from_millis(5), Duration::from_secs(5)));
            let _ = std::fs::remove_dir_all(dir);
        }

        #[test]
        fn lecture_rapide_et_cas_illisibles() {
            let dir = tmp("rapide");
            let off = fake(&dir, "off", "echo false");
            let on = fake(&dir, "on", "echo true");
            let ko = fake(&dir, "ko", "exit 1");
            let wait = Duration::from_secs(2);
            assert!(Arc::new(Probe::default()).read(&off, wait, wait));
            assert!(!Arc::new(Probe::default()).read(&on, wait, wait));
            assert!(!Arc::new(Probe::default()).read(&ko, wait, wait));
            assert!(!Arc::new(Probe::default()).read("/nonexistent/gsettings", wait, wait));
            let _ = std::fs::remove_dir_all(dir);
        }

        #[test]
        fn gsettings_bloque_est_tue_apres_le_delai_maximal() {
            let dir = tmp("tue");
            let marker = dir.join("pid");
            let prog = fake(&dir, "gsettings", &format!("echo $$ > {}; exec sleep 30", marker.display()));
            let t = Instant::now();
            assert_eq!(query(&prog, Duration::from_millis(300)), None);
            assert!(t.elapsed() < Duration::from_secs(2));
            let pid: i32 = std::fs::read_to_string(&marker).unwrap().trim().parse().unwrap();
            assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists(), "le processus doit etre tue et recolte");
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// Pur : sortie de `gsettings get ... enable-animations` -> animations desactivees ?
#[cfg(any(target_os = "linux", test))]
fn animations_off(gsettings_out: &str) -> bool {
    gsettings_out.trim() == "false"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn animations_gnome() {
        assert!(animations_off("false\n"));
        assert!(!animations_off("true\n"));
        assert!(!animations_off(""));
    }
}
