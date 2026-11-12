//! Diagnostic fin (`CUBELINK_STAGE_DEBUG`) : journal horodate sur la sortie d'erreur, mesures du minuteur et du rendu. Les etapes
//! cles, elles, sont TOUJOURS journalisees par l'observateur (`Shared::log`, `HostEvent::Log`).

use std::sync::OnceLock;

pub(super) fn dbg_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("CUBELINK_STAGE_DEBUG").is_some())
}

pub(super) fn dbg(m: &str) {
    if dbg_on() {
        static T0: OnceLock<std::time::Instant> = OnceLock::new();
        eprintln!("[Cubelink/zone] +{:>6} ms {m}", T0.get_or_init(std::time::Instant::now).elapsed().as_millis());
    }
}

/// Mesures : reveils du minuteur et rendu du rideau, journalises toutes les 5 s.
pub(super) mod stats {
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    use std::sync::Mutex;
    use std::time::Instant;
    pub static TICKS: AtomicU64 = AtomicU64::new(0);
    pub static FRAMES: AtomicU64 = AtomicU64::new(0);
    pub static RENDER_US: AtomicU64 = AtomicU64::new(0);
    pub static RENDER_MAX_US: AtomicU64 = AtomicU64::new(0);
    /// Plus long trou entre deux battements pendant une animation.
    pub static GAP_MAX_US: AtomicU64 = AtomicU64::new(0);
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    pub fn flush() {
        let mut l = LAST.lock().unwrap_or_else(|e| e.into_inner());
        let t = l.get_or_insert_with(Instant::now);
        if t.elapsed().as_secs() < 5 {
            return;
        }
        let secs = t.elapsed().as_secs_f32();
        let (ticks, frames, us, max) = (TICKS.swap(0, Relaxed), FRAMES.swap(0, Relaxed), RENDER_US.swap(0, Relaxed), RENDER_MAX_US.swap(0, Relaxed));
        *l = Some(Instant::now());
        super::dbg(&format!(
            "mesures : {:.1} reveils/s, {frames} images rideau, rendu moyen {:.2} ms, max {:.2} ms, trou max en animation {:.0} ms",
            ticks as f32 / secs,
            if frames > 0 { us as f32 / frames as f32 / 1000.0 } else { 0.0 },
            max as f32 / 1000.0,
            GAP_MAX_US.swap(0, Relaxed) as f32 / 1000.0
        ));
    }
}
