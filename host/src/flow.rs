//! Deroulement de la vue (launcher <-> rideau <-> jeu) : machine a etats EXPLICITE et PURE.
//!
//! Aucun appel systeme ici : la couche Win32 (`win`) traduit ce qui se passe en [`Event`], les passe a [`Machine::apply`] avec
//! l'heure courante, puis execute les [`Effect`] rendus. Le temps est toujours un parametre (jamais `Instant::now()` ici) : tout
//! est testable sans fenetre, a la milliseconde pres.
//!
//! Le rideau a trois positions : retire ([`Flow::Launcher`], [`Flow::Pending`], [`Flow::InGame`], [`Flow::Panel`]), ferme ou en
//! train de se fermer ([`Flow::Closing`], [`Flow::Loading`], [`Flow::Held`], [`Flow::Switching`]) et en train de s'ouvrir
//! ([`Flow::Revealing`]). La fenetre du jeu n'est posee sur la zone de jeu, ou retiree, qu'a l'abri des panneaux fermes.

use std::time::Instant;

/// Fermeture des deux panneaux.
pub const CLOSE_MS: f32 = 520.0;
/// Apparition du contenu (logo, titre, barre) une fois les panneaux fermes.
pub const CARD_IN_MS: f32 = 260.0;
/// Passage launcher <-> jeu : l'ecran de chargement n'apparait que si le basculement dure plus que ca (sinon : panneaux seuls).
pub const QUICK_CARD_DELAY_MS: f32 = 2000.0;
/// Delai avant l'ouverture (le contenu s'efface d'abord).
pub const OPEN_DELAY_MS: f32 = 170.0;
/// Ouverture des deux panneaux.
pub const OPEN_MS: f32 = 650.0;
/// Panneaux consideres comme fermes a partir de ce niveau.
const COVERED: f32 = 0.999;

/// Allure des panneaux. `Calm` (mouvement reduit, preference systeme) : aucun glissement, les panneaux sont poses puis retires d'un
/// bloc. Les fondus du contenu (opacite, pas un mouvement) et le delai d'ouverture restent : c'est ce delai, panneaux fermes, qui
/// cache la pose ou le retrait de la fenetre du jeu.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Pace {
    #[default]
    Normal,
    Calm,
}

impl Pace {
    /// Duree de la fermeture des panneaux.
    pub fn close_ms(self) -> f32 {
        match self {
            Pace::Normal => CLOSE_MS,
            Pace::Calm => 0.0,
        }
    }

    /// Duree de l'ouverture des panneaux.
    pub fn open_ms(self) -> f32 {
        match self {
            Pace::Normal => OPEN_MS,
            Pace::Calm => 0.0,
        }
    }
}

/// Part ecoulee d'une duree `dur` apres `e` ms ; une duree nulle est terminee des son debut (jamais de division par zero).
fn part(e: f32, dur: f32) -> f32 {
    if dur > 0.0 {
        e / dur
    } else if e >= 0.0 {
        1.0
    } else {
        0.0
    }
}

/// Ce que le joueur doit voir une fois le rideau retire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Launcher,
    Game,
}

/// Portion de la barre occupee par un lancement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bar {
    /// Lancement simple : toute la barre.
    Full,
    /// Changement de modpack : la fermeture de l'ancien jeu a occupe 0-50 %, le lancement occupe 50-100 %.
    SecondHalf,
}

/// Ce que le rideau affiche sous le titre.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Content {
    /// « Un instant… », barre pleine (basculement, annulation, rideau tenu a la main).
    Wait,
    /// Fermeture propre de l'ancien jeu : premiere moitie de la barre.
    Close,
    /// Etapes du lancement.
    Launch(Bar),
}

/// Avancement affiche pour un avancement brut `raw` (0..1) du suivi correspondant a `content`.
pub fn bar_fraction(content: Content, raw: f32) -> f32 {
    match content {
        Content::Wait => 1.0,
        Content::Close => raw * 0.5,
        Content::Launch(Bar::Full) => raw,
        Content::Launch(Bar::SecondHalf) => 0.5 + 0.5 * raw,
    }
}

/// Panneaux qui se ferment (puis restent fermes) : depart et position de depart (continuite si on referme en cours d'ouverture).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shut {
    pub from: Instant,
    pub cover0: f32,
}

/// Panneaux qui s'ouvrent : depart, position des panneaux et opacite du contenu a cet instant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Open {
    pub from: Instant,
    pub cover0: f32,
    pub card0: f32,
}

/// Etat de la vue. `placed` : la fenetre du jeu est posee sur la zone de jeu, sous le rideau (sinon hors ecran ou cachee).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Flow {
    /// Rideau retire, le launcher est visible.
    Launcher,
    /// Rideau retire, vue « jeu » demandee, mais la fenetre du jeu n'est pas posee (pas encore apparue, ou le jeu n'a pas encore
    /// annonce qu'il est pret) : le launcher reste visible en attendant [`Event::Reveal`] ou un nouveau basculement.
    Pending,
    /// Rideau retire, le joueur est dans le jeu.
    InGame,
    /// Dans le jeu, panneau « en jeu » ouvert : bande de `width` px decoupee a droite de la fenetre du jeu.
    Panel { width: i32 },
    /// Changement de modpack, premiere moitie : les panneaux sont fermes sur l'ancien jeu, qui se ferme proprement.
    Closing { shut: Shut, placed: bool },
    /// Lancement : panneaux fermes, etapes du chargement ; l'ouverture attend [`Event::Reveal`].
    Loading { shut: Shut, placed: bool, bar: Bar },
    /// Rideau tenu ferme ([`Event::Hold`]) ou fermeture abandonnee : l'ouverture attend [`Event::Reveal`].
    Held { shut: Shut, to: View, placed: bool },
    /// Basculement launcher <-> jeu (ou retour au launcher a la fin du jeu) : les panneaux se ferment, la fenetre du jeu est posee
    /// ou retiree dessous, puis ils se rouvrent tout seuls.
    Switching { shut: Shut, to: View, placed: bool },
    /// Les panneaux s'ouvrent sur `to`.
    Revealing { open: Open, to: View, placed: bool, content: Content },
}

/// Ce qui arrive a la vue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// Un lancement commence (`begin`) : apres une fermeture propre, il occupe la seconde moitie de la barre.
    BeginLaunch,
    /// Un changement de modpack commence (`begin_close`) : fermeture propre de l'ancien jeu.
    BeginClose,
    /// Fermer le rideau sans rien changer d'autre.
    Hold,
    /// Le jeu est pret, ouvrir le rideau (`reveal`).
    Reveal,
    /// L'ancien jeu ne s'est pas ferme a temps : il reste en place, le rideau reste ferme (l'appelant decidera).
    CloseAbandoned,
    /// Idem, et le rideau se rouvre sur l'ancien jeu (`cancel_close`).
    CancelClose,
    /// La tache de jeu est terminee (`run_finished`) : pendant une fermeture propre, la barre atteint 50 % et rien d'autre ; sinon
    /// retour au launcher.
    GameExited,
    /// Retour au launcher sans condition : le jeu n'est plus dans la Scene. C'est aussi le REPLI (`GameHost::release`, adoption
    /// impossible, aucune fenetre) : la couche systeme rend sa fenetre au jeu, la machine rouvre le rideau sur le launcher.
    Leave,
    /// Basculer vers le jeu ou le launcher sans arreter le jeu (`set_view`).
    SetView(View),
    /// Un nouveau processus de jeu est surveille (`watch`) : sa fenetre n'existe pas encore.
    Watch,
    /// La fenetre du jeu est apparue (adoptee et affichee par le jeu).
    WindowShown,
    /// Ouvrir le panneau « en jeu » (accepte seulement dans le jeu).
    PanelOpen { width: i32 },
    /// Fermer le panneau « en jeu ».
    PanelClose,
    /// Battement du minuteur du rideau.
    Tick,
}

