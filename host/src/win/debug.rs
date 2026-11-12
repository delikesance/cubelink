//! Diagnostic (`CUBELINK_STAGE_DEBUG`) : journal, mesures du minuteur et du rendu, sonde des clics.

use std::sync::OnceLock;
use std::time::Duration;

use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use super::util::{class_name, hw};

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

/// A chaque clic REEL du joueur, journalise la chaine de fenetres sous le curseur, le focus du fil du jeu et la zone de confinement
/// du curseur. N'envoie aucune entree.
pub(super) fn mouse_diag() {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    let mut down = false;
    loop {
        std::thread::sleep(Duration::from_millis(15));
        let now = unsafe { GetAsyncKeyState(0x01) } < 0;
        if now && !down {
            unsafe {
                let mut pt = POINT { x: 0, y: 0 };
                GetCursorPos(&mut pt);
                let under = WindowFromPoint(pt);
                let mut chain = String::new();
                let mut w = under;
                while !w.is_null() {
                    chain.push_str(&format!(" {}({:#x})", class_name(w), w as isize));
                    w = GetParent(w);
                }
                let cur = super::shared().and_then(|s| s.lock().current);
                let mut info: GUITHREADINFO = std::mem::zeroed();
                info.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
                let mut pid = 0u32;
                let tid = cur.map(|c| GetWindowThreadProcessId(hw(c), &mut pid)).unwrap_or(0);
                GetGUIThreadInfo(tid, &mut info);
                let mut clip: RECT = std::mem::zeroed();
                GetClipCursor(&mut clip);
                let fg = GetForegroundWindow();
                dbg(&format!(
                    "clic a ({},{}) sous le curseur :{chain} | jeu : actif={:#x} focus={:#x} capture={:#x} | avant-plan={:#x} | clip=({},{},{},{})",
                    pt.x, pt.y, info.hwndActive as isize, info.hwndFocus as isize, info.hwndCapture as isize, fg as isize, clip.left, clip.top, clip.right, clip.bottom
                ));
            }
        }
        down = now;
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
