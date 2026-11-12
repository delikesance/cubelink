//! Surveillance des fenetres du processus de jeu : hooks WinEvent restreints a SON pid, sur un fil dedie, et balayage de secours.

use std::collections::HashSet;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use super::debug::{dbg, dbg_on};
use super::util::class_name;
use super::{game_window, mark_shown, shared, Shared, WM_APP_HOOK};

const EVENT_OBJECT_CREATE: u32 = 0x8000;
const EVENT_OBJECT_SHOW: u32 = 0x8002;
const EVENT_OBJECT_LOCATIONCHANGE: u32 = 0x800B;

static WATCH_TID: AtomicU32 = AtomicU32::new(0);

/// Demarre le fil de surveillance et attend qu'il soit pret a recevoir `set_hooks`.
pub(super) fn start() -> Result<(), String> {
    let (tx, rx) = mpsc::channel::<bool>();
    std::thread::Builder::new().name("cubelink-host-watch".into()).spawn(move || watch_thread(tx)).map_err(|e| e.to_string())?;
    if !rx.recv_timeout(Duration::from_secs(5)).unwrap_or(false) {
        return Err("fil de surveillance des fenetres : delai depasse".into());
    }
    Ok(())
}

/// Surveillance des fenetres : fil DEDIE (boucle de messages propre) pour que l'adoption ne dépende jamais du dessin du
/// rideau : chaque milliseconde de retard est visible comme une fenetre de jeu autonome.
fn watch_thread(ready: mpsc::Sender<bool>) {
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        PeekMessageW(&mut msg, null_mut(), WM_USER, WM_USER, PM_NOREMOVE); // cree la file de messages du fil
        WATCH_TID.store(GetCurrentThreadId(), Ordering::SeqCst);
        let _ = ready.send(true);
        // Hooks poses seulement pendant qu'un jeu tourne, et restreints a SON processus : sans jeu, ce fil dort.
        let mut hooks: [HWINEVENTHOOK; 2] = [null_mut(); 2];
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            if msg.hwnd.is_null() && msg.message == WM_APP_HOOK {
                for h in hooks.iter_mut() {
                    if !h.is_null() {
                        UnhookWinEvent(*h);
                        *h = null_mut();
                    }
                }
                let pid = msg.wParam as u32;
                if pid != 0 {
                    hooks[0] = SetWinEventHook(EVENT_OBJECT_CREATE, EVENT_OBJECT_SHOW, null_mut(), Some(win_event), pid, 0, 0 /* OUTOFCONTEXT */);
                    // le jeu se recentre lui-meme apres l'adoption : on remet sa fenetre a sa place des qu'elle bouge
                    hooks[1] = SetWinEventHook(EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE, null_mut(), Some(win_loc), pid, 0, 0);
                    dbg(&format!("hooks poses sur le processus {pid} : {}", if hooks.iter().all(|h| !h.is_null()) { "oui" } else { "ECHEC" }));
                } else {
                    dbg("hooks retires");
                }
                continue;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Surveille les fenetres du processus `pid` (0 : plus aucune surveillance).
pub(super) fn set_hooks(pid: u32) {
    let tid = WATCH_TID.load(Ordering::SeqCst);
    if tid != 0 {
        unsafe { PostThreadMessageW(tid, WM_APP_HOOK, pid as usize, 0) };
    }
}

/// Classes des fenetres de jeu (GLFW 3, SDL 3, LWJGL 2). Les fenetres d'aide cachees sont adoptees aussi : sans effet.
fn is_game_class(c: &str) -> bool {
    c.starts_with("GLFW") || c.starts_with("SDL") || c == "LWJGL"
}

/// Hors ecran, sans attendre le jeu (au cas ou la fenetre serait affichee avant son adoption).
unsafe fn park_offscreen(hwnd: HWND) {
    SetWindowPos(hwnd, null_mut(), -32000, -32000, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
}

unsafe extern "system" fn win_loc(_hook: HWINEVENTHOOK, _event: u32, hwnd: HWND, id_object: i32, _idc: i32, _tid: u32, _time: u32) {
    if id_object != 0 || hwnd.is_null() {
        return;
    }
    let Some(s) = shared() else { return };
    s.mode.on_moved(s, hwnd);
}

unsafe extern "system" fn win_event(_hook: HWINEVENTHOOK, event: u32, hwnd: HWND, id_object: i32, _idc: i32, _tid: u32, _time: u32) {
    if id_object != 0 || hwnd.is_null() || (event != EVENT_OBJECT_CREATE && event != EVENT_OBJECT_SHOW) {
        return;
    }
    let Some(s) = shared() else { return };
    let h = hwnd as isize;
    // fenetre deja adoptee qui s'affiche : c'est LA fenetre du jeu
    if s.mode.is_attached(s, hwnd) {
        if event == EVENT_OBJECT_SHOW {
            let class = class_name(hwnd);
            let became_current = {
                let mut st = s.lock();
                let first = st.adopted.contains(&h) && st.current.is_none();
                if first {
                    mark_shown(&mut st, h, Some(&class));
                }
                first
            };
            if became_current {
                dbg(&format!("fenetre du jeu {h:#x} affichee par le jeu (deja adoptee)"));
                s.cv.notify_all();
                s.mode.on_game_shown(s, hwnd);
            }
        }
        return;
    }
    if !GetParent(hwnd).is_null() || !GetWindow(hwnd, GW_OWNER).is_null() {
        return;
    }
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, &mut pid);
    let class = class_name(hwnd);
    if !is_game_class(&class) {
        return;
    }
    let fresh = {
        let mut st = s.lock();
        if !st.pids.contains(&pid) {
            return;
        }
        let t = st.started.map_or(0, |x| x.elapsed().as_millis());
        st.adopt_log.push(format!("+{t} ms : fenetre {class:?} detectee ({})", if event == EVENT_OBJECT_CREATE { "creation" } else { "affichage" }));
        st.adopting.insert(h)
    };
    // Hors ecran tout de suite, puis adoption sur le fil de la fenetre du jeu : rattacher la fenetre attend le fil du jeu, qui ne
    // pompe pas ses messages pendant qu'il cree son contexte graphique.
    if dbg_on() {
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(hwnd, &mut r);
        // visible ET sur l'ecran avant l'adoption : c'est la fenetre autonome que le joueur peut apercevoir
        let seen = IsWindowVisible(hwnd) != 0 && r.right > 0 && r.bottom > 0 && r.left > -30000;
        dbg(&format!(
            "fenetre detectee : classe {class:?}, hwnd {h:#x}, {}, deja vue={}, rect=({},{},{},{}){}",
            if event == EVENT_OBJECT_CREATE { "creation" } else { "affichage" },
            !fresh,
            r.left,
            r.top,
            r.right,
            r.bottom,
            if seen { " VISIBLE AVANT ADOPTION" } else { "" }
        ));
    }
    park_offscreen(hwnd);
    s.mode.on_detected(s, h);
    if fresh {
        game_window::adopt(s, h);
    }
}

/// Filet de securite (mode integre, au repos) : adopte une fenetre du jeu que les evenements auraient manquee (fenetre ephemere,
/// affichee avant que le hook ne la voie). Ignore les petites fenetres techniques.
pub(super) fn scan_adopt(s: &Shared) {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    {
        let mut l = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if l.is_some_and(|t| t.elapsed() < Duration::from_millis(500)) {
            return;
        }
        *l = Some(Instant::now());
    }
    let pids = s.lock().pids.clone();
    if pids.is_empty() {
        return;
    }
    struct Ctx<'a> {
        pids: &'a HashSet<u32>,
        found: Vec<isize>,
    }
    unsafe extern "system" fn cb(h: HWND, lp: LPARAM) -> i32 {
        let ctx = &mut *(lp as *mut Ctx);
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, &mut pid);
        if ctx.pids.contains(&pid) && IsWindowVisible(h) != 0 && GetWindow(h, GW_OWNER).is_null() && GetParent(h).is_null() && is_game_class(&class_name(h)) {
            let mut r: RECT = std::mem::zeroed();
            GetWindowRect(h, &mut r);
            if r.right - r.left >= 200 && r.bottom - r.top >= 150 {
                ctx.found.push(h as isize);
            }
        }
        1
    }
    let mut ctx = Ctx { pids: &pids, found: Vec::new() };
    unsafe { EnumWindows(Some(cb), &mut ctx as *mut Ctx as LPARAM) };
    if ctx.found.is_empty() {
        return;
    }
    let fresh: Vec<isize> = {
        let mut st = s.lock();
        ctx.found.into_iter().filter(|h| st.adopting.insert(*h)).collect()
    };
    for h in fresh {
        unsafe { park_offscreen(h as HWND) };
        dbg("fenetre du jeu retrouvee par le balayage lent");
        game_window::adopt(s, h);
    }
}
