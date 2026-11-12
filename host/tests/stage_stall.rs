//! Scene, jeu qui ne repond pas : un gros modpack AFFICHE sa fenetre puis ne pompe plus ses messages pendant le chargement des
//! mods (mesure : NeoForge, 20 s). Le garage hors ecran et l'adoption attendent son fil : la fenetre reste a l'ecran, avec son
//! cadre. L'hote ne peut que la COUVRIR (rideau maintenu au-dessus dans l'ordre Z), le dit au journal, et surtout ne prend jamais
//! ce silence pour un refus : pas de repli, meme au-dela du delai de garde du rattachement (delais de PRODUCTION). Quand le jeu
//! repond a nouveau, l'adoption se termine normalement. Les fenetres d'aide de GLFW, creees avant la fenetre du jeu, ne sont pas
//! touchees (elles ne passent donc jamais devant elle dans la file de l'hote). Voir `common`.
#![cfg(windows)]

mod common;

use std::time::{Duration, Instant};

use common::*;
use cubelink_host::guard::ADOPTION_GUARD;
use cubelink_host::GameHost;

const T: Duration = Duration::from_secs(15);
/// Silence du faux jeu apres l'affichage de sa fenetre : plus long que le delai de garde du rattachement.
const STALL: Duration = Duration::from_millis(12_000);

#[test]
fn fenetre_affichee_par_un_jeu_qui_ne_repond_pas() {
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
    let mut game = FakeGame::spawn(&["GLFW30", "300", "glfwhelper", "stall=12000"]);
    host.watch(game.pid());

    // le jeu affiche sa fenetre lui-meme et se tait : elle est a l'ecran, avec son cadre, pas rattachee
    wait_until("fenetre affichee par le jeu", T, || game.window("GLFW30").is_some_and(visible));
    let shown_at = Instant::now();
    let win = game.window("GLFW30").unwrap();
    let helpers: Vec<isize> = windows_of(game.pid(), "GLFW3 Helper").into_iter().chain(windows_of(game.pid(), "GLFW30").into_iter().filter(|h| title(*h) == GLFW_HELPER_TITLE)).collect();
    assert_eq!(helpers.len(), 2, "fenetres d'aide du faux jeu (GLFW 3.4 et GLFW 3.3)");
    let before: Vec<_> = helpers.iter().map(|h| (style(*h), exstyle(*h), owner(*h), rect(*h))).collect();

    // tant que le jeu se tait : le rideau reste AU-DESSUS de sa fenetre (il repasse devant ~300 ms apres l'affichage), aucun repli
    let quiet_until = shown_at + STALL - Duration::from_millis(1500);
    let mut checks = 0;
    while Instant::now() < quiet_until {
        // la remontee suit le constat d'un battement (au plus 100 ms) : on laisse une seconde, jamais plus
        wait_until("rideau au-dessus de la fenetre du jeu", Duration::from_secs(1), || above(curtain, win));
        assert!(visible(curtain), "rideau ferme pendant tout le chargement ({:?} apres l'affichage)
{}
{}", shown_at.elapsed(), desktop_state(parent.hwnd), events.logs().join("
"));
        assert!(visible(win) && framed(win) && on_screen(win) && owner(win) == 0, "le jeu ne repond pas : sa fenetre n'a pas pu etre touchee");
        assert!(events.fallbacks().is_empty(), "repli a tort {:?} apres l'affichage : {:?}", shown_at.elapsed(), events.fallbacks());
        assert!(host.staged());
        pump_for(Duration::from_millis(150));
        checks += 1;
    }
    assert!(checks > 30 && shown_at.elapsed() > ADOPTION_GUARD, "silence plus long que le delai de garde : {:?}", shown_at.elapsed());
    assert!(events.adopted().is_empty());
    assert_eq!(events.logs().iter().filter(|l| l.contains("affichee avant son rattachement : le jeu ne repond pas, rideau maintenu au-dessus")).count(), 1, "une ligne, une seule :\n{}", events.logs().join("\n"));

    // le jeu repond a nouveau : adoption normale, pose sur la zone sous le rideau
    wait_until("Adopted", T, || !events.adopted().is_empty());
    wait_until("fenetre posee sur la zone de jeu", T, || visible(win) && rect(win) == parent.to_screen(zone));
    assert!(owner(win) == parent.hwnd && !framed(win));
    host.reveal();
    wait_until("dans le jeu", T, || host.in_game() && !visible(curtain));
    pump_for(Duration::from_millis(1000));

    assert!(events.fallbacks().is_empty(), "aucun repli : {:?}", events.fallbacks());
    assert!(!events.logged("rattachement refuse"), "un fil qui ne repond pas n'est pas un refus :\n{}", events.logs().join("\n"));
    assert!(host.staged());
    assert_eq!(events.adopted(), vec!["GLFW30".to_string()]);

    // fenetres d'aide de GLFW : jamais touchees, jamais journalisees
    for (h, was) in helpers.iter().zip(&before) {
        assert_eq!((style(*h), exstyle(*h), owner(*h), rect(*h)), *was, "fenetre d'aide {} modifiee", class_of(*h));
        assert!(!visible(*h));
    }
    assert!(!events.logs().iter().any(|l| l.contains("Helper") || l.contains("1 x 1") || l.contains("136 x 39")), "fenetre d'aide journalisee :\n{}", events.logs().join("\n"));
    assert_eq!(events.logs().iter().filter(|l| l.contains("fenetre detectee")).count(), 1, "seule LA fenetre du jeu est detectee :\n{}", events.logs().join("\n"));

    // fermeture : aucune correction ni ligne « cadre » sur une fenetre qui se ferme ou n'existe plus
    host.request_close();
    wait_until("le faux jeu s'arrete", T, || game.exited());
    pump_for(Duration::from_millis(1500));
    host.run_finished();
    assert!(!events.logged("cadre de la fenetre remis"), "ligne « cadre » a tort :\n{}", events.logs().join("\n"));
}
