//! La zone de jeu vit dans la fenetre du launcher.
//!
//! SDL3 n'accorde le focus clavier (et donc la souris relative) qu'a la fenetre qui est AU PREMIER PLAN de Windows
//! (`GetForegroundWindow() == hwnd`) : une fenetre fille n'y arrive jamais. Le jeu reste donc une fenetre de premier niveau, sans
//! cadre, POSSEDEE par celle du launcher (toujours au-dessus d'elle, absente de la barre des taches, reduite avec elle), et on la
//! cale exactement sur la zone de jeu a chaque deplacement / redimensionnement. Le rideau est une fenetre possedee elle aussi, a
//! transparence par pixel, traversee par la souris, maintenue juste au-dessus du jeu.

use std::ptr::{null, null_mut};
use std::sync::atomic::Ordering;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_F11};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use super::curtain_window::{self, TIMER_CURTAIN};
use super::debug::dbg;
use super::game_window::{self, FRAME_BITS, STRIP};
use super::util::{hw, rect_differs, register_class, wide};
use super::{guard_tick, on_reveal, Exposed, run_if, shared, watch, Shared, WM_APP_CURTAIN_ON, WM_APP_RAISE, WM_APP_REVEAL, WM_APP_SYNC};
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
            super::util::paint_background(hwnd);
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
            settle_pose(s, false);
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
            cover_exposed(s);
            // en dernier : un repli referme le rideau et remet lui-meme le minuteur en cadence rapide
            guard_tick(s);
            0
        }
        WM_ERASEBKGND => 1,
        // le contenu est pousse par `present` (fenetre a transparence par pixel) ; la souris traverse le rideau
        WM_NCHITTEST => HTTRANSPARENT as LRESULT,
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// `a` est au-dessus de `b` dans l'ordre Z. Lectures locales ; parcours borne (une fenetre de jeu qui vient de s'afficher est en
/// haut de la pile : quelques pas).
unsafe fn above(a: HWND, b: HWND) -> bool {
    let mut w = b;
    for _ in 0..4096 {
        w = GetWindow(w, GW_HWNDPREV);
        if w.is_null() {
            return false;
        }
        if w == a {
            return true;
        }
    }
    false
}

/// Duree au bout de laquelle une fenetre de jeu visible sans etre rattachee est signalee au journal (pas sur un etat fugitif :
/// une adoption normale prend quelques millisecondes).
const EXPOSED_AFTER: std::time::Duration = std::time::Duration::from_millis(300);