/// Cadence du minuteur du rideau.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cadence {
    /// Ni animation ni jeu : plus aucun reveil.
    Off,
    /// Animation en cours.
    Fast,
    /// Jeu rattache, rideau retire : filet de securite lent.
    Slow,
}

/// Ce que la couche systeme doit faire apres une transition, dans l'ordre.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Afficher le rideau au-dessus de la zone de jeu et l'animer ; `fresh` : il etait retire (les panneaux partent de zero).
    ShowCurtain { fresh: bool },
    /// Les panneaux sont ouverts : retirer le rideau.
    HideCurtain,
    /// Poser la fenetre du jeu sur la zone de jeu (sous le rideau).
    PlaceGame,
    /// Cacher la fenetre du jeu (le launcher redevient visible).
    HideGame,
    /// Le panneau « en jeu » se ferme : rendre a la fenetre du jeu toute sa surface.
    ClearRegion,
    /// Rendre le premier plan au jeu.
    FocusGame,
    /// Recaler la fenetre du jeu (decoupe du panneau comprise).
    SyncGame,
    /// Le jeu est parti : oublier ses fenetres et son suivi, retirer la surveillance.
    Detach,
    /// L'ancien jeu s'est ferme : la barre de fermeture atteint 50 %.
    CloseBarDone,
    /// Cadence du minuteur (rendue a chaque battement).
    Timer(Cadence),
}

/// Acceleration puis ralentissement (cubique), 0..1 -> 0..1.
pub fn ease(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    if x < 0.5 {
        4.0 * x * x * x
    } else {
        1.0 - (-2.0 * x + 2.0).powi(3) / 2.0
    }
}

fn ms(now: Instant, from: Instant) -> f32 {
    now.saturating_duration_since(from).as_secs_f32() * 1000.0
}

impl Flow {
    /// Vue visee : ce que le joueur verra une fois le rideau retire.
    pub fn view(&self) -> View {
        match *self {
            Flow::Launcher => View::Launcher,
            Flow::Pending | Flow::InGame | Flow::Panel { .. } | Flow::Closing { .. } | Flow::Loading { .. } => View::Game,
            Flow::Held { to, .. } | Flow::Switching { to, .. } | Flow::Revealing { to, .. } => to,
        }
    }

    /// La fenetre du jeu est posee sur la zone de jeu.
    pub fn placed(&self) -> bool {
        match *self {
            Flow::Launcher | Flow::Pending => false,
            Flow::InGame | Flow::Panel { .. } => true,
            Flow::Closing { placed, .. } | Flow::Loading { placed, .. } | Flow::Held { placed, .. } | Flow::Switching { placed, .. } | Flow::Revealing { placed, .. } => placed,
        }
    }

    /// Le rideau est a l'ecran (ferme, en fermeture ou en ouverture).
    pub fn curtain_on(&self) -> bool {
        !matches!(self, Flow::Launcher | Flow::Pending | Flow::InGame | Flow::Panel { .. })
    }

    /// Le joueur est dans le jeu : fenetre du jeu posee, rideau retire.
    pub fn in_game(&self) -> bool {
        matches!(self, Flow::InGame | Flow::Panel { .. })
    }

    /// Largeur du panneau « en jeu » (0 = ferme).
    pub fn panel_width(&self) -> i32 {
        match *self {
            Flow::Panel { width } => width,
            _ => 0,
        }
    }

    /// Ce que le rideau affiche.
    pub fn content(&self) -> Content {
        match *self {
            Flow::Closing { .. } => Content::Close,
            Flow::Loading { bar, .. } => Content::Launch(bar),
            Flow::Revealing { content, .. } => content,
            _ => Content::Wait,
        }
    }

    /// Fermeture en cours ou terminee : (animation, contenu differe de `QUICK_CARD_DELAY_MS`).
    fn shut(&self) -> Option<(Shut, bool)> {
        match *self {
            Flow::Closing { shut, .. } | Flow::Loading { shut, .. } | Flow::Held { shut, .. } => Some((shut, false)),
            Flow::Switching { shut, .. } => Some((shut, true)),
            _ => None,
        }
    }

    /// Etat des panneaux a `now` : (fermeture 0..1, opacite du contenu 0..1).
    pub fn levels(&self, now: Instant, pace: Pace) -> (f32, f32) {
        if let Flow::Revealing { open, .. } = *self {
            // ouverture : le contenu s'efface, puis les panneaux s'ecartent
            let e = ms(now, open.from);
            let card = (1.0 - e / (OPEN_DELAY_MS * 0.9)).clamp(0.0, 1.0) * open.card0;
            return (open.cover0 * (1.0 - ease(part(e - OPEN_DELAY_MS, pace.open_ms()))), card);
        }
        match self.shut() {
            Some((shut, quick)) => {
                let e = ms(now, shut.from);
                let cover = shut.cover0 + (1.0 - shut.cover0) * ease(part(e, pace.close_ms()));
                let delay = if quick { QUICK_CARD_DELAY_MS } else { 0.0 };
                let card = ((e - pace.close_ms() - delay) / CARD_IN_MS).clamp(0.0, 1.0);
                (cover, if cover >= COVERED { card } else { 0.0 })
            }
            None => (0.0, 0.0),
        }
    }

    /// Panneaux fermes : on peut basculer dessous sans que le joueur voie la coupure.
    pub fn covered(&self, now: Instant, pace: Pace) -> bool {
        self.shut().is_some() && self.levels(now, pace).0 >= COVERED
    }

    fn set_placed(&mut self, v: bool) {
        match self {
            Flow::Closing { placed, .. } | Flow::Loading { placed, .. } | Flow::Held { placed, .. } | Flow::Switching { placed, .. } | Flow::Revealing { placed, .. } => *placed = v,
            Flow::InGame | Flow::Panel { .. } if !v => *self = Flow::Pending,
            _ => {}
        }
    }

    /// Debut d'ouverture a `now`, en continuite avec l'etat courant des panneaux et du contenu.
    fn open(&self, now: Instant, pace: Pace) -> Open {
        let (cover, card) = self.levels(now, pace);
        Open { from: now, cover0: if cover >= COVERED { 1.0 } else { cover }, card0: card }
    }
}

/// La machine : l'etat de la vue et les trois faits qui le traversent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Machine {
    flow: Flow,
    /// La fenetre du jeu surveille est connue (apparue).
    window: bool,
    /// Un processus de jeu est rattache (surveille).
    attached: bool,
    /// Fermeture propre de l'ancien jeu en cours : sa fin n'est pas un retour au launcher, et le lancement qui suit prend la
    /// seconde moitie de la barre.
    closing: bool,
    /// Allure des panneaux (mouvement reduit).
    pace: Pace,
}

impl Default for Machine {
    fn default() -> Self {
        Machine::new()
    }
}

impl Machine {
    pub fn new() -> Machine {
        Machine { flow: Flow::Launcher, window: false, attached: false, closing: false, pace: Pace::Normal }
    }

    /// Mouvement reduit : les panneaux ne glissent plus (voir [`Pace`]). Pris en compte tout de suite, animation en cours comprise.
    pub fn set_calm(&mut self, calm: bool) {
        self.pace = if calm { Pace::Calm } else { Pace::Normal };
    }

    pub fn calm(&self) -> bool {
        self.pace == Pace::Calm
    }

    pub fn flow(&self) -> Flow {
        self.flow
    }

