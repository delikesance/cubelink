//! Faux hote pour les tests : aucune fenetre, mais le VRAI deroulement du rideau (`cubelink_host::flow::Machine`, la machine
//! pure que pilote `GameHost`), avec la meme correspondance appel -> evenement que `host/src/win/mod.rs` et une horloge
//! simulee. Ce qu'il prouve : apres chaque chemin du launcher, le rideau finit retire.

use super::*;
use cubelink_host::flow::{Event, Flow, Machine, View};
use std::time::{Duration, Instant};

struct Inner {
    m: Machine,
    now: Instant,
    ready: bool,
    calls: Vec<&'static str>,
}

#[derive(Clone)]
pub(crate) struct FakeHost(Arc<Mutex<Inner>>);

impl FakeHost {
    pub(crate) fn new() -> FakeHost {
        FakeHost(Arc::new(Mutex::new(Inner { m: Machine::new(), now: Instant::now(), ready: false, calls: Vec::new() })))
    }

    /// Une Scene de test pilotee par ce faux hote (reglage « Jeu integre » actif, comme par defaut).
    pub(crate) fn stage() -> (Stage, FakeHost) {
        let (stage, fake) = (Stage::default(), FakeHost::new());
        stage.set_wanted(true);
        fake.install(&stage);
        (stage, fake)
    }

    pub(crate) fn install(&self, stage: &Stage) {
        stage.install_test_host(Box::new(self.clone()), Arc::new(|_, _| {}));
    }

    fn send(&self, call: &'static str, ev: Event) {
        let mut i = lock(&self.0);
        i.calls.push(call);
        let now = i.now;
        i.m.apply(ev, now);
    }

    /// La fenetre du jeu est apparue (adoptee).
    pub(crate) fn window_shown(&self) {
        self.send("window_shown", Event::WindowShown);
    }

    /// Repli de l'hote : la fenetre est rendue au jeu, la machine revient au launcher.
    pub(crate) fn leave(&self) {
        self.send("leave", Event::Leave);
    }

    /// Laisse passer `ms` millisecondes, par battements de 16 ms comme le minuteur du rideau.
    pub(crate) fn run(&self, ms: u64) {
        let mut i = lock(&self.0);
        let end = i.now + Duration::from_millis(ms);
        while i.now < end {
            let now = (i.now + Duration::from_millis(16)).min(end);
            i.now = now;
            i.m.apply(Event::Tick, now);
        }
    }

    pub(crate) fn flow(&self) -> Flow {
        lock(&self.0).m.flow()
    }

    /// Le rideau est a l'ecran apres avoir laisse aux animations le temps de finir (10 s : bien plus que toute animation).
    pub(crate) fn curtain_stuck(&self) -> bool {
        self.run(10_000);
        self.flow().curtain_on()
    }

    pub(crate) fn calls(&self) -> Vec<&'static str> {
        lock(&self.0).calls.clone()
    }

    pub(crate) fn count(&self, call: &str) -> usize {
        self.calls().iter().filter(|c| **c == call).count()
    }
}

impl HostApi for FakeHost {
    fn relayout(&self, _x: i32, _y: i32, _w: i32, _h: i32) {}
    fn begin(&self, _title: &str, _mods_dir: &Path, _profile_path: &Path) {
        lock(&self.0).ready = false;
        self.send("begin", Event::BeginLaunch);
    }
    fn watch(&self, _pid: u32) {
        self.send("watch", Event::Watch);
    }
    fn feed_line(&self, _line: &str) {}
    fn is_ready(&self) -> bool {
        lock(&self.0).ready
    }
    fn mark_ready(&self, _profile_path: &Path) {
        lock(&self.0).ready = true;
    }
    fn reveal(&self) {
        self.send("reveal", Event::Reveal);
    }
    fn begin_close(&self, _title: &str) {
        self.send("begin_close", Event::BeginClose);
    }
    fn request_close(&self) {
        lock(&self.0).calls.push("request_close");
    }
    fn cancel_close(&self) {
        self.send("cancel_close", Event::CancelClose);
    }
    fn run_finished(&self) {
        self.send("run_finished", Event::GameExited);
    }
    fn set_view(&self, game: bool) {
        self.send("set_view", Event::SetView(if game { View::Game } else { View::Launcher }));
    }
    fn view(&self) -> (bool, bool) {
        let i = lock(&self.0);
        (i.m.attached(), i.m.view() == View::Game)
    }
    fn panel(&self) -> bool {
        lock(&self.0).m.panel_width() > 0
    }
    fn set_panel(&self, open: bool, width_px: i32) -> bool {
        let before = self.panel();
        self.send("set_panel", if open { Event::PanelOpen { width: width_px } } else { Event::PanelClose });
        self.panel() != before
    }
    fn set_calm(&self, calm: bool) {
        lock(&self.0).m.set_calm(calm);
    }
    fn expect_hidden(&self, hidden: bool) {
        lock(&self.0).calls.push(if hidden { "expect_hidden(true)" } else { "expect_hidden(false)" });
    }
}

