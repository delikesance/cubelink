//! La zone de jeu vit dans la fenetre du launcher.
//!
//! La fenetre du jeu est REPARENTEE dans la « scene », une sous-fenetre du launcher (toujours au-dessus de ses autres enfants) :
//! elle n'a plus ni cadre ni bouton de barre des taches (le gestionnaire de fenetres ne la gere plus), suit la scene au pixel pres,
//! et peut recevoir le focus clavier (`XSetInputFocus`). Le panneau « en jeu » reduit la scene : le jeu garde sa taille, rognee.
//! Le rideau est une fenetre ARGB `override-redirect` au-dessus de tout, calee sur la zone, traversee par la souris.
//!
//! Repli si le reparentage est refuse : `WM_TRANSIENT_FOR` = launcher, `_MOTIF_WM_HINTS` sans decoration, `SKIP_TASKBAR`, calage
//! en coordonnees ecran (non verifie sans gestionnaire de fenetres reel).

use std::time::Instant;

use x11rb::connection::Connection;
use x11rb::protocol::shape;
use x11rb::protocol::xproto::*;
use x11rb::wrapper::ConnectionExt as _;

use super::util::{self, clamp16, clamp_u16, Atoms};
use super::{curtain_window, dbg, mark_shown, Shared};
use crate::flow::Cadence;
use crate::guard;
use crate::health::{FallbackReason, HostEvent};

/// Fenetre du jeu telle qu'elle etait avant d'etre touchee : de quoi la rendre.
#[derive(Clone, Copy, Debug)]
pub(super) struct Origin {
    /// `(x, y, largeur, hauteur)` en coordonnees ecran.
    pub rect: (i32, i32, i32, i32),
    /// Rattachee par le repli `WM_TRANSIENT_FOR` (le reparentage a ete refuse).
    pub transient: bool,
}

/// Une fenetre a rendre au jeu ; `main` : LA fenetre du jeu (toujours montree).
pub(super) struct Returned {
    pub win: u32,
    pub origin: Origin,
    pub main: bool,
}

pub(super) struct Made {
    pub stage: u32,
    pub curtain: u32,
    pub gc: u32,
    pub depth: u8,
}

fn set_ident(c: &impl Connection, a: &Atoms, win: u32, class: &str) -> Result<(), String> {
    let cls = format!("{}\0{class}\0", class.to_lowercase());
    c.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, cls.as_bytes()).map_err(|e| e.to_string())?;
    c.change_property32(PropMode::REPLACE, win, a._NET_WM_PID, AtomEnum::CARDINAL, &[std::process::id()]).map_err(|e| e.to_string())?;
    Ok(())
}

/// Cree la scene (fille de `parent`, depuis la visual de la racine : le jeu en vient) et le rideau.
pub(super) fn create(c: &impl Connection, a: &Atoms, screen: &Screen, parent: u32) -> Result<Made, String> {
    let e = |e: x11rb::errors::ConnectionError| format!("creation des fenetres de la Scene : {e}");
    let stage = c.generate_id().map_err(|e| e.to_string())?;
    let aux = CreateWindowAux::new().background_pixel(screen.black_pixel).border_pixel(0).colormap(screen.default_colormap);
    c.create_window(screen.root_depth, stage, parent, 0, 0, 10, 10, 0, WindowClass::INPUT_OUTPUT, screen.root_visual, &aux).map_err(e)?;
    set_ident(c, a, stage, "CubelinkStage")?;
    // rideau : visual 32 bits (ARGB) si le serveur en offre une, sinon celle de la racine (sans transparence)
    let argb = screen.allowed_depths.iter().filter(|d| d.depth == 32).flat_map(|d| d.visuals.iter()).find(|v| v.class == VisualClass::TRUE_COLOR).map(|v| v.visual_id);
    let curtain = c.generate_id().map_err(|e| e.to_string())?;
    let (depth, visual, cmap) = match argb {
        Some(v) => {
            let cmap = c.generate_id().map_err(|e| e.to_string())?;
            c.create_colormap(ColormapAlloc::NONE, cmap, screen.root, v).map_err(e)?;
            (32, v, cmap)
        }
        None => (screen.root_depth, screen.root_visual, screen.default_colormap),
    };
    let aux = CreateWindowAux::new().background_pixel(if argb.is_some() { 0 } else { screen.black_pixel }).border_pixel(0).colormap(cmap).override_redirect(1);
    c.create_window(depth, curtain, screen.root, 0, 0, 10, 10, 0, WindowClass::INPUT_OUTPUT, visual, &aux).map_err(e)?;
    set_ident(c, a, curtain, "CubelinkCurtain")?;
    // la souris traverse le rideau : region d'entree vide (sans l'extension, il reste cliquable)
    if let Err(e) = shape::rectangles(c, shape::SO::SET, shape::SK::INPUT, ClipOrdering::UNSORTED, curtain, 0, 0, &[]).map_err(|e| e.to_string()).and_then(|k| k.check().map_err(|e| e.to_string())) {
        dbg(&format!("region d'entree du rideau non posee ({e}) : il captera la souris"));
    }
    let gc = c.generate_id().map_err(|e| e.to_string())?;
    c.create_gc(gc, curtain, &CreateGCAux::new()).map_err(e)?;
    c.flush().map_err(|e| e.to_string())?;
    Ok(Made { stage, curtain, gc, depth })
}

