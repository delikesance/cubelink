//! Scene, fenetres d'aide du jeu : SDL 3 cree, APRES sa fenetre principale, des fenetres « message seulement »
//! (`SDLHelperWindowInputCatcher`, `SDL_HIDAPI_DEVICE_DETECTION`) qui ne peuvent pas avoir de proprietaire. Elles ne sont pas des
//! fenetres de jeu : l'hote n'y touche pas, et elles ne provoquent jamais de repli (regression mesuree sur un vrai Minecraft
//! SDL 3 : repli « adoption refusee » 10 s apres une adoption reussie). De meme une vraie fenetre secondaire (`SDL_aux` du faux
//! jeu) qui refuse son rattachement ne provoque aucun repli tant que LA fenetre du jeu est tenue. Delais de garde de PRODUCTION.
//! Voir `common`.
#![cfg(windows)]

mod common;

use std::time::{Duration, Instant};

use common::*;
use cubelink_host::guard::ADOPTION_GUARD;
use cubelink_host::GameHost;

const T: Duration = Duration::from_secs(15);
const HELPERS: [&str; 2] = ["SDLHelperWindowInputCatcher", "SDL_HIDAPI_DEVICE_DETECTION"];

#[test]
fn fenetres_d_aide_intactes_et_sans_repli() {
    let parent = Parent::new();
    let host = GameHost::embed(parent.hwnd).expect("embed");
    let events = Events::default();
    host.set_observer(events.observer());
    let (cw, ch) = parent.client_size();
    let zone = (0, 40, cw, ch - 40);
    host.relayout(zone.0, zone.1, zone.2, zone.3);
    let curtain = curtain();

    host.begin("Modpack de test", "Lancement", None, None);
    let mut game = FakeGame::spawn(&["SDL_app", "300", "helpers"]);
    host.watch(game.pid());
    wait_until("Adopted", T, || !events.adopted().is_empty());
    let adopted_at = Instant::now();
    let win = game.window("SDL_app").unwrap();

    // les fenetres d'aide arrivent apres la fenetre principale
    wait_until("fenetres d'aide creees", T, || HELPERS.iter().all(|c| !message_windows_of(game.pid(), c).is_empty() || !windows_of(game.pid(), c).is_empty()));
    let helpers: Vec<isize> = HELPERS.iter().map(|c| *message_windows_of(game.pid(), c).first().unwrap_or_else(|| panic!("{c} n'est plus une fenetre « message seulement »"))).collect();
    let styles: Vec<(u32, u32)> = helpers.iter().map(|h| (style(*h), exstyle(*h))).collect();

    // la fenetre secondaire, elle, est une candidate : garee, rattachement refuse, signalee une fois, puis laissee tranquille
    wait_until("fenetre secondaire signalee", T, || events.logged("fenetre secondaire (SDL_aux) non rattachee"));
    let aux = *windows_of(game.pid(), "SDL_aux").first().expect("fenetre secondaire");

    host.reveal();
    wait_until("dans le jeu", T, || host.in_game() && !visible(curtain) && rect(win) == parent.to_screen(zone));

    // le jeu occupe de lui-meme tout l'ecran (plein ecran) : sa fenetre est ramenee sur la zone
    game_grabs_screen(win);
    wait_until("fenetre du jeu ramenee sur la zone", T, || rect(win) == parent.to_screen(zone));

    // plus longtemps que le delai de garde du rattachement : c'est la qu'arrivait le repli a tort
    let until = adopted_at + ADOPTION_GUARD + Duration::from_millis(2500);
    while Instant::now() < until {
        pump_for(Duration::from_millis(100));
        assert!(events.fallbacks().is_empty(), "repli a tort {:?} apres l'adoption : {:?}\n{}", adopted_at.elapsed(), events.fallbacks(), events.logs().join("\n"));
    }
    assert!(adopted_at.elapsed() >= Duration::from_secs(12));
    assert!(host.staged() && host.in_game());
    assert!(visible(win) && owner(win) == parent.hwnd && !framed(win), "la fenetre du jeu est restee dans la Scene : visible {}, proprietaire {:#x}, style {:#x}
{}
{}", visible(win), owner(win), style(win), desktop_state(parent.hwnd), events.logs().join("
"));
    assert_eq!(rect(win), parent.to_screen(zone));

    // fenetres d'aide intactes : toujours « message seulement », memes styles, sans proprietaire, jamais affichees
    for ((h, before), class) in helpers.iter().zip(&styles).zip(HELPERS) {
        assert!(exists(*h), "{class} detruite");
        assert_eq!(message_windows_of(game.pid(), class), vec![*h], "{class} n'est plus une fenetre « message seulement »");
        assert!(windows_of(game.pid(), class).is_empty(), "{class} est devenue une fenetre de premier niveau");
        assert_eq!((style(*h), exstyle(*h)), *before, "styles de {class} modifies");
        assert_eq!(owner(*h), 0, "{class} a recu un proprietaire");
        assert!(!visible(*h));
    }
    assert!(exists(aux) && !visible(aux) && owner(aux) == 0);
    assert_eq!(events.logs().iter().filter(|l| l.contains("fenetre secondaire")).count(), 1, "une ligne de journal, pas une par tentative");
    assert!(!events.logged("rattachement refuse"), "une fenetre d'aide a ete prise pour une fenetre de jeu :\n{}", events.logs().join("\n"));
    assert!(!events.logs().iter().any(|l| l.contains("fenetre detectee") && l.contains("classe SDLHelper")), "fenetre d'aide retenue :\n{}", events.logs().join("\n"));
    assert_eq!(events.adopted(), vec!["SDL_app".to_string()]);

    host.request_close();
    wait_until("le faux jeu s'arrete", T, || game.exited());
    host.run_finished();
    assert!(events.fallbacks().is_empty());
}
