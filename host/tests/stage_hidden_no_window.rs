//! Scene, lancement CACHE, delai de garde « aucune fenetre » : le jeu, lance cache, cree sa fenetre APRES que la Scene a renonce.
//! Plus personne ne l'afficherait (le jeu obeit a la consigne, l'hote ne surveille plus) : l'hote la guette et la MONTRE.
//! Le delai de production (4 minutes) est remplace par un delai court (`GameHost::set_guards`). Voir `common`.
#![cfg(windows)]

mod common;

use std::time::Duration;

use common::*;
use cubelink_host::{FallbackReason, GameHost, Guards};

const T: Duration = Duration::from_secs(15);

#[test]
fn fenetre_cachee_creee_apres_le_repli_est_montree() {
    let parent = Parent::new();
    let host = GameHost::embed(parent.hwnd).expect("embed");
    let events = Events::default();
    host.set_observer(events.observer());
    let (cw, ch) = parent.client_size();
    host.relayout(0, 40, cw, ch - 40);
    let curtain = curtain();
    host.set_guards(Guards { no_window: Duration::from_millis(1200), ..Guards::default() });

    host.begin("Modpack de test", "Lancement", None, None);
    host.expect_hidden(true);
    let mut game = FakeGame::spawn_hidden(&["GLFW30", "300", "late=3000"]);
    host.watch(game.pid());

    wait_until("Fallback NoWindow", T, || !events.fallbacks().is_empty());
    assert_eq!(events.fallbacks()[0].0, FallbackReason::NoWindow);
    assert!(game.window("GLFW30").is_none(), "le faux jeu n'a pas encore de fenetre");
    events.assert_logged("lancement cache : aucune fenetre de jeu a rendre");
    wait_until("rideau rouvert sur le launcher", T, || !visible(curtain));

    // la fenetre arrive, cachee ; l'hote la montre, telle que le jeu l'a creee
    wait_until("fenetre du faux jeu creee", T, || game.window("GLFW30").is_some());
    let win = game.window("GLFW30").unwrap();
    assert!(!visible(win));
    wait_until("fenetre montree par l'hote", T, || visible(win));
    wait_until("affichage journalise", T, || events.logged("montree hors de la Scene"));
    assert_given_back(win);
    pump_for(Duration::from_millis(2000));
    assert_given_back(win);
    assert!(!game.exited() && !host.staged());
    assert_eq!(events.fallbacks().len(), 1);
    host.run_finished();
}
