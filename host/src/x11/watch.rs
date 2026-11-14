//! Surveillance des fenetres du processus de jeu : le fil des evenements X11 et le balayage de secours.
//!
//! Detection : `SubstructureNotify` sur la racine (JAMAIS `SubstructureRedirect` : ce serait se faire gestionnaire de fenetres),
//! puis, pour chaque fenetre de premier niveau creee, `StructureNotify | PropertyChange` sur elle et lecture de ses proprietes
//! tout de suite (elles sont souvent posees avant qu'on ait pu s'abonner) puis a chaque changement. Une fenetre est celle du jeu
//! quand `_NET_WM_PID` est un processus surveille ; a defaut de pid (XWayland ne le donne pas toujours), quand sa classe est celle
//! d'un jeu ET qu'elle est apparue apres le debut de la surveillance.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;

use super::{embedded, util, Shared};
use crate::guard;

/// Balayage de secours : au plus un par ce delai, et seulement tant que la fenetre du jeu n'est pas connue.
const SCAN_EVERY: Duration = Duration::from_millis(500);

/// Classe d'une fenetre de jeu (repli sans `_NET_WM_PID`) : Minecraft, GLFW, SDL, LWJGL 2, AWT de Java. Les fenetres d'aide de SDL
/// et de GLFW n'en sont pas.
fn is_game_class(c: &str) -> bool {
    let l = c.to_lowercase();
    !guard::is_helper_class(c) && ["minecraft", "glfw", "sdl", "lwjgl", "sun-awt", "java"].iter().any(|k| l.contains(k))
}

/// Demarre le fil des evenements.
pub(super) fn start(s: &Arc<Shared>) -> Result<(), String> {
    let c = &s.conn;
    c.change_window_attributes(s.root, &ChangeWindowAttributesAux::new().event_mask(EventMask::SUBSTRUCTURE_NOTIFY)).map_err(|e| e.to_string())?;
    c.change_window_attributes(s.parent, &ChangeWindowAttributesAux::new().event_mask(EventMask::STRUCTURE_NOTIFY | EventMask::FOCUS_CHANGE)).map_err(|e| e.to_string())?;
    c.flush().map_err(|e| e.to_string())?;
    let s2 = s.clone();
    std::thread::Builder::new()
        .name("cubelink-host-x11".into())
        .spawn(move || {
            while !s2.stop.load(Ordering::SeqCst) {
                match s2.conn.wait_for_event() {
                    Ok(ev) => handle(&s2, ev),
                    Err(_) => break, // connexion perdue : le jeu n'est plus tenu, la machine ne bouge plus
                }
            }
        })
        .map_err(|e| format!("fil des evenements X11 : {e}"))?;
    Ok(())
}

fn watching(s: &Shared) -> bool {
    let st = s.lock();
    !st.pids.is_empty() && !st.fell_back
}

fn handle(s: &Shared, ev: Event) {
    match ev {
        Event::CreateNotify(e) if e.parent == s.root && !e.override_redirect && !s.own(e.window) => {
            if watching(s) {
                subscribe(s, e.window);
                consider(s, e.window, "creation");
            }
        }
        Event::PropertyNotify(e) if !s.own(e.window) => {
            let a = &s.atoms;
            if (e.atom == a._NET_WM_PID || e.atom == u32::from(AtomEnum::WM_CLASS)) && watching(s) {
                consider(s, e.window, "proprietes");
            }
        }
        Event::MapNotify(e) if !s.own(e.window) => {
            let known = s.lock().adopted.contains(&e.window);
            if known {
                embedded::shown(s, e.window);
            } else if watching(s) && !e.override_redirect {
                consider(s, e.window, "affichage");
            }
        }
        // ConfigureNotify SYNTHETIQUE (celui que NOUS envoyons au jeu, geometrie absolue) : jamais une geometrie a corriger
        Event::ConfigureNotify(e) if e.response_type & 0x80 != 0 && e.window != s.parent => {}
        Event::ConfigureNotify(e) => {
            if e.window == s.parent {
                embedded::sync(s); // le launcher a bouge ou change de taille : la zone suit
                s.flush();
            } else if s.lock().adopted.contains(&e.window) {
                embedded::game_moved(s, e.window, (e.x as i32, e.y as i32, e.width as i32, e.height as i32));
            } else if watching(s) && !s.own(e.window) {
                consider(s, e.window, "redimensionnement");
            }
        }
        // le gestionnaire de fenetres rend le focus au launcher (pas a la fenetre fille reparentee) : le jeu le reprend
        Event::FocusIn(e) if e.event == s.parent && e.mode == NotifyMode::NORMAL && matches!(e.detail, NotifyDetail::ANCESTOR | NotifyDetail::NONLINEAR) => {
            embedded::refocus(s);
        }
        Event::MapNotify(_) | Event::UnmapNotify(_) => {}
        Event::ReparentNotify(e) => {
            if e.parent != s.stage && s.lock().adopted.contains(&e.window) && !s.lock().origin.get(&e.window).is_some_and(|o| o.transient) {
                s.log(format!("le jeu a defait le rattachement de sa fenetre {:#x} (nouveau parent {:#x})", e.window, e.parent));
                embedded::reparented_away(s, e.window);
            }
        }
        // GLFW / SDL : plein ecran demande en message client a la racine (livre a l'hote par son `SubstructureNotify`)
        Event::ClientMessage(m) if m.type_ == s.atoms._NET_WM_STATE && m.format == 32 && !s.own(m.window) => {
            let d = m.data.as_data32();
            let fs = s.atoms._NET_WM_STATE_FULLSCREEN;
            if (d[1] == fs || d[2] == fs) && s.lock().adopted.contains(&m.window) {
                embedded::net_wm_state_request(s, m.window, d[0]);
            }
        }
        Event::DestroyNotify(e) => forget(s, e.window),
        _ => {}
    }
}

