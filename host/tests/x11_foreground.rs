//! Premier plan X11 : `focus_own_window` ne rend vrai que si la fenetre est VRAIMENT devant (focus clavier ou `_NET_ACTIVE_WINDOW`),
//! plus « le serveur a repondu a GetInputFocus ». Sans gestionnaire (serveur nu de `xvfb-run`) puis sous un vrai (`openbox`, qui se
//! retire s'il est absent ; `CUBELINK_REQUIRE_WM=1` en CI : echec). Openbox n'est pas Mutter ni KWin. Voir `common_x11`.
#![cfg(target_os = "linux")]

mod common_x11;

use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use common_x11::*;
use cubelink_host::foreground::{bring_game_to_front, focus_own_window};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt, CreateWindowAux, WindowClass};

const T: Duration = Duration::from_secs(20);

/// Un seul serveur X pour tout le fichier et UN SEUL focus clavier : les scenarios ne tournent pas en parallele.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// Fenetre creee mais JAMAIS affichee : le gestionnaire comme le serveur refusent de lui donner le focus (BadMatch).
fn unmapped_window() -> u32 {
    let (c, root) = x();
    let w = c.generate_id().unwrap();
    c.create_window(0, w, *root, 0, 0, 200, 100, 0, WindowClass::INPUT_OUTPUT, 0, &CreateWindowAux::new()).unwrap();
    c.flush().unwrap();
    w
}

#[test]
fn sans_gestionnaire_le_verdict_est_le_focus_reel() {
    let _serial = serial();
    if !available() {
        return;
    }
    let a = Parent::new();
    let b = Parent::new();
    give_focus(a.xid);
    assert_ne!(focus(), b.xid);

    assert!(focus_own_window(b.id()), "la fenetre affichee prend le focus : vrai");
    assert_eq!(focus(), b.xid, "et elle l'a vraiment");

    // le focus reste a `b` : redemander `b` reste vrai ; redemander `a` le lui rend
    assert!(focus_own_window(b.id()));
    assert!(focus_own_window(a.id()));
    assert_eq!(focus(), a.xid);

    // NON-REGRESSION : l'ancien code rendait vrai des que le serveur repondait, quoi qu'il arrive
    assert!(!focus_own_window(0x7fff_fff0), "XID inexistant : faux");
    let hidden = unmapped_window();
    assert!(!focus_own_window(hidden as isize), "fenetre jamais affichee : le focus est refuse, donc faux");
    assert_eq!(focus(), a.xid, "le focus n'a pas bouge");
    assert!(!focus_own_window(-1), "XID invalide : faux");
    let _ = x().0.destroy_window(hidden);

    // la connexion partagee survit a ces refus (erreurs X videes) : une nouvelle demande reussit
    assert!(focus_own_window(b.id()));
    assert_eq!(focus(), b.xid);
}

#[test]
fn la_connexion_partagee_tient_de_nombreux_appels() {
    let _serial = serial();
    if !available() {
        return;
    }
    let a = Parent::new();
    let b = Parent::new();
    for i in 0..30 {
        let w = if i % 2 == 0 { &a } else { &b };
        assert!(focus_own_window(w.id()), "appel {i}");
        assert_eq!(focus(), w.xid, "appel {i}");
    }
}

#[test]
fn sous_openbox_la_fenetre_activee_est_devant() {
    let _serial = serial();
    if !available() {
        return;
    }
    let Some(_wm) = RealWm::start() else { return };
    let launcher = Parent::new();
    let other = Parent::new();
    pump_for(Duration::from_millis(400)); // le gestionnaire a cadre les fenetres
    give_focus(other.xid);
    wait_until("la fenetre voisine a le focus", T, || focus() == other.xid);

    // le gestionnaire traite `_NET_ACTIVE_WINDOW` de facon asynchrone : le verdict attend sa reponse
    assert!(focus_own_window(launcher.id()), "le gestionnaire active la fenetre demandee");
    wait_until("focus sur le launcher", T, || focus() == launcher.xid);
    assert!(!focus_own_window(0x7fff_fff0), "XID inexistant : faux meme sous un gestionnaire");
    assert!(focus_own_window(other.id()));
    assert!(focus_own_window(launcher.id()), "idempotent et reutilisable");
}

#[test]
fn bring_game_to_front_met_le_jeu_devant_sous_openbox() {
    let _serial = serial();
    if !available() {
        return;
    }
    let Some(_wm) = RealWm::start() else { return };
    let launcher = Parent::new();
    pump_for(Duration::from_millis(400));
    let game = FakeGame::spawn(&["GLFW30", "300"]);
    wait_until("fenetre du jeu affichee", T, || game.window("GLFW30").is_some_and(visible));
    let win = game.window("GLFW30").unwrap();
    give_focus(launcher.xid);
    wait_until("le launcher a le focus", T, || focus() == launcher.xid);

    bring_game_to_front(game.pid());
    wait_until("le jeu a le focus", T, || focus() == win);
}
