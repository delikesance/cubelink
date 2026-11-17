//! Scene X11, plein ecran : un jeu qui prend de lui-meme la taille de l'ecran (F11) est ramene sur la zone de jeu, et le launcher en
//! est informe (`HostEvent::Fullscreen`) pour passer en plein ecran lui-meme ; la sortie est annoncee de meme. Voir `common_x11`.
#![cfg(target_os = "linux")]

mod common_x11;

use std::time::Duration;

use common_x11::*;
use cubelink_host::GameHost;

const T: Duration = Duration::from_secs(15);

#[test]
fn plein_ecran_annonce_et_fenetre_ramenee_sur_la_zone() {
    if !available() {
        return;
    }
    let parent = Parent::new();
    let host = GameHost::embed(parent.id()).expect("embed");
    let events = Events::default();
    host.set_observer(events.observer());
    let (cw, ch) = parent.client_size();
    let zone = (0, 40, cw, ch - 40);
    host.relayout(zone.0, zone.1, zone.2, zone.3);
    let curtain = curtain();

    host.begin("Modpack de test", "Lancement", None, None);
    let mut game = FakeGame::spawn(&["SDL_app", "300"]);
    host.watch(game.pid());
    wait_until("Adopted", T, || !events.adopted().is_empty());
    let win = game.window("SDL_app").unwrap();
    host.reveal();
    wait_until("dans le jeu", T, || host.in_game() && !visible(curtain) && rect(win) == parent.to_screen(zone));

    // le jeu occupe de lui-meme tout l'ecran : annonce, puis fenetre ramenee sur la zone
    game_grabs_screen(win);
    wait_until("plein ecran annonce", T, || events.fullscreens() == vec![true]);
    wait_until("fenetre du jeu ramenee sur la zone", T, || rect(win) == parent.to_screen(zone));
    assert!(visible(win) && host.in_game());

    // il en sort : annonce, sans autre bascule
    game_leaves_screen(win);
    wait_until("sortie du plein ecran annoncee", T, || events.fullscreens() == vec![true, false]);
    wait_until("fenetre du jeu de nouveau sur la zone", T, || rect(win) == parent.to_screen(zone));
    pump_for(Duration::from_millis(500));
    assert_eq!(events.fullscreens(), vec![true, false], "chaque changement est annonce une seule fois");
    assert!(events.fallbacks().is_empty());

    // l'hote s'arrete alors que le jeu est encore dans la scene : il est rendu au bureau, jamais detruit avec elle
    drop(host);
    pump_for(Duration::from_millis(300));
    assert_given_back(win);
    assert!(!game.exited(), "le jeu survit a l'arret de l'hote");
}
