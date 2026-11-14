//! Fenetre du rideau : contenu (texte et avancement), rendu a transparence par pixel, minuteur. Sa procedure de fenetre est dans
//! `embedded.rs` et s'appuie sur `tick` / `finish_tick`.

use std::cell::RefCell;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU8, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{HWND, POINT, RECT, SIZE};
use windows_sys::Win32::Graphics::Dwm::DwmFlush;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use super::debug::{dbg, dbg_on, stats};
use super::util::{bitmap_info, dpi_scale};
use super::{dispatch, Shared, Todo, WM_APP_SYNC};
use crate::curtain::{self, Curtain, Frame};
use crate::flow::{Cadence, Effect, Event};

/// Minuteur du rideau : rapide pendant une animation, lent (filet de securite) sinon.
const TICK_FAST_MS: u32 = 16;
const TICK_IDLE_MS: u32 = 250;
pub(super) const TIMER_CURTAIN: usize = 1;

thread_local! {
    /// Moteur de rendu (caches de texte et de fond) du fil de l'interface.
    pub(super) static CURTAIN: RefCell<Curtain> = RefCell::new(Curtain::new());
}

/// Cadence actuelle du minuteur (il est cree rapide).
static TICK: AtomicU8 = AtomicU8::new(Cadence::Fast as u8);
static SYNC_PENDING: AtomicBool = AtomicBool::new(false);

/// Tout ce que montre le rideau maintenant (une seule prise du verrou).
fn frame(s: &Shared) -> Frame {
    let st = &mut *s.lock();
    Frame::capture(&st.machine, &mut st.closer, &mut st.tracker, &st.title, &st.caption, st.curtain_t0)
}

/// Regle la cadence du minuteur du rideau (un seul appel systeme quand elle change). A appeler sur le fil du rideau.
pub(super) unsafe fn set_tick(curtain: HWND, cadence: Cadence) {
    if TICK.swap(cadence as u8, Ordering::SeqCst) != cadence as u8 {
        match cadence {
            Cadence::Off => {
                dbg("minuteur : arrete");
                KillTimer(curtain, TIMER_CURTAIN);
            }
            Cadence::Fast => {
                dbg("minuteur : rapide");
                // animation : cadence donnee par le compositeur (voir `pacer`) ; le minuteur reste en plancher (~37 battements/s)
                // si le compositeur ralentit, les battements trop rapproches etant ecartes par `too_soon`
                *LAST_TICK.lock().unwrap_or_else(|e| e.into_inner()) = None; // le repos precedent n'est pas un trou d'animation
                SetTimer(curtain, TIMER_CURTAIN, TICK_FAST_MS, None);
                pacer_start(curtain);
            }
            Cadence::Slow => {
                dbg("minuteur : lent");
                SetTimer(curtain, TIMER_CURTAIN, TICK_IDLE_MS, None);
            }
        }
    }
}

/// Fil cadenceur des animations. `WM_TIMER` a 16 ms ne tient pas 60 images/s (mesure : ~37 reveils/s) : pendant une animation,
/// ce fil attend chaque composition de l'ecran (`DwmFlush`) et poste un battement au rideau, au plus un en attente et au plus
/// ~70 par seconde (ecrans rapides). Hors animation il dort (`park`) : aucun reveil.
static PACER: OnceLock<Option<std::thread::Thread>> = OnceLock::new();
static PACER_HWND: AtomicIsize = AtomicIsize::new(0);
static FRAME_PENDING: AtomicBool = AtomicBool::new(false);

/// Instant du dernier battement accepte.
static LAST_TICK: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);

/// En animation, deux sources battent (cadenceur et minuteur) : ce battement arrive-t-il trop pres du precedent (< 10 ms) ?
/// Sinon il est accepte et devient le dernier. Mesure aussi le plus long trou entre deux battements d'animation.
pub(super) fn too_soon() -> bool {
    let fast = TICK.load(Ordering::SeqCst) == Cadence::Fast as u8;
    let mut last = LAST_TICK.lock().unwrap_or_else(|e| e.into_inner());
    let gap = last.map(|l| l.elapsed());
    if fast && gap.is_some_and(|g| g < Duration::from_millis(10)) {
        return true;
    }
    if let (true, true, Some(g)) = (fast, dbg_on(), gap) {
        stats::GAP_MAX_US.fetch_max(g.as_micros() as u64, Ordering::Relaxed);
    }
    *last = Some(Instant::now());
    false
}

/// Demarre (ou reveille) le cadenceur pour `curtain`. Faux si le fil n'a pas pu etre cree.
fn pacer_start(curtain: HWND) -> bool {
    PACER_HWND.store(curtain as isize, Ordering::SeqCst);
    let t = PACER.get_or_init(|| std::thread::Builder::new().name("cubelink-host-pacer".into()).spawn(pacer).ok().map(|j| j.thread().clone()));
    match t {
        Some(t) => {
            t.unpark();
            true
        }
        None => false,
    }
}