/// Fenetre de jeu AFFICHEE par le jeu avant son rattachement, quand son fil ne pompe plus ses messages (mesure : NeoForge, 20 s
/// pendant le chargement des mods). Le garage hors ecran et l'adoption attendent ce fil : on ne peut ni la deplacer ni lui
/// retirer son cadre. Reste a la COUVRIR : le rideau, qui est a nous, est remis juste au-dessus d'elle dans l'ordre Z chaque fois
/// qu'elle repasse devant (le jeu se met au premier plan en s'affichant, APRES la remontee faite a sa creation). Ce qui deborde de
/// la zone de jeu reste visible : le journal le dit. A chaque battement du rideau (fil de l'interface), au plus dix fois par seconde.
unsafe fn cover_exposed(s: &Shared) {
    static LAST: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    let now = std::time::Instant::now();
    {
        let mut l = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if l.is_some_and(|t| now.saturating_duration_since(t) < std::time::Duration::from_millis(100)) {
            return;
        }
        *l = Some(now);
    }
    let (wins, view) = {
        let st = s.lock();
        if st.fell_back || st.pids.is_empty() {
            return;
        }
        (st.origin.keys().copied().collect::<Vec<isize>>(), st.view)
    };
    let curtain = s.curtain();
    let covering = IsWindowVisible(curtain) != 0;
    for w in wins {
        let g = hw(w);
        let exposed = IsWindow(g) != 0 && IsWindowVisible(g) != 0 && GetWindow(g, GW_OWNER) as isize != s.parent && !MonitorFromWindow(g, MONITOR_DEFAULTTONULL).is_null();
        if exposed && covering && !above(curtain, g) {
            let prev = GetWindow(g, GW_HWNDPREV);
            SetWindowPos(curtain, if prev.is_null() { HWND_TOP } else { prev }, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
        let report = {
            let mut st = s.lock();
            if !st.origin.contains_key(&w) {
                continue; // rendue ou oubliee entre-temps
            }
            if !exposed {
                if st.exposed.get(&w).is_some_and(|e| !e.logged) {
                    st.exposed.remove(&w); // etat fugitif : le constat repartira de zero
                }
                continue;
            }
            let e = st.exposed.entry(w).or_insert(Exposed { since: now, logged: false });
            let report = !e.logged && now.saturating_duration_since(e.since) >= EXPOSED_AFTER;
            e.logged |= report;
            report
        };
        if report {
            let mut r: windows_sys::Win32::Foundation::RECT = std::mem::zeroed();
            GetWindowRect(g, &mut r);
            let rect = (r.left, r.top, r.right - r.left, r.bottom - r.top);
            let spill = match to_screen(s, view) {
                Some(zone) if !crate::guard::overflows(rect, zone) => "dans la zone de jeu",
                _ => "DEBORDE de la zone de jeu",
            };
            let cover = if covering { "rideau maintenu au-dessus" } else { "rideau ouvert, rien ne la couvre" };
            s.log(format!("fenetre du jeu affichee avant son rattachement : le jeu ne repond pas, {cover} (fenetre {} x {} en ({}, {}), {spill})", rect.2, rect.3, rect.0, rect.1));
        }
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

/// Une fenetre du jeu bouge d'elle-meme (recentrage, plein ecran, agrandissement).
/// * LA fenetre du jeu, posee : recalage sur la zone, via le fil de l'interface.
/// * Une fenetre a nous (garee, adoptee) qui n'est PAS posee et que le jeu ramene VISIBLE sur un ecran : regaree tout de suite.
///   Une fenetre adoptee ne se voit que posee sur la zone, sous le rideau ; sans cela elle resterait a l'ecran, sans cadre, jusqu'a
///   sa pose.
///
/// Dans les deux cas la demande est asynchrone : elle prend effet quand le jeu pompe ses messages. Entre le moment ou il se
/// deplace lui-meme et celui ou il traite notre demande, sa fenetre est la ou IL l'a mise : ce delai-la est le sien.
pub(super) unsafe fn on_moved(s: &Shared, hwnd: HWND) {
    let h = hwnd as isize;
    let (placed, ours, view) = {
        let mut st = s.lock();
        let placed = st.machine.flow().placed() && !st.pose_pending && st.current == Some(h);
        // Pas encore posee, et le jeu lui a remis son cadre (il change ses attributs de fenetre pendant son demarrage) : il est
        // retire tout de suite, la ou elle est garee, et la pose attendra le calme (`guard::pose_waits`).
        let reframed = !placed && st.adopted.contains(&h) && GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 & FRAME_BITS != 0;
        if reframed {
            st.reframed_at = Some(std::time::Instant::now());
            let closing = st.close_asked;
            drop(st);
            if !closing {
                game_window::request_fix(s, h, s.parent, None);
            }
            st = s.lock();
        }
        (placed, st.origin.contains_key(&h), st.view)
    };
    if placed {
        note_fullscreen(s, hwnd, true);
        unminimize(s, hwnd);
        // Recalage DIRECT, d'ici (fil de surveillance : sans fenetre, donc vraiment asynchrone) : il n'attend ni le fil de
        // l'interface ni la file des appels synchrones, ou une correction de cadre peut occuper le jeu pendant deux allers-retours.
        // Une fenetre posee a qui le jeu remet cadre et taille (mesure : 8/31/8/8 px autour de la zone) revient ainsi sur la zone,
        // donc sous le rideau, au premier passage du jeu dans sa file ; le cadre est retire ensuite (`sync`).
        if let Some((x, y, w, hgt)) = to_screen(s, view) {
            if rect_differs(hwnd, x, y, w, hgt) {
                SetWindowPos(hwnd, null_mut(), x, y, w, hgt, SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
            }
        }
        curtain_window::request_sync(s);
    } else if ours && IsWindowVisible(hwnd) != 0 && !MonitorFromWindow(hwnd, MONITOR_DEFAULTTONULL).is_null() {
        dbg(&format!("fenetre {h:#x} revenue a l'ecran avant sa pose : regaree"));
        watch::park_offscreen(hwnd);
    }
}

/// Plein ecran du jeu, quelle que soit la version de Minecraft (lectures locales, aucun message vers le jeu). Trois bibliotheques
/// de fenetrage selon la version : LWJGL2 (jusqu'a 1.12.2), GLFW (1.13 a 26.2), SDL3 (26.3 et plus). Ce qu'elles font TOUTES en
/// passant en plein ecran : retirer la barre de titre et donner elles-memes a la fenetre la taille de son ecran ; en sortant :
/// remettre leur style normal, AVEC barre de titre. D'ou :
/// * entree : pas de barre de titre ET (`moved`, la fenetre vient de bouger) taille = celle de l'ecran, alors que notre zone de
///   jeu est ailleurs. La TAILLE seule, pas le rectangle : SDL3 (mesure sur 26.3 Fabric) deplace en (0,0) puis redimensionne en
///   deux appels, et notre recalage passe entre les deux (on lit (138,167,1920x1080)). En renfort, `WS_EX_TOPMOST` (GLFW, mesure
///   sur 26.2 Fabric), qui ne depend pas de l'instant de lecture. Notre style a nous (`WS_POPUP` seul) ne l'a pas ;
/// * sortie : la barre de titre revient (GLFW et SDL3 mesures).
///
/// La regle du rectangle est suspendue `FULLSCREEN_SETTLE` apres chaque changement : c'est alors NOUS qui deplacons la fenetre
/// (zone agrandie a tout l'ecran, puis rendue), et un recalage en retard la poserait sur l'ecran entier sans demande du jeu.
/// Lu AVANT notre correction du cadre (`sync`). Annonce chaque changement a l'observateur (hors verrou).
unsafe fn note_fullscreen(s: &Shared, g: HWND, moved: bool) {
    // Nos propres recalages apres une bascule durent ~150 ms (mesure 26.3) ; un joueur qui rappuie sur F11 met ~0,8 s. A 1,5 s,
    // une entree rapide passait inapercue et il fallait trois appuis pour retrouver le plein ecran.
    const FULLSCREEN_SETTLE: std::time::Duration = std::time::Duration::from_millis(400);
    let style = GetWindowLongPtrW(g, GWL_STYLE) as u32;
    let topmost = GetWindowLongPtrW(g, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST != 0;
    let caption = style & WS_CAPTION == WS_CAPTION;
    let mut r: RECT = std::mem::zeroed();
    GetWindowRect(g, &mut r);
    let covers = monitor_sized(g, &r);
    if super::debug::dbg_on() {
        let ex = GetWindowLongPtrW(g, GWL_EXSTYLE) as u32;
        dbg(&format!(
            "plein ecran ? moved={moved} style={style:#010x} ex={ex:#010x} rect=({},{},{}x{}) ecran_entier={covers}",
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top
        ));
    }
    let on = {
        let mut st = s.lock();
        if st.current != Some(g as isize) || st.close_asked {
            return;
        }
        let settled = st.fullscreen_at.is_none_or(|t| t.elapsed() >= FULLSCREEN_SETTLE);
        let elsewhere = to_screen(s, st.view).is_none_or(|(x, y, w, h)| (r.left, r.top, r.right - r.left, r.bottom - r.top) != (x, y, w, h));
        let on = match st.fullscreen {
            false => !caption && (topmost || (moved && settled && covers && elsewhere)),
            true => !caption,
        };
        if on == st.fullscreen {
            return;
        }
        st.fullscreen = on;
        st.fullscreen_at = Some(std::time::Instant::now());
        st.refocus = true;
        on
    };
    let why = if topmost { "marque GLFW" } else { "fenetre a la taille de l'ecran" };
    s.log(if on { format!("plein ecran demande par le jeu ({why}) : le launcher passe en plein ecran") } else { "plein ecran quitte par le jeu".to_string() });
    s.emit(crate::HostEvent::Fullscreen(on));
}

/// Le rectangle ecran `r` de `g` a exactement la taille de son moniteur (position ignoree).
unsafe fn monitor_sized(g: HWND, r: &RECT) -> bool {
    let mon = MonitorFromWindow(g, MONITOR_DEFAULTTONULL);
    if mon.is_null() {
        return false;
    }
    let mut mi: MONITORINFO = std::mem::zeroed();
    mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    GetMonitorInfoW(mon, &mut mi) != 0
        && (mi.rcMonitor.right - mi.rcMonitor.left, mi.rcMonitor.bottom - mi.rcMonitor.top) == (r.right - r.left, r.bottom - r.top)
}

/// En plein ecran, GLFW REDUIT sa fenetre des qu'elle perd le focus (mesure : 26.2 Fabric, quand le launcher passe en plein
/// ecran ou qu'on ouvre le panneau). Une fenetre du jeu posee et reduite est rouverte, sans l'activer, mais seulement quand le
/// launcher (ou le jeu) est au premier plan : le joueur parti sur une autre application (Alt+Tab) ne doit pas voir revenir par-dessus elle
/// un jeu qui est toujours « au-dessus de tout ». Le focus suit si le panneau n'est pas ouvert.
unsafe fn unminimize(s: &Shared, g: HWND) {
    let parent = hw(s.parent);
    if IsIconic(g) == 0 || IsIconic(parent) != 0 {
        return;
    }
    // premier plan : personne (bascule en cours), le launcher ou le jeu ; jamais une autre application
    let fg = GetForegroundWindow();
    let mut fg_pid = 0u32;
    if !fg.is_null() {
        GetWindowThreadProcessId(fg, &mut fg_pid);
    }
    let (closing, panel, ours) = {
        let st = s.lock();
        (st.close_asked, st.machine.panel_width() > 0, fg.is_null() || fg_pid == std::process::id() || st.pids.contains(&fg_pid))
    };
    if closing || !ours {
        return;
    }
    dbg(&format!("fenetre du jeu {:#x} reduite par le jeu : rouverte", g as isize));
    ShowWindowAsync(g, SW_SHOWNOACTIVATE);
    if !panel {
        game_window::foreground(s, g as isize);
    }
}

/// Le launcher qui entre en plein ecran ou en sort (`setFullscreen` de Tauri) PREND l'activation : le joueur qui vient de faire
/// F11 perd le clavier. Une seule fois par bascule, dans les 2 s, quand le launcher est au premier plan (jamais une autre
/// application) et que le panneau est ferme, le focus est rendu au jeu. Appele a chaque `sync` (le launcher qui gagne le focus
/// en declenche un). Jamais touche F11 encore enfoncee : le jeu qui recoit le focus avec F11 appuyee y voit un second F11 et
/// rebascule (mesure sur 26.3 Fabric : entree, puis sortie toute seule 0,7 s apres ; focus rendu 80 ms apres l'appui).
unsafe fn refocus(s: &Shared, g: HWND) {
    const WINDOW: std::time::Duration = std::time::Duration::from_secs(3);
    let fg = GetForegroundWindow();
    let f11_down = GetAsyncKeyState(VK_F11 as i32) as u16 & 0x8000 != 0;
    {
        let mut st = s.lock();
        if !st.refocus {
            return;
        }
        if st.close_asked || st.fullscreen_at.is_none_or(|t| t.elapsed() > WINDOW) {
            st.refocus = false;
            return;
        }
        if fg != hw(s.parent) || st.machine.panel_width() > 0 || f11_down {
            return; // pas encore (bascule en cours, F11 tenue) ou panneau ouvert : on reessaie au prochain sync, dans les 3 s
        }
        st.refocus = false;
    }
    dbg("bascule plein ecran du launcher : focus rendu au jeu");
    game_window::foreground(s, g as isize);
}

/// Issue d'une adoption.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Adoption {
    /// La fenetre n'existe deja plus (fenetre d'essai) ou a ete rendue entre-temps : rien n'est retenu.
    Gone,
    /// Les appels sont revenus. Le proprietaire reste a verifier (`game_window::note_owner`) : le jeu peut l'avoir refuse.
    Done,
}

/// Adoption : fenetre de premier niveau sans cadre, possedee par le launcher ; elle reste hors ecran jusqu'a ce que le rideau soit
/// ferme. Sur le fil de la fenetre du jeu (appels synchrones vers le jeu).
pub(super) unsafe fn adopt(s: &Shared, h: isize) -> Adoption {
    let win = hw(h);
    let style = GetWindowLongPtrW(win, GWL_STYLE) as u32;
    // Cet appel attend le fil du jeu : mesure, plus de 20 s quand il charge ses mods. Le style lu juste avant peut donc etre
    // perime quand il s'applique. Le cas qui compte : le jeu a AFFICHE sa fenetre entre-temps ; lui reecrire un style sans
    // `WS_VISIBLE` la laisse « invisible » pour Windows, plus aucun affichage ne viendra, et elle ne serait jamais reconnue comme
    // LA fenetre du jeu (rideau ferme jusqu'au delai de garde). On le constate (ancien style rendu par l'appel, ou evenement
    // d'affichage vu par la surveillance) et on la reaffiche plus bas, une fois garee.
    let old = SetWindowLongPtrW(win, GWL_STYLE, ((style & !STRIP) | WS_POPUP) as isize) as u32;
    let was_shown = style & WS_VISIBLE == 0 && (old & WS_VISIBLE != 0 || s.lock().shown_early.contains(&h));
    SetWindowLongPtrW(win, GWLP_HWNDPARENT, s.parent);
    game_window::hide_from_taskbar(win);
    // SDL cree puis detruit plusieurs fenetres d'essai (choix du contexte graphique) avant la vraie : mesure, 7 creations pour une
    // seule fenetre de jeu. Une fenetre deja detruite n'est pas retenue.
    if IsWindow(win) == 0 {
        dbg(&format!("fenetre {h:#x} ephemere (deja detruite) : ignoree"));
        s.lock().origin.remove(&h);
        return Adoption::Gone;
    }
    {
        let mut st = s.lock();
        if !st.origin.contains_key(&h) {
            return Adoption::Gone; // rendue au jeu pendant l'adoption (repli) : la restitution, en file derriere nous, la remet en etat
        }
        st.adopted.push(h);
        st.adopted_at.insert(h, std::time::Instant::now());
    }
    // Synchrone, comme les appels precedents : quand il revient, le jeu a traite le rattachement, et le proprietaire que l'appelant
    // relit est celui qui tient vraiment.
    SetWindowPos(win, null_mut(), -32000, -32000, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
    if IsWindow(win) == 0 {
        return Adoption::Gone;
    }
    if was_shown && IsWindowVisible(win) == 0 {
        dbg(&format!("fenetre {h:#x} affichee par le jeu pendant son adoption : reaffichee (hors ecran)"));
        ShowWindow(win, SW_SHOWNA);
    }
    Adoption::Done
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
    let (view, placed, cur, pw, applied, closing) = {
        let st = s.lock();
        // premiere pose en attente : la fenetre est garee hors ecran, rien ne la cale sur la zone
        (st.view, st.machine.flow().placed() && !st.pose_pending, st.current, st.machine.panel_width(), st.region, st.close_asked)
    };
    let Some((x, y, w, h)) = to_screen(s, view) else { return };
    let curtain = s.curtain();
    if rect_differs(curtain, x, y, w, h) {
        SetWindowPos(curtain, null_mut(), x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
    }
    // une fenetre detruite (jeu ferme ou tue, avant que le launcher ne l'apprenne) n'est ni calee ni corrigee
    if let (true, Some(g)) = (placed, cur.filter(|g| IsWindow(hw(*g)) != 0)) {
        let g = hw(g);
        note_fullscreen(s, g, false);
        unminimize(s, g);
        refocus(s, g);
        // Le calage part AVANT la correction, et sur le fil de la fenetre du jeu (voir `game_window::show`) : il est asynchrone, le
        // jeu le traite a son prochain passage dans sa file. La correction du cadre, elle, lui coute deux allers-retours synchrones.
        // Quand le jeu repose son cadre ET une taille plus grande que la zone (mesure : NeoForge au demarrage, ~185 ms), sa fenetre
        // revient ainsi d'abord sous le rideau, puis perd son cadre, au lieu de deborder de la zone pendant toute la correction.
        let rect = rect_differs(g, x, y, w, h).then_some((x, y, w, h));
        let below = (IsWindowVisible(curtain) != 0 && GetWindow(curtain, GW_HWNDNEXT) != g).then_some(curtain as isize);
        if rect.is_some() || below.is_some() {
            game_window::place(s, g as isize, rect, below);
        }
        // Detection (lectures locales, sans message vers le jeu). Les CORRECTIONS (style, proprietaire, decoupe) sont synchrones
        // vers le fil du jeu, qui ne pompe plus ses messages pendant le chargement : elles partent sur le fil de la fenetre du jeu,
        // jamais sur celui de l'interface (sinon le launcher « ne repond plus »).
        let ex = GetWindowLongPtrW(g, GWL_EXSTYLE) as u32;
        let style = GetWindowLongPtrW(g, GWL_STYLE) as u32;
        // la decoupe peut disparaitre sans nous (fenetre recreee par le jeu) : on la verifie, pas seulement la valeur memorisee
        let lost = applied == (w, h, pw) && {
            let probe = CreateRectRgn(0, 0, 0, 0);
            let kind = GetWindowRgn(g, probe);
            DeleteObject(probe as _);
            kind == 0 // ERROR : plus de region
        };
        // une region TOUJOURS posee (pleine sans panneau, voir `game_window::apply_region`)
        let need_region = lost || applied != (w, h, pw);
        let need_ex = ((ex & !WS_EX_APPWINDOW) | WS_EX_TOOLWINDOW) != ex; // le jeu peut reposer ses styles (plein ecran)
        let need_owner = GetWindow(g, GW_OWNER) as isize != s.parent; // adoption incomplete (fil du jeu occupe)
        let need_style = style & FRAME_BITS != 0 || style & WS_POPUP == 0; // F11 : le jeu rend son cadre
        // jamais apres une demande de fermeture : la fenetre se defait, ses styles ne sont plus a corriger
        if !closing && (need_ex || need_owner || need_style || need_region) {
            game_window::request_fix(s, g as isize, s.parent, need_region.then_some((w, h, pw)));
        }
    }
}

/// Affiche la fenetre du jeu sur la zone de jeu (sous le rideau) et lui donne le premier plan si le launcher l'a.
///
/// La PREMIERE pose attend que la fenetre soit calme (`guard::pose_waits`) : d'ici la elle reste garee hors ecran, et
/// `settle_pose` la pose a un battement suivant.
pub(super) unsafe fn show_game(s: &Shared, g: HWND) {
    pose(s, g, false);
}

/// Pose en attente : la faire si son heure est venue (a chaque battement), ou tout de suite (`force`). Fil de l'interface.
pub(super) unsafe fn settle_pose(s: &Shared, force: bool) {
    let g = {
        let mut st = s.lock();
        if !st.pose_pending {
            return;
        }
        match st.current.filter(|_| st.machine.flow().placed()) {
            Some(g) => g,
            None => {
                st.pose_pending = false; // retour au launcher, jeu rendu ou termine entre-temps
                return;
            }
        }
    };
    pose(s, hw(g), force);
}

unsafe fn pose(s: &Shared, g: HWND, force: bool) {
    let framed = GetWindowLongPtrW(g, GWL_STYLE) as u32 & FRAME_BITS != 0;
    let (view, waits, was_pending) = {
        let mut st = s.lock();
        let now = std::time::Instant::now();
        let since = |t: Option<std::time::Instant>| t.map(|t| now.saturating_duration_since(t));
        let waits = !force && crate::guard::pose_waits(since(st.shown_at), since(st.reframed_at), framed);
        let was_pending = std::mem::replace(&mut st.pose_pending, waits);
        (st.view, waits, was_pending)
    };
    if waits {
        if !was_pending {
            dbg("premiere pose : attente du calme de la fenetre (garee hors ecran)");
        }
        if framed {
            game_window::request_fix(s, g as isize, s.parent, None);
        }
        return;
    }
    s.log("fenetre du jeu posee sur la zone de jeu");
    let Some((x, y, w, h)) = to_screen(s, view) else { return };
    game_window::show(s, g as isize, s.curtain() as isize, (x, y, w, h));
    if GetForegroundWindow() == hw(s.parent) {
        game_window::foreground(s, g as isize);
    }
}

/// Cache la fenetre du jeu (retour au launcher) et rend le premier plan au launcher.
pub(super) unsafe fn hide_game(s: &Shared, g: HWND) {
    s.lock().pose_pending = false;
    game_window::hide(s, g as isize);
    if GetForegroundWindow() == g {
        game_window::foreground(s, s.parent);
    }
}
