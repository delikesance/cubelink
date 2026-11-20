use super::*;

#[test]
fn scene_linux_seulement_avec_x11() {
    assert!(stage_supported_on(true, false, false));
    assert!(stage_supported_on(false, true, true));
    assert!(!stage_supported_on(false, true, false));
    assert!(!stage_supported_on(false, false, true));
}

type Events = Arc<Mutex<Vec<(String, serde_json::Value)>>>;

fn core() -> (Core, Events) {
    let events: Events = Arc::new(Mutex::new(Vec::new()));
    let ev = events.clone();
    let c = Core::default();
    let sink: EventSink = Arc::new(move |n, v| lock(&ev).push((n.to_string(), v)));
    let _ = c.sink.set(sink);
    (c, events)
}

#[test]
fn deux_replis_de_suite_coupent_la_session() {
    let mut h = Health::default();
    h.begin();
    assert_eq!(h.fallback(true, "no_window"), None, "un seul repli : la Scene reste proposee");
    assert_eq!(h.fallback(true, "no_window"), None, "au plus un repli compte par lancement");
    assert!(h.finish(), "ce lancement s'etait replie");
    assert!(!h.finish(), "fin deja traitee");
    assert!(!h.off());
    h.begin();
    let cut = h.fallback(true, "adoption_failed");
    assert_eq!(cut, Some(vec!["no_window".to_string(), "adoption_failed".to_string()]), "coupure annoncee une fois, avec les deux raisons");
    h.finish();
    assert!(h.off());
    h.begin();
    assert_eq!(h.fallback(true, "no_window"), None, "deja coupee : pas d'annonce en double");
    assert!(h.off());
}

#[test]
fn reessai_leve_la_coupure_et_recompte_depuis_zero() {
    let mut h = Health::default();
    for r in ["a", "b"] {
        h.begin();
        h.fallback(true, r);
        h.finish();
    }
    assert!(h.off());
    h.reset();
    assert!(!h.off(), "coupure levee");
    // un seul repli ne recoupe pas : la serie precedente est oubliee
    h.begin();
    assert_eq!(h.fallback(true, "c"), None);
    h.finish();
    assert!(!h.off());
    h.begin();
    assert_eq!(h.fallback(true, "d"), Some(vec!["c".to_string(), "d".to_string()]), "recoupe au deuxieme repli depuis le reessai");
}

#[test]
fn reessai_pendant_un_lancement_garde_ses_drapeaux() {
    let mut h = Health::default();
    h.begin();
    h.adopted();
    h.fallback(true, "a");
    h.reset();
    assert!(h.finish(), "le lancement en cours reste replie : `finish` le rend une seule fois");
    // lancement adopte et sain : sa fin efface la serie, meme apres un reset
    h.begin();
    h.adopted();
    h.reset();
    assert!(!h.finish());
}

#[test]
fn stage_retry_rend_la_scene_au_prochain_lancement() {
    let dir = tempfile::tempdir().unwrap();
    let s = Stage::default();
    s.open_log(dir.path());
    s.set_wanted(true);
    for _ in 0..2 {
        s.core.begin_launch();
        s.core.on_fallback("adoption_failed", "styles refuses");
        s.run_finished();
    }
    assert!(lock(&s.core.health).off(), "session coupee apres deux replis");
    // sans hote (tests, Wayland natif) : rien a retenter, la reponse le dit, mais la sante est quand meme remise a zero
    assert!(!s.retry());
    assert!(!lock(&s.core.health).off());
    assert_eq!(s.unavailable_reason(), Some("zone de jeu integree absente"), "plus la coupure de session");
    // un nouveau repli ne recoupe pas tout de suite
    s.core.begin_launch();
    s.core.on_fallback("no_window", "x");
    s.run_finished();
    assert!(!lock(&s.core.health).off());
    let Stage { core, .. } = s;
    let core = Arc::try_unwrap(core).ok().expect("seul proprietaire");
    core.outbox.into_inner().unwrap().finish();
    let text = std::fs::read_to_string(dir.path().join(crate::stage_log::FILE_NAME)).unwrap();
    assert!(text.contains("[launcher] Scene reactivee par le joueur"), "{text}");
}

