//! Scene X11, jeu qui ne repond pas : un gros modpack AFFICHE sa fenetre puis ne pompe plus ses evenements pendant le chargement des
//! mods (faux jeu `stall=`). Sous X11 le reparentage est fait par le SERVEUR : un client muet est adopte et pose quand meme, et son
//! silence n'est jamais pris pour un refus (aucun repli, meme au-dela du delai de garde du rattachement, delais de PRODUCTION).
//! Voir `common_x11`.
#![cfg(target_os = "linux")]

mod common_x11;

use std::time::{Duration, Instant};

use common_x11::*;
use cubelink_host::guard::ADOPTION_GUARD;
use cubelink_host::GameHost;

const T: Duration = Duration::from_secs(15);
/// Silence du faux jeu apres l'affichage de sa fenetre : plus long que le delai de garde du rattachement.
const STALL: Duration = Duration::from_millis(12_000);

#[test]
fn jeu_muet_adopte_sans_repli() {
    assert!(STALL > ADOPTION_GUARD);
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
    let (stage, curtain) = (stage(), curtain());

    host.begin("Gros modpack", "Lancement", None, None);
    let mut game = FakeGame::spawn(&["GLFW30", "300", "stall=12000"]);
    host.watch(game.pid());

    // le jeu affiche sa fenetre puis se tait : elle est quand meme dans la scene, posee sur la zone, sous le rideau
    wait_until("fenetre du jeu dans la scene", T, || game.window("GLFW30").is_some_and(|w| parent_of(w) == stage));
    let win = game.window("GLFW30").unwrap();
    let t0 = Instant::now();
    wait_until("fenetre posee sur la zone de jeu pendant que le jeu se tait", T, || visible(win) && rect(win) == parent.to_screen(zone));
    assert!(t0.elapsed() < STALL - Duration::from_secs(3), "le jeu muet a ete pose APRES son silence");
    assert!(visible(curtain), "le rideau reste ferme jusqu'a `reveal`");
    wait_until("Adopted", T, || !events.adopted().is_empty());

    // plus longtemps que le delai de garde du rattachement : jamais de repli, jamais de « refus »
    let until = t0 + ADOPTION_GUARD + Duration::from_millis(1500);
    while Instant::now() < until {
        pump_for(Duration::from_millis(100));
        assert!(events.fallbacks().is_empty(), "repli a tort : {:?}\n{}", events.fallbacks(), events.logs().join("\n"));
        assert_eq!(parent_of(win), stage);
    }
    assert!(!events.logged("rattachement refuse"), "un client muet n'est pas un refus :\n{}", events.logs().join("\n"));
    assert!(host.staged());

    host.reveal();
    wait_until("dans le jeu", T, || host.in_game() && !visible(curtain));

    // la fermeture est demandee pendant le silence ; le jeu l'honore des qu'il pompe a nouveau
    host.request_close();
    wait_until("le faux jeu s'arrete apres son silence", T + STALL, || game.exited());
    host.run_finished();
    wait_until("retour au launcher", T, || host.view() == (false, false) && !visible(curtain));
    assert!(events.fallbacks().is_empty());
}
