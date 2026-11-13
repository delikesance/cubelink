//! Scene X11 : ICCCM 4.1.5. La fenetre du jeu reparentee n'a plus de gestionnaire de fenetres ; l'hote lui envoie le
//! `ConfigureNotify` SYNTHETIQUE (coordonnees absolues en racine) que SDL 3 (`SDL_SyncWindow`) et GLFW attendent apres chaque
//! deplacement ou redimensionnement, et quand seule sa position absolue change (launcher deplace). Voir `common_x11`.
#![cfg(target_os = "linux")]

mod common_x11;

use std::time::Duration;

use common_x11::*;
use cubelink_host::GameHost;

const T: Duration = Duration::from_secs(20);

#[test]
fn le_jeu_recoit_les_configure_notify_synthetiques() {
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
    host.begin("Modpack de test", "Lancement", None, None);
    // comme SDL : trois demandes de deplacement + redimensionnement, chacune attend sa reponse synthetique
    let game = FakeGame::spawn(&["SDL_app", "300", "syncwait", "syncmax=3"]);
    host.watch(game.pid());
    wait_until("fenetre posee sur la zone de jeu", T, || game.window("SDL_app").is_some_and(|w| visible(w) && rect(w) == parent.to_screen(zone)));
    let win = game.window("SDL_app").unwrap();

    // ---- chaque demande du jeu est suivie d'un ConfigureNotify synthetique : aucun echec de synchronisation
    wait_until("trois synchronisations reussies", T, || card1(win, "_FAKE_GAME_SYNC_OK") >= 3);
    assert_eq!(card1(win, "_FAKE_GAME_SYNC_FAIL"), 0, "le jeu n'a recu aucune reponse a une de ses demandes");
    wait_until("jeu recale apres ses demandes", T, || rect(win) == parent.to_screen(zone));
    wait_until("derniere geometrie annoncee = geometrie reelle", T, || {
        let r = rect(win);
        card_prop(win, "_FAKE_GAME_CN").get(..4) == Some(&[r.0 as u32, r.1 as u32, r.2 as u32, r.3 as u32][..])
    });
    // ---- proprietes de « fenetre geree » (lues par SDL / GLFW)
    assert_eq!(card_prop(win, "_NET_FRAME_EXTENTS"), vec![0, 0, 0, 0], "pas de cadre");
    assert_eq!(card_prop_typed(win, "WM_STATE", "WM_STATE"), vec![1, 0], "WM_STATE Normal");

    // ---- le launcher se deplace SANS que le jeu ait rien demande : sa position absolue change, pas sa position dans la scene
    let before = card_prop(win, "_FAKE_GAME_CN")[4];
    let old = rect(win);
    move_window(parent.xid, (260, 170));
    wait_until("fenetre du jeu suivie", T, || rect(win) != old && rect(win) == parent.to_screen(zone));
    wait_until("ConfigureNotify synthetique apres le deplacement du launcher", T, || {
        let (cn, r) = (card_prop(win, "_FAKE_GAME_CN"), rect(win));
        cn[4] > before && cn[..4] == [r.0 as u32, r.1 as u32, r.2 as u32, r.3 as u32]
    });
    assert_eq!(card1(win, "_FAKE_GAME_SYNC_FAIL"), 0);

    // ---- filet de securite : fenetre affichee depuis plus que `max`, le jeu parle encore : ouverture forcee, journalisee avec sa raison
    assert!(!host.is_ready(Duration::from_secs(3600), Duration::from_secs(3600)), "pas pret : ni quietude ni delai atteints");
    assert!(!events.logged("ouverture forcee"));
    assert!(host.is_ready(Duration::from_secs(3600), Duration::from_millis(50)), "delai maximal depasse : le rideau doit s'ouvrir");
    events.assert_logged("ouverture forcee du rideau");
    assert!(events.fallbacks().is_empty(), "aucun repli attendu : {:?}", events.fallbacks());
}