#[test]
fn un_lancement_reussi_efface_les_replis_precedents() {
    let mut h = Health::default();
    h.begin();
    h.fallback(true, "a");
    h.finish();
    // lancement adopte et joue jusqu'au bout
    h.begin();
    h.adopted();
    assert!(!h.finish());
    h.begin();
    assert_eq!(h.fallback(true, "b"), None, "les replis ne sont plus consecutifs");
    assert!(!h.off());
    // un lancement qui s'arrete avant toute fenetre ne prouve rien : il n'efface pas le repli
    h.finish();
    h.begin();
    h.finish();
    h.begin();
    assert_eq!(h.fallback(true, "c"), Some(vec!["b".to_string(), "c".to_string()]));
}

#[test]
fn repli_demande_ou_apres_adoption() {
    let mut h = Health::default();
    for _ in 0..5 {
        h.begin();
        assert_eq!(h.fallback(false, "requested"), None, "un repli demande par le launcher n'est pas un echec");
        h.finish();
    }
    assert!(!h.off());
    // adopte PUIS replie : c'est un echec, il ne remet pas le compteur a zero
    h.begin();
    h.fallback(true, "a");
    h.finish();
    h.begin();
    h.adopted();
    assert!(h.fallback(true, "b").is_some());
}

#[test]
fn repli_le_lancement_quitte_la_scene_et_l_interface_est_prevenue() {
    let (c, events) = core();
    c.begin_launch();
    assert!(c.staged());
    c.set_state(StageState { attached: true, game: true, panel: false }, false);
    c.on_fallback("no_window", "aucune fenetre apres 60 s");
    assert!(!c.staged(), "plus de reveal ni de feed_line pour ce lancement");
    assert_eq!(*lock(&c.last), StageState::default(), "fermer le launcher ne ferme plus le jeu");
    let ev = lock(&events).clone();
    assert_eq!(ev.len(), 3);
    assert_eq!(ev[1], ("stage://state".to_string(), serde_json::json!({"attached": false, "game": false, "panel": false})));
    assert_eq!(ev[2], ("stage://fallback".to_string(), serde_json::json!({"reason": "no_window", "session_off": false})));
    // une publication calculee juste avant le repli ne ressuscite pas l'etat « rattache »
    c.set_state(StageState { attached: true, game: true, panel: true }, false);
    assert_eq!(*lock(&c.last), StageState::default());
    assert_eq!(lock(&events).len(), 3);
    assert_eq!(c.end_launch(), Ending::FellBack, "fin du jeu apres repli : l'hote n'a plus rien a refermer");
    assert_eq!(c.end_launch(), Ending::Outside, "deuxieme appel (filet) : plus rien");
}

#[test]
fn deuxieme_repli_annonce_la_coupure_de_session() {
    let (c, events) = core();
    for _ in 0..2 {
        c.begin_launch();
        c.on_fallback("adoption_failed", "styles refuses");
        c.end_launch();
    }
    let ev = lock(&events).clone();
    let falls: Vec<_> = ev.iter().filter(|(n, _)| n == "stage://fallback").map(|(_, v)| v["session_off"].as_bool().unwrap()).collect();
    assert_eq!(falls, vec![false, true]);
    assert!(lock(&c.health).off());
}

