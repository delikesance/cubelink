//! Mode integre (production) : la zone de jeu vit dans la fenetre du launcher.
//!
//! SDL3 n'accorde le focus clavier (et donc la souris relative) qu'a la fenetre qui est AU PREMIER PLAN de Windows
//! (`GetForegroundWindow() == hwnd`) : une fenetre fille n'y arrive jamais. Le jeu reste donc une fenetre de premier niveau, sans
//! cadre, POSSEDEE par celle du launcher (toujours au-dessus d'elle, absente de la barre des taches, reduite avec elle), et on la
//! cale exactement sur la zone de jeu a chaque deplacement / redimensionnement. Le rideau est une fenetre possedee elle aussi, a
//! transparence par pixel, traversee par la souris, maintenue juste au-dessus du jeu.

use std::ptr::{null, null_mut};
use std::sync::atomic::Ordering;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use super::curtain_window::{self, TIMER_CURTAIN};
use super::debug::dbg;
use super::game_window::{self, FRAME_BITS, STRIP};
use super::util::{hw, rect_differs, register_class, wide};
use super::{on_reveal, run_if, shared, watch, Shared, WM_APP_CURTAIN_ON, WM_APP_RAISE, WM_APP_REVEAL, WM_APP_SYNC};
use crate::flow::{Cadence, Effect};

/// Cree la zone du jeu (fenetre fille de la fenetre du launcher : cible des messages de l'API) et le rideau. Sur le fil du launcher.
pub(super) unsafe fn create(s: &Shared) -> Result<(), String> {
    let parent = hw(s.parent);
    let hinst = GetModuleHandleW(null());
    register_class("CubelinkStage", Some(stage_proc), GetStockObject(BLACK_BRUSH) as isize);
    register_class("CubelinkCurtain", Some(curtain_proc), 0);
    let scls = wide("CubelinkStage");
    let stage = CreateWindowExW(0, scls.as_ptr(), null(), WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS, 0, 0, 10, 10, parent, null_mut(), hinst, null());
    if stage.is_null() {
        return Err("CreateWindowExW (zone du jeu) a echoue".into());
    }
    s.host.store(stage as isize, Ordering::SeqCst);
    let ccls = wide("CubelinkCurtain");
    // Fenetre « possedee » par celle du launcher (toujours au-dessus d'elle, absente de la barre des taches), traversee par la souris.
    let curtain = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
        ccls.as_ptr(),
        null(),
        WS_POPUP,
        0,
        0,
        10,
        10,
        parent,
        null_mut(),
        hinst,
        null(),
    );
    if curtain.is_null() {
        return Err("CreateWindowExW (rideau) a echoue".into());
    }
    s.curtain.store(curtain as isize, Ordering::SeqCst);
    SetTimer(curtain, TIMER_CURTAIN, 16, None);
    Ok(())
}

