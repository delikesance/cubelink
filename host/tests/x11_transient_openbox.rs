//! Scene X11, chemin TRANSITOIRE sous un VRAI gestionnaire de fenetres (`openbox`, lance sur le serveur nu de `xvfb-run`). Complete
//! `x11_transient_adopt` (gestionnaire simule par une marque) avec ce qu'une marque ne peut pas dire : le gestionnaire gere la fenetre du
//! jeu (cadre, contraintes de position, `WM_STATE`), honore-t-il le garage a -32000, le decoupage XShape du panneau, la restitution.
//! Assertions volontairement sobres : un vrai gestionnaire place de facon asynchrone. Se retire sans `openbox` (`CUBELINK_REQUIRE_WM=1`
//! en CI : echec). Openbox n'est pas Mutter ni KWin : cela ne prouve rien de leur comportement. Voir `common_x11`.
#![cfg(target_os = "linux")]

mod common_x11;

use std::time::Duration;

use common_x11::*;
use cubelink_host::{FallbackReason, GameHost};

const T: Duration = Duration::from_secs(20);

#[test]
fn jeu_transitoire_sous_openbox() {
    if !available() {
        return;
    }
    let Some(_wm) = RealWm::start() else { return }; // AVANT la fenetre du launcher et l'hote (wm_present est lu une fois)
    let parent = Parent::new();
    pump_for(Duration::from_millis(400)); // le gestionnaire a pose (et cadre) la fenetre du launcher
    let host = GameHost::embed(parent.id()).expect("embed");
    let events = Events::default();
    host.set_observer(events.observer());
    let (cw, ch) = parent.client_size();
    let zone = (100, 60, cw - 200, ch - 120);
    host.relayout(zone.0, zone.1, zone.2, zone.3);
    let curtain = curtain();
    let (_, _, zw, zh) = parent.to_screen(zone);

    give_focus(parent.xid);
    host.begin("Modpack de test", "Lancement", None, None);
    let mut game = FakeGame::spawn(&["GLFW30", "500", "glfwhelper", "late=150"]);
    host.watch(game.pid());
    wait_until("evenement Adopted", T, || !events.adopted().is_empty());
    let win = game.window("GLFW30").expect("fenetre du jeu");

    // ---- adopte en transitoire : la fenetre n'est pas dans la scene, elle porte les marques du repli, geree par le gestionnaire
    assert_ne!(parent_of(win), stage(), "chemin transitoire : jamais dans la scene");
    assert_eq!(prop32_any(win, "WM_TRANSIENT_FOR"), vec![parent.xid]);
    assert_eq!(prop32_any(win, "_MOTIF_WM_HINTS"), vec![2, 0, 0, 0, 0]);
    assert!(prop32_any(win, "_NET_WM_STATE").contains(&atom_named("_NET_WM_STATE_SKIP_TASKBAR")));
    wait_until("fenetre posee sur la zone de jeu (zone cliente, cadre exclu)", T, || visible(win) && rect(win) == parent.to_screen(zone));
    assert!(visible(curtain) && !host.in_game());
    host.reveal();
    wait_until("rideau ouvert sur le jeu", T, || host.in_game() && !visible(curtain));
    assert!(on_screen(win));

    // ---- panneau : fenetre transitoire redimensionnee (largeur - panneau)
    assert!(host.set_panel(true, 150));
    wait_until("jeu redimensionne par le panneau", T, || size(win) == (zw - 150, zh));
    assert!(!bounding_shaped(win) && !input_shaped(win), "pas de rognage XShape en mode transitoire");
    assert!(host.set_panel(false, 0));
    wait_until("taille d'origine rendue", T, || size(win) == (zw, zh));

    // ---- retour au launcher : le jeu est gare (10 x 10). Openbox, comme Mutter, ramene une fenetre geree dans l'ecran : le garage
    // a -32000 n'est pas tenu a la lettre. Ce qui compte : plus de jeu sur la zone de jeu, pas un jeu visible (reste au plus un coin).
    host.set_view(false);
    wait_until("jeu gare, plus sur la zone de jeu", T, || size(win) == (10, 10));
    pump_for(Duration::from_millis(500));
    let (gx, gy, gw, gh) = rect(win);
    let visible_px = (gw.min(gx + gw).max(0)) * (gh.min(gy + gh).max(0));
    assert!(visible_px <= gw * gh / 10, "jeu gare encore visible : {:?}", rect(win));
    assert!(mapped(win) && !game.exited(), "gare, pas ferme");
    assert_eq!(host.view(), (true, false));
    host.set_view(true);
    wait_until("retour dans le jeu", T, || host.in_game() && on_screen(win) && rect(win) == parent.to_screen(zone));

    // ---- plein ecran annonce par le jeu, puis quitte (EWMH : le gestionnaire agit sur le launcher)
    game_sends_net_wm_state(win, 1);
    wait_until("plein ecran annonce", T, || events.fullscreens() == vec![true]);
    game_sends_net_wm_state(win, 0);
    wait_until("sortie annoncee", T, || events.fullscreens() == vec![true, false]);
    wait_until("jeu de nouveau sur la zone", T, || rect(win) == parent.to_screen(zone));

    // ---- repli : la fenetre est rendue (plus de marque transitoire, visible, a l'ecran)
    host.release("demande par le test");
    wait_until("evenement Fallback", T, || !events.fallbacks().is_empty());
    assert_eq!(events.fallbacks(), vec![(FallbackReason::Requested, "demande par le test".to_string())]);
    wait_until("fenetre rendue sans marque transitoire", T, || prop32_any(win, "WM_TRANSIENT_FOR").is_empty() && prop32_any(win, "_MOTIF_WM_HINTS").is_empty());
    // le gestionnaire garde la fenetre sous son cadre : la restitution ne doit PAS la reparenter vers la racine (elle serait demontee)
    wait_until("fenetre rendue affichee a l'ecran", T, || exists(win) && visible(win) && on_screen(win));
    pump_for(Duration::from_millis(500));
    assert!(visible(win) && on_screen(win), "fenetre rendue de nouveau cachee : {:?}", rect(win));
    assert!(!game.exited());
    game.kill();
}