// ---------------------------------------------------------------------------------------------------- calage

/// Zone de jeu `view` (coordonnees client du launcher) en coordonnees ecran ; `None` si le launcher n'est pas visible ou la zone vide.
pub(super) fn to_screen(s: &Shared, view: (i32, i32, i32, i32)) -> Option<(i32, i32, i32, i32)> {
    let (x, y, w, h) = view;
    if w <= 0 || h <= 0 || !util::is_viewable(&s.conn, s.parent) {
        return None;
    }
    let (px, py) = util::position_in(&s.conn, s.parent, s.root)?;
    Some((px + x, py + y, w, h))
}

fn place(s: &Shared, win: u32, r: (i32, i32, i32, i32)) {
    let aux = ConfigureWindowAux::new().x(clamp16(r.0) as i32).y(clamp16(r.1) as i32).width(clamp_u16(r.2) as u32).height(clamp_u16(r.3) as u32);
    let _ = s.conn.configure_window(win, &aux);
}

fn raise(s: &Shared, win: u32) {
    let _ = s.conn.configure_window(win, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE));
}

/// Cale le rideau (toujours) et le jeu (quand il est pose) sur la zone de jeu.
pub(super) fn sync(s: &Shared) {
    let (view, placed, cur, pw, transient) = {
        let st = s.lock();
        // premiere pose en attente : la fenetre reste dans la scene cachee, rien ne la cale
        let placed = st.machine.flow().placed() && !st.pose_pending;
        let transient = st.current.is_some_and(|g| st.origin.get(&g).is_some_and(|o| o.transient));
        (st.view, placed, st.current, st.machine.panel_width(), transient)
    };
    let Some((x, y, w, h)) = to_screen(s, view) else { return };
    let moved = {
        let mut st = s.lock();
        let moved = st.curtain_rect != Some((x, y, w, h));
        st.curtain_rect = Some((x, y, w, h));
        moved
    };
    if moved {
        place(s, s.curtain, (x, y, w, h));
    }
    let Some(g) = cur.filter(|_| placed).filter(|g| util::exists(&s.conn, *g)) else { return };
    let inner = (w - pw).max(1);
    if transient {
        let want = Some((g, inner, h));
        if std::mem::replace(&mut s.lock().game_rect, want) != want {
            place(s, g, (x, y, inner, h));
            let _ = s.conn.map_window(g);
        }
        return;
    }
    let (restage, remap, regame) = {
        let mut st = s.lock();
        let want = (view.0, view.1, inner, h);
        let restage = st.stage_rect != Some(want);
        st.stage_rect = Some(want);
        let remap = !std::mem::replace(&mut st.stage_mapped, true);
        let regame = st.game_rect != Some((g, w, h));
        st.game_rect = Some((g, w, h));
        (restage, remap, regame)
    };
    if restage {
        place(s, s.stage, (view.0, view.1, inner, h));
    }
    if remap {
        let _ = s.conn.map_window(s.stage);
        raise(s, s.stage);
    }
    if regame {
        // la fenetre du jeu garde la taille de la zone entiere, rognee par la scene quand le panneau est ouvert
        place(s, g, (0, 0, w, h));
    }
    // ICCCM 4.1.5 : la fenetre reparentee n'a plus de gestionnaire de fenetres, c'est a nous de lui dire ou elle est (absolu,
    // racine), y compris quand seule sa position absolue change (launcher deplace) : SDL / GLFW attendent ce message.
    if moved || restage || regame {
        notify_game(s, g, false);
    }
}

