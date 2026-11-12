//! Scene, lancement CACHE (`GameHost::expect_hidden`) : le jeu est demarre avec la consigne « fenetre cachee » et, comme GLFW 3.4,
//! n'affiche donc jamais sa fenetre lui-meme. L'hote la tient pour LA fenetre du jeu sans evenement d'affichage et la montre a
//! la pose : elle ne se voit JAMAIS ailleurs que sur la zone de jeu, sans cadre. Puis un jeu qui ignore la consigne et s'affiche
//! lui-meme, toujours avec `expect_hidden(true)` : le chemin habituel, sans double traitement. Voir `common`.
#![cfg(windows)]

mod common;

use std::time::Duration;

use common::*;
use cubelink_host::GameHost;
use windows_sys::Win32::UI::WindowsAndMessaging::WS_EX_TOOLWINDOW;

const T: Duration = Duration::from_secs(15);
const PROMOTED: &str = "tenue pour affichee";

#[test]
fn fenetre_jamais_affichee_par_le_jeu() {
    let parent = Parent::new();
    let host = GameHost::embed(parent.hwnd).expect("embed");
    let events = Events::default();
    host.set_observer(events.observer());
    let (cw, ch) = parent.client_size();
    let zone = (0, 40, cw, ch - 40);
    host.relayout(zone.0, zone.1, zone.2, zone.3);
    let curtain = curtain();

    // ---- le jeu obeit a la consigne : jamais visible avant la pose, puis visible sur la zone
    host.begin("Modpack de test", "Lancement", None, None);
    host.expect_hidden(true);
    let mut game = FakeGame::spawn_hidden(&["GLFW30", "300"]);
    let sampler = Sampler::strict(game.pid(), "GLFW30", parent.to_screen(zone));
    host.watch(game.pid());
    wait_until("fenetre posee sur la zone de jeu", T, || game.window("GLFW30").is_some_and(|w| visible(w) && rect(w) == parent.to_screen(zone)));
    let win = game.window("GLFW30").unwrap();
    assert!(!framed(win) && owner(win) == parent.hwnd, "style {:#x}", style(win));
    assert_ne!(exstyle(win) & WS_EX_TOOLWINDOW, 0, "fenetre adoptee presente dans la barre des taches");
    assert!(visible(curtain) && above(curtain, win), "posee SOUS le rideau ferme");
    wait_until("evenement Adopted", T, || !events.adopted().is_empty());
    events.assert_logged(PROMOTED);
    assert!(host.is_ready(Duration::from_secs(1), Duration::from_secs(90)) || !host.in_game());
    host.reveal();
    wait_until("rideau ouvert sur le jeu", T, || host.in_game() && !visible(curtain));
    pump_for(Duration::from_millis(500));
    let (samples, seen) = sampler.stop();
    assert!(samples > 200, "echantillonnage trop maigre pour conclure : {samples}");
    assert!(seen.is_empty(), "fenetre du jeu vue ailleurs que posee sur la zone ({samples} echantillons) :\n{}", seen.join("\n"));
    assert_eq!(visible_windows_of(game.pid()), vec![win], "une seule fenetre visible : celle du jeu");
    host.request_close();
    wait_until("le faux jeu s'arrete", T, || game.exited());
    host.run_finished();
    wait_until("retour au launcher", T, || host.view() == (false, false) && !visible(curtain));
    assert_eq!(events.adopted(), vec!["GLFW30".to_string()]);

    // ---- le jeu IGNORE la consigne (GLFW 3.3, SDL, LWJGL 2) et s'affiche lui-meme : chemin habituel, inchange
    let promoted_before = events.logs().iter().filter(|l| l.contains(PROMOTED)).count();
    host.begin("Vieux modpack", "Lancement", None, None);
    host.expect_hidden(true);
    let mut old = FakeGame::spawn_hidden(&["LWJGL", "200", "nostartup"]);
    let sampler = Sampler::start(old.pid(), "LWJGL");
    host.watch(old.pid());
    wait_until("second Adopted", T, || events.adopted().len() == 2);
    let win2 = old.window("LWJGL").unwrap();
    wait_until("seconde fenetre posee", T, || visible(win2) && rect(win2) == parent.to_screen(zone));
    assert!(!framed(win2) && owner(win2) == parent.hwnd);
    host.reveal();
    wait_until("rideau ouvert sur le second jeu", T, || host.in_game() && !visible(curtain));
    pump_for(Duration::from_millis(800)); // plus que le delai de la reconnaissance sans affichage
    let (_, seen) = sampler.stop();
    assert!(seen.is_empty(), "fenetre de jeu autonome apercue :\n{}", seen.join("\n"));
    assert_eq!(events.logs().iter().filter(|l| l.contains(PROMOTED)).count(), promoted_before, "le jeu s'est affiche lui-meme : pas de reconnaissance sans affichage");
    assert_eq!(events.adopted().len(), 2, "Adopted une fois par lancement");
    host.request_close();
    wait_until("le second faux jeu s'arrete", T, || old.exited());
    host.run_finished();
    assert!(events.fallbacks().is_empty(), "aucun repli : {:?}", events.fallbacks());
}
