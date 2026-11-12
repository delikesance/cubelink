//! Scene, lancement CACHE d'un jeu qui ne repond pas : le gros modpack cree sa fenetre puis ne pompe plus ses messages pendant
//! le chargement (plus long que le delai de garde du rattachement, delais de PRODUCTION). Lance cache, il n'affiche rien : pendant
//! tout le silence le jeu n'a AUCUNE fenetre visible (donc ni fenetre autonome ni bouton dans la barre des taches), et aucun
//! repli. Quand il repond, l'adoption se termine et l'hote pose la fenetre, qu'il affiche lui-meme. Voir `common`.
#![cfg(windows)]

mod common;

use std::time::{Duration, Instant};

use common::*;
use cubelink_host::guard::ADOPTION_GUARD;
use cubelink_host::GameHost;

const T: Duration = Duration::from_secs(15);
const STALL: Duration = Duration::from_millis(12_000);

#[test]
fn jeu_cache_qui_ne_repond_pas() {
    assert!(STALL > ADOPTION_GUARD);
    let parent = Parent::new();
    let host = GameHost::embed(parent.hwnd).expect("embed");
    let events = Events::default();
    host.set_observer(events.observer());
    let (cw, ch) = parent.client_size();
    let zone = (0, 40, cw, ch - 40);
    host.relayout(zone.0, zone.1, zone.2, zone.3);
    let curtain = curtain();

    host.begin("Gros modpack", "Lancement", None, None);
    host.expect_hidden(true);
    let mut game = FakeGame::spawn_hidden(&["GLFW30", "300", "glfwhelper", "stall=12000"]);
    let sampler = Sampler::strict(game.pid(), "GLFW30", parent.to_screen(zone));
    host.watch(game.pid());
    wait_until("fenetre du faux jeu creee", T, || game.window("GLFW30").is_some());
    let t0 = Instant::now();
    let win = game.window("GLFW30").unwrap();

    // pendant le silence : rien de visible, rideau ferme, aucun repli
    let mut checks = 0;
    while t0.elapsed() < STALL - Duration::from_millis(1500) {
        assert_eq!(visible_windows_of(game.pid()), Vec::<isize>::new(), "fenetre du jeu visible pendant le chargement, a {:?}", t0.elapsed());
        assert!(visible(curtain) && host.staged());
        assert!(events.fallbacks().is_empty(), "repli a tort a {:?} : {:?}", t0.elapsed(), events.fallbacks());
        pump_for(Duration::from_millis(150));
        checks += 1;
    }
    assert!(checks > 30 && t0.elapsed() > ADOPTION_GUARD);
    assert!(events.adopted().is_empty());

    // le jeu repond : adoption, puis pose par l'hote (le jeu n'affichera jamais sa fenetre)
    wait_until("fenetre posee sur la zone de jeu", T, || visible(win) && rect(win) == parent.to_screen(zone));
    assert!(owner(win) == parent.hwnd && !framed(win));
    wait_until("Adopted", T, || !events.adopted().is_empty());
    host.reveal();
    wait_until("dans le jeu", T, || host.in_game() && !visible(curtain));
    pump_for(Duration::from_millis(500));
    let (samples, seen) = sampler.stop();
    assert!(samples > 2000, "echantillonnage trop maigre pour conclure : {samples}");
    assert!(seen.is_empty(), "fenetre du jeu vue ailleurs que posee sur la zone ({samples} echantillons) :\n{}", seen.join("\n"));
    assert_eq!(visible_windows_of(game.pid()), vec![win]);
    assert!(events.fallbacks().is_empty(), "aucun repli : {:?}", events.fallbacks());
    assert!(!events.logged("rattachement refuse"), "un fil qui ne repond pas n'est pas un refus :\n{}", events.logs().join("\n"));
    assert!(!events.logged("affichee avant son rattachement"), "rien n'a ete affiche avant le rattachement :\n{}", events.logs().join("\n"));

    host.request_close();
    wait_until("le faux jeu s'arrete", T, || game.exited());
    host.run_finished();
}
