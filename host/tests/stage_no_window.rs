//! Scene, delai de garde : un jeu qui tourne mais n'affiche jamais de fenetre ne laisse pas le rideau ferme sur le launcher.
//! Le delai de production (4 minutes) est remplace par un delai court (`GameHost::set_guards`). Voir `common`.
#![cfg(windows)]

mod common;

use std::time::{Duration, Instant};

use common::*;
use cubelink_host::{FallbackReason, GameHost, Guards};

const T: Duration = Duration::from_secs(15);

#[test]
fn aucune_fenetre_le_rideau_se_rouvre() {
    let parent = Parent::new();
    let host = GameHost::embed(parent.hwnd).expect("embed");
    let events = Events::default();
    host.set_observer(events.observer());
    let (cw, ch) = parent.client_size();
    host.relayout(0, 40, cw, ch - 40);
    let curtain = curtain();
    host.set_guards(Guards { no_window: Duration::from_millis(1500), ..Guards::default() });

    host.begin("Modpack de test", "Lancement", None, None);
    let mut game = FakeGame::spawn(&["none"]);
    let t0 = Instant::now();
    host.watch(game.pid());

    // avant le delai : rideau ferme, lancement toujours dans la Scene
    pump_for(Duration::from_millis(700));
    assert!(events.fallbacks().is_empty(), "repli premature : {:?}", events.fallbacks());
    assert!(visible(curtain) && host.staged());
    assert!(!host.is_ready(Duration::from_secs(1), Duration::from_secs(90)));

    // apres : repli « aucune fenetre », le rideau se rouvre sur le launcher
    wait_until("Fallback NoWindow", T, || !events.fallbacks().is_empty());
    assert!(t0.elapsed() >= Duration::from_millis(1500), "repli avant le delai de garde : {:?}", t0.elapsed());
    let fb = events.fallbacks();
    assert_eq!(fb.len(), 1);
    assert_eq!(fb[0].0, FallbackReason::NoWindow, "{fb:?}");
    assert!(!host.staged());
    assert_eq!(host.view(), (false, false));
    wait_until("rideau rouvert sur le launcher", T, || !visible(curtain));
    assert!(!game.exited(), "le jeu n'est jamais tue par un repli");
    events.assert_logged("repli decide (no_window)");

    // la suite du deroulement normal du launcher est sans effet, et rien n'est emis deux fois
    assert!(host.is_ready(Duration::from_secs(1), Duration::from_secs(90)));
    host.reveal();
    host.run_finished();
    pump_for(Duration::from_millis(500));
    assert!(!visible(curtain));
    assert_eq!(events.fallbacks().len(), 1);
}