    pub fn view(&self) -> View {
        self.flow.view()
    }

    pub fn in_game(&self) -> bool {
        self.flow.in_game()
    }

    pub fn panel_width(&self) -> i32 {
        self.flow.panel_width()
    }

    pub fn content(&self) -> Content {
        self.flow.content()
    }

    pub fn levels(&self, now: Instant) -> (f32, f32) {
        self.flow.levels(now, self.pace)
    }

    /// Une fermeture propre de l'ancien jeu est en cours.
    pub fn closing(&self) -> bool {
        self.closing
    }

    /// La fenetre du jeu est connue.
    pub fn window(&self) -> bool {
        self.window
    }

    /// Un processus de jeu est rattache.
    pub fn attached(&self) -> bool {
        self.attached
    }

    /// Cadence du minuteur dans l'etat courant.
    pub fn cadence(&self) -> Cadence {
        if self.flow.curtain_on() {
            Cadence::Fast
        } else if self.attached {
            Cadence::Slow
        } else {
            Cadence::Off
        }
    }

    /// Transition sans mutation : (nouvel etat, effets).
    pub fn step(mut self, ev: Event, now: Instant) -> (Machine, Vec<Effect>) {
        let fx = self.apply(ev, now);
        (self, fx)
    }

    /// Applique `ev` a l'instant `now` et rend les effets a executer, dans l'ordre.
    pub fn apply(&mut self, ev: Event, now: Instant) -> Vec<Effect> {
        let mut fx = Vec::new();
        match ev {
            Event::BeginLaunch => {
                // la fermeture de l'ancien jeu vient d'occuper la premiere moitie de la barre
                let bar = if self.closing { Bar::SecondHalf } else { Bar::Full };
                self.closing = false;
                let (shut, placed) = self.cover(now, &mut fx);
                self.flow = Flow::Loading { shut, placed, bar };
            }
            Event::BeginClose => {
                self.closing = true;
                let (shut, placed) = self.cover(now, &mut fx);
                self.flow = Flow::Closing { shut, placed };
            }
            Event::Hold => {
                if self.flow.shut().is_some() {
                    fx.push(Effect::ShowCurtain { fresh: false }); // deja ferme : seul le texte change
                } else {
                    let to = self.flow.view();
                    let (shut, placed) = self.cover(now, &mut fx);
                    self.flow = Flow::Held { shut, to, placed };
                }
            }
            Event::Reveal => self.reveal(now, &mut fx),
            Event::CloseAbandoned => self.abandon_close(),
            Event::CancelClose => {
                self.abandon_close();
                self.reveal(now, &mut fx);
            }
            Event::GameExited => {
                self.close_panel(&mut fx);
                if self.closing {
                    fx.push(Effect::CloseBarDone);
                } else {
                    self.leave(now, &mut fx);
                }
            }
            Event::Leave => self.leave(now, &mut fx),
            Event::SetView(to) => {
                self.close_panel(&mut fx);
                // deja la bonne vue et aucun basculement en cours : rien a faire
                if self.flow.view() != to || matches!(self.flow, Flow::Switching { .. }) {
                    let (shut, placed) = self.cover(now, &mut fx);
                    self.flow = Flow::Switching { shut, to, placed };
                }
            }
            Event::Watch => {
                self.close_panel(&mut fx);
                self.attached = true;
                self.window = false;
                self.flow.set_placed(false);
            }
            Event::WindowShown => self.window = true,
            Event::PanelOpen { width } => {
                if self.flow == Flow::InGame && self.window {
                    self.flow = Flow::Panel { width };
                    fx.push(Effect::SyncGame);
                }
            }
            Event::PanelClose => {
                if self.close_panel(&mut fx) {
                    fx.push(Effect::FocusGame);
                }
            }
            Event::Tick => self.tick(now, &mut fx),
        }
        fx
    }

    /// Ferme le panneau « en jeu » s'il est ouvert. Vrai s'il l'etait.
    fn close_panel(&mut self, fx: &mut Vec<Effect>) -> bool {
        let was = matches!(self.flow, Flow::Panel { .. });
        if was {
            self.flow = Flow::InGame;
            fx.push(Effect::ClearRegion);
        }
        was
    }

    /// Ferme le rideau (ou le laisse ferme) : animation a reprendre et position de la fenetre du jeu. Depuis une ouverture en cours,
    /// les panneaux repartent de leur position actuelle.
    fn cover(&mut self, now: Instant, fx: &mut Vec<Effect>) -> (Shut, bool) {
        self.close_panel(fx);
        let placed = self.flow.placed();
        let (shut, fresh) = match self.flow.shut() {
            Some((shut, _)) => (shut, false),
            None if self.flow.curtain_on() => (Shut { from: now, cover0: self.flow.levels(now, self.pace).0 }, false),
            None => (Shut { from: now, cover0: 0.0 }, true),
        };
        fx.push(Effect::ShowCurtain { fresh });
        (shut, placed)
    }

    fn reveal(&mut self, now: Instant, fx: &mut Vec<Effect>) {
        let open = self.flow.open(now, self.pace);
        match self.flow {
            Flow::Loading { placed, bar, .. } => self.flow = Flow::Revealing { open, to: View::Game, placed, content: Content::Launch(bar) },
            Flow::Closing { placed, .. } => self.flow = Flow::Revealing { open, to: View::Game, placed, content: Content::Close },
            Flow::Held { to, placed, .. } => self.flow = Flow::Revealing { open, to, placed, content: Content::Wait },
            // rideau deja retire sur la vue « jeu » alors que la fenetre attendait hors ecran : elle arrive telle quelle
            Flow::Pending if self.window => {
                self.flow = Flow::InGame;
                fx.push(Effect::PlaceGame);
            }
            // un basculement se rouvre tout seul ; ailleurs il n'y a rien a ouvrir
            _ => {}
        }
    }

    fn abandon_close(&mut self) {
        self.closing = false;
        match self.flow {
            Flow::Closing { shut, placed } => self.flow = Flow::Held { shut, to: View::Game, placed },
            Flow::Revealing { open, to, placed, content: Content::Close } => self.flow = Flow::Revealing { open, to, placed, content: Content::Wait },
            _ => {}
        }
    }

    /// Le jeu est parti : retour au launcher sous les panneaux.
    fn leave(&mut self, now: Instant, fx: &mut Vec<Effect>) {
        self.close_panel(fx);
        self.closing = false;
        self.attached = false;
        self.window = false;
        self.flow.set_placed(false); // la fenetre du jeu n'existe plus : rien a cacher
        fx.push(Effect::Detach);
        let (shut, _) = self.cover(now, fx);
        self.flow = Flow::Switching { shut, to: View::Launcher, placed: false };
    }

