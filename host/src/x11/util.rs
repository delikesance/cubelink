//! Petits outils X11 : atomes, lectures de proprietes, geometrie. Toute lecture tolere une fenetre disparue (`None`) : un jeu cree
//! et detruit des fenetres d'essai a toute allure, une requete sur l'une d'elles n'est jamais une erreur.

use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::rust_connection::RustConnection;

x11rb::atom_manager! {
    pub Atoms: AtomsCookie {
        WM_PROTOCOLS,
        WM_DELETE_WINDOW,
        _NET_WM_PID,
        _NET_WM_STATE,
        _NET_WM_STATE_FULLSCREEN,
        _NET_WM_STATE_SKIP_TASKBAR,
        _NET_WM_STATE_SKIP_PAGER,
        _NET_WM_WINDOW_TYPE,
        _NET_WM_WINDOW_TYPE_UTILITY,
        _NET_WM_NAME,
        _NET_WORKAREA,
        WM_STATE,
        _NET_FRAME_EXTENTS,
        _NET_SUPPORTING_WM_CHECK,
        _MOTIF_WM_HINTS,
        UTF8_STRING,
        _CUBELINK_HOST,
    }
}

/// Valeurs 32 bits d'une propriete (`None` : absente, mal formee, ou fenetre disparue).
pub fn prop32(c: &RustConnection, win: u32, prop: u32, ty: impl Into<u32>, max: u32) -> Option<Vec<u32>> {
    let r = c.get_property(false, win, prop, ty, 0, max).ok()?.reply().ok()?;
    let v: Vec<u32> = r.value32()?.collect();
    Some(v)
}

/// Octets d'une propriete.
pub fn prop8(c: &RustConnection, win: u32, prop: u32, ty: impl Into<u32>) -> Option<Vec<u8>> {
    let r = c.get_property(false, win, prop, ty, 0, 256).ok()?.reply().ok()?;
    (r.format == 8).then_some(r.value)
}

pub fn pid_of(c: &RustConnection, a: &Atoms, win: u32) -> Option<u32> {
    prop32(c, win, a._NET_WM_PID, AtomEnum::CARDINAL, 1)?.first().copied()
}

/// `(instance, classe)` de `WM_CLASS`.
pub fn wm_class(c: &RustConnection, win: u32) -> Option<(String, String)> {
    let v = prop8(c, win, AtomEnum::WM_CLASS.into(), AtomEnum::STRING)?;
    let mut it = v.split(|b| *b == 0).map(|p| String::from_utf8_lossy(p).into_owned());
    let inst = it.next()?;
    Some((inst.clone(), it.next().filter(|s| !s.is_empty()).unwrap_or(inst)))
}

/// Classe d'affichage d'une fenetre, ou `?`.
pub fn class_name(c: &RustConnection, win: u32) -> String {
    wm_class(c, win).map_or_else(|| "?".to_string(), |(_, class)| class)
}

/// `(x, y, largeur, hauteur)` dans le repere du parent.
pub fn geometry(c: &RustConnection, win: u32) -> Option<(i32, i32, i32, i32)> {
    let g = c.get_geometry(win).ok()?.reply().ok()?;
    Some((g.x as i32, g.y as i32, g.width as i32, g.height as i32))
}

/// Position du coin haut gauche de `win` dans le repere de `to`.
pub fn position_in(c: &RustConnection, win: u32, to: u32) -> Option<(i32, i32)> {
    let t = c.translate_coordinates(win, to, 0, 0).ok()?.reply().ok()?;
    Some((t.dst_x as i32, t.dst_y as i32))
}

pub fn parent_of(c: &RustConnection, win: u32) -> Option<u32> {
    Some(c.query_tree(win).ok()?.reply().ok()?.parent)
}

pub fn attrs(c: &RustConnection, win: u32) -> Option<GetWindowAttributesReply> {
    c.get_window_attributes(win).ok()?.reply().ok()
}

/// La fenetre existe et son propre drapeau « affichee » est leve (elle peut etre non visible si un ancetre est cache).
pub fn is_mapped(c: &RustConnection, win: u32) -> bool {
    attrs(c, win).is_some_and(|a| a.map_state != MapState::UNMAPPED)
}

