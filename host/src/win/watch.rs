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
use super::game_window::Origin;
use super::util::class_name;
use super::{embedded, fallback, game_window, mark_shown, shared, Shared, WM_APP_HOOK};
use crate::health::FallbackReason;

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
                    let posed = hooks.iter().all(|h| !h.is_null());
                    if !posed {
                        // Sans le hook de creation, la fenetre du jeu ne serait vue que par le balayage lent, donc APRES avoir ete
                        // montree au joueur avec son cadre, puis reprise sous ses yeux : mieux vaut la lui laisser.
                        for h in hooks.iter_mut().filter(|h| !h.is_null()) {
                            UnhookWinEvent(*h);
                            *h = null_mut();
                        }
                    }
                    if let Some(s) = shared() {
                        if posed {
                            s.log(format!("hooks poses sur le processus {pid}"));
                        } else if s.lock().pids.contains(&pid) {
                            s.log(format!("ECHEC de la pose des hooks sur le processus {pid}"));
                            fallback(s, FallbackReason::HookFailed, format!("SetWinEventHook refuse pour le processus {pid}"));
                        }
                    }
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

/// Surveille les fenetres du processus `pid` (0 : plus aucune surveillance). Faux si le fil de surveillance est injoignable.
pub(super) fn set_hooks(pid: u32) -> bool {
    let tid = WATCH_TID.load(Ordering::SeqCst);
    tid != 0 && unsafe { PostThreadMessageW(tid, WM_APP_HOOK, pid as usize, 0) } != 0
}

/// Classes des fenetres de jeu (GLFW 3, SDL 3, LWJGL 2), fenetres d'aide exclues.
///
/// SDL 3 cree, apres sa fenetre principale, des fenetres techniques « message seulement » (`SDLHelperWindowInputCatcher` pour les
/// entrees brutes, `SDL_HIDAPI_DEVICE_DETECTION` pour les manettes). Elles ne s'affichent jamais et refusent tout proprietaire :
/// mesure sur un vrai Minecraft SDL 3, les prendre pour des fenetres de jeu declenchait un repli « rattachement refuse » alors
/// que LA fenetre du jeu etait adoptee et posee, et les transformait en fenetres de premier niveau. On n'y touche pas.
///
/// GLFW 3.4 a lui aussi sa fenetre d'aide, `GLFW3 Helper` (cachee, creee AVANT la fenetre du jeu). Mesure sur un gros modpack :
/// son adoption attendait 21 s le fil du jeu, et celle de LA fenetre du jeu attendait derriere dans la file unique.
fn is_game_class(c: &str) -> bool {
    let helper = c.starts_with("SDLHelperWindow") || c.starts_with("SDL_HIDAPI") || c.starts_with("GLFW3 Helper");
    !helper && (c.starts_with("GLFW") || c.starts_with("SDL") || c == "LWJGL")
}

/// `hwnd` peut etre LA fenetre d'un jeu : une vraie fenetre de premier niveau (fille du bureau ; ni fenetre fille, ni fenetre
/// « message seulement » dont le parent est `HWND_MESSAGE`, ni fenetre deja possedee) d'une classe de jeu. Lectures locales.
/// Toute fenetre qui ne passe pas ce filtre n'est jamais garee, adoptee, comptee comme refus ni « rendue ».
unsafe fn is_candidate(hwnd: HWND) -> bool {
    GetAncestor(hwnd, GA_PARENT) == GetDesktopWindow()
        && GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 & WS_CHILD == 0
        && GetWindow(hwnd, GW_OWNER).is_null()
        && is_game_class(&class_name(hwnd))
        && !is_helper_title(&local_title(hwnd))
}

/// GLFW 3.3 et plus anciens : la fenetre d'aide a la MEME classe que la fenetre du jeu (`GLFW30`) ; seul son titre la distingue.
fn is_helper_title(t: &str) -> bool {
    t == "GLFW message window"
}

