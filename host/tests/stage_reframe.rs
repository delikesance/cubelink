//! Scene, jeu qui remet son cadre : pendant son demarrage un jeu repose le cadre de sa fenetre et l'agrandit autour de sa zone
//! cliente (mesure : NeoForge, ~185 ms apres l'adoption, fenetre deja affichee). Tant que la fenetre n'a pas ete posee, elle ne
//! doit JAMAIS se voir a l'ecran avec ce cadre : la premiere pose attend que la fenetre soit calme, le cadre est retire la ou elle
//! est garee, hors ecran, et seulement ensuite elle est posee. Une fois posee, si le jeu recommence (F11), elle revient sur la
//! zone, donc sous le rideau, puis reperd son cadre. Voir `common`.
#![cfg(windows)]

mod common;

use std::time::Duration;

use common::*;
use cubelink_host::GameHost;

const T: Duration = Duration::from_secs(15);

#[test]
fn cadre_remis_par_le_jeu_jamais_vu_avant_la_pose() {
    let parent = Parent::new();
    let host = GameHost::embed(parent.hwnd).expect("embed");
    let events = Events::default();
    host.set_observer(events.observer());
    let (cw, ch) = parent.client_size();
    let zone = (0, 40, cw, ch - 40);
    host.relayout(zone.0, zone.1, zone.2, zone.3);
    let curtain = curtain();

    // le faux jeu affiche sa fenetre 300 ms apres l'avoir creee, puis lui remet son cadre 200 ms plus tard
    host.begin("Modpack de test", "Lancement", None, None);
    let mut game = FakeGame::spawn(&["GLFW30", "300", "reframe=500"]);
    let sampler = Sampler::start(game.pid(), "GLFW30");
    host.watch(game.pid());
    wait_until("fenetre adoptee", T, || game.window("GLFW30").is_some_and(|w| owner(w) == parent.hwnd && !framed(w)));
    let win = game.window("GLFW30").unwrap();

    // posee sur la zone, SANS cadre, apres que le jeu a remis le sien (hors ecran)
    wait_until("fenetre posee sur la zone de jeu", T, || visible(win) && rect(win) == parent.to_screen(zone));
    assert!(!framed(win), "posee avec son cadre : style {:#x}", style(win));
    events.assert_logged("cadre de la fenetre remis par le jeu");
    pump_for(Duration::from_millis(600));
    let (samples, seen) = sampler.stop();
    assert!(samples > 200, "echantillonnage trop maigre pour conclure : {samples}");
    assert!(seen.is_empty(), "fenetre vue a l'ecran avec son cadre avant ou pendant sa pose ({samples} echantillons) :\n{}", seen.join("\n"));
    assert_eq!(rect(win), parent.to_screen(zone));
    assert!(visible(curtain) && owner(win) == parent.hwnd && !framed(win));

    // deja posee, le jeu recommence : elle revient sur la zone (sous le rideau, toujours ferme) et reperd son cadre
    game_reframes(win);
    wait_until("fenetre revenue sur la zone, sans cadre", T, || rect(win) == parent.to_screen(zone) && !framed(win));
    assert!(visible(curtain) && above(curtain, win), "le rideau couvre toujours la fenetre du jeu");

    host.reveal();
    wait_until("dans le jeu", T, || host.in_game() && !visible(curtain));
    host.request_close();
    wait_until("le faux jeu s'arrete", T, || game.exited());
    host.run_finished();
    assert!(events.fallbacks().is_empty(), "aucun repli : {:?}", events.fallbacks());
}
