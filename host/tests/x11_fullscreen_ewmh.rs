//! Scene X11, plein ecran EWMH : un vrai GLFW / SDL (F11) envoie `_NET_WM_STATE` (ajout / retrait / bascule de
//! `_NET_WM_STATE_FULLSCREEN`) a la RACINE ; la fenetre du jeu etant reparentee, le gestionnaire de fenetres l'ignore et c'est l'hote
//! (`SubstructureNotify` sur la racine) qui le recoit : `HostEvent::Fullscreen`, launcher mis en plein ecran (message client a la racine,
//! note ici par un faux gestionnaire), etat pose sur la fenetre du jeu, puis tout rendu a la sortie et a la fin du jeu.
//! Non verifiable sans vrai gestionnaire : que la fenetre du launcher prenne reellement la taille de l'ecran.
#![cfg(target_os = "linux")]

mod common_x11;

use std::time::Duration;

use common_x11::*;
use cubelink_host::GameHost;

const T: Duration = Duration::from_secs(15);

#[test]
fn plein_ecran_ewmh_launcher_et_jeu() {
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

    // le faux gestionnaire n'arrive qu'ici : la fenetre du jeu s'est affichee seule (sinon son `MapRequest` serait capte)
    let wm = FakeWm::start();
    let fs = fullscreen_atom();
    assert!(!net_wm_state(parent.xid).contains(&fs));

    // F11 : ajout
    game_sends_net_wm_state(win, 1);
    wait_until("plein ecran annonce", T, || events.fullscreens() == vec![true]);
    wait_until("launcher demande en plein ecran", T, || wm.requests_for(parent.xid) == vec![(parent.xid, 1, fs)]);
    wait_until("etat pose sur la fenetre du jeu", T, || net_wm_state(win).contains(&fs));
    assert!(visible(win) && host.in_game());
    // sans gestionnaire la zone ne bouge pas : la fenetre du jeu reste calee dessus
    wait_until("fenetre du jeu sur la zone", T, || rect(win) == parent.to_screen(zone));

    // un second ajout ne change rien (ni annonce, ni nouvelle demande)
    game_sends_net_wm_state(win, 1);
    pump_for(Duration::from_millis(300));
    assert_eq!(events.fullscreens(), vec![true]);
    assert_eq!(wm.requests_for(parent.xid).len(), 1);

    // F11 de nouveau : bascule -> sortie
    game_sends_net_wm_state(win, 2);
    wait_until("sortie annoncee", T, || events.fullscreens() == vec![true, false]);
    wait_until("launcher rendu", T, || wm.requests_for(parent.xid) == vec![(parent.xid, 1, fs), (parent.xid, 0, fs)]);
    wait_until("etat retire de la fenetre du jeu", T, || !net_wm_state(win).contains(&fs));

    // bascule -> entree, puis le jeu se termine alors que le launcher est en plein ecran : rendu quand meme
    game_sends_net_wm_state(win, 2);
    wait_until("nouvelle entree", T, || events.fullscreens() == vec![true, false, true]);
    wait_until("launcher redemande", T, || wm.requests_for(parent.xid).len() == 3);
    game.kill();
    host.run_finished();
    wait_until("launcher rendu a la fin du jeu", T, || wm.requests_for(parent.xid).len() == 4 && events.fullscreens() == vec![true, false, true, false]);
    assert_eq!(wm.requests_for(parent.xid)[3], (parent.xid, 0, fs));

    // aucune fuite : plus aucun message apres coup, ni a l'arret de l'hote
    drop(host);
    pump_for(Duration::from_millis(500));
    assert_eq!(wm.requests_for(parent.xid).len(), 4, "pas de message en trop apres la fin du jeu et l'arret de l'hote");
    assert_eq!(events.fullscreens(), vec![true, false, true, false]);
    assert!(events.fallbacks().is_empty());
    wm.stop();
}
