//! Scene X11, cas nominal : le faux jeu est adopte (reparente dans la scene du launcher) avant d'avoir ete vu, pose sur la zone de
//! jeu, suit ses changements, puis se ferme a la demande. Voir `common_x11` (serveur X11 nu, aucune entree souris / clavier).
#![cfg(target_os = "linux")]

mod common_x11;

use std::time::Duration;

use common_x11::*;
use cubelink_host::GameHost;

const T: Duration = Duration::from_secs(15);

#[test]
fn jeu_adopte_pose_bascule_et_ferme() {
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
    assert_eq!(parent_of(stage), parent.xid, "la scene est une fille du launcher");
    assert!(!visible(curtain), "rideau retire tant que rien n'est lance");
    assert!(host.staged());
    assert_eq!(host.view(), (false, false));

    // ---- lancement : le rideau se ferme, le faux jeu demarre, sa fenetre est adoptee des sa creation (avant son affichage)
    host.begin("Modpack de test", "Lancement", None, None);
    assert!(visible(curtain), "rideau ferme des `begin`");
    let mut game = FakeGame::spawn(&["GLFW30", "500", "glfwhelper", "late=150"]);
    let sampler = Sampler::start(game.pid(), "GLFW30");
    host.watch(game.pid());
    assert_eq!(host.view(), (true, true));
    wait_until("fenetre du faux jeu dans la scene", T, || game.window("GLFW30").is_some_and(|w| parent_of(w) == stage));
    let win = game.window("GLFW30").unwrap();

    // ---- affichee par le jeu : posee sur la zone de jeu SOUS le rideau ferme, visible dans la scene
    wait_until("fenetre posee sur la zone de jeu", T, || visible(win) && rect(win) == parent.to_screen(zone));
    assert_eq!(rect(stage), parent.to_screen(zone), "la scene couvre la zone de jeu");
    assert!(visible(curtain), "le rideau reste ferme jusqu'a `reveal`");
    assert!(!host.in_game());
    wait_until("evenement Adopted", T, || !events.adopted().is_empty());
    pump_for(Duration::from_millis(600));
    assert!(visible(curtain) && !host.in_game(), "jamais d'ouverture sans `reveal`");

    // ---- pret : le rideau s'ouvre sur le jeu, qui a le focus clavier
    host.reveal();
    wait_until("rideau ouvert sur le jeu", T, || host.in_game() && !visible(curtain));
    assert_eq!(rect(win), parent.to_screen(zone));
    assert!(visible(win) && parent_of(win) == stage);
    assert_eq!(focus(), win, "le focus clavier est au jeu");
    let (samples, seen) = sampler.stop();
    assert!(samples > 200, "echantillonnage trop maigre pour conclure : {samples}");
    assert!(seen.is_empty(), "fenetre de jeu autonome apercue avant ou pendant l'adoption ({samples} echantillons) :\n{}", seen.join("\n"));

    // ---- la zone de jeu change (redimensionnement du launcher) : la scene, la fenetre et le rideau suivent
    let small = (20, 60, 500, 300);
    host.relayout(small.0, small.1, small.2, small.3);
    wait_until("fenetre recalee sur la nouvelle zone", T, || rect(win) == parent.to_screen(small));
    assert_eq!(rect(curtain), parent.to_screen(small), "le rideau suit la zone lui aussi");
    assert_eq!(rect(stage), parent.to_screen(small));

    // ---- panneau « en jeu » : bande a droite rendue au launcher (scene reduite, le jeu garde sa taille), puis refermee
    assert!(host.set_panel(true, 150));
    assert!(host.panel());
    wait_until("scene reduite par le panneau", T, || size(stage) == (small.2 - 150, small.3));
    assert_eq!(size(win), (small.2, small.3), "le jeu garde sa taille (rognee)");
    assert!(host.set_panel(false, 0));
    wait_until("scene rendue au jeu", T, || size(stage) == (small.2, small.3));

    // ---- retour au launcher sans arreter le jeu, puis retour au jeu
    host.set_view(false);
    wait_until("scene retiree, rideau rouvert sur le launcher", T, || !visible(win) && !visible(stage) && !visible(curtain));
    assert_eq!(host.view(), (true, false));
    assert!(!game.exited(), "basculer ne ferme pas le jeu");
    host.set_view(true);
    wait_until("retour dans le jeu", T, || host.in_game() && visible(win) && !visible(curtain));
    assert_eq!(rect(win), parent.to_screen(small));

    // ---- fermeture demandee : le jeu recoit WM_DELETE_WINDOW et s'arrete de lui-meme
    host.request_close();
    wait_until("le faux jeu s'arrete sur WM_DELETE_WINDOW", T, || game.exited());
    host.run_finished();
    wait_until("retour au launcher apres la fin du jeu", T, || host.view() == (false, false) && !visible(curtain) && !visible(stage));

    // ---- journal permanent et evenements
    assert_eq!(events.adopted(), vec!["GLFW30".to_string()], "Adopted une fois par lancement");
    for needle in ["lancement", "hooks poses", "fenetre detectee (", "classe GLFW30", "adoptee en", "posee sur la zone de jeu", "fermeture demandee", "jeu termine"] {
        events.assert_logged(needle);
    }
    assert!(!events.logs().iter().any(|l| l.contains("fenetre detectee") && (l.contains("Helper") || l.contains("SDLHelper"))), "fenetre d'aide retenue :\n{}", events.logs().join("\n"));
    assert!(events.fallbacks().is_empty(), "aucun repli attendu : {:?}", events.fallbacks());
    assert!(host.staged());

    // ---- second lancement dans le meme hote, autre classe de fenetre (LWJGL 2), en mouvement reduit
    host.set_calm(true);
    host.relayout(zone.0, zone.1, zone.2, zone.3);
    host.begin("Vieux modpack", "Lancement", None, None);
    let mut old = FakeGame::spawn(&["LWJGL", "300"]);
    host.watch(old.pid());
    wait_until("second Adopted", T, || events.adopted().len() == 2);
    assert_eq!(events.adopted()[1], "LWJGL");
    let win2 = old.window("LWJGL").unwrap();
    wait_until("seconde fenetre posee", T, || visible(win2) && rect(win2) == parent.to_screen(zone));
    host.reveal();
    wait_until("rideau ouvert (mouvement reduit)", T, || host.in_game() && !visible(curtain));
    host.request_close();
    wait_until("le second faux jeu s'arrete", T, || old.exited());
    host.run_finished();
    wait_until("retour au launcher", T, || host.view() == (false, false) && !visible(curtain));
    assert!(events.fallbacks().is_empty());
}