/// Titre de la fenetre, lu dans les structures du systeme : aucun message vers le jeu (`GetWindowTextW` peut en envoyer un).
unsafe fn local_title(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = InternalGetWindowText(hwnd, buf.as_mut_ptr(), buf.len() as i32);
    String::from_utf16_lossy(&buf[..n.clamp(0, buf.len() as i32) as usize])
}

/// Candidates principales des processus `pids`, CACHEES comprises : `(fenetre, visible)`. Balayage, lectures locales (aucun
/// message vers le jeu).
pub(super) fn main_candidates(pids: &HashSet<u32>) -> Vec<(isize, bool)> {
    struct Ctx<'a> {
        pids: &'a HashSet<u32>,
        found: Vec<(isize, bool)>,
    }
    unsafe extern "system" fn cb(h: HWND, lp: LPARAM) -> i32 {
        let ctx = &mut *(lp as *mut Ctx);
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, &mut pid);
        if ctx.pids.contains(&pid) && is_candidate(h) {
            let mut r: RECT = std::mem::zeroed();
            GetWindowRect(h, &mut r);
            if crate::guard::main_sized(r.right - r.left, r.bottom - r.top) {
                ctx.found.push((h as isize, IsWindowVisible(h) != 0));
            }
        }
        1
    }
    let mut ctx = Ctx { pids, found: Vec::new() };
    unsafe { EnumWindows(Some(cb), &mut ctx as *mut Ctx as LPARAM) };
    ctx.found
}

/// Lancement cache, apres un repli qui n'avait aucune fenetre principale a rendre (fenetre pas encore creee, hooks refuses) :
/// guette, sur un fil a part, une candidate principale du jeu et la MONTRE si elle reste cachee, puisque le jeu ne le fera pas.
/// S'arrete des qu'une fenetre principale est visible (montree ici, ou par un jeu qui ignore la consigne), quand le lancement
/// n'est plus le courant (`gen`), ou apres `guard::RESCUE_FOR`.
pub(super) fn rescue_hidden(pids: HashSet<u32>, gen: u64) {
    use crate::guard::{RESCUE_EVERY, RESCUE_FOR, RESCUE_SETTLE};
    let _ = std::thread::Builder::new().name("cubelink-host-rescue".into()).spawn(move || {
        let t0 = Instant::now();
        let mut seen: std::collections::HashMap<isize, Instant> = std::collections::HashMap::new();
        while t0.elapsed() < RESCUE_FOR {
            std::thread::sleep(RESCUE_EVERY);
            let Some(s) = shared() else { return };
            if s.lock().gen != gen {
                return;
            }
            let found = main_candidates(&pids);
            if found.iter().any(|(_, visible)| *visible) {
                return; // le jeu a une fenetre principale visible : plus rien a sauver
            }
            for (h, _) in found {
                if seen.entry(h).or_insert_with(Instant::now).elapsed() >= RESCUE_SETTLE {
                    game_window::rescue(s, h, gen);
                    return;
                }
            }
        }
    });
}