/// Une fenetre disparait : on l'oublie. `current` reste (la fin du jeu est annoncee par `run_finished`).
fn forget(s: &Shared, win: u32) {
    let mut st = s.lock();
    st.origin.remove(&win);
    st.adopted.retain(|w| *w != win);
    st.refused.remove(&win);
    st.adopted_at.remove(&win);
    st.known_before.remove(&win);
    if st.game_rect.is_some_and(|(g, _, _)| g == win) {
        st.game_rect = None;
    }
}

fn subscribe(s: &Shared, win: u32) {
    let _ = s.conn.change_window_attributes(win, &ChangeWindowAttributesAux::new().event_mask(EventMask::STRUCTURE_NOTIFY | EventMask::PROPERTY_CHANGE));
    s.flush();
}

/// `win` est-elle une fenetre du jeu surveille ? Si oui, elle est adoptee. Lectures locales d'abord, jamais d'erreur sur une fenetre
/// disparue.
fn consider(s: &Shared, win: u32, how: &str) {
    let c = &s.conn;
    {
        let st = s.lock();
        if st.fell_back || st.pids.is_empty() || st.origin.contains_key(&win) {
            return;
        }
    }
    // une vraie fenetre de premier niveau (ni fille, ni menu `override-redirect`)
    let Some(attrs) = util::attrs(c, win) else { return };
    if attrs.override_redirect || attrs.class == WindowClass::INPUT_ONLY || util::parent_of(c, win) != Some(s.root) {
        return;
    }
    let pid = util::pid_of(c, &s.atoms, win);
    let (class, geo) = (util::class_name(c, win), util::geometry(c, win));
    let ours = {
        let st = s.lock();
        match pid {
            // un wrapper (script, nix, bac a sable) lance la JVM en fils : un ancetre surveille suffit
            Some(p) => st.pids.contains(&p) || util::has_ancestor(p, &st.pids),
            // sans pid, la classe ne suffit qu'une fois la fenetre AFFICHEE (ses proprietes sont alors definitives : une autre
            // application qui n'a pas encore pose son pid ne doit jamais etre prise pour le jeu)
            None => is_game_class(&class) && attrs.map_state != MapState::UNMAPPED && !st.known_before.contains(&win),
        }
    };
    // fenetre d'essai ou technique (SDL en cree une quinzaine) : jamais touchee ; une taille de jeu viendra peut-etre (`ConfigureNotify`)
    if !ours || !geo.is_some_and(|(_, _, w, h)| guard::main_sized(w, h)) || guard::is_helper_class(&class) {
        return;
    }
    embedded::adopt(s, win, how);
}

/// Balayage de la racine : fenetres creees avant `watch` (`initial`) ou rattrapage d'un evenement manque (a chaque battement lent).
pub(super) fn scan(s: &Shared, initial: bool) {
    let now = Instant::now();
    {
        let mut st = s.lock();
        if st.fell_back || st.pids.is_empty() {
            return;
        }
        if !initial && (st.current.is_some() || st.scanned_at.is_some_and(|t| now.saturating_duration_since(t) < SCAN_EVERY)) {
            return;
        }
        st.scanned_at = Some(now);
    }
    let Some(tree) = s.conn.query_tree(s.root).ok().and_then(|c| c.reply().ok()) else { return };
    if initial {
        // la classe seule n'est jamais retenue pour une fenetre deja la : elle peut etre a n'importe quelle application
        s.lock().known_before = tree.children.iter().copied().collect();
    }
    for w in tree.children {
        if s.own(w) {
            continue;
        }
        subscribe(s, w);
        consider(s, w, if initial { "balayage" } else { "balayage lent" });
    }
}