    fn tick(&mut self, now: Instant, fx: &mut Vec<Effect>) {
        // 1. pose / retrait de la fenetre du jeu : seulement sous les panneaux fermes (ou pendant qu'ils s'ouvrent sur le jeu)
        let covered = self.flow.covered(now, self.pace);
        if self.window {
            let (to, placed) = (self.flow.view(), self.flow.placed());
            if to == View::Game && !placed && (covered || matches!(self.flow, Flow::Revealing { .. })) {
                self.flow.set_placed(true);
                fx.push(Effect::PlaceGame);
            } else if to == View::Launcher && placed && covered {
                self.flow.set_placed(false);
                fx.push(Effect::HideGame);
            }
        }
        // 2. basculement fait sous les panneaux (ou plus de fenetre de jeu a attendre) : ils se rouvrent
        if let Flow::Switching { to, placed, .. } = self.flow {
            if covered && (placed == (to == View::Game) || !self.window) {
                self.flow = Flow::Revealing { open: self.flow.open(now, self.pace), to, placed, content: Content::Wait };
            }
        }
        // 3. panneaux ouverts : le rideau se retire
        if let Flow::Revealing { open, to, placed, .. } = self.flow {
            if ms(now, open.from) >= OPEN_DELAY_MS + self.pace.open_ms() {
                self.flow = match (to, placed) {
                    (View::Game, true) => Flow::InGame,
                    (View::Game, false) => Flow::Pending,
                    (View::Launcher, false) => Flow::Launcher,
                    (View::Launcher, true) => {
                        fx.push(Effect::HideGame); // ouverture forcee avant le retrait : jamais de jeu visible sur la vue launcher
                        Flow::Launcher
                    }
                };
                self.closing = false;
                fx.push(Effect::HideCurtain);
            }
        }
        fx.push(Effect::Timer(self.cadence()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Pilote de test : horloge simulee, journal des effets.
    struct Sim {
        m: Machine,
        t0: Instant,
        now: Instant,
    }

    impl Sim {
        fn new() -> Sim {
            let t0 = Instant::now();
            Sim { m: Machine::new(), t0, now: t0 }
        }

        fn send(&mut self, ev: Event) -> Vec<Effect> {
            self.m.apply(ev, self.now)
        }

        /// Avance de `ms` millisecondes par battements de 16 ms ; rend tous les effets (hors cadence) et verifie au passage que
        /// le niveau des panneaux ne saute jamais.
        fn run(&mut self, ms: u64) -> Vec<Effect> {
            let end = self.now + Duration::from_millis(ms);
            let mut all = Vec::new();
            let mut last = self.m.levels(self.now).0;
            while self.now < end {
                self.now = (self.now + Duration::from_millis(16)).min(end);
                let before = self.m.levels(self.now).0;
                // pente maximale de `ease` : 3, soit 0,093 par battement de 16 ms sur CLOSE_MS
                assert!((before - last).abs() < 0.1, "saut des panneaux entre deux battements : {last} -> {before} ({:?})", self.m.flow());
                let fx = self.m.apply(Event::Tick, self.now);
                let after = self.m.levels(self.now).0;
                assert!((after - before).abs() < 0.002, "saut des panneaux dans un battement : {before} -> {after} ({:?})", self.m.flow());
                last = after;
                all.extend(fx.into_iter().filter(|e| !matches!(e, Effect::Timer(_))));
            }
            all
        }

        fn cover(&self) -> f32 {
            self.m.levels(self.now).0
        }

        fn card(&self) -> f32 {
            self.m.levels(self.now).1
        }

        fn at_ms(&self) -> u128 {
            self.now.duration_since(self.t0).as_millis()
        }
    }

    const REVEAL_MS: u64 = (OPEN_DELAY_MS + OPEN_MS) as u64 + 32;

    /// Lancement complet depuis le launcher jusque dans le jeu.
    fn launched() -> Sim {
        let mut s = Sim::new();
        s.send(Event::BeginLaunch);
        s.send(Event::Watch);
        s.run(1500);
        s.send(Event::WindowShown);
        s.run(3000);
        s.send(Event::Reveal);
        s.run(REVEAL_MS);
        assert_eq!(s.m.flow(), Flow::InGame);
        s
    }

    #[test]
    fn courbe_et_constantes() {
        assert_eq!((CLOSE_MS, CARD_IN_MS, QUICK_CARD_DELAY_MS, OPEN_DELAY_MS, OPEN_MS), (520.0, 260.0, 2000.0, 170.0, 650.0));
        assert_eq!(ease(-1.0), 0.0);
        assert_eq!(ease(0.0), 0.0);
        assert!((ease(0.5) - 0.5).abs() < 1e-6);
        assert_eq!(ease(1.0), 1.0);
        assert_eq!(ease(7.0), 1.0);
        let mut last = 0.0;
        for i in 0..=100 {
            let v = ease(i as f32 / 100.0);
            assert!(v >= last, "ease doit etre monotone");
            last = v;
        }
    }

    #[test]
    fn lancement_normal() {
        let mut s = Sim::new();
        assert_eq!(s.m.flow(), Flow::Launcher);
        assert_eq!(s.m.cadence(), Cadence::Off);
        // debut : les panneaux partent de zero, toute la barre est au lancement
        assert_eq!(s.send(Event::BeginLaunch), vec![Effect::ShowCurtain { fresh: true }]);
        assert!(matches!(s.m.flow(), Flow::Loading { placed: false, bar: Bar::Full, .. }));
        assert_eq!(s.m.view(), View::Game);
        assert_eq!(s.m.content(), Content::Launch(Bar::Full));
        assert_eq!(s.cover(), 0.0);
        assert_eq!(s.send(Event::Watch), vec![]);
        // la fenetre du jeu apparait AVANT la fin de la fermeture : elle reste hors ecran
        s.run(200);
        s.send(Event::WindowShown);
        assert_eq!(s.run(200), vec![], "rien n'est pose tant que les panneaux ne sont pas fermes");
        assert!(s.cover() < 0.999 && s.card() == 0.0);
        // panneaux fermes : la fenetre est posee dessous, le contenu apparait ensuite
        assert_eq!(s.run(200), vec![Effect::PlaceGame]);
        assert!(matches!(s.m.flow(), Flow::Loading { placed: true, .. }));
        assert_eq!(s.cover(), 1.0);
        s.run(300);
        assert_eq!(s.card(), 1.0);
        assert!(!s.m.in_game(), "pas encore dans le jeu : rideau ferme");
        // le rideau ne s'ouvre jamais tout seul pendant un chargement
        assert_eq!(s.run(5000), vec![]);
        assert!(matches!(s.m.flow(), Flow::Loading { .. }));
        // pret : le contenu s'efface, puis les panneaux s'ecartent
        assert_eq!(s.send(Event::Reveal), vec![]);
        assert!(matches!(s.m.flow(), Flow::Revealing { to: View::Game, placed: true, content: Content::Launch(Bar::Full), .. }));
        assert_eq!((s.cover(), s.card()), (1.0, 1.0));
        s.run(160);
        assert_eq!(s.cover(), 1.0, "les panneaux attendent que le contenu soit efface");
        assert_eq!(s.card(), 0.0);
        assert_eq!(s.send(Event::Reveal), vec![], "une seconde demande ne relance pas l'ouverture");
        let fx = s.run(REVEAL_MS - 160);
        assert_eq!(fx, vec![Effect::HideCurtain]);
        assert_eq!(s.m.flow(), Flow::InGame);
        assert!(s.m.in_game());
        assert_eq!(s.m.cadence(), Cadence::Slow, "jeu rattache, rideau retire : filet de securite lent");
        assert_eq!(s.m.levels(s.now), (0.0, 0.0));
    }

    #[test]
    fn fenetre_apparue_apres_la_fermeture_est_posee_au_battement_suivant() {
        let mut s = Sim::new();
        s.send(Event::BeginLaunch);
        s.send(Event::Watch);
        s.run(2000);
        s.send(Event::WindowShown);
        assert_eq!(s.run(16), vec![Effect::PlaceGame]);
    }

    #[test]
    fn changement_de_modpack_barre_monotone() {
        let mut s = launched();
        // premiere moitie : fermeture propre de l'ancien jeu, panneaux fermes sur lui
        assert_eq!(s.send(Event::BeginClose), vec![Effect::ShowCurtain { fresh: true }]);
        assert!(matches!(s.m.flow(), Flow::Closing { placed: true, .. }));
        assert!(s.m.closing());
        let mut bar = Vec::new();
        for raw in [0.0, 0.3, 0.8, 1.0] {
            s.run(300);
            bar.push(bar_fraction(s.m.content(), raw));
        }
        // l'ancien jeu s'arrete : 50 %, et RIEN d'autre (pas de retour au launcher)
        assert_eq!(s.send(Event::GameExited), vec![Effect::CloseBarDone]);
        assert!(matches!(s.m.flow(), Flow::Closing { .. }));
        bar.push(bar_fraction(s.m.content(), 1.0));
        assert_eq!(*bar.last().unwrap(), 0.5);
        assert_eq!(s.run(500), vec![], "le rideau reste ferme entre les deux jeux");
        // seconde moitie : lancement du nouveau jeu, sans rouvrir ni refermer
        let shut_before = match s.m.flow() {
            Flow::Closing { shut, .. } => shut,
            f => panic!("{f:?}"),
        };
        assert_eq!(s.send(Event::BeginLaunch), vec![Effect::ShowCurtain { fresh: false }]);
        assert!(matches!(s.m.flow(), Flow::Loading { bar: Bar::SecondHalf, shut, .. } if shut == shut_before));
        assert!(!s.m.closing());
        assert_eq!(s.cover(), 1.0);
        s.send(Event::Watch);
        assert!(matches!(s.m.flow(), Flow::Loading { placed: false, .. }), "l'ancienne fenetre n'existe plus");
        for raw in [0.0, 0.2, 0.6, 1.0] {
            s.run(300);
            bar.push(bar_fraction(s.m.content(), raw));
        }
        assert!(bar.windows(2).all(|w| w[1] >= w[0]), "barre non monotone : {bar:?}");
        assert_eq!(bar[5], 0.5, "le lancement reprend la ou la fermeture s'est arretee");
        assert_eq!(*bar.last().unwrap(), 1.0);
        s.send(Event::WindowShown);
        assert_eq!(s.run(32), vec![Effect::PlaceGame]);
        s.send(Event::Reveal);
        assert_eq!(s.run(REVEAL_MS), vec![Effect::HideCurtain]);
        assert_eq!(s.m.flow(), Flow::InGame);
        // un lancement simple ensuite reprend toute la barre
        s.send(Event::BeginLaunch);
        assert_eq!(s.m.content(), Content::Launch(Bar::Full));
    }

    #[test]
    fn fermeture_refusee_rouvre_sur_l_ancien_jeu() {
        let mut s = launched();
        s.send(Event::BeginClose);
        s.run(180_000);
        assert!(matches!(s.m.flow(), Flow::Closing { placed: true, .. }), "jamais d'ouverture ni de retour au launcher sans decision");
        assert_eq!((s.cover(), s.card()), (1.0, 1.0));
        // delai depasse : le jeu reste, le rideau se rouvre sur lui
        assert_eq!(s.send(Event::CancelClose), vec![]);
        assert!(matches!(s.m.flow(), Flow::Revealing { to: View::Game, placed: true, content: Content::Wait, .. }));
        assert!(!s.m.closing());
        assert_eq!(s.run(REVEAL_MS), vec![Effect::HideCurtain], "ni Detach ni HideGame : le jeu n'est pas touche");
        assert_eq!(s.m.flow(), Flow::InGame);
        // la fin du jeu, plus tard, est alors un vrai retour au launcher
        let fx = s.send(Event::GameExited);
        assert_eq!(fx, vec![Effect::Detach, Effect::ShowCurtain { fresh: true }]);
        assert!(matches!(s.m.flow(), Flow::Switching { to: View::Launcher, placed: false, .. }));
    }

    #[test]
    fn fermeture_abandonnee_sans_ouverture() {
        let mut s = launched();
        s.send(Event::BeginClose);
        s.run(1000);
        assert_eq!(s.send(Event::CloseAbandoned), vec![]);
        assert!(matches!(s.m.flow(), Flow::Held { to: View::Game, placed: true, .. }));
        assert_eq!(s.m.content(), Content::Wait);
        assert_eq!(s.run(3000), vec![], "le rideau reste ferme tant que l'appelant n'a pas decide");
        s.send(Event::Reveal);
        s.run(REVEAL_MS);
        assert_eq!(s.m.flow(), Flow::InGame);
    }

    #[test]
    fn jeu_plante_pendant_le_chargement() {
        let mut s = Sim::new();
        s.send(Event::BeginLaunch);
        s.send(Event::Watch);
        s.run(1200);
        s.send(Event::WindowShown);
        s.run(800);
        assert!(matches!(s.m.flow(), Flow::Loading { placed: true, .. }));
        // le jeu meurt : sa fenetre n'existe plus (rien a cacher), retour au launcher avec l'animation du rideau
        let fx = s.send(Event::GameExited);
        assert_eq!(fx, vec![Effect::Detach, Effect::ShowCurtain { fresh: false }]);
        assert!(matches!(s.m.flow(), Flow::Switching { to: View::Launcher, placed: false, .. }));
        assert!(!s.m.attached() && !s.m.window());
        assert_eq!(s.cover(), 1.0, "les panneaux etaient fermes : ils le restent");
        s.run(16);
        assert!(matches!(s.m.flow(), Flow::Revealing { to: View::Launcher, content: Content::Wait, .. }));
        assert_eq!(s.run(REVEAL_MS), vec![Effect::HideCurtain]);
        assert_eq!(s.m.flow(), Flow::Launcher);
        assert_eq!(s.m.cadence(), Cadence::Off, "ni animation ni jeu : plus aucun reveil");
    }

    #[test]
    fn echec_avant_toute_fenetre_depuis_le_launcher() {
        let mut s = Sim::new();
        // le jeu se termine alors que le launcher est affiche, rideau retire : fermeture complete puis ouverture
        s.send(Event::Watch);
        assert_eq!(s.send(Event::GameExited), vec![Effect::Detach, Effect::ShowCurtain { fresh: true }]);
        assert_eq!(s.cover(), 0.0);
        s.run(400);
        assert!(matches!(s.m.flow(), Flow::Switching { .. }) && s.cover() > 0.5);
        s.run(200);
        assert!(matches!(s.m.flow(), Flow::Revealing { .. }));
        s.run(REVEAL_MS);
        assert_eq!(s.m.flow(), Flow::Launcher);
    }

    #[test]
    fn bascule_jeu_launcher_et_retour() {
        let mut s = launched();
        assert_eq!(s.send(Event::SetView(View::Game)), vec![], "deja dans le jeu");
        // vers le launcher : la fenetre du jeu n'est cachee qu'une fois les panneaux fermes
        assert_eq!(s.send(Event::SetView(View::Launcher)), vec![Effect::ShowCurtain { fresh: true }]);
        assert_eq!(s.m.view(), View::Launcher);
        assert_eq!(s.run(470), vec![]);
        assert_eq!(s.run(64), vec![Effect::HideGame]);
        assert!(matches!(s.m.flow(), Flow::Revealing { to: View::Launcher, placed: false, .. }));
        assert_eq!(s.card(), 0.0, "basculement court : panneaux seuls, pas d'ecran de chargement");
        assert_eq!(s.run(REVEAL_MS), vec![Effect::HideCurtain]);
        assert_eq!(s.m.flow(), Flow::Launcher);
        assert!(s.m.attached(), "le jeu tourne toujours");
        assert_eq!(s.m.cadence(), Cadence::Slow);
        // retour au jeu
        assert_eq!(s.send(Event::SetView(View::Launcher)), vec![]);
        assert_eq!(s.send(Event::SetView(View::Game)), vec![Effect::ShowCurtain { fresh: true }]);
        assert_eq!(s.run(560), vec![Effect::PlaceGame]);
        assert_eq!(s.run(REVEAL_MS), vec![Effect::HideCurtain]);
        assert_eq!(s.m.flow(), Flow::InGame);
    }

    #[test]
    fn bascule_pendant_une_animation_sans_saut() {
        let mut s = launched();
        // `run` verifie la continuite du niveau des panneaux a chaque battement
        s.send(Event::SetView(View::Launcher));
        s.run(260); // a mi-fermeture
        let mid = s.cover();
        assert!(mid > 0.05 && mid < 0.95);
        assert_eq!(s.send(Event::SetView(View::Game)), vec![Effect::ShowCurtain { fresh: false }]);
        assert_eq!(s.cover(), mid, "retournement en cours de fermeture : meme position");
        assert_eq!(s.m.view(), View::Game);
        // les panneaux finissent de se fermer puis se rouvrent sur le jeu, qui n'a jamais ete cache
        let fx = s.run(300);
        assert_eq!(fx, vec![]);
        assert!(matches!(s.m.flow(), Flow::Revealing { to: View::Game, placed: true, .. }));
        // retournement en cours d'OUVERTURE : les panneaux repartent de leur position
        s.run(500);
        let open = s.cover();
        assert!(open > 0.05 && open < 0.95, "{open}");
        assert_eq!(s.send(Event::SetView(View::Launcher)), vec![Effect::ShowCurtain { fresh: false }]);
        assert!((s.cover() - open).abs() < 1e-6, "pas de saut a la refermeture");
        assert_eq!(s.card(), 0.0);
        let fx = s.run(560);
        assert_eq!(fx, vec![Effect::HideGame], "cache seulement une fois les panneaux refermes");
        s.run(REVEAL_MS);
        assert_eq!(s.m.flow(), Flow::Launcher);
        // rafale de clics : toujours continu, et l'etat final suit le dernier
        for i in 0..12 {
            s.send(Event::SetView(if i % 2 == 0 { View::Game } else { View::Launcher }));
            s.run(90 + i * 37);
        }
        s.run(3000);
        assert_eq!(s.m.flow(), Flow::Launcher);
        assert!(s.at_ms() > 0);
    }

    #[test]
    fn niveaux_continus_a_la_refermeture() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        // ouverture commencee a 0 (contenu visible), refermee a 500 ms
        let opening = Flow::Revealing { open: Open { from: t0, cover0: 1.0, card0: 1.0 }, to: View::Game, placed: true, content: Content::Wait };
        let n = Pace::Normal;
        assert_eq!(opening.levels(at(0), n), (1.0, 1.0));
        let (c170, k170) = opening.levels(at(170), n);
        assert_eq!((c170, k170), (1.0, 0.0), "le contenu est efface avant que les panneaux ne bougent");
        let (c500, _) = opening.levels(at(500), n);
        assert!(c500 > 0.3 && c500 < 0.7, "{c500}");
        assert_eq!(opening.levels(at(820), n).0, 0.0);
        let mut m = Machine { flow: opening, window: true, attached: true, closing: false, pace: n };
        m.apply(Event::BeginLaunch, at(500));
        let Flow::Loading { shut, .. } = m.flow() else { panic!("{:?}", m.flow()) };
        assert_eq!(shut, Shut { from: at(500), cover0: c500 });
        assert_eq!(m.levels(at(500)).0, c500, "la refermeture part de la position atteinte");
        // monotone jusqu'a la fermeture complete, puis le contenu revient
        let mut last = c500;
        for ms in (500..=1020).step_by(4) {
            let (c, k) = m.levels(at(ms));
            assert!(c >= last - 1e-6 && c - last < 0.03, "{last} -> {c} a {ms} ms");
            assert!(c >= 0.999 || k == 0.0, "pas de contenu sur des panneaux entrouverts");
            last = c;
        }
        assert_eq!(m.levels(at(1020)).0, 1.0);
        assert_eq!(m.levels(at(1020 + 260)).1, 1.0);
        // un basculement differe son contenu de QUICK_CARD_DELAY_MS
        let quick = Flow::Switching { shut: Shut { from: t0, cover0: 0.0 }, to: View::Game, placed: false };
        assert_eq!(quick.levels(at(520 + 1990), n), (1.0, 0.0));
        assert_eq!(quick.levels(at(520 + 2000 + 260), n), (1.0, 1.0));
    }

    #[test]
    fn ouverture_demandee_avant_la_fermeture_complete_sans_saut() {
        let mut s = Sim::new();
        s.send(Event::BeginLaunch);
        s.run(250);
        let c = s.cover();
        assert!(c > 0.0 && c < 0.999);
        s.send(Event::Reveal);
        assert_eq!(s.cover(), c);
        s.run(REVEAL_MS); // continuite verifiee par `run`
        assert_eq!(s.m.flow(), Flow::Pending, "aucune fenetre de jeu : vue jeu demandee, rien de pose");
    }

    #[test]
    fn panneau_refuse_hors_du_jeu() {
        let open = Event::PanelOpen { width: 380 };
        let mut s = Sim::new();
        assert_eq!(s.send(open), vec![], "launcher");
        s.send(Event::BeginLaunch);
        s.send(Event::Watch);
        assert_eq!(s.send(open), vec![], "fermeture du rideau");
        s.run(1000);
        s.send(Event::WindowShown);
        s.run(100);
        assert_eq!(s.send(open), vec![], "chargement");
        s.send(Event::Reveal);
        s.run(400);
        assert_eq!(s.send(open), vec![], "ouverture du rideau");
        s.run(REVEAL_MS);
        // dans le jeu : accepte une fois
        assert_eq!(s.send(open), vec![Effect::SyncGame]);
        assert_eq!(s.m.flow(), Flow::Panel { width: 380 });
        assert_eq!(s.m.panel_width(), 380);
        assert!(s.m.in_game(), "le panneau n'enleve pas le joueur du jeu");
        assert_eq!(s.send(open), vec![], "deja ouvert");
        // fermeture : la fenetre retrouve sa surface et le premier plan
        assert_eq!(s.send(Event::PanelClose), vec![Effect::ClearRegion, Effect::FocusGame]);
        assert_eq!(s.m.flow(), Flow::InGame);
        assert_eq!(s.send(Event::PanelClose), vec![], "deja ferme");
        // pendant un basculement et sur le launcher : refuse
        s.send(Event::SetView(View::Launcher));
        assert_eq!(s.send(open), vec![]);
        s.run(2000);
        assert_eq!(s.m.flow(), Flow::Launcher);
        assert_eq!(s.send(open), vec![]);
        assert_eq!(s.m.panel_width(), 0);
    }

    #[test]
    fn tout_depart_du_jeu_ferme_le_panneau() {
        let open = Event::PanelOpen { width: 300 };
        for (ev, rest) in [
            (Event::SetView(View::Launcher), vec![Effect::ShowCurtain { fresh: true }]),
            (Event::SetView(View::Game), vec![]),
            (Event::BeginLaunch, vec![Effect::ShowCurtain { fresh: true }]),
            (Event::BeginClose, vec![Effect::ShowCurtain { fresh: true }]),
            (Event::GameExited, vec![Effect::Detach, Effect::ShowCurtain { fresh: true }]),
            (Event::Leave, vec![Effect::Detach, Effect::ShowCurtain { fresh: true }]),
        ] {
            let mut s = launched();
            s.send(open);
            let fx = s.send(ev);
            assert_eq!(fx[0], Effect::ClearRegion, "{ev:?}");
            assert_eq!(fx[1..], rest[..], "{ev:?}");
            assert_eq!(s.m.panel_width(), 0, "{ev:?}");
        }
    }

    #[test]
    fn retour_au_launcher_pendant_un_chargement_puis_pret() {
        let mut s = Sim::new();
        s.send(Event::BeginLaunch);
        s.send(Event::Watch);
        s.run(1000);
        // le joueur retourne au launcher avant que la fenetre du jeu n'existe : ouverture immediate (rien a cacher)
        s.send(Event::SetView(View::Launcher));
        s.run(16);
        assert!(matches!(s.m.flow(), Flow::Revealing { to: View::Launcher, .. }));
        s.run(REVEAL_MS);
        assert_eq!(s.m.flow(), Flow::Launcher);
        // la fenetre apparait en arriere-plan : rien ne bouge
        s.send(Event::WindowShown);
        assert_eq!(s.run(500), vec![]);
        // le jeu annonce qu'il est pret alors que le launcher est affiche : rien ne bouge non plus
        assert_eq!(s.send(Event::Reveal), vec![]);
        assert_eq!(s.m.flow(), Flow::Launcher);
        // retour au jeu : pose sous les panneaux, ouverture automatique
        s.send(Event::SetView(View::Game));
        assert_eq!(s.run(560), vec![Effect::PlaceGame]);
        s.run(REVEAL_MS);
        assert_eq!(s.m.flow(), Flow::InGame);
    }

    #[test]
    fn vue_jeu_sans_fenetre_puis_pret() {
        let mut s = Sim::new();
        s.send(Event::BeginLaunch);
        s.send(Event::Watch);
        s.run(1000);
        s.send(Event::SetView(View::Launcher));
        s.run(100);
        // retour vers le jeu alors que sa fenetre n'existe pas encore : les panneaux se rouvrent, rien n'est pose
        s.send(Event::SetView(View::Game));
        s.run(2000);
        assert_eq!(s.m.flow(), Flow::Pending);
        assert_eq!(s.m.view(), View::Game);
        assert!(!s.m.in_game());
        s.send(Event::WindowShown);
        assert_eq!(s.run(1000), vec![], "la fenetre attend hors ecran que le jeu soit pret");
        // pret : la fenetre est posee directement, sans rideau
        assert_eq!(s.send(Event::Reveal), vec![Effect::PlaceGame]);
        assert_eq!(s.m.flow(), Flow::InGame);
    }

    #[test]
    fn ouverture_ignoree_pendant_un_basculement() {
        let mut s = launched();
        s.send(Event::SetView(View::Launcher));
        s.run(100);
        assert_eq!(s.send(Event::Reveal), vec![]);
        assert!(matches!(s.m.flow(), Flow::Switching { to: View::Launcher, placed: true, .. }), "le basculement va a son terme");
        assert_eq!(s.run(480), vec![Effect::HideGame]);
        s.run(REVEAL_MS);
        assert_eq!(s.m.flow(), Flow::Launcher);
    }

    #[test]
    fn bascule_pendant_la_fermeture_de_l_ancien_jeu() {
        let mut s = launched();
        s.send(Event::BeginClose);
        s.run(1000);
        // le joueur retourne au launcher : la fermeture propre continue derriere
        s.send(Event::SetView(View::Launcher));
        assert!(s.m.closing());
        assert_eq!(s.m.content(), Content::Wait);
        assert_eq!(s.run(16), vec![Effect::HideGame]);
        // l'ancien jeu s'arrete pendant l'ouverture : toujours pas un retour au launcher « de fin de jeu »
        assert_eq!(s.send(Event::GameExited), vec![Effect::CloseBarDone]);
        // le lancement suivant prend la seconde moitie et referme depuis la position atteinte
        s.run(400);
        let c = s.cover();
        s.send(Event::BeginLaunch);
        assert!(matches!(s.m.flow(), Flow::Loading { bar: Bar::SecondHalf, .. }));
        assert!((s.cover() - c).abs() < 1e-6);
        // une ouverture arrivee a son terme oublie la fermeture propre
        let mut s = launched();
        s.send(Event::BeginClose);
        s.run(1000);
        s.send(Event::SetView(View::Launcher));
        s.run(2000);
        assert_eq!(s.m.flow(), Flow::Launcher);
        assert!(!s.m.closing());
        assert_eq!(s.send(Event::GameExited)[0], Effect::Detach);
    }

    #[test]
    fn rideau_tenu_a_la_main() {
        let mut s = launched();
        assert_eq!(s.send(Event::Hold), vec![Effect::ShowCurtain { fresh: true }]);
        assert!(matches!(s.m.flow(), Flow::Held { to: View::Game, placed: true, .. }));
        s.run(5000);
        assert!(matches!(s.m.flow(), Flow::Held { .. }), "jamais d'ouverture automatique");
        assert_eq!(s.send(Event::Hold), vec![Effect::ShowCurtain { fresh: false }]);
        s.send(Event::Reveal);
        s.run(REVEAL_MS);
        assert_eq!(s.m.flow(), Flow::InGame);
        // tenir le rideau pendant un chargement ne change pas d'etat
        s.send(Event::BeginLaunch);
        let before = s.m.flow();
        s.send(Event::Hold);
        assert_eq!(s.m.flow(), before);
    }

    #[test]
    fn cadence_du_minuteur() {
        let mut s = Sim::new();
        let cadence = |s: &mut Sim| {
            s.now += Duration::from_millis(16);
            *s.m.apply(Event::Tick, s.now).last().unwrap()
        };
        assert_eq!(cadence(&mut s), Effect::Timer(Cadence::Off));
        s.send(Event::Watch);
        assert_eq!(cadence(&mut s), Effect::Timer(Cadence::Slow));
        s.send(Event::BeginLaunch);
        assert_eq!(cadence(&mut s), Effect::Timer(Cadence::Fast));
        s.send(Event::Reveal);
        assert_eq!(cadence(&mut s), Effect::Timer(Cadence::Fast));
        s.run(REVEAL_MS);
        assert_eq!(cadence(&mut s), Effect::Timer(Cadence::Slow));
        s.send(Event::Leave);
        assert_eq!(cadence(&mut s), Effect::Timer(Cadence::Fast));
        s.run(3000);
        assert_eq!(cadence(&mut s), Effect::Timer(Cadence::Off));
    }

    #[test]
    fn step_ne_modifie_pas_l_original() {
        let m = Machine::new();
        let now = Instant::now();
        let (n, fx) = m.step(Event::BeginLaunch, now);
        assert_eq!(m.flow(), Flow::Launcher);
        assert!(matches!(n.flow(), Flow::Loading { .. }));
        assert_eq!(fx, vec![Effect::ShowCurtain { fresh: true }]);
    }

    /// Battements de 16 ms pendant `ms` millisecondes, SANS le controle de continuite de `run` (en mouvement reduit les panneaux
    /// sautent, c'est voulu) ; verifie a chaque battement que les panneaux sont soit ouverts soit fermes, jamais entre les deux.
    fn run_calm(s: &mut Sim, ms: u64) -> Vec<Effect> {
        let end = s.now + Duration::from_millis(ms);
        let mut all = Vec::new();
        while s.now < end {
            s.now = (s.now + Duration::from_millis(16)).min(end);
            let fx = s.m.apply(Event::Tick, s.now);
            let c = s.cover();
            assert!(c == 0.0 || c == 1.0, "glissement en mouvement reduit : {c} ({:?})", s.m.flow());
            all.extend(fx.into_iter().filter(|e| !matches!(e, Effect::Timer(_))));
        }
        all
    }

    #[test]
    fn mouvement_reduit_sans_glissement() {
        assert_eq!((Pace::Calm.close_ms(), Pace::Calm.open_ms()), (0.0, 0.0));
        assert_eq!((Pace::Normal.close_ms(), Pace::Normal.open_ms()), (CLOSE_MS, OPEN_MS));
        let mut s = Sim::new();
        assert!(!s.m.calm());
        s.m.set_calm(true);
        assert!(s.m.calm());
        // lancement : panneaux poses d'un bloc, des l'instant du `begin`
        assert_eq!(s.send(Event::BeginLaunch), vec![Effect::ShowCurtain { fresh: true }]);
        assert_eq!(s.cover(), 1.0);
        s.send(Event::Watch);
        s.send(Event::WindowShown);
        assert_eq!(run_calm(&mut s, 16), vec![Effect::PlaceGame], "panneaux deja fermes : pose au premier battement");
        run_calm(&mut s, 300);
        assert_eq!(s.card(), 1.0, "le contenu apparait toujours en fondu");
        assert_eq!(run_calm(&mut s, 3000), vec![], "jamais d'ouverture sans `reveal`");
        // ouverture : le contenu s'efface (fondu), puis les panneaux sont retires d'un bloc
        s.send(Event::Reveal);
        assert_eq!(run_calm(&mut s, 160), vec![]);
        assert_eq!(s.cover(), 1.0);
        assert_eq!(run_calm(&mut s, 32), vec![Effect::HideCurtain]);
        assert_eq!(s.m.flow(), Flow::InGame);
        assert_eq!(s.cover(), 0.0);
        // basculement : la fenetre du jeu est retiree sous des panneaux FERMES, qui le restent le temps du delai d'ouverture
        assert_eq!(s.send(Event::SetView(View::Launcher)), vec![Effect::ShowCurtain { fresh: true }]);
        assert_eq!(s.cover(), 1.0);
        assert_eq!(run_calm(&mut s, 16), vec![Effect::HideGame]);
        assert_eq!(s.cover(), 1.0, "le retrait est cache : panneaux encore fermes");
        assert_eq!(run_calm(&mut s, 200), vec![Effect::HideCurtain]);
        assert_eq!(s.m.flow(), Flow::Launcher);
        s.send(Event::SetView(View::Game));
        assert_eq!(run_calm(&mut s, 16), vec![Effect::PlaceGame]);
        assert_eq!(run_calm(&mut s, 200), vec![Effect::HideCurtain]);
        assert_eq!(s.m.flow(), Flow::InGame);
        // retour au mouvement normal : les panneaux glissent a nouveau (continuite verifiee par `run`)
        s.m.set_calm(false);
        s.send(Event::SetView(View::Launcher));
        s.run(260);
        assert!(s.cover() > 0.05 && s.cover() < 0.95);
    }

    #[test]
    fn mouvement_reduit_niveaux() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let shut = Flow::Loading { shut: Shut { from: t0, cover0: 0.0 }, placed: false, bar: Bar::Full };
        assert_eq!(shut.levels(at(0), Pace::Calm), (1.0, 0.0));
        assert!(shut.covered(at(0), Pace::Calm));
        assert!(!shut.covered(at(0), Pace::Normal));
        assert_eq!(shut.levels(at(260), Pace::Calm), (1.0, 1.0));
        let open = Flow::Revealing { open: Open { from: t0, cover0: 1.0, card0: 1.0 }, to: View::Game, placed: true, content: Content::Wait };
        assert_eq!(open.levels(at(0), Pace::Calm), (1.0, 1.0));
        assert_eq!(open.levels(at(169), Pace::Calm).0, 1.0);
        assert_eq!(open.levels(at(170), Pace::Calm), (0.0, 0.0));
    }

    #[test]
    fn repli_pendant_un_chargement() {
        // aucune fenetre : le rideau, ferme sur le launcher, se rouvre sur lui
        let mut s = Sim::new();
        s.send(Event::BeginLaunch);
        s.send(Event::Watch);
        s.run(5000);
        assert_eq!(s.send(Event::Leave), vec![Effect::Detach, Effect::ShowCurtain { fresh: false }]);
        assert!(!s.m.attached() && !s.m.window());
        assert_eq!(s.cover(), 1.0);
        s.run(16);
        assert!(matches!(s.m.flow(), Flow::Revealing { to: View::Launcher, placed: false, .. }));
        assert_eq!(s.run(REVEAL_MS), vec![Effect::HideCurtain]);
        assert_eq!(s.m.flow(), Flow::Launcher);
        assert_eq!(s.m.cadence(), Cadence::Off);
        // fenetre deja posee sous le rideau : la machine ne demande NI de la cacher NI de la poser (la couche systeme la rend)
        let mut s = Sim::new();
        s.send(Event::BeginLaunch);
        s.send(Event::Watch);
        s.run(1000);
        s.send(Event::WindowShown);
        s.run(500);
        assert!(matches!(s.m.flow(), Flow::Loading { placed: true, .. }));
        assert_eq!(s.send(Event::Leave), vec![Effect::Detach, Effect::ShowCurtain { fresh: false }]);
        assert_eq!(s.run(3000), vec![Effect::HideCurtain]);
        assert_eq!(s.m.flow(), Flow::Launcher);
        // le jeu rendu annonce plus tard qu'il est pret : rien ne bouge
        assert_eq!(s.send(Event::WindowShown), vec![]);
        assert_eq!(s.send(Event::Reveal), vec![]);
        assert_eq!(s.run(1000), vec![]);
        assert_eq!(s.m.flow(), Flow::Launcher);
    }

    #[test]
    fn repli_depuis_le_jeu_puis_nouveau_lancement() {
        // `release` en plein jeu, panneau ouvert : decoupe retiree, panneaux fermes puis rouverts sur le launcher
        let mut s = launched();
        s.send(Event::PanelOpen { width: 380 });
        assert_eq!(s.send(Event::Leave), vec![Effect::ClearRegion, Effect::Detach, Effect::ShowCurtain { fresh: true }]);
        assert!(matches!(s.m.flow(), Flow::Switching { to: View::Launcher, placed: false, .. }));
        assert_eq!(s.run(3000), vec![Effect::HideCurtain], "jamais HideGame : la fenetre rendue reste visible");
        assert_eq!(s.m.flow(), Flow::Launcher);
        // le lancement suivant repart de zero, sur toute la barre
        assert_eq!(s.send(Event::BeginLaunch), vec![Effect::ShowCurtain { fresh: true }]);
        assert_eq!(s.m.content(), Content::Launch(Bar::Full));
        s.send(Event::Watch);
        s.run(1000);
        s.send(Event::WindowShown);
        assert_eq!(s.run(32), vec![Effect::PlaceGame]);
        // repli pendant la fermeture propre de l'ancien jeu (changement de modpack) : la fermeture est oubliee
        let mut s = launched();
        s.send(Event::BeginClose);
        s.run(1000);
        assert_eq!(s.send(Event::Leave), vec![Effect::Detach, Effect::ShowCurtain { fresh: false }]);
        assert!(!s.m.closing());
        s.run(3000);
        assert_eq!(s.m.flow(), Flow::Launcher);
    }

    #[test]
    fn avancement_de_la_barre() {
        assert_eq!(bar_fraction(Content::Wait, 0.2), 1.0);
        assert_eq!(bar_fraction(Content::Close, 1.0), 0.5);
        assert_eq!(bar_fraction(Content::Launch(Bar::Full), 0.4), 0.4);
        assert_eq!(bar_fraction(Content::Launch(Bar::SecondHalf), 0.0), 0.5);
        assert_eq!(bar_fraction(Content::Launch(Bar::SecondHalf), 1.0), 1.0);
    }
}