/// Hors ecran, sans attendre le jeu (au cas ou la fenetre serait affichee avant son adoption).
pub(super) unsafe fn park_offscreen(hwnd: HWND) {
    SetWindowPos(hwnd, null_mut(), -32000, -32000, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
}

unsafe extern "system" fn win_loc(_hook: HWINEVENTHOOK, _event: u32, hwnd: HWND, id_object: i32, _idc: i32, _tid: u32, _time: u32) {
    if id_object != 0 || hwnd.is_null() {
        return;
    }
    let Some(s) = shared() else { return };
    embedded::on_moved(s, hwnd);
}

unsafe extern "system" fn win_event(_hook: HWINEVENTHOOK, event: u32, hwnd: HWND, id_object: i32, _idc: i32, _tid: u32, _time: u32) {
    if id_object != 0 || hwnd.is_null() || (event != EVENT_OBJECT_CREATE && event != EVENT_OBJECT_SHOW) {
        return;
    }
    let Some(s) = shared() else { return };
    let h = hwnd as isize;
    // fenetre deja adoptee qui s'affiche : c'est LA fenetre du jeu
    if embedded::is_attached(s, hwnd) {
        if event == EVENT_OBJECT_SHOW {
            let became_current = {
                let mut st = s.lock();
                let first = st.adopted.contains(&h) && st.current.is_none();
                if first {
                    mark_shown(&mut st, h);
                }
                first
            };
            if became_current {
                s.log(format!("fenetre du jeu ({}) affichee par le jeu", class_name(hwnd)));
                game_window::announce(s, h);
            }
        }
        return;
    }
    if !is_candidate(hwnd) {
        return;
    }
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, &mut pid);
    let class = class_name(hwnd);
    // releve AVANT le garage : c'est l'etat qu'on rendra a la fenetre en cas de repli
    let before = Origin::of(hwnd);
    let fresh = {
        let mut st = s.lock();
        if !st.pids.contains(&pid) {
            return;
        }
        let fresh = st.adopting.insert(h);
        if fresh {
            st.origin.insert(h, before);
        }
        if event == EVENT_OBJECT_SHOW {
            st.shown_early.insert(h); // affichee par le jeu avant la fin de son adoption : `embedded::adopt` en tient compte
        }
        // Hors ecran tout de suite, SOUS le verrou (l'appel est asynchrone, il n'attend personne) : un repli decide juste apres
        // trouve la fenetre dans `origin` et la ramene ; sans le verrou il pourrait passer entre le releve et le garage, et la
        // fenetre resterait garee sans que personne ne le sache.
        park_offscreen(hwnd);
        fresh
    };
    // Adoption sur le fil de la fenetre du jeu : rattacher la fenetre attend le fil du jeu, qui ne pompe pas ses messages pendant
    // qu'il cree son contexte graphique.
    // Seules les candidates principales entrent au journal permanent : SDL 3 cree puis detruit une quinzaine de fenetres d'essai
    // (10 x 10, 136 x 39) par lancement, qui le noyaient. Le diagnostic fin les garde.
    if fresh && crate::guard::main_sized(before.rect.2, before.rect.3) {
        let (x, y, w, hgt) = before.rect;
        let how = if event == EVENT_OBJECT_CREATE { "creation" } else { "affichage" };
        s.log(format!("fenetre detectee ({how}) : classe {class}, {w} x {hgt} en ({x}, {y})"));
    }
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
    embedded::on_detected(s, h);
    if fresh {
        game_window::adopt(s, h);
    }
}

/// Filet de securite (au repos) : adopte une fenetre du jeu que les evenements auraient manquee (fenetre ephemere,
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
        if ctx.pids.contains(&pid) && IsWindowVisible(h) != 0 && is_candidate(h) {
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
        if st.fell_back {
            return; // le jeu a ete rendu pendant le balayage
        }
        let fresh: Vec<isize> = ctx.found.into_iter().filter(|h| st.adopting.insert(*h)).collect();
        for h in &fresh {
            // meme ordre que pour un evenement : releve, puis garage, sous le verrou
            unsafe {
                st.origin.insert(*h, Origin::of(*h as HWND));
                park_offscreen(*h as HWND);
            }
        }
        fresh
    };
    for h in fresh {
        s.log(format!("fenetre du jeu ({}) retrouvee par le balayage lent", unsafe { class_name(h as HWND) }));
        game_window::adopt(s, h);
    }
}

#[cfg(test)]
mod tests {
    use super::is_game_class;

    #[test]
    fn classes_de_jeu_sans_les_fenetres_d_aide() {
        for c in ["GLFW30", "SDL_app", "LWJGL"] {
            assert!(is_game_class(c), "{c}");
        }
        // fenetres techniques de SDL 3 (« message seulement »), et tout le reste
        for c in ["SDLHelperWindowInputCatcher", "SDL_HIDAPI_DEVICE_DETECTION", "GLFW3 Helper", "SunAwtFrame", "IME", "MSCTFIME UI", ""] {
            assert!(!is_game_class(c), "{c}");
        }
        assert!(super::is_helper_title("GLFW message window"));
        assert!(!super::is_helper_title("Minecraft 1.21.1") && !super::is_helper_title(""));
    }
}