/// Affichee ET tous ses ancetres aussi : ce que le joueur peut voir.
pub fn is_viewable(c: &RustConnection, win: u32) -> bool {
    attrs(c, win).is_some_and(|a| a.map_state == MapState::VIEWABLE)
}

pub fn exists(c: &RustConnection, win: u32) -> bool {
    attrs(c, win).is_some()
}

/// Taille de l'ecran (racine).
pub fn screen_size(c: &RustConnection, root: u32) -> (i32, i32) {
    geometry(c, root).map_or((1280, 720), |(_, _, w, h)| (w, h))
}

/// Zone de travail du bureau `(gauche, haut, droite, bas)` : `_NET_WORKAREA` s'il existe, sinon tout l'ecran.
pub fn work_area(c: &RustConnection, a: &Atoms, root: u32) -> (i32, i32, i32, i32) {
    if let Some(v) = prop32(c, root, a._NET_WORKAREA, AtomEnum::CARDINAL, 4) {
        if let [x, y, w, h, ..] = v[..] {
            if w > 0 && h > 0 {
                return (x as i32, y as i32, (x + w) as i32, (y + h) as i32);
            }
        }
    }
    let (w, h) = screen_size(c, root);
    (0, 0, w, h)
}

/// Message client `WM_PROTOCOLS / WM_DELETE_WINDOW` : la fermeture que declenche la croix de la fenetre. Faux si la fenetre ne la
/// gere pas (elle n'est alors jamais forcee).
pub fn ask_close(c: &RustConnection, a: &Atoms, win: u32) -> bool {
    let protocols = prop32(c, win, a.WM_PROTOCOLS, AtomEnum::ATOM, 32).unwrap_or_default();
    if !protocols.contains(&a.WM_DELETE_WINDOW) {
        return false;
    }
    let ev = ClientMessageEvent::new(32, win, a.WM_PROTOCOLS, [a.WM_DELETE_WINDOW, 0, 0, 0, 0]);
    c.send_event(false, win, EventMask::NO_EVENT, ev).is_ok() && c.flush().is_ok()
}

/// `(x, y, largeur, hauteur)` d'une fenetre en coordonnees ecran.
pub fn screen_rect(c: &RustConnection, root: u32, win: u32) -> Option<(i32, i32, i32, i32)> {
    let (_, _, w, h) = geometry(c, win)?;
    let (x, y) = position_in(c, win, root)?;
    Some((x, y, w, h))
}

pub fn clamp16(v: i32) -> i16 {
    v.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

pub fn clamp_u16(v: i32) -> u16 {
    v.clamp(1, u16::MAX as i32) as u16
}

/// `pid` descend-il d'un des processus de `pids` ? Remonte les parents par `/proc/<pid>/stat` (quelques niveaux).
pub fn has_ancestor(pid: u32, pids: &std::collections::HashSet<u32>) -> bool {
    let mut p = pid;
    for _ in 0..8 {
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{p}/stat")) else { return false };
        // `pid (comm) etat ppid ...` : comm peut contenir espaces et parentheses, on repart de la derniere
        let Some(rest) = stat.rsplit_once(')').map(|(_, r)| r) else { return false };
        let Some(ppid) = rest.split_whitespace().nth(1).and_then(|v| v.parse::<u32>().ok()) else { return false };
        if ppid <= 1 || ppid == p {
            return false;
        }
        if pids.contains(&ppid) {
            return true;
        }
        p = ppid;
    }
    false
}

/// Fenetre de premier niveau (fille de la racine) qui contient `win` (`win` elle-meme si elle l'est) ; avec un gestionnaire de
/// fenetres reparentant, c'est son cadre. `None` : fenetre disparue.
pub fn toplevel_of(c: &RustConnection, root: u32, win: u32) -> Option<u32> {
    let mut w = win;
    for _ in 0..32 {
        let p = parent_of(c, w)?;
        if p == root || p == 0 {
            return Some(w);
        }
        w = p;
    }
    None
}

/// Un gestionnaire de fenetres (EWMH) tourne-t-il ? Sans lui, le focus suit la souris et rien n'est jamais « en arriere-plan ».
pub fn has_wm(c: &RustConnection, a: &Atoms, root: u32) -> bool {
    prop32(c, root, a._NET_SUPPORTING_WM_CHECK, AtomEnum::WINDOW, 1).is_some_and(|v| v.first().is_some_and(|w| *w != 0))
}