unsafe extern "system" fn stage_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let Some(s) = shared() else { return DefWindowProcW(hwnd, msg, wp, lp) };
    match msg {
        WM_SIZE => {
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            super::util::paint_background(hwnd, false);
            0
        }
        WM_SETFOCUS => 0,
        WM_APP_CURTAIN_ON => {
            curtain_on(s);
            0
        }
        WM_APP_REVEAL => {
            on_reveal(s, hwnd, wp != 0);
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Le rideau se ferme (ou change de texte) : minuteur rapide, calage, rideau affiche et dessine tout de suite.
unsafe fn curtain_on(s: &Shared) {
    let c = s.curtain();
    curtain_window::set_tick(c, Cadence::Fast);
    sync(s);
    // au-dessus de tout seulement si le joueur est dans le launcher ou le jeu (sinon : juste au-dessus du launcher)
    let fg = GetForegroundWindow();
    let cur = s.lock().current;
    let ours = fg == hw(s.parent) || cur.is_some_and(|g| hw(g) == fg);
    let (after, flag) = if ours { (HWND_TOP, 0) } else { (null_mut(), SWP_NOZORDER) };
    SetWindowPos(c, after, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW | flag);
    curtain_window::present(s, c);
}

unsafe extern "system" fn curtain_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let Some(s) = shared() else { return DefWindowProcW(hwnd, msg, wp, lp) };
    match msg {
        WM_APP_RAISE => {
            // une nouvelle fenetre se place au-dessus de tout, rideau compris : tant qu'elle n'est pas adoptee (le jeu ne repond pas pendant
            // son chargement), le rideau la recouvre au lieu de la laisser visible, blanche et avec son cadre
            let g = lp as HWND;
            if IsWindowVisible(hwnd) != 0 && IsWindow(g) != 0 {
                let prev = GetWindow(g, GW_HWNDPREV);
                let after = if prev.is_null() { HWND_TOP } else { prev };
                if after != hwnd {
                    SetWindowPos(hwnd, after, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                }
            }
            0
        }
        WM_APP_SYNC => {
            curtain_window::sync_taken();
            if curtain_window::tick_is_off() {
                curtain_window::set_tick(hwnd, Cadence::Slow);
            }
            sync(s);
            0
        }
        WM_TIMER => {
            if curtain_window::too_soon() {
                curtain_window::frame_taken();
                return 0;
            }
            // panneaux fermes : la machine bascule sans que le joueur voie la coupure (jeu <-> launcher)
            let t0 = std::time::Instant::now();
            let todo = curtain_window::tick(s);
            let t1 = t0.elapsed();
            run_if(s, &todo, |e| matches!(e, Effect::PlaceGame | Effect::HideGame));
            let t2 = t0.elapsed();
            sync(s);
            let t3 = t0.elapsed();
            curtain_window::finish_tick(s, hwnd, &todo);
            let t4 = t0.elapsed();
            if t4.as_millis() >= 30 {
                dbg(&format!("battement lent : machine {} ms, effets {} ms, calage {} ms, dessin {} ms", t1.as_millis(), (t2 - t1).as_millis(), (t3 - t2).as_millis(), (t4 - t3).as_millis()));
            }
            // au repos : filet de securite lent (adoption tardive, styles reposes par le jeu) ; en animation : 16 ms ; ni animation ni
            // jeu : plus aucun reveil (relance par la fermeture du rideau / `watch`)
            for e in &todo.effects {
                if let Effect::Timer(cadence) = *e {
                    curtain_window::set_tick(hwnd, cadence);
                    if cadence == Cadence::Slow {
                        watch::scan_adopt(s);
                    }
                }
            }
            0
        }
        _ => curtain_window::base_proc(hwnd, msg, wp, lp),
    }
}

/// Les fenetres adoptees sont POSSEDEES par celle du launcher.
pub(super) unsafe fn is_attached(s: &Shared, hwnd: HWND) -> bool {
    let parent = hw(s.parent);
    !parent.is_null() && GetWindow(hwnd, GW_OWNER) == parent
}

/// Une fenetre du jeu vient d'etre creee : le rideau remonte juste au-dessus d'elle (sur son fil).
pub(super) unsafe fn on_detected(s: &Shared, win: isize) {
    let c = s.curtain();
    if !c.is_null() {
        PostMessageW(c, WM_APP_RAISE, 0, win);
    }
}

/// Si c'est la fenetre du jeu qui bouge (recentrage, plein ecran) : recalage sur le fil de l'interface.
pub(super) unsafe fn on_moved(s: &Shared, hwnd: HWND) {
    let ours = {
        let st = s.lock();
        st.machine.flow().placed() && st.current == Some(hwnd as isize)
    };
    if ours {
        curtain_window::request_sync(s);
    }
}

/// Adoption : fenetre de premier niveau sans cadre, possedee par le launcher ; elle reste hors ecran jusqu'a ce que le rideau soit
/// ferme. Sur le fil de la fenetre du jeu (appels synchrones vers le jeu).
pub(super) unsafe fn adopt(s: &Shared, h: isize) {
    let win = hw(h);
    let style = GetWindowLongPtrW(win, GWL_STYLE) as u32;
    SetWindowLongPtrW(win, GWL_STYLE, ((style & !STRIP) | WS_POPUP) as isize);
    SetWindowLongPtrW(win, GWLP_HWNDPARENT, s.parent);
    game_window::hide_from_taskbar(win);
    // SDL cree puis detruit plusieurs fenetres d'essai (choix du contexte graphique) avant la vraie : mesure, 7 creations pour une
    // seule fenetre de jeu. Une fenetre deja detruite n'est pas retenue.
    if IsWindow(win) == 0 {
        dbg(&format!("fenetre {h:#x} ephemere (deja detruite) : ignoree"));
        return;
    }
    s.lock().adopted.push(h);
    SetWindowPos(win, null_mut(), -32000, -32000, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
    let owner = GetWindow(win, GW_OWNER) as isize;
    if owner != s.parent && IsWindow(win) != 0 {
        dbg(&format!("ECHEC : proprietaire de la fenetre {h:#x} = {owner:#x} (attendu {:#x})", s.parent));
    }
}

/// Zone de jeu `view` (coordonnees client du launcher) en coordonnees ecran ; `None` si le launcher est reduit ou la zone vide.
unsafe fn to_screen(s: &Shared, view: (i32, i32, i32, i32)) -> Option<(i32, i32, i32, i32)> {
    let parent = hw(s.parent);
    if IsIconic(parent) != 0 {
        return None;
    }
    let (x, y, w, h) = view;
    if w <= 0 || h <= 0 {
        return None;
    }
    let mut pt = POINT { x, y };
    ClientToScreen(parent, &mut pt);
    Some((pt.x, pt.y, w, h))
}

/// Cale le rideau (toujours) et le jeu (quand il est pose) sur la zone de jeu ; maintient le jeu SOUS le rideau.
pub(super) unsafe fn sync(s: &Shared) {
    let (view, placed, cur, pw, applied) = {
        let st = s.lock();
        (st.view, st.machine.flow().placed(), st.current, st.machine.panel_width(), st.region)
    };
    let Some((x, y, w, h)) = to_screen(s, view) else { return };
    let curtain = s.curtain();
    if rect_differs(curtain, x, y, w, h) {
        SetWindowPos(curtain, null_mut(), x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
    }
    if let (true, Some(g)) = (placed, cur) {
        let g = hw(g);
        // Detection (lectures locales, sans message vers le jeu). Les CORRECTIONS (style, proprietaire, decoupe) sont synchrones
        // vers le fil du jeu, qui ne pompe plus ses messages pendant le chargement : elles partent sur le fil de la fenetre du jeu,
        // jamais sur celui de l'interface (sinon le launcher « ne repond plus »).
        let ex = GetWindowLongPtrW(g, GWL_EXSTYLE) as u32;
        let style = GetWindowLongPtrW(g, GWL_STYLE) as u32;
        // la decoupe peut disparaitre sans nous (fenetre recreee par le jeu) : on la verifie, pas seulement la valeur memorisee
        let lost = pw > 0 && applied == (w, h, pw) && {
            let probe = CreateRectRgn(0, 0, 0, 0);
            let kind = GetWindowRgn(g, probe);
            DeleteObject(probe as _);
            kind == 0 // ERROR : plus de region
        };
        let need_region = lost || (pw > 0 && applied != (w, h, pw)) || (pw == 0 && applied != (0, 0, 0));
        let need_ex = ((ex & !WS_EX_APPWINDOW) | WS_EX_TOOLWINDOW) != ex; // le jeu peut reposer ses styles (plein ecran)
        let need_owner = GetWindow(g, GW_OWNER) as isize != s.parent; // adoption incomplete (fil du jeu occupe)
        let need_style = style & FRAME_BITS != 0 || style & WS_POPUP == 0; // F11 : le jeu rend son cadre
        if need_ex || need_owner || need_style || need_region {
            game_window::request_fix(s, g as isize, s.parent, need_region.then_some((w, h, pw)));
        }
        // sur le fil de la fenetre du jeu (voir `game_window::show`) : ne bloque jamais l'interface sur un jeu occupe
        let rect = rect_differs(g, x, y, w, h).then_some((x, y, w, h));
        let below = (IsWindowVisible(curtain) != 0 && GetWindow(curtain, GW_HWNDNEXT) != g).then_some(curtain as isize);
        if rect.is_some() || below.is_some() {
            game_window::place(s, g as isize, rect, below);
        }
    }
}

/// Affiche la fenetre du jeu sur la zone de jeu (sous le rideau) et lui donne le premier plan si le launcher l'a.
pub(super) unsafe fn show_game(s: &Shared, g: HWND) {
    let view = s.lock().view;
    let Some((x, y, w, h)) = to_screen(s, view) else { return };
    game_window::show(s, g as isize, s.curtain() as isize, (x, y, w, h));
    if GetForegroundWindow() == hw(s.parent) {
        game_window::foreground(s, g as isize);
    }
}

/// Cache la fenetre du jeu (retour au launcher) et rend le premier plan au launcher.
pub(super) unsafe fn hide_game(s: &Shared, g: HWND) {
    game_window::hide(s, g as isize);
    if GetForegroundWindow() == g {
        game_window::foreground(s, s.parent);
    }
}
