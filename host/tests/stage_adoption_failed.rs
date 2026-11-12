//! Scene, rattachement refuse : un jeu qui defait le rattachement de sa fenetre (faux jeu `disown`) ne reste pas gare hors ecran,
//! invisible. Apres le delai de garde (raccourci ici) sa fenetre lui est rendue. Voir `common`.
#![cfg(windows)]

mod common;

use std::time::Duration;

use common::*;
use cubelink_host::{FallbackReason, GameHost, Guards};

const T: Duration = Duration::from_secs(20);

#[test]
fn rattachement_refuse_la_fenetre_est_rendue() {
    let parent = Parent::new();
    let host = GameHost::embed(parent.hwnd).expect("embed");
    let events = Events::default();
    host.set_observer(events.observer());
    let (cw, ch) = parent.client_size();
    host.relayout(0, 40, cw, ch - 40);
    let curtain = curtain();
    host.set_guards(Guards { adoption: Duration::from_millis(3000), ..Guards::default() });

    host.begin("Modpack de test", "Lancement", None, None);
    let mut game = FakeGame::spawn(&["GLFW30", "300", "disown"]);
    host.watch(game.pid());

    // Affichee par le jeu mais pas rattachee : garee hors ecran. Si le jeu la ramene de lui-meme a l'ecran (plein ecran) avant
    // d'etre pose, elle est regaree : une fenetre prise en charge ne se voit jamais hors de la zone de jeu.
    wait_until("fenetre affichee par le jeu, garee", T, || game.window("GLFW30").is_some_and(|w| visible(w) && !on_screen(w)));
    let parked = game.window("GLFW30").unwrap();
    game_grabs_screen(parked);
    wait_until("fenetre regaree hors ecran", T, || !on_screen(parked));
    assert!(events.fallbacks().is_empty(), "repli avant le delai de garde : {:?}", events.fallbacks());

    wait_until("Fallback AdoptionFailed", T, || !events.fallbacks().is_empty());
    let fb = events.fallbacks();
    assert_eq!(fb.len(), 1);
    assert_eq!(fb[0].0, FallbackReason::AdoptionFailed, "{fb:?}");
    let win = game.window("GLFW30").expect("fenetre du faux jeu");
    assert_given_back(win);
    assert!(!host.staged());
    assert!(events.adopted().is_empty(), "jamais d'Adopted pour une fenetre refusee : {:?}", events.adopted());
    events.assert_logged("rattachement refuse");
    events.assert_logged("repli decide (adoption_failed)");

    wait_until("rideau rouvert sur le launcher", T, || !visible(curtain));
    pump_for(Duration::from_millis(2000));
    assert_given_back(win);
    assert!(!game.exited(), "le jeu n'est jamais tue par un repli");
    assert_eq!(events.fallbacks().len(), 1);
}