/// Amene la Scene « en jeu » : lancement, fenetre adoptee, rideau ouvert sur le jeu.
pub(crate) fn play(stage: &Stage, fake: &FakeHost) {
    let p = Path::new("x");
    stage.begin("Ancien", p, p);
    stage.watch(1);
    fake.window_shown();
    stage.reveal();
    fake.run(10_000);
    assert!(fake.flow().in_game(), "prealable : en jeu ({:?})", fake.flow());
}

/// Amene la Scene la ou le defaut se produisait : changement de modpack, ancien jeu ferme, rideau ferme a mi-barre dans
/// l'attente du lancement suivant.
pub(crate) fn switch_pending(stage: &Stage, fake: &FakeHost) {
    play(stage, fake);
    stage.begin_close("Nouveau");
    stage.request_close();
    fake.run(2_000);
    stage.run_finished(); // fin de la tache de l'ancien jeu
    assert!(!stage.staged());
    assert!(fake.curtain_stuck(), "prealable : sans lancement suivant le rideau reste ferme ({:?})", fake.flow());
}

#[test]
fn changement_de_modpack_reussi_le_rideau_s_ouvre_sur_le_nouveau_jeu() {
    let (stage, fake) = FakeHost::stage();
    switch_pending(&stage, &fake);
    let p = Path::new("x");
    stage.begin("Nouveau", p, p);
    stage.watch(2);
    fake.window_shown();
    stage.reveal();
    assert!(!fake.curtain_stuck());
    assert!(fake.flow().in_game(), "{:?}", fake.flow());
    assert!(!stage.abort_switch("rien a interrompre"), "le lancement a eu lieu : plus de changement en attente");
    assert_eq!(fake.count("cancel_close"), 0);
}

#[test]
fn lancement_suivant_absent_le_rideau_se_rouvre_sur_le_launcher() {
    let (stage, fake) = FakeHost::stage();
    switch_pending(&stage, &fake);
    assert!(stage.abort_switch("lancement refuse"));
    assert!(!fake.curtain_stuck(), "{:?}", fake.flow());
    assert_eq!(fake.flow(), Flow::Launcher);
    assert_eq!(stage.state(), StageState::default());
    // une seule fois : un second appel (deux filets sur le meme echec) ne touche plus l'hote
    let before = fake.calls().len();
    assert!(!stage.abort_switch("encore"));
    assert_eq!(fake.calls().len(), before);
    // et la Scene resservira : lancement suivant normal
    play(&stage, &fake);
}

#[test]
fn ancien_jeu_encore_la_abort_ne_le_detache_pas() {
    let (stage, fake) = FakeHost::stage();
    play(&stage, &fake);
    stage.begin_close("Nouveau");
    fake.run(2_000);
    // l'ancien jeu tourne encore : ce n'est pas a `abort_switch` de rouvrir (il annoncerait une fin de jeu a l'hote)
    assert!(!stage.abort_switch("trop tot"));
    assert_eq!(fake.count("run_finished"), 0);
    // delai de fermeture depasse : le rideau se rouvre sur le jeu, toujours la
    stage.cancel_close();
    assert!(!fake.curtain_stuck());
    assert!(fake.flow().in_game(), "{:?}", fake.flow());
    // sa fin, plus tard, ramene au launcher
    stage.run_finished();
    assert!(!fake.curtain_stuck());
    assert_eq!(fake.flow(), Flow::Launcher);
}