/// Envoie au jeu le `ConfigureNotify` SYNTHETIQUE de l'ICCCM 4.1.5 : sa geometrie reelle (taille du serveur) en coordonnees
/// ABSOLUES de la racine. `force` : meme si rien n'a change depuis le dernier envoi (le jeu vient de demander un deplacement ou un
/// redimensionnement et en attend la reponse). Sans effet pour le repli transitoire (le gestionnaire de fenetres s'en charge).
pub(super) fn notify_game(s: &Shared, g: u32, force: bool) {
    let c = &s.conn;
    if s.lock().origin.get(&g).is_none_or(|o| o.transient) {
        return;
    }
    let Some((_, _, w, h)) = util::geometry(c, g) else { return };
    let Some((x, y)) = util::position_in(c, g, s.root) else { return };
    let rect = (x, y, w, h);
    {
        let mut st = s.lock();
        if !force && st.notified == Some((g, rect)) {
            return;
        }
        st.notified = Some((g, rect));
    }
    let ev = ConfigureNotifyEvent {
        response_type: CONFIGURE_NOTIFY_EVENT,
        sequence: 0,
        event: g,
        window: g,
        above_sibling: x11rb::NONE,
        x: clamp16(x),
        y: clamp16(y),
        width: clamp_u16(w),
        height: clamp_u16(h),
        border_width: 0,
        override_redirect: false,
    };
    let _ = c.send_event(false, g, EventMask::STRUCTURE_NOTIFY, ev);
    s.flush();
}

/// Proprietes que SDL / GLFW lisent chez un gestionnaire de fenetres : pas de cadre (`_NET_FRAME_EXTENTS` a 0), fenetre `Normal`.
fn mark_managed(s: &Shared, win: u32) {
    let (c, a) = (&s.conn, &s.atoms);
    let _ = c.change_property32(PropMode::REPLACE, win, a._NET_FRAME_EXTENTS, AtomEnum::CARDINAL, &[0, 0, 0, 0]);
    let _ = c.change_property32(PropMode::REPLACE, win, a.WM_STATE, a.WM_STATE, &[1, 0]); // NormalState, pas d'icone
}

// ---------------------------------------------------------------------------------------------------- rideau

fn map_curtain(s: &Shared) -> bool {
    let view = s.lock().view;
    if to_screen(s, view).is_none() || !launcher_has_focus(s) {
        return false; // launcher reduit, cache ou en arriere-plan : rien a couvrir (il sera montre au battement suivant)
    }
    if !s.curtain_mapped.swap(true, std::sync::atomic::Ordering::SeqCst) {
        let _ = s.conn.map_window(s.curtain);
    }
    raise(s, s.curtain);
    true
}

/// Le rideau se ferme (ou change de texte) : minuteur rapide, calage, rideau affiche et dessine tout de suite.
pub(super) fn curtain_on(s: &Shared) {
    curtain_window::set_tick(s, Cadence::Fast);
    sync(s);
    if map_curtain(s) {
        curtain_window::present(s);
    }
}

pub(super) fn curtain_off(s: &Shared) {
    if s.curtain_mapped.swap(false, std::sync::atomic::Ordering::SeqCst) {
        let _ = s.conn.unmap_window(s.curtain);
    }
}

/// Un gestionnaire de fenetres qui remonte le launcher (clic) le met au-dessus du rideau : il est remonte, au plus dix fois par
/// seconde. Montre aussi le rideau si le launcher vient de redevenir visible alors qu'il devait l'etre.
pub(super) fn keep_curtain_on_top(s: &Shared) {
    static LAST: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);
    let now = Instant::now();
    {
        let mut l = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if l.is_some_and(|t| now.saturating_duration_since(t) < std::time::Duration::from_millis(100)) {
            return;
        }
        *l = Some(now);
    }
    let wanted = s.lock().machine.flow().curtain_on();
    let mapped = s.curtain_mapped.load(std::sync::atomic::Ordering::SeqCst);
    let view = s.lock().view;
    if !wanted {
        if mapped {
            curtain_off(s); // la machine ne le veut plus (ouverture faite, repli) : jamais de rideau oublie
            s.flush();
        }
        return;
    }
    // Le rideau ne couvre QUE la zone de jeu du launcher, et seulement tant que le launcher est visible ET la fenetre active : launcher
    // reduit, cache, ou Alt+Tab vers une autre application, il disparait (il revient avec le launcher).
    let shown = to_screen(s, view).is_some() && launcher_has_focus(s);
    if !shown {
        if mapped {
            curtain_off(s);
            s.flush();
        }
    } else if !mapped {
        if map_curtain(s) {
            curtain_window::present(s);
        }
    } else {
        let top = s.conn.query_tree(s.root).ok().and_then(|c| c.reply().ok()).and_then(|t| t.children.last().copied());
        if top.is_some_and(|t| t != s.curtain) {
            raise(s, s.curtain);
        }
    }
}

// ---------------------------------------------------------------------------------------------------- pose du jeu