#[test]
fn scene_proposee_seulement_avec_hote_reglage_et_session_saine() {
    let s = Stage::default();
    // sans `attach` il n'y a pas d'hote (tests, autres plateformes) : jamais disponible, quel que soit le reglage
    s.set_wanted(true);
    assert!(s.wanted() && !s.available());
    assert_eq!(s.unavailable_reason(), Some("zone de jeu integree absente"));
    assert!(!s.staged());
    // sans hote, tout le pilotage est inerte et l'etat reste « non rattache »
    s.watch(1);
    s.feed_line("x");
    s.reveal();
    s.run_finished();
    assert!(!s.is_ready());
    assert_eq!(s.state(), StageState::default());
}

#[test]
fn journal_decisions_et_evenements_de_l_hote() {
    let dir = tempfile::tempdir().unwrap();
    let s = Stage::default();
    s.open_log(dir.path());
    s.set_wanted(true);
    s.note("lancement de abc (1.20.1, Fabric)");
    s.core.begin_launch();
    s.core.on_log("hooks poses");
    s.core.on_adopted("GLFW30", 4200);
    s.core.on_fallback("hook_failed", "acces refuse");
    // fin du jeu rendu : l'hote n'en dit rien, c'est au launcher de laisser la trace (avec le code de sortie)
    s.game_exited(-1);
    s.run_finished();
    s.run_finished();
    // deuxieme lancement replie : la coupure de session donne les deux raisons
    s.core.begin_launch();
    s.core.on_fallback("no_window", "rien apres 240 s");
    s.run_finished();
    let Stage { core, .. } = s;
    let core = Arc::try_unwrap(core).ok().expect("seul proprietaire");
    core.outbox.into_inner().unwrap().finish();
    let text = std::fs::read_to_string(dir.path().join(crate::stage_log::FILE_NAME)).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 9, "{text}");
    assert!(lines[0].contains("[launcher] reglage « Jeu integre » active"));
    assert!(lines[1].ends_with("[launcher] lancement de abc (1.20.1, Fabric)"));
    assert!(lines[2].ends_with("[hote] hooks poses"));
    assert!(lines[3].contains("[hote] fenetre du jeu adoptee (classe GLFW30, apres 4200 ms)"));
    assert!(lines[4].ends_with("[hote] repli (hook_failed) : acces refuse"));
    assert!(lines[5].ends_with("[launcher] jeu termine (hors Scene apres repli), code de sortie -1"), "{text}");
    assert!(lines[6].ends_with("[hote] repli (no_window) : rien apres 240 s"));
    assert!(lines[7].contains("[launcher] Scene desactivee pour la session (jusqu'au reessai du joueur ou au redemarrage de Cubelink) : 2 replis de suite (hook_failed, puis no_window)"), "{text}");
    assert!(lines[8].ends_with("[launcher] jeu termine (hors Scene apres repli)"), "sans code de sortie connu : {text}");
}

#[test]
fn marges_conservees_au_redimensionnement() {
    // zone sous une barre de titre de 36 px, fenetre 1280 x 720
    let l = Layout::from_rect(0, 36, 1280, 684, 380, 1280, 720);
    assert_eq!(l, Layout { left: 0, top: 36, right: 0, bottom: 0, panel_w: 380 });
    assert_eq!(l.rect(1280, 720), (0, 36, 1280, 684));
    assert_eq!(l.rect(1920, 1080), (0, 36, 1920, 1044), "la zone suit la fenetre sans nouvelle publication");
}

#[test]
fn rectangle_jamais_vide_ni_negatif() {
    let l = Layout::from_rect(-5, -5, 5000, 5000, -3, 800, 600);
    assert_eq!(l, Layout { left: 0, top: 0, right: 0, bottom: 0, panel_w: 0 });
    // echelle 125 % : y = 45, h = 854 dans une zone cliente de 900 px -> 1 px d'arrondi en bas, pas une marge
    assert_eq!(Layout::from_rect(0, 45, 1600, 854, 475, 1600, 900).bottom, 0);
    let l = Layout::from_rect(10, 40, 100, 100, 0, 800, 600);
    assert_eq!(l.rect(20, 20), (10, 40, 1, 1));
}
