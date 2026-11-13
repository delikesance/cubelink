//! Scene X11 : le rideau ne bloque jamais le joueur. Region d'entree vide (la souris le traverse), il ne couvre QUE la zone de jeu,
//! disparait quand le launcher est reduit ou n'est plus la fenetre active (autre application), revient avec lui, et est retire a
//! l'ouverture. Un gestionnaire de fenetres est SIMULE par la seule propriete `_NET_SUPPORTING_WM_CHECK` de la racine (le focus a
//! alors un sens) ; aucune entree souris / clavier reelle. Voir `common_x11`.
#![cfg(target_os = "linux")]

mod common_x11;

use std::time::Duration;

use common_x11::*;
use cubelink_host::GameHost;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::wrapper::ConnectionExt as _;

const T: Duration = Duration::from_secs(15);

/// Pose `_NET_SUPPORTING_WM_CHECK` sur la racine et la retire a la fin, meme en cas d'echec.
struct FakeWmMark;

impl FakeWmMark {
    fn set(check_window: u32) -> FakeWmMark {
        let (c, root) = x();
        let a = c.intern_atom(false, b"_NET_SUPPORTING_WM_CHECK").unwrap().reply().unwrap().atom;
        c.change_property32(PropMode::REPLACE, *root, a, AtomEnum::WINDOW, &[check_window]).unwrap();
        c.flush().unwrap();
        FakeWmMark
    }
}

impl Drop for FakeWmMark {
    fn drop(&mut self) {
        let (c, root) = x();
        if let Ok(a) = c.intern_atom(false, b"_NET_SUPPORTING_WM_CHECK").unwrap().reply() {
            let _ = c.delete_property(*root, a.atom);
        }
        let _ = c.flush();
    }
}

fn give_focus(w: u32) {
    let (c, _) = x();
    c.set_input_focus(InputFocus::PARENT, w, x11rb::CURRENT_TIME).unwrap();
    c.flush().unwrap();
}

#[test]
fn le_rideau_ne_bloque_jamais() {
    if !available() {
        return;
    }
    let parent = Parent::new();
    let _wm = FakeWmMark::set(parent.xid);
    let host = GameHost::embed(parent.id()).expect("embed");
    let (cw, ch) = parent.client_size();
    let zone = (100, 60, cw - 200, ch - 120); // PAS toute la fenetre : de quoi verifier qu'il ne deborde pas
    host.relayout(zone.0, zone.1, zone.2, zone.3);
    let curtain = curtain();

    // la region d'entree est vide : l'extension SHAPE l'a prise
    assert!(input_shape(curtain).is_empty(), "region d'entree non vide : le rideau capterait la souris : {:?}", input_shape(curtain));

    // ---- lancement, launcher au premier plan : le rideau couvre la zone de jeu, et elle seule
    give_focus(parent.xid);
    host.begin("Modpack de test", "Lancement", None, None);
    wait_until("rideau ferme", T, || visible(curtain));
    assert_eq!(rect(curtain), parent.to_screen(zone), "le rideau ne couvre que la zone de jeu");
    let (zx, zy, zw, zh) = parent.to_screen(zone);
    // la souris le traverse : au centre de la zone, la fenetre atteinte est celle du launcher (ou sa scene), jamais le rideau
    let under = window_under_pointer((zx + zw / 2) as i16, (zy + zh / 2) as i16);
    assert_ne!(under, curtain, "le rideau capte la souris");
    assert_eq!(under, parent.xid, "la souris atteint le launcher");

    // ---- une autre application prend le focus (Alt+Tab) : le rideau disparait, jamais par-dessus elle
    let (c, root) = x();
    let other = c.generate_id().unwrap();
    c.create_window(0, other, *root, 5, 5, 300, 200, 0, WindowClass::INPUT_OUTPUT, 0, &CreateWindowAux::new()).unwrap();
    c.map_window(other).unwrap();
    c.flush().unwrap();
    give_focus(other);
    wait_until("rideau retire : le launcher n'est plus la fenetre active", T, || !visible(curtain));
    assert!(!mapped(curtain));
    // ---- retour au launcher : le rideau revient
    give_focus(parent.xid);
    wait_until("rideau de retour avec le launcher", T, || visible(curtain));

    // ---- launcher reduit (non affiche) : le rideau disparait, et revient quand il est de nouveau affiche
    c.unmap_window(parent.xid).unwrap();
    c.flush().unwrap();
    wait_until("rideau retire : launcher reduit", T, || !visible(curtain));
    c.map_window(parent.xid).unwrap();
    c.flush().unwrap();
    wait_until("launcher de nouveau affiche", T, || visible(parent.xid));
    give_focus(parent.xid);
    wait_until("rideau de retour", T, || visible(curtain));

    // ---- ouverture : le rideau est retire pour de bon, meme si le launcher perd puis reprend le focus
    host.reveal();
    wait_until("rideau retire a l'ouverture", T, || !mapped(curtain));
    give_focus(other);
    give_focus(parent.xid);
    pump_for(Duration::from_millis(700));
    assert!(!mapped(curtain), "le rideau ne doit jamais revenir une fois ouvert");
    let _ = c.destroy_window(other);
    let _ = c.flush();
}
