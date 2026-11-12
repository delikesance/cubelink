//! Scene, lancement CACHE, garantie « jamais de jeu invisible » : quand la Scene renonce, la fenetre du jeu lui est rendue
//! VISIBLE meme s'il ne l'a jamais affichee (il ne le fera pas : il a ete lance cache). Ici le renoncement est demande
//! (`release`) pendant que le jeu charge, avant que sa fenetre ait ete reconnue ni posee. Voir `common`.
#![cfg(windows)]

mod common;

use std::time::Duration;

use common::*;
use cubelink_host::{FallbackReason, GameHost};

const T: Duration = Duration::from_secs(15);

#[test]
fn fenetre_cachee_rendue_visible() {
    let parent = Parent::new();
    let host = GameHost::embed(parent.hwnd).expect("embed");
    let events = Events::default();
    host.set_observer(events.observer());
    let (cw, ch) = parent.client_size();
    host.relayout(0, 40, cw, ch - 40);
    let curtain = curtain();

    host.begin("Modpack de test", "Lancement", None, None);
    host.expect_hidden(true);
    // le jeu ne repond pas pendant 3 s apres avoir cree sa fenetre : elle n'est ni adoptee ni reconnue quand la Scene renonce
    let mut game = FakeGame::spawn_hidden(&["GLFW30", "300", "stall=3000"]);
    host.watch(game.pid());
    wait_until("fenetre du faux jeu creee", T, || game.window("GLFW30").is_some());
    let win = game.window("GLFW30").unwrap();
    pump_for(Duration::from_millis(800));
    assert!(!visible(win) && events.adopted().is_empty() && !events.logged("tenue pour affichee"));

    host.release("essai");
    wait_until("Fallback Requested", T, || !events.fallbacks().is_empty());
    assert_eq!(events.fallbacks()[0].0, FallbackReason::Requested);
    assert_given_back(win);
    assert_eq!(visible_windows_of(game.pid()), vec![win]);
    wait_until("rideau rouvert sur le launcher", T, || !visible(curtain));
    // elle le reste (le jeu, en retard, traite encore ses messages)
    pump_for(Duration::from_millis(2500));
    assert_given_back(win);
    assert!(!game.exited() && !host.staged());
    assert_eq!(events.fallbacks().len(), 1);
}
