//! Scene, repli demande (`GameHost::release`) : la fenetre adoptee est RENDUE au jeu (visible, cadre, sans proprietaire, a
//! l'ecran), `Fallback` est emis une seule fois, et rien ne retouche la fenetre ensuite. Voir `common`.
#![cfg(windows)]

mod common;

use std::time::Duration;

use common::*;
use cubelink_host::{FallbackReason, GameHost};

const T: Duration = Duration::from_secs(15);

#[test]
fn release_rend_la_fenetre_au_jeu() {
    let parent = Parent::new();
    let host = GameHost::embed(parent.hwnd).expect("embed");
    let events = Events::default();
    host.set_observer(events.observer());
    let (cw, ch) = parent.client_size();
    let zone = (0, 40, cw, ch - 40);
    host.relayout(zone.0, zone.1, zone.2, zone.3);
    let curtain = curtain();

    // sans jeu : sans effet
    host.release("rien a rendre");
    pump_for(Duration::from_millis(100));
    assert!(events.fallbacks().is_empty() && host.staged());

    // ---- un jeu dans la Scene, joueur dans le jeu, panneau ouvert (fenetre decoupee)
    host.begin("Modpack de test", "Lancement", None, None);
    let mut game = FakeGame::spawn(&["SDL_app", "300"]);
    host.watch(game.pid());
    wait_until("Adopted", T, || !events.adopted().is_empty());
    let win = game.window("SDL_app").unwrap();
    host.reveal();
    wait_until("dans le jeu", T, || host.in_game() && !visible(curtain) && rect(win) == parent.to_screen(zone));
    assert!(host.set_panel(true, 200));
    wait_until("decoupe du panneau", T, || has_region(win));
    assert!(!framed(win) && owner(win) == parent.hwnd);

    // ---- repli demande
    host.release("demande par le test");
    assert!(!host.staged(), "`staged` est faux des la demande");
    assert_eq!(host.view(), (false, false), "le jeu n'est plus rattache");
    wait_until("evenement Fallback", T, || !events.fallbacks().is_empty());
    assert_eq!(events.fallbacks(), vec![(FallbackReason::Requested, "demande par le test".to_string())]);
    // a l'instant de l'evenement la fenetre EST rendue
    assert_given_back(win);
    let (x, y, w, h) = rect(win);
    assert_eq!((w, h), (870, 520), "taille d'origine du faux jeu retrouvee");
    assert_ne!((x, y), (parent.to_screen(zone).0, parent.to_screen(zone).1), "la fenetre a quitte la zone de jeu");
    assert!(host.is_ready(Duration::from_secs(1), Duration::from_secs(90)), "plus rien a attendre de la Scene");

    // ---- le rideau se rouvre sur le launcher ; la fenetre rendue n'est plus jamais retouchee (verifications differees comprises)
    host.release("seconde demande");
    host.reveal();
    host.mark_ready(None);
    wait_until("rideau rouvert sur le launcher", T, || !visible(curtain));
    pump_for(Duration::from_millis(2000));
    assert_given_back(win);
    assert_eq!(rect(win), (x, y, w, h), "la fenetre rendue a ete deplacee apres coup");
    assert_eq!(events.fallbacks().len(), 1, "Fallback emis UNE fois : {:?}", events.fallbacks());
    assert!(!game.exited(), "le jeu n'est jamais tue par un repli");
    assert!(!host.staged());
    events.assert_logged("repli decide (requested)");
    events.assert_logged("1 fenetre(s) rendue(s)");

    // ---- le launcher peut toujours fermer proprement le jeu rendu ; sa fin ne rejoue aucun rideau
    host.request_close();
    wait_until("le jeu rendu se ferme sur WM_CLOSE", T, || game.exited());
    host.run_finished();
    assert!(!visible(curtain), "la fin d'un jeu rendu ne referme pas le rideau");
    pump_for(Duration::from_millis(300));
    assert!(!visible(curtain));

    // ---- lancement suivant : de nouveau dans la Scene ; repli AVANT que le jeu n'ait affiche sa fenetre
    host.begin("Autre modpack", "Lancement", None, None);
    assert!(host.staged(), "`begin` remet le lancement dans la Scene");
    let slow = FakeGame::spawn(&["GLFW30", "60000"]);
    host.watch(slow.pid());
    wait_until("fenetre cachee adoptee", T, || slow.window("GLFW30").is_some_and(|w| owner(w) == parent.hwnd));
    let hidden = slow.window("GLFW30").unwrap();
    assert!(!visible(hidden) && !on_screen(hidden), "fenetre pas encore affichee : garee hors ecran");
    host.release("avant affichage");
    wait_until("second Fallback", T, || events.fallbacks().len() == 2);
    // elle n'est PAS montree a la place du jeu (il l'affichera lui-meme), mais elle est de nouveau la sienne, a sa place d'origine
    assert!(!visible(hidden), "une fenetre que le jeu n'a pas affichee reste cachee");
    assert_eq!(owner(hidden), 0);
    assert!(framed(hidden) && on_screen(hidden));
    assert_eq!(rect(hidden), (200, 150, 870, 520), "position et taille d'origine du faux jeu");
    wait_until("rideau rouvert sur le launcher", T, || !visible(curtain));
    pump_for(Duration::from_millis(2000));
    assert_eq!(rect(hidden), (200, 150, 870, 520));
    assert_eq!(owner(hidden), 0);
    assert_eq!(events.adopted().len(), 1, "pas d'Adopted pour une fenetre jamais affichee");
}
