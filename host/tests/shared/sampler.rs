//! Echantillonneur commun aux scenarios Win32 et X11 : la boucle (fil a part, un echantillon toutes les ~0,5 ms, arret, bilan) ;
//! ce qu'un echantillon lit et juge fautif est fourni par chaque plateforme.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct Sampler {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<(u64, Vec<String>)>>,
}

impl Sampler {
    /// Lance le fil. `init` s'execute SUR ce fil (une connexion X11 y est propre) et rend la fonction d'echantillonnage : elle recoit
    /// l'instant de depart et la liste des apparitions fautives a completer.
    pub fn spawn<S: FnMut(Instant, &mut Vec<String>)>(init: impl FnOnce() -> S + Send + 'static) -> Sampler {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::spawn(move || {
            let mut sample = init();
            let t0 = Instant::now();
            let (mut samples, mut seen) = (0u64, Vec::new());
            while !flag.load(Ordering::Relaxed) {
                sample(t0, &mut seen);
                samples += 1;
                std::thread::sleep(Duration::from_micros(500));
            }
            (samples, seen)
        });
        Sampler { stop, thread: Some(thread) }
    }

    /// Arrete l'echantillonnage : (nombre d'echantillons, apparitions fautives).
    pub fn stop(mut self) -> (u64, Vec<String>) {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().map(|t| t.join().expect("fil d'echantillonnage")).unwrap_or_default()
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