/// Le launcher (ou son jeu) est-il la fenetre active ? Fenetre du focus clavier : le launcher, la scene, le jeu, ou n'importe quelle
/// fenetre du MEME cadre de premier niveau. Sans gestionnaire de fenetres, le focus suit la souris : vrai. Avec lui, `None` /
/// `PointerRoot` ou la fenetre d'une autre application : faux (le rideau ne passe jamais par-dessus une autre application).
fn launcher_has_focus(s: &Shared) -> bool {
    let Some(f) = s.conn.get_input_focus().ok().and_then(|c| c.reply().ok()).map(|r| r.focus) else { return false };
    if f <= 1 {
        return !s.wm_present;
    }
    if s.own(f) || s.lock().adopted.contains(&f) {
        return true;
    }
    match (util::toplevel_of(&s.conn, s.root, f), util::toplevel_of(&s.conn, s.root, s.parent)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// Donne le focus clavier a la fenetre du jeu (elle doit etre visible : une fenetre cachee refuserait).
///
/// Jamais quand le launcher n'est pas la fenetre active : le focus que le gestionnaire de fenetres a donne ailleurs n'est pas vole.
pub(super) fn foreground(s: &Shared, g: u32) {
    if util::is_viewable(&s.conn, g) && launcher_has_focus(s) {
        let _ = s.conn.set_input_focus(InputFocus::PARENT, g, x11rb::CURRENT_TIME);
        s.flush();
    }
}

/// Affiche la fenetre du jeu sur la zone de jeu (sous le rideau) et lui donne le focus si le launcher l'a.
///
/// La PREMIERE pose attend que la fenetre soit calme (`guard::pose_waits`) : d'ici la elle reste dans la scene cachee, et
/// `settle_pose` la pose a un battement suivant.
pub(super) fn show_game(s: &Shared, g: u32) {
    pose(s, g, false);
}

/// Pose en attente : la faire si son heure est venue (a chaque battement), ou tout de suite (`force`).
pub(super) fn settle_pose(s: &Shared, force: bool) {
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
    pose(s, g, force);
}

fn pose(s: &Shared, g: u32, force: bool) {
    let (waits, was_pending, hidden) = {
        let mut st = s.lock();
        let now = Instant::now();
        let since = |t: Option<Instant>| t.map(|t| now.saturating_duration_since(t));
        let waits = !force && guard::pose_waits(since(st.shown_at), since(st.reframed_at), false);
        let was_pending = std::mem::replace(&mut st.pose_pending, waits);
        (waits, was_pending, st.expect_hidden)
    };
    if waits {
        if !was_pending {
            dbg("premiere pose : attente du calme de la fenetre (cachee dans la scene)");
        }
        return;
    }
    s.log("fenetre du jeu posee sur la zone de jeu");
    if hidden && !util::is_mapped(&s.conn, g) {
        s.log(format!("lancement cache : fenetre du jeu ({}) affichee par l'hote", util::class_name(&s.conn, g)));
        let _ = s.conn.map_window(g);
    }
    sync(s);
    s.flush();
    if launcher_has_focus(s) {
        foreground(s, g);
    }
}

/// Le jeu n'est plus rattache (fin du jeu, depart) : la scene vide ne doit pas rester affichee par-dessus le launcher.
pub(super) fn unstage(s: &Shared) {
    {
        let mut st = s.lock();
        st.stage_mapped = false;
        st.stage_rect = None;
        st.game_rect = None;
    }
    let _ = s.conn.unmap_window(s.stage);
    s.flush();
}

/// Le launcher reprend le focus (Alt+Tab, clic sur la barre de titre) : le jeu le reprend s'il est dans le jeu, panneau ferme.
pub(super) fn refocus(s: &Shared) {
    let g = {
        let st = s.lock();
        if !st.machine.in_game() || st.machine.panel_width() > 0 || st.pose_pending {
            return;
        }
        st.current
    };
    if let Some(g) = g {
        foreground(s, g);
    }
}

/// Retire la scene (retour au launcher) et rend le focus au launcher si le jeu l'avait.
pub(super) fn hide_game(s: &Shared, g: u32) {
    let parked = {
        let mut st = s.lock();
        st.pose_pending = false;
        st.stage_mapped = false;
        st.game_rect = None;
        st.origin.get(&g).is_some_and(|o| o.transient)
    };
    if parked {
        place(s, g, (-32000, -32000, 10, 10));
    }
    let _ = s.conn.unmap_window(s.stage);
    let had_focus = s.conn.get_input_focus().ok().and_then(|c| c.reply().ok()).is_some_and(|r| r.focus == g);
    if had_focus {
        let _ = s.conn.set_input_focus(InputFocus::PARENT, s.parent, x11rb::CURRENT_TIME);
    }
    s.flush();
}

// ---------------------------------------------------------------------------------------------------- adoption

/// Adoption : la fenetre est garee hors ecran puis reparentee dans la scene (cachee) avant d'avoir pu etre vue par le joueur.
/// Appelee depuis le fil des evenements (ou le balayage), des que la fenetre est reconnue pour etre celle du jeu.
pub(super) fn adopt(s: &Shared, win: u32, how: &str) {
    let t = Instant::now();
    let c = &s.conn;
    let Some(rect) = util::screen_rect(c, s.root, win) else { return };
    let mapped = util::is_mapped(c, win);
    let class = util::class_name(c, win);
    {
        let mut st = s.lock();
        if st.fell_back || st.pids.is_empty() || st.origin.contains_key(&win) {
            return;
        }
        st.origin.insert(win, Origin { rect, transient: false });
    }
    s.log(format!("fenetre detectee ({how}) : classe {class}, {} x {} en ({}, {})", rect.2, rect.3, rect.0, rect.1));
    // gare hors ecran puis rattache : les deux requetes partent ensemble
    place(s, win, (-32000, -32000, rect.2, rect.3));
    // Avec un gestionnaire de fenetres (Mutter/XWayland surtout), une fenetre reparentee n'est plus geree : le focus clavier
    // que nous lui donnons est repris par le launcher en quelques instants, le jeu se met en pause et ne recoit plus rien.
    // On prefere alors la fenetre transitoire (le gestionnaire la focalise normalement) ; CUBELINK_X11_REPARENT=1 force le reparentage.
    let reparented = if s.wm_present && std::env::var_os("CUBELINK_X11_REPARENT").is_none() {
        Err("fenetre geree par le gestionnaire de fenetres".to_string())
    } else {
        c.reparent_window(win, s.stage, 0, 0).map_err(|e| e.to_string()).and_then(|k| k.check().map_err(|e| e.to_string()))
    };
    if !util::exists(c, win) {
        dbg(&format!("fenetre {win:#x} ephemere (deja detruite) : ignoree"));
        s.lock().origin.remove(&win);
        return;
    }
    if let Err(e) = reparented {
        dbg(&format!("pas de reparentage ({e}) : WM_TRANSIENT_FOR"));
        transient(s, win);
    }
    {
        let mut st = s.lock();
        if !st.origin.contains_key(&win) {
            return; // rendue au jeu pendant l'adoption (repli) : la restitution l'a remise en etat
        }
        st.adopted.push(win);
        st.adopted_at.insert(win, Instant::now());
    }
    let owned = note_owner(s, win);
    if owned && !s.lock().origin.get(&win).is_some_and(|o| o.transient) {
        mark_managed(s, win);
        notify_game(s, win, true); // ICCCM : apres un reparentage, le client recoit sa geometrie
    }
    s.log(format!("fenetre {class} adoptee en {} ms{}", t.elapsed().as_millis(), if owned { "" } else { " (rattachement refuse)" }));
    if mapped && owned && guard::main_sized(rect.2, rect.3) {
        shown(s, win); // deja affichee (adoption tardive) : equivalent d'un affichage
    }
}

/// Repli du reparentage refuse : la fenetre devient transitoire du launcher, sans decoration ni bouton de barre des taches.
fn transient(s: &Shared, win: u32) {
    let (c, a) = (&s.conn, &s.atoms);
    let _ = c.change_property32(PropMode::REPLACE, win, AtomEnum::WM_TRANSIENT_FOR, AtomEnum::WINDOW, &[s.parent]);
    let _ = c.change_property32(PropMode::REPLACE, win, a._MOTIF_WM_HINTS, a._MOTIF_WM_HINTS, &[2, 0, 0, 0, 0]); // decorations : aucune
    let _ = c.change_property32(PropMode::REPLACE, win, a._NET_WM_STATE, AtomEnum::ATOM, &[a._NET_WM_STATE_SKIP_TASKBAR, a._NET_WM_STATE_SKIP_PAGER]);
    if util::is_mapped(c, win) {
        let ev = ClientMessageEvent::new(32, win, a._NET_WM_STATE, [1, a._NET_WM_STATE_SKIP_TASKBAR, a._NET_WM_STATE_SKIP_PAGER, 1, 0]);
        let _ = c.send_event(false, s.root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, ev);
    }
    if let Some(o) = s.lock().origin.get_mut(&win) {
        o.transient = true;
    }
    s.flush();
}

/// La fenetre est-elle bien tenue par la scene (ou transitoire du launcher) ? Sinon : refus compte (`guard::verdict`).
pub(super) fn note_owner(s: &Shared, win: u32) -> bool {
    let transient = s.lock().origin.get(&win).is_some_and(|o| o.transient);
    let owner = util::parent_of(&s.conn, win);
    let owned = if transient { util::prop32(&s.conn, win, AtomEnum::WM_TRANSIENT_FOR.into(), AtomEnum::WINDOW, 1).is_some_and(|v| v.first() == Some(&s.parent)) } else { owner == Some(s.stage) };
    let mut st = s.lock();
    if !st.origin.contains_key(&win) {
        st.refused.remove(&win);
        return true;
    }
    if owned {
        return true; // un refus deja constate n'est oublie qu'apres un moment de calme (`guard_tick`)
    }
    let now = Instant::now();
    let r = st.refused.entry(win).or_insert(super::Refused { since: now, attempts: 0, asked: now, bad_at: now });
    r.attempts += 1;
    r.bad_at = now;
    drop(st);
    s.log(format!("rattachement refuse : parent de la fenetre = {:#x} (attendu {:#x}), nouvelles tentatives", owner.unwrap_or(0), s.stage));
    false
}

/// Nouvelle tentative de rattachement d'une fenetre refusee.
pub(super) fn retry(s: &Shared, win: u32) {
    if !util::exists(&s.conn, win) {
        return;
    }
    let transient = s.lock().origin.get(&win).is_some_and(|o| o.transient);
    if transient || util::parent_of(&s.conn, win) == Some(s.stage) {
        return;
    }
    // la suite est asynchrone : le jeu qui refuse le fait apres coup, `reparented_away` compte alors la tentative
    place(s, win, (-32000, -32000, 10, 10));
    let _ = s.conn.reparent_window(win, s.stage, 0, 0).map_err(|_| ()).and_then(|k| k.check().map_err(|_| ()));
    s.lock().game_rect = None;
}

/// Annonce `HostEvent::Adopted` (une fois par lancement) : `win` est LA fenetre du jeu, affichee et dans la scene.
pub(super) fn announce(s: &Shared, win: u32) {
    let class = util::class_name(&s.conn, win);
    let transient = s.lock().origin.get(&win).is_some_and(|o| o.transient);
    if !util::exists(&s.conn, win) || (!transient && util::parent_of(&s.conn, win) != Some(s.stage)) {
        return;
    }
    let after_ms = {
        let mut st = s.lock();
        if st.announced || st.current != Some(win) {
            return;
        }
        st.announced = true;
        st.started.map_or(0, |t| t.elapsed().as_millis() as u64)
    };
    s.log(format!("fenetre du jeu ({class}) rattachee a la Scene, {after_ms} ms apres le lancement"));
    s.emit(HostEvent::Adopted { class, after_ms });
}

/// LA fenetre du jeu vient d'etre affichee par le jeu.
pub(super) fn shown(s: &Shared, win: u32) {
    {
        let mut st = s.lock();
        let main = st.origin.get(&win).is_some_and(|o| guard::main_sized(o.rect.2, o.rect.3));
        if st.fell_back || st.current.is_some() || !main || !st.adopted.contains(&win) {
            return;
        }
        mark_shown(&mut st, win);
    }
    s.log(format!("fenetre du jeu ({}) affichee par le jeu", util::class_name(&s.conn, win)));
    announce(s, win);
}

/// Le jeu a deplace ou redimensionne SA fenetre (`e` : sa geometrie dans la scene). Ce qui n'est pas notre calage est une
/// correction a refaire, et peut etre un plein ecran : fenetre de la taille de l'ecran alors que la zone n'est pas l'ecran.
pub(super) fn game_moved(s: &Shared, win: u32, got: (i32, i32, i32, i32)) {
    let (zone, current, closing, fullscreen, placed, known) = {
        let st = s.lock();
        (st.view, st.current == Some(win), st.close_asked, st.fullscreen, st.machine.flow().placed() && !st.pose_pending, st.origin.contains_key(&win) && st.adopted.contains(&win))
    };
    if !known || got == (0, 0, zone.2, zone.3) {
        return;
    }
    if got.0 <= -30000 {
        return; // notre propre garage
    }
    let (sw, sh) = util::screen_size(&s.conn, s.root);
    if current && !closing {
        let covers = (got.2, got.3) == (sw, sh);
        let on = if fullscreen { covers || s.lock().fullscreen_msg } else { covers && (zone.2, zone.3) != (sw, sh) };
        if on != fullscreen {
            set_fullscreen(s, on, if on { "fenetre a la taille de l'ecran" } else { "fenetre plus a la taille de l'ecran" });
        }
    }
    let note = {
        let mut st = s.lock();
        st.game_rect = None; // a recaler
        if !placed {
            st.reframed_at = Some(Instant::now());
        }
        // le jeu et l'hote se disputent la geometrie : un compte, journalise au plus toutes les 5 s
        st.fights += 1;
        let now = Instant::now();
        if st.fight_logged.is_none_or(|t| now.saturating_duration_since(t) >= std::time::Duration::from_secs(5)) {
            st.fight_logged = Some(now);
            Some(std::mem::take(&mut st.fights))
        } else {
            None
        }
    };
    if let Some(n) = note {
        s.log(format!("le jeu a deplace ou redimensionne sa fenetre {:#x} : {} x {} en ({}, {}) au lieu de {} x {} ; recalee ({n} correction(s))", win, got.2, got.3, got.0, got.1, zone.2, zone.3));
    }
    if !closing && placed {
        sync(s);
    }
    // le jeu attend la reponse a sa demande (SDL_SyncWindow) : geometrie definitive, meme si rien n'a bouge apres correction
    if !closing {
        notify_game(s, win, true);
    }
    s.flush();
}

// ---------------------------------------------------------------------------------------------------- plein ecran

/// Pose / retire `_NET_WM_STATE_FULLSCREEN` dans la propriete `_NET_WM_STATE` de `win` (les autres etats sont conserves) : GLFW et SDL
/// relisent cette propriete pour savoir s'ils sont en plein ecran.
fn mark_net_fullscreen(s: &Shared, win: u32, on: bool) {
    let (c, a) = (&s.conn, &s.atoms);
    if !util::exists(c, win) {
        return;
    }
    let mut v = util::prop32(c, win, a._NET_WM_STATE, AtomEnum::ATOM, 64).unwrap_or_default();
    let had = v.contains(&a._NET_WM_STATE_FULLSCREEN);
    if on == had {
        return;
    }
    if on {
        v.push(a._NET_WM_STATE_FULLSCREEN);
    } else {
        v.retain(|x| *x != a._NET_WM_STATE_FULLSCREEN);
    }
    if v.is_empty() {
        let _ = c.delete_property(win, a._NET_WM_STATE);
    } else {
        let _ = c.change_property32(PropMode::REPLACE, win, a._NET_WM_STATE, AtomEnum::ATOM, &v);
    }
}

/// Demande (en client normal : message client `_NET_WM_STATE` a la racine) au gestionnaire de fenetres d'ajouter (1) ou de retirer
/// (0) le plein ecran de la fenetre du launcher.
fn ask_launcher_fullscreen(s: &Shared, on: bool) {
    let a = &s.atoms;
    let ev = ClientMessageEvent::new(32, s.parent, a._NET_WM_STATE, [u32::from(on), a._NET_WM_STATE_FULLSCREEN, 0, 1, 0]);
    let _ = s.conn.send_event(false, s.root, EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT, ev);
}

/// Le jeu entre dans / sort du plein ecran (message EWMH ou fenetre a la taille de l'ecran) : annonce `HostEvent::Fullscreen`, met la
/// fenetre du launcher en plein ecran (ou la rend), pose l'etat sur la fenetre du jeu, recale la zone. Sans effet si rien ne change.
pub(super) fn set_fullscreen(s: &Shared, on: bool, why: &str) {
    // fenetre du launcher deja en plein ecran avant nous : on n'y touche pas, on ne la « rendra » donc pas
    let already = on && util::prop32(&s.conn, s.parent, s.atoms._NET_WM_STATE, AtomEnum::ATOM, 64).is_some_and(|v| v.contains(&s.atoms._NET_WM_STATE_FULLSCREEN));
    let (changed, game, ask) = {
        let mut st = s.lock();
        let changed = st.fullscreen != on;
        st.fullscreen = on;
        let ask = if on { !st.launcher_fs && !already } else { st.launcher_fs };
        if ask {
            st.launcher_fs = on;
        }
        (changed, st.current, ask)
    };
    if let Some(g) = game {
        mark_net_fullscreen(s, g, on);
    }
    if ask {
        ask_launcher_fullscreen(s, on);
    }
    if changed {
        s.log(if on { format!("plein ecran demande par le jeu ({why}) : le launcher passe en plein ecran") } else { format!("plein ecran quitte par le jeu ({why}) : le launcher revient a son etat") });
        s.emit(HostEvent::Fullscreen(on));
    }
    s.lock().game_rect = None; // a recaler
    sync(s);
    s.flush();
}

/// `_NET_WM_STATE` visant la fenetre du jeu (GLFW / SDL l'envoient a la RACINE, le gestionnaire de fenetres ne gerant plus cette
/// fenetre reparentee) : `action` 0 retire, 1 ajoute, 2 bascule le plein ecran.
pub(super) fn net_wm_state_request(s: &Shared, win: u32, action: u32) {
    let (current, closing, on) = {
        let st = s.lock();
        (st.current == Some(win) && st.adopted.contains(&win), st.close_asked || st.fell_back, st.fullscreen)
    };
    if !current || closing {
        return;
    }
    let want = match action {
        0 => false,
        1 => true,
        2 => !on,
        _ => return,
    };
    s.lock().fullscreen_msg = want;
    set_fullscreen(s, want, "message _NET_WM_STATE");
}

/// Fin du plein ecran (fin du jeu, repli, arret de l'hote) : le launcher revient a son etat d'avant ; `game` perd l'etat. Idempotent.
pub(super) fn end_fullscreen(s: &Shared, game: Option<u32>) {
    let (was, ours) = {
        let mut st = s.lock();
        st.fullscreen_msg = false;
        (std::mem::take(&mut st.fullscreen), std::mem::take(&mut st.launcher_fs))
    };
    if let Some(g) = game {
        mark_net_fullscreen(s, g, false);
    }
    if ours && util::exists(&s.conn, s.parent) {
        ask_launcher_fullscreen(s, false);
    }
    if was || ours {
        s.log("fin du plein ecran : le launcher revient a son etat");
        s.flush();
    }
    if was {
        s.emit(HostEvent::Fullscreen(false));
    }
}

// ---------------------------------------------------------------------------------------------------- restitution

/// Rend sa fenetre au jeu : plus de scene, plus de marques de repli, position visible a l'ecran (au bureau, le gestionnaire de
/// fenetres la reprend avec son cadre). LA fenetre du jeu (et toute fenetre deja affichee) est montree, centree dans la zone de
/// travail ; une fenetre que le jeu n'a pas encore affichee reste cachee, a sa place d'origine : il l'affichera lui-meme.
fn give_back(s: &Shared, r: &Returned, front: bool) {
    let (c, a) = (&s.conn, &s.atoms);
    let g = r.win;
    mark_net_fullscreen(s, g, false);
    let mapped = util::is_mapped(c, g);
    let show = r.main || mapped;
    if !r.origin.transient {
        let _ = c.delete_property(g, a._NET_FRAME_EXTENTS); // le gestionnaire de fenetres les posera lui-meme
        let _ = c.delete_property(g, a.WM_STATE);
    }
    if r.origin.transient {
        let _ = c.delete_property(g, AtomEnum::WM_TRANSIENT_FOR.into());
        let _ = c.delete_property(g, a._MOTIF_WM_HINTS);
        let _ = c.delete_property(g, a._NET_WM_STATE);
    }
    let (ox, oy, ow, oh) = r.origin.rect;
    let (x, y, w, h) = if show || ox <= -30000 || oy <= -30000 { guard::returned_rect(util::work_area(c, a, s.root), (ow, oh)) } else { (ox, oy, ow, oh) };
    let _ = c.reparent_window(g, s.root, clamp16(x), clamp16(y));
    place(s, g, (x, y, w, h));
    if show {
        raise(s, g);
        if !mapped {
            let _ = c.map_window(g);
        }
        if front {
            let _ = c.set_input_focus(InputFocus::PARENT, g, x11rb::CURRENT_TIME);
        }
    }
}

/// REPLI : rend les fenetres, sur le fil de l'appelant ; a leur retour elles SONT rendues. Emet `Fallback` en dernier.
pub(super) fn release(s: &Shared, wins: Vec<Returned>, reason: FallbackReason, detail: String) {
    let t = Instant::now();
    let front = launcher_has_focus(s);
    {
        let mut st = s.lock();
        st.stage_mapped = false;
        st.stage_rect = None;
        st.game_rect = None;
    }
    let _ = s.conn.unmap_window(s.stage);
    let alive: Vec<&Returned> = wins.iter().filter(|r| util::exists(&s.conn, r.win)).collect();
    for r in &alive {
        give_back(s, r, front);
    }
    s.flush();
    let _ = s.conn.get_input_focus().ok().and_then(|c| c.reply().ok()); // aller-retour : le serveur a traite les requetes
    s.log(format!("repli ({}) : {} fenetre(s) rendue(s) au jeu en {} ms", reason.as_str(), alive.len(), t.elapsed().as_millis()));
    s.emit(HostEvent::Fallback { reason, detail });
}

/// Le jeu a defait le rattachement d'une fenetre adoptee (il l'a reparentee ailleurs) : refus compte, retente aux battements.
pub(super) fn reparented_away(s: &Shared, win: u32) {
    if s.lock().adopted.contains(&win) {
        note_owner(s, win);
    }
}
