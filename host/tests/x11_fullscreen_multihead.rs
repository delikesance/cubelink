//! Scene X11, plein ecran et restitution avec DEUX moniteurs (RandR 1.5) : la racine couvre les deux (3840 x 1080), le launcher est
//! sur le second. Un jeu qui prend la taille de SON moniteur (1920 x 1080, pas celle de la racine) est en plein ecran ; la fenetre
//! rendue au jeu est posee dans le moniteur du launcher, jamais a cheval sur la jonction. Voir `common_x11`. Se met en retrait si le
//! serveur de test ne sait pas decrire deux moniteurs.
#![cfg(target_os = "linux")]

mod common_x11;

use std::time::Duration;

use common_x11::*;
use cubelink_host::GameHost;

const T: Duration = Duration::from_secs(15);

#[test]
fn plein_ecran_et_restitution_sur_le_moniteur_du_launcher() {
    if !available() {
        return;
    }
    let Some([_left, right]) = two_monitors(1920, 1080) else {
        return;
    };
    let parent = Parent::new();
    move_window(parent.xid, (right.0 + 300, 100)); // sur le moniteur de droite
    wait_until("launcher sur le second moniteur", T, || rect(parent.xid).0 == right.0 + 300);
    let host = GameHost::embed(parent.id()).expect("embed");
    let events = Events::default();
    host.set_observer(events.observer());
    let (cw, ch) = parent.client_size();
    let zone = (0, 40, cw, ch - 40);
    host.relayout(zone.0, zone.1, zone.2, zone.3);
    let curtain = curtain();

    host.begin("Modpack de test", "Lancement", None, None);
    let grab = format!("grab={},{},{},{}", right.0, right.1, right.2, right.3);
    let mut game = FakeGame::spawn(&["SDL_app", "300", &grab]);
    host.watch(game.pid());
    wait_until("Adopted", T, || !events.adopted().is_empty());
    let win = game.window("SDL_app").unwrap();
    host.reveal();
    wait_until("dans le jeu", T, || host.in_game() && !visible(curtain) && rect(win) == parent.to_screen(zone));

    // le jeu prend tout SON moniteur (1920 x 1080 sur un bureau de 3840 x 1080) : annonce, puis fenetre ramenee sur la zone
    game_grabs_screen(win);
    wait_until("plein ecran annonce", T, || events.fullscreens() == vec![true]);
    wait_until("fenetre du jeu ramenee sur la zone", T, || rect(win) == parent.to_screen(zone));

    // il en sort : annonce, une seule fois chacune
    game_leaves_screen(win);
    wait_until("sortie du plein ecran annoncee", T, || events.fullscreens() == vec![true, false]);
    wait_until("fenetre du jeu de nouveau sur la zone", T, || rect(win) == parent.to_screen(zone));
    pump_for(Duration::from_millis(500));
    assert_eq!(events.fullscreens(), vec![true, false]);
    assert!(events.fallbacks().is_empty());

    // l'hote s'arrete : la fenetre rendue est dans le moniteur du launcher, pas centree sur la jonction des deux ecrans
    drop(host);
    pump_for(Duration::from_millis(300));
    assert_given_back(win);
    let (x, y, w, h) = rect(win);
    assert!(x >= right.0 && x + w <= right.0 + right.2 && y >= right.1 && y + h <= right.1 + right.3, "fenetre rendue hors du moniteur du launcher : {:?}", (x, y, w, h));
    assert!(!game.exited(), "le jeu survit a l'arret de l'hote");
}