#[test]
fn echec_entre_begin_et_la_fin_normale_le_rideau_se_rouvre_toujours() {
    let p = Path::new("x");
    // chaque sortie anticipee de la tache de jeu, reperee par l'etape atteinte quand elle s'arrete
    for (step, from_switch) in [(0, false), (1, false), (2, false), (3, false), (0, true), (1, true), (2, true), (3, true)] {
        let (stage, fake) = FakeHost::stage();
        if from_switch {
            switch_pending(&stage, &fake);
        }
        stage.begin("Nouveau", p, p); // launch_with
        if step >= 1 {
            fake.run(700); // preparation (telechargements) : rideau ferme
        }
        if step >= 2 {
            stage.watch(7); // processus cree
        }
        if step >= 3 {
            fake.window_shown(); // le jeu plante pendant son chargement
            stage.feed_line("x");
        }
        assert!(fake.flow().curtain_on(), "prealable : rideau ferme (etape {step})");
        stage.run_finished(); // fin de la tache : `launch_with`, ou a defaut `GameGuard`
        stage.run_finished(); // les deux filets peuvent se suivre
        assert!(!fake.curtain_stuck(), "etape {step}, changement {from_switch} : {:?}", fake.flow());
        assert_eq!(fake.flow(), Flow::Launcher, "etape {step}, changement {from_switch}");
        assert_eq!(fake.count("run_finished"), 1 + usize::from(from_switch), "une fin par tache de jeu");
        assert_eq!(stage.state(), StageState::default());
    }
}

#[test]
fn fenetre_cachee_au_depart_seulement_dans_la_scene_et_avec_l_interrupteur() {
    assert!(hide_first_window(true, true));
    assert!(!hide_first_window(false, true) && !hide_first_window(true, false) && !hide_first_window(false, false));
    let p = Path::new("x");
    // dans la Scene, interrupteur pose : drapeau transmis, hote prevenu avant `watch`
    let (stage, fake) = FakeHost::stage();
    stage.begin("Jeu", p, p);
    assert!(stage.hide_start(true));
    stage.watch(1);
    assert_eq!(fake.calls(), ["begin", "expect_hidden(true)", "watch"]);
    // dans la Scene, sans l'interrupteur : rien ne change, et l'hote le sait
    let (stage, fake) = FakeHost::stage();
    stage.begin("Jeu", p, p);
    assert!(!stage.hide_start(false));
    assert_eq!(fake.count("expect_hidden(true)"), 0);
    // hors Scene, interrupteur pose : JAMAIS (ce serait un jeu invisible), et l'hote n'est pas touche
    let (stage, fake) = FakeHost::stage();
    assert!(!stage.hide_start(true), "aucun lancement dans la Scene");
    stage.set_wanted(false);
    assert!(!stage.available() && !stage.hide_start(true), "reglage coupe : le lanceur n'appelle pas `begin`");
    stage.set_wanted(true);
    stage.begin("Jeu", p, p);
    stage.core.on_fallback("no_window", "x");
    assert!(!stage.hide_start(true), "repli : le jeu a repris sa fenetre");
    stage.run_finished();
    stage.begin("Jeu", p, p);
    stage.core.on_fallback("no_window", "x");
    stage.run_finished();
    assert!(!stage.available() && !stage.hide_start(true), "session coupee");
    assert_eq!(fake.count("expect_hidden(true)"), 0);
    assert!(!Stage::default().hide_start(true), "pas de zone de jeu integree (non-Windows, mode test)");
}

#[test]
fn repli_apres_un_changement_de_modpack_le_rideau_se_rouvre() {
    let (stage, fake) = FakeHost::stage();
    switch_pending(&stage, &fake);
    let p = Path::new("x");
    stage.begin("Nouveau", p, p);
    stage.watch(2);
    // l'hote renonce (fenetre rendue au jeu) puis l'annonce
    fake.leave();
    stage.core.on_fallback("adoption_failed", "refus");
    assert!(!fake.curtain_stuck());
    assert_eq!(fake.flow(), Flow::Launcher);
    // la fin du jeu rendu ne concerne plus l'hote
    let before = fake.count("run_finished");
    stage.run_finished();
    assert_eq!(fake.count("run_finished"), before);
}