fn pacer() {
    loop {
        if TICK.load(Ordering::SeqCst) != Cadence::Fast as u8 {
            std::thread::park();
            continue;
        }
        // attend la prochaine composition ; composition indisponible : cadence fixe
        if unsafe { DwmFlush() } != 0 {
            std::thread::sleep(Duration::from_millis(16));
        }
        let hwnd = PACER_HWND.load(Ordering::SeqCst) as HWND;
        if !hwnd.is_null() && !FRAME_PENDING.swap(true, Ordering::SeqCst) && unsafe { PostMessageW(hwnd, WM_TIMER, TIMER_CURTAIN, 0) } == 0 {
            FRAME_PENDING.store(false, Ordering::SeqCst);
        }
    }
}

/// Le minuteur est arrete (ni animation ni jeu).
pub(super) fn tick_is_off() -> bool {
    TICK.load(Ordering::SeqCst) == Cadence::Off as u8
}

/// Demande un recalage sur le fil de l'interface (fusion : au plus un message en attente).
pub(super) fn request_sync(s: &Shared) {
    if SYNC_PENDING.swap(true, Ordering::SeqCst) {
        return;
    }
    let c = s.curtain();
    if c.is_null() || unsafe { PostMessageW(c, WM_APP_SYNC, 0, 0) } == 0 {
        SYNC_PENDING.store(false, Ordering::SeqCst);
    }
}

/// Le recalage demande est en cours de traitement : le suivant peut etre poste.
pub(super) fn sync_taken() {
    SYNC_PENDING.store(false, Ordering::SeqCst);
}

/// Un battement poste par le cadenceur a ete recu : le suivant peut etre poste.
pub(super) fn frame_taken() {
    FRAME_PENDING.store(false, Ordering::SeqCst);
}

/// Battement du minuteur : fait avancer la machine. L'appelant execute les effets puis appelle `finish_tick`.
pub(super) fn tick(s: &Shared) -> Todo {
    FRAME_PENDING.store(false, Ordering::SeqCst);
    if dbg_on() {
        stats::TICKS.fetch_add(1, Ordering::Relaxed);
        stats::flush();
    }
    dispatch(&mut s.lock(), Event::Tick)
}

/// Fin du battement : rideau retire si les panneaux sont ouverts, sinon redessine.
pub(super) unsafe fn finish_tick(s: &Shared, hwnd: HWND, todo: &Todo) {
    if todo.has(|e| *e == Effect::HideCurtain) {
        ShowWindow(hwnd, SW_HIDE);
    } else if IsWindowVisible(hwnd) != 0 {
        present(s, hwnd);
    }
}

/// Dessine le rideau (panneaux + contenu) avec transparence par pixel.
pub(super) unsafe fn present(s: &Shared, hwnd: HWND) {
    let mut r: RECT = std::mem::zeroed();
    GetClientRect(hwnd, &mut r);
    let (w, h) = (r.right.max(1), r.bottom.max(1));
    let scale = dpi_scale(hwnd);
    let f = frame(s);
    let t0 = Instant::now();
    let m = f.model();
    let screen = GetDC(null_mut());
    let mem = CreateCompatibleDC(screen);
    let bi = bitmap_info(w, h);
    let mut bits: *mut core::ffi::c_void = null_mut();
    let dib = CreateDIBSection(screen, &bi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
    if !dib.is_null() && !bits.is_null() {
        // rendu en cache + conversion premultipliee directement dans la DIB (aucune allocation par image)
        let dst = std::slice::from_raw_parts_mut(bits as *mut u8, (w as usize) * (h as usize) * 4);
        CURTAIN.with(|c| {
            // jamais de panic dans une procedure de fenetre : si le moteur est deja emprunte, cette image est sautee
            if let Ok(mut c) = c.try_borrow_mut() {
                let pm = c.render_ref(&m, w as u32, h as u32, scale);
                if pm.data().len() == dst.len() {
                    curtain::bgra_into_slice(pm.data(), dst);
                }
            }
        });
        let old = SelectObject(mem, dib as _);
        let size = SIZE { cx: w, cy: h };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION { BlendOp: 0, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: 1 };
        UpdateLayeredWindow(hwnd, screen, null(), &size, mem, &src, 0, &blend, ULW_ALPHA);
        SelectObject(mem, old);
    }
    if !dib.is_null() {
        DeleteObject(dib as _);
    }
    DeleteDC(mem);
    ReleaseDC(null_mut(), screen);
    if dbg_on() {
        let us = t0.elapsed().as_micros() as u64;
        stats::FRAMES.fetch_add(1, Ordering::Relaxed);
        stats::RENDER_US.fetch_add(us, Ordering::Relaxed);
        stats::RENDER_MAX_US.fetch_max(us, Ordering::Relaxed);
    }
}
