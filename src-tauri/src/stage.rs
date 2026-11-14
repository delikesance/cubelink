//! Zone de jeu integree au launcher (Windows) : la fenetre native du jeu est adoptee DANS la fenetre du launcher, sous la barre
//! de titre, et un rideau (deux panneaux, barre de chargement) masque les lancements et les changements de modpack. Voir le crate
//! `cubelink-host`. Hors Windows (ou en mode test) tout est inerte : le jeu garde sa propre fenetre.
//!
//! Filets de securite, tous ici :
//! - reglage « Jeu integre » (`set_wanted`) : lu au lancement, un jeu deja ouvert n'est jamais arrache a la fenetre ;
//! - repli : quand l'hote renonce (`HostEvent::Fallback`), le lancement en cours n'est plus « dans la Scene » (`staged`),
//!   l'interface redevient celle du launcher et le joueur est prevenu (`stage://fallback`) ;
//! - apres `MAX_FALLBACK_STREAK` replis de suite, la Scene est coupee jusqu'au redemarrage (le reglage enregistre ne bouge pas) ;
//! - un rideau ferme a toujours une suite : tout lancement qui n'a pas lieu apres un changement de modpack rouvre le rideau
//!   sur le launcher (`abort_switch`), toute tache de jeu qui s'arrete le rouvre aussi (`run_finished`) ;
//! - tout est consigne dans `logs/stage.log` (voir `stage_log`).
//!
//! Deux questions distinctes, a ne pas confondre : `available()` (le PROCHAIN lancement ira-t-il dans la Scene ?) et
//! `staged()` (le lancement EN COURS y est-il ?).

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;
use tauri::State;

use crate::stage_log::Outbox;
use crate::state::{lock, AppState, EventSink};

/// Geometrie de secours (pixels CSS), utilisee seulement tant que l'interface n'a pas publie la sienne (`stage_layout`) :
/// la source de verite est le DOM (`--titlebar-h`, `--panel-w`).
const FALLBACK_TITLEBAR_CSS_PX: f64 = 36.0;
const FALLBACK_PANEL_CSS_PX: f64 = 380.0;
/// Le jeu est « pret » quand il ne dit plus rien depuis ce delai (une fois textures et fenetre la).
pub const READY_QUIET: Duration = Duration::from_millis(2500);
/// Au-dela, on ouvre le rideau quoi qu'il arrive.
pub const READY_MAX: Duration = Duration::from_secs(90);
/// Replis de suite (un par lancement) au-dela desquels la Scene est coupee pour le reste de la session : sur cette machine
/// elle ne marche visiblement pas, inutile d'imposer le delai de garde a chaque lancement.
pub const MAX_FALLBACK_STREAK: u8 = 2;
/// Raison de repli qui ne compte pas comme un echec : c'est le launcher qui l'a demande (`GameHost::release`).
const REASON_REQUESTED: &str = "requested";

/// Etat publie a l'interface (`stage://state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
pub struct StageState {
    /// Un jeu est rattache a la fenetre du launcher.
    pub attached: bool,
    /// La zone du jeu est celle qu'on voit (sinon : le launcher).
    pub game: bool,
    /// Le panneau « en jeu » est ouvert (bande de droite du jeu).
    pub panel: bool,
}

/// Evenement `stage://fallback` : le jeu du lancement en cours a quitte la Scene pour sa propre fenetre.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FallbackNotice {
    /// Identifiant stable de la raison (`FallbackReason::as_str`).
    pub reason: String,
    /// La Scene vient d'etre coupee pour le reste de la session (trop de replis de suite).
    pub session_off: bool,
}

/// Pur : compteur de sante de la Scene sur la session. Un lancement compte au plus une fois ; un lancement adopte sans repli
/// remet le compteur a zero (« consecutifs »).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Health {
    /// Raisons des replis de suite (une par lancement), dans l'ordre : c'est ce que le journal donne a la coupure.
    streak: Vec<String>,
    adopted: bool,
    fell_back: bool,
    off: bool,
}

/// Comment s'est terminee la tache de jeu, vu de la Scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// Le jeu etait dans la Scene : l'hote doit revenir au launcher.
    Staged,
    /// Le jeu avait quitte la Scene par un repli : l'hote est deja revenu au launcher.
    FellBack,
    /// Le jeu n'a jamais ete dans la Scene (ou sa fin a deja ete traitee).
    Outside,
}

impl Health {
    /// Un lancement commence dans la Scene.
    pub fn begin(&mut self) {
        self.adopted = false;
        self.fell_back = false;
    }

    pub fn adopted(&mut self) {
        self.adopted = true;
    }

    /// Repli du lancement en cours pour `reason` ; `counts` : c'est un echec (pas une demande du launcher). Si la Scene VIENT
    /// d'etre coupee pour la session : les raisons des replis qui l'ont decide, dans l'ordre.
    pub fn fallback(&mut self, counts: bool, reason: &str) -> Option<Vec<String>> {
        if self.fell_back {
            return None;
        }
        self.fell_back = true;
        if !counts || self.off {
            return None;
        }
        self.streak.push(reason.to_string());
        if self.streak.len() >= usize::from(MAX_FALLBACK_STREAK) {
            self.off = true;
            return Some(self.streak.clone());
        }
        None
    }

    /// Fin du lancement : seul un lancement reellement adopte et jamais replie efface les echecs precedents (un lancement qui
    /// echoue avant toute fenetre ne prouve rien). Vrai si ce lancement s'etait replie (rendu une seule fois).
    pub fn finish(&mut self) -> bool {
        if self.adopted && !self.fell_back {
            self.streak.clear();
        }
        self.adopted = false;
        std::mem::take(&mut self.fell_back)
    }

    /// La Scene est coupee jusqu'au redemarrage.
    pub fn off(&self) -> bool {
        self.off
    }
}

/// Tout ce qui ne depend pas de la fenetre native : partage avec l'observateur de l'hote (appele depuis SES fils), donc sans
/// jamais toucher a l'hote ici.
#[derive(Default)]
struct Core {
    sink: OnceLock<EventSink>,
    last: Mutex<StageState>,
    health: Mutex<Health>,
    /// Le lancement en cours est dans la Scene (de `begin` au repli ou a la fin du jeu).
    live: AtomicBool,
    /// Rideau ferme pour un changement de modpack (`begin_close`) : il attend le `begin` du jeu suivant, ou `abort_switch`.
    switching: AtomicBool,
    /// Le rideau a ete ouvert sur le jeu de ce lancement : le joueur a pu y ouvrir un monde, il ne se ferme plus que proprement.
    /// Avant cela (preparation, chargement) le jeu n'a rien a enregistrer et peut etre arrete net.
    revealed: AtomicBool,
    /// Le launcher se ferme : le rideau ne s'ouvre plus sur aucun jeu (sinon la croix aurait « lance » le jeu).
    no_reveal: AtomicBool,
    /// Code de sortie du jeu, s'il s'est termine de lui-meme (pour la ligne de fin du journal).
    exit_code: Mutex<Option<i32>>,
    /// Reglage « Jeu integre ».
    wanted: AtomicBool,
    /// Mouvement reduit transmis a l'hote (pour ne journaliser que les changements).
    calm: AtomicBool,
    /// Journal et envois differes ; absent en test et hors Windows (tout se fait alors sur place, sans journal).
    outbox: OnceLock<Outbox>,
}

impl Core {
    fn note(&self, source: &str, msg: &str) {
        if let Some(o) = self.outbox.get() {
            o.line(source, msg);
        }
    }

    fn staged(&self) -> bool {
        self.live.load(Ordering::SeqCst)
    }

    /// Publie l'etat s'il a change. `deferred` : l'envoi a l'interface part du fil de sortie (appel depuis un fil de l'hote, qui
    /// ne doit pas attendre la fenetre du launcher).
    fn set_state(&self, mut now: StageState, deferred: bool) {
        let mut last = lock(&self.last);
        // Un repli a pu arriver entre le calcul de `now` et ici : il a le dernier mot.
        if now.attached && !self.staged() {
            now = StageState::default();
        }
        if *last == now {
            return;
        }
        *last = now;
        // Envoye sous le verrou : deux publications concurrentes arrivent a l'interface dans l'ordre ou elles ont ete decidees.
        if let Ok(v) = serde_json::to_value(now) {
            self.emit("stage://state", v, deferred);
        }
    }

    fn emit(&self, name: &'static str, payload: serde_json::Value, deferred: bool) {
        let Some(sink) = self.sink.get() else { return };
        match self.outbox.get() {
            Some(o) if deferred => {
                let sink = sink.clone();
                o.run(Box::new(move || sink(name, payload)));
            }
            _ => sink(name, payload),
        }
    }

    fn begin_launch(&self) {
        lock(&self.health).begin();
        *lock(&self.exit_code) = None;
        self.switching.store(false, Ordering::SeqCst);
        self.revealed.store(false, Ordering::SeqCst);
        self.live.store(true, Ordering::SeqCst);
    }

    /// Fin de la tache de jeu (sans effet la deuxieme fois : `run_finished` peut etre appele par plusieurs filets).
    fn end_launch(&self) -> Ending {
        let fell_back = lock(&self.health).finish();
        if self.live.swap(false, Ordering::SeqCst) {
            Ending::Staged
        } else if fell_back {
            Ending::FellBack
        } else {
            Ending::Outside
        }
    }

    // --- observateur de l'hote : appele depuis ses fils, doit rendre la main vite et ne jamais rappeler l'hote ---

    #[cfg_attr(not(any(windows, target_os = "linux", test)), allow(dead_code))]
    fn on_log(&self, line: &str) {
        self.note("hote", line);
    }

    #[cfg_attr(not(any(windows, target_os = "linux", test)), allow(dead_code))]
    fn on_adopted(&self, class: &str, after_ms: u64) {
        lock(&self.health).adopted();
        self.note("hote", &format!("fenetre du jeu adoptee (classe {class}, apres {after_ms} ms)"));
    }

    /// Le jeu entre ou sort du plein ecran : l'interface met la fenetre du launcher en plein ecran pour lui (`stage://fullscreen`).
    #[cfg_attr(not(any(windows, target_os = "linux", test)), allow(dead_code))]
    fn on_fullscreen(&self, on: bool) {
        self.emit("stage://fullscreen", serde_json::Value::Bool(on), true);
    }

    /// L'hote a rendu sa fenetre au jeu : a partir d'ici le lancement en cours se comporte comme hors Scene (plus de rideau a
    /// ouvrir, plus de sortie a suivre, fermer le launcher ne ferme plus le jeu).
    #[cfg_attr(not(any(windows, target_os = "linux", test)), allow(dead_code))]
    fn on_fallback(&self, reason: &str, detail: &str) {
        self.live.store(false, Ordering::SeqCst);
        let cut = lock(&self.health).fallback(reason != REASON_REQUESTED, reason);
        self.note("hote", &format!("repli ({reason}) : {detail}"));
        if let Some(reasons) = &cut {
            self.note("launcher", &session_off_line(reasons));
        }
        let session_off = cut.is_some();
        self.set_state(StageState::default(), true);
        if let Ok(v) = serde_json::to_value(FallbackNotice { reason: reason.to_string(), session_off }) {
            self.emit("stage://fallback", v, true);
        }
    }
}

/// Pur : ligne du journal a la coupure de session, avec la raison de chacun des replis qui l'ont decidee.
pub fn session_off_line(reasons: &[String]) -> String {
    format!(
        "Scene desactivee jusqu'au redemarrage de Cubelink : {} replis de suite ({}) ; le reglage enregistre ne change pas",
        reasons.len(),
        reasons.join(", puis ")
    )
}

/// Pur : ligne du journal a la fin d'un jeu sorti de la Scene par un repli (l'hote, deja revenu au launcher, n'en dit rien).
pub fn fell_back_exit_line(code: Option<i32>) -> String {
    match code {
        Some(c) => format!("jeu termine (hors Scene apres repli), code de sortie {c}"),
        None => "jeu termine (hors Scene apres repli)".to_string(),
    }
}

/// Ce que la Scene demande a l'hote. `cubelink_host::GameHost` en production ; un faux hote dans les tests, pour prouver
/// sans fenetre que chaque chemin rend le rideau.
pub(crate) trait HostApi: Send + Sync {
    fn relayout(&self, x: i32, y: i32, w: i32, h: i32);
    fn begin(&self, title: &str, mods_dir: &Path, profile_path: &Path);
    fn watch(&self, pid: u32);
    fn feed_line(&self, line: &str);
    fn is_ready(&self) -> bool;
    fn mark_ready(&self, profile_path: &Path);
    fn reveal(&self);
    fn begin_close(&self, title: &str);
    fn request_close(&self);
    fn cancel_close(&self);
    fn run_finished(&self);
    fn set_view(&self, game: bool);
    /// (un jeu est rattache, la zone du jeu est celle qu'on voit)
    fn view(&self) -> (bool, bool);
    fn panel(&self) -> bool;
    fn set_panel(&self, open: bool, width_px: i32) -> bool;
    fn set_calm(&self, calm: bool);
    /// Le jeu de ce lancement est cree avec la consigne « premiere fenetre cachee » : c'est a l'hote de l'afficher a la pose.
    fn expect_hidden(&self, hidden: bool);
}

#[cfg(any(windows, target_os = "linux"))]
impl HostApi for cubelink_host::GameHost {
    fn relayout(&self, x: i32, y: i32, w: i32, h: i32) {
        self.relayout(x, y, w, h)
    }
    fn begin(&self, title: &str, mods_dir: &Path, profile_path: &Path) {
        self.begin(title, "Lancement", Some(mods_dir), Some(profile_path))
    }
    fn watch(&self, pid: u32) {
        self.watch(pid)
    }
    fn feed_line(&self, line: &str) {
        self.feed_line(line)
    }
    fn is_ready(&self) -> bool {
        self.is_ready(READY_QUIET, READY_MAX)
    }
    fn mark_ready(&self, profile_path: &Path) {
        self.mark_ready(Some(profile_path))
    }
    fn reveal(&self) {
        self.reveal()
    }
    fn begin_close(&self, title: &str) {
        self.begin_close(title)
    }
    fn request_close(&self) {
        self.request_close()
    }
    fn cancel_close(&self) {
        self.cancel_close()
    }
    fn run_finished(&self) {
        self.run_finished()
    }
    fn set_view(&self, game: bool) {
        self.set_view(game)
    }
    fn view(&self) -> (bool, bool) {
        self.view()
    }
    fn panel(&self) -> bool {
        self.panel()
    }
    fn set_panel(&self, open: bool, width_px: i32) -> bool {
        self.set_panel(open, width_px)
    }
    fn set_calm(&self, calm: bool) {
        self.set_calm(calm)
    }
    fn expect_hidden(&self, hidden: bool) {
        self.expect_hidden(hidden)
    }
}

/// Essai « fenetre jamais affichee par le jeu » : `CUBELINK_STAGE_HIDE_START=1`, lue une fois. Sans elle rien ne change.
pub fn hide_start_switch() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("CUBELINK_STAGE_HIDE_START").is_ok_and(|v| v.trim() == "1"))
}

/// Pur : le processus du jeu peut-il etre cree avec sa premiere fenetre cachee ? Seulement si l'essai est demande ET que ce
/// lancement est dans la Scene, ou l'hote affichera lui-meme la fenetre : partout ailleurs ce serait un jeu invisible.
pub fn hide_first_window(switch: bool, staged: bool) -> bool {
    switch && staged
}

#[derive(Default)]
pub struct Stage {
    host: OnceLock<Box<dyn HostApi>>,
    core: Arc<Core>,
    /// Derniere geometrie publiee par l'interface.
    layout: Mutex<Option<Layout>>,
}

/// Zone de jeu publiee par l'interface, retenue en MARGES par rapport a la zone cliente (pixels physiques) : un redimensionnement
/// s'applique ainsi tout de suite, sans attendre la publication suivante.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub panel_w: i32,
}

impl Layout {
    /// Pur : marges deduites d'un rectangle `(x, y, w, h)` dans une zone cliente `(cw, ch)`.
    pub fn from_rect(x: i32, y: i32, w: i32, h: i32, panel_w: i32, cw: i32, ch: i32) -> Layout {
        // L'interface arrondit x, y, w, h separement : a une echelle fractionnaire (125 %, 150 %) le bord oppose peut tomber a
        // 1 px du bord de la fenetre. Une marge de 1 px est donc un artefact d'arrondi, pas une marge voulue.
        let snap = |m: i32| if m <= 1 { 0 } else { m };
        Layout { left: x.max(0), top: y.max(0), right: snap(cw - x - w), bottom: snap(ch - y - h), panel_w: panel_w.max(0) }
    }

    /// Pur : rectangle `(x, y, w, h)` de la zone de jeu pour une zone cliente `(cw, ch)`.
    pub fn rect(&self, cw: i32, ch: i32) -> (i32, i32, i32, i32) {
        (self.left, self.top, (cw - self.left - self.right).max(1), (ch - self.top - self.bottom).max(1))
    }
}

impl Stage {
    pub fn state(&self) -> StageState {
        *lock(&self.core.last)
    }

    /// Ouvre le journal permanent (`<dir>/stage.log`). Sans cet appel (tests, hors Windows) rien n'est ecrit.
    pub fn open_log(&self, dir: &Path) {
        match Outbox::start(dir.join(crate::stage_log::FILE_NAME)) {
            Ok(o) => {
                let _ = self.core.outbox.set(o);
            }
            Err(e) => eprintln!("[Cubelink] journal de la zone de jeu indisponible : {e}"),
        }
    }

    /// Une decision du launcher, consignee dans le journal.
    pub fn note(&self, msg: &str) {
        self.core.note("launcher", msg);
    }

    /// Reglage « Jeu integre » : pris en compte au prochain lancement.
    pub fn set_wanted(&self, on: bool) {
        if self.core.wanted.swap(on, Ordering::SeqCst) != on {
            self.note(if on { "reglage « Jeu integre » active (prochain lancement)" } else { "reglage « Jeu integre » desactive (prochain lancement)" });
        }
    }

    pub fn wanted(&self) -> bool {
        self.core.wanted.load(Ordering::SeqCst)
    }

    /// Le lancement EN COURS est dans la Scene : rideau a piloter, sortie du jeu a suivre, panneau « en jeu » possible. Faux
    /// des qu'un repli a rendu sa fenetre au jeu, et entre deux lancements.
    pub fn staged(&self) -> bool {
        self.core.staged()
    }

    /// Pourquoi le prochain lancement n'ira pas dans la Scene (`None` : il ira).
    pub fn unavailable_reason(&self) -> Option<&'static str> {
        if !self.has_host() {
            Some("zone de jeu integree absente")
        } else if !self.wanted() {
            Some("reglage « Jeu integre » desactive")
        } else if lock(&self.core.health).off() {
            Some("Scene desactivee pour la session apres des replis")
        } else {
            None
        }
    }

    /// Le PROCHAIN lancement ira dans la Scene : hote present, reglage active, pas de coupure de session.
    pub fn available(&self) -> bool {
        self.unavailable_reason().is_none()
    }
}

impl Stage {
    fn has_host(&self) -> bool {
        self.host.get().is_some()
    }

    fn publish(&self) {
        if let Some(h) = self.host.get() {
            let (attached, game) = h.view();
            let attached = attached && self.core.staged();
            self.core.set_state(StageState { attached, game: attached && game, panel: attached && h.panel() }, false);
        }
    }

    /// L'hote du lancement en cours, seulement tant que ce lancement est dans la Scene.
    fn live_host(&self) -> Option<&dyn HostApi> {
        self.host.get().filter(|_| self.core.staged()).map(|h| &**h)
    }

    /// Transmet a l'hote la preference systeme « afficher les animations » (relue a chaque lancement : pas besoin de
    /// surveiller les changements de reglages de Windows).
    fn refresh_calm(&self, h: &dyn HostApi, first: bool) {
        let calm = reduced_motion();
        if self.core.calm.swap(calm, Ordering::SeqCst) != calm || first {
            h.set_calm(calm);
            self.note(if calm { "mouvement reduit : rideau sans animation" } else { "animations du rideau actives" });
        }
    }

    /// Cree la zone du jeu dans la fenetre principale. A appeler depuis le fil principal (`setup`). Linux : la zone est une
    /// sous-fenetre X11 de celle du launcher (GDK_BACKEND=x11, voir `lib.rs`) ; sous Wayland natif (pas de XID) ou sans serveur
    /// X11 joignable, le jeu garde sa propre fenetre.
    #[cfg(any(windows, target_os = "linux"))]
    pub fn attach(&self, window: &tauri::WebviewWindow, sink: EventSink) {
        let Some(handle) = own_handle(window) else {
            self.note("Scene indisponible : fenetre du launcher sans identifiant natif (X11 requis sous Linux)");
            return;
        };
        match cubelink_host::GameHost::embed(handle) {
            Ok(h) => {
                let _ = self.core.sink.set(sink);
                let core = self.core.clone();
                h.set_observer(Arc::new(move |ev| match ev {
                    cubelink_host::HostEvent::Log(line) => core.on_log(line),
                    cubelink_host::HostEvent::Adopted { class, after_ms } => core.on_adopted(class, *after_ms),
                    cubelink_host::HostEvent::Fallback { reason, detail } => core.on_fallback(reason.as_str(), detail),
                    cubelink_host::HostEvent::Fullscreen(on) => core.on_fullscreen(*on),
                }));
                self.refresh_calm(&h, true);
                let _ = self.host.set(Box::new(h));
                self.note(match self.unavailable_reason() {
                    None => "Scene activee : le jeu s'affichera dans la fenetre du launcher",
                    Some(_) => "Scene prete mais reglage « Jeu integre » desactive : le jeu gardera sa propre fenetre",
                });
                self.relayout(window);
            }
            Err(e) => {
                eprintln!("[Cubelink] zone de jeu integree indisponible : {e}");
                self.note(&format!("Scene indisponible : {e}"));
            }
        }
    }

    /// Hors Windows la fenetre d'un autre processus ne s'adopte pas : le jeu garde la sienne.
    #[cfg(not(any(windows, target_os = "linux")))]
    pub fn attach(&self, _window: &tauri::WebviewWindow, _sink: EventSink) {}

    /// Recale la zone du jeu apres un deplacement, un redimensionnement ou un changement de dpi : marges publiees par l'interface
    /// appliquees a la taille actuelle (a defaut : toute la zone sous la barre de titre).
    pub fn relayout(&self, window: &tauri::WebviewWindow) {
        let (Some(h), Ok(size)) = (self.host.get(), window.inner_size()) else { return };
        let (cw, ch) = (size.width as i32, size.height as i32);
        let published = *lock(&self.layout);
        let (x, y, w, hgt) = match published {
            Some(l) => l.rect(cw, ch),
            None => {
                let tb = (FALLBACK_TITLEBAR_CSS_PX * window.scale_factor().unwrap_or(1.0)).round() as i32;
                (0, tb, cw.max(1), (ch - tb).max(1))
            }
        };
        h.relayout(x, y, w, hgt);
    }

    /// L'interface publie la zone de jeu (rectangle de l'element reserve) et la largeur du panneau, en pixels physiques.
    pub fn set_layout(&self, window: &tauri::WebviewWindow, x: i32, y: i32, w: i32, h: i32, panel_w: i32) {
        if !self.has_host() {
            return;
        }
        let Ok(size) = window.inner_size() else { return };
        *lock(&self.layout) = Some(Layout::from_rect(x, y, w, h, panel_w, size.width as i32, size.height as i32));
        self.relayout(window);
    }

    /// Ouvre/ferme le panneau « en jeu ». A l'ouverture, le focus passe au launcher (Echap le ferme) ; refuse hors du jeu.
    pub fn set_panel(&self, window: &tauri::WebviewWindow, open: bool) -> bool {
        let Some(h) = self.live_host() else { return false };
        let published = lock(&self.layout).map(|l| l.panel_w);
        let pw = match published {
            Some(pw) if pw > 0 => pw,
            _ => (FALLBACK_PANEL_CSS_PX * window.scale_factor().unwrap_or(1.0)).round() as i32,
        };
        let changed = h.set_panel(open, pw);
        if changed {
            if open {
                #[cfg(any(windows, target_os = "linux"))]
                if let Some(handle) = own_handle(window) {
                    crate::foreground::focus_own_window(handle);
                }
                let _ = window.set_focus();
            }
            self.publish();
        }
        changed
    }

    /// Un lancement commence dans la Scene : les panneaux se ferment, le chargement s'affiche. L'appelant a verifie
    /// `available()` ; sans hote, sans effet.
    pub fn begin(&self, title: &str, mods_dir: &Path, profile_path: &Path) {
        if let Some(h) = self.host.get() {
            self.refresh_calm(&**h, false);
            self.core.begin_launch();
            h.begin(title, mods_dir, profile_path);
            self.publish();
        }
    }

    pub fn watch(&self, pid: u32) {
        if let Some(h) = self.live_host() {
            h.watch(pid);
            self.publish();
        }
    }

    pub fn feed_line(&self, line: &str) {
        if let Some(h) = self.live_host() {
            h.feed_line(line);
        }
    }

    pub fn is_ready(&self) -> bool {
        self.live_host().is_some_and(|h| h.is_ready())
    }

    pub fn mark_ready(&self, profile_path: &Path) {
        if let Some(h) = self.live_host() {
            h.mark_ready(profile_path);
        }
    }

    /// Le jeu est pret : le rideau s'ouvre sur lui. Refuse des que le launcher se ferme ou que ce jeu est en train d'etre
    /// remplace (changement de modpack) : un jeu qu'on arrete ne doit pas apparaitre.
    pub fn reveal(&self) {
        if let Some(h) = self.live_host() {
            if self.core.no_reveal.load(Ordering::SeqCst) || self.core.switching.load(Ordering::SeqCst) {
                self.note("jeu pret, mais il est en cours d'arret : le rideau reste ferme");
                return;
            }
            self.core.revealed.store(true, Ordering::SeqCst);
            h.reveal();
            self.note("jeu pret : rideau ouvert sur le jeu");
            self.publish();
        }
    }

    /// Le rideau a deja ete ouvert sur le jeu de ce lancement (faux pendant la preparation et le chargement).
    pub fn revealed(&self) -> bool {
        self.core.staged() && self.core.revealed.load(Ordering::SeqCst)
    }

    /// Le launcher se ferme : plus aucun `reveal`.
    pub fn forbid_reveal(&self) {
        self.core.no_reveal.store(true, Ordering::SeqCst);
    }

    /// Inverse de [`Self::forbid_reveal`] : le launcher n'a pas quitté (masqué en fond), un prochain jeu pourra être montré.
    pub fn allow_reveal(&self) {
        self.core.no_reveal.store(false, Ordering::SeqCst);
    }

    /// Attend (brievement) que le journal soit ecrit, avant de quitter le processus.
    pub fn flush_log(&self) {
        if let Some(o) = self.core.outbox.get() {
            o.flush(std::time::Duration::from_secs(1));
        }
    }

    /// Changement de modpack : le rideau se ferme sur le jeu en cours et RESTE ferme apres sa fin, en attendant le `begin` du
    /// jeu suivant. Si ce lancement n'a pas lieu, `abort_switch` doit le rouvrir.
    pub fn begin_close(&self, title: &str) {
        if let Some(h) = self.live_host() {
            self.core.switching.store(true, Ordering::SeqCst);
            h.begin_close(title);
            self.publish();
        }
    }

    pub fn request_close(&self) {
        if let Some(h) = self.live_host() {
            h.request_close();
        }
    }

    /// Le jeu ne s'est pas ferme dans le delai : le rideau se rouvre sur lui (il est toujours la).
    pub fn cancel_close(&self) {
        if let Some(h) = self.live_host() {
            if self.core.switching.swap(false, Ordering::SeqCst) {
                h.cancel_close();
                self.publish();
            }
        }
    }

    /// Le jeu qui devait suivre un changement de modpack ne sera pas lance dans la Scene (lancement refuse, echec avant
    /// `begin`, jeu suivant hors Scene) : le rideau, reste ferme a mi-barre, se rouvre sur le launcher. Sans effet s'il n'y a
    /// pas de changement en attente, ou si l'ancien jeu tourne encore (c'est alors `cancel_close` qui rouvre, sur lui). Vrai si
    /// le rideau a ete rouvert.
    pub fn abort_switch(&self, why: &str) -> bool {
        let Some(h) = self.host.get() else { return false };
        if self.core.staged() || !self.core.switching.swap(false, Ordering::SeqCst) {
            return false;
        }
        // L'hote n'a pas d'ordre « retour au launcher » : on abandonne la fermeture (le rideau n'attend plus de lancement),
        // puis on lui annonce une fin de jeu, qui ramene au launcher avec l'animation habituelle.
        h.cancel_close();
        h.run_finished();
        self.note(&format!("changement de modpack interrompu ({why}) : rideau rouvert sur le launcher"));
        self.publish();
        true
    }

    /// A appeler entre `begin` et `watch`, juste avant de creer le processus du jeu : ce lancement doit-il partir avec sa
    /// premiere fenetre cachee (`switch` : voir `hide_start_switch`) ? L'hote est prevenu dans le meme geste, pour que le
    /// drapeau ne soit jamais transmis au processus sans que quelqu'un se charge d'afficher la fenetre.
    pub fn hide_start(&self, switch: bool) -> bool {
        let Some(h) = self.live_host() else { return false };
        let hide = hide_first_window(switch, true);
        h.expect_hidden(hide);
        if hide {
            self.note("lancement avec fenetre cachee au depart (essai)");
        }
        hide
    }

    /// Le jeu s'est termine de lui-meme avec ce code (consigne a la fin de la tache, voir `run_finished`).
    pub fn game_exited(&self, code: i32) {
        *lock(&self.core.exit_code) = Some(code);
    }

    /// La tache de jeu est terminee (fin du jeu, echec avant ou apres le demarrage, kill). L'hote n'est prevenu que si ce
    /// lancement etait encore dans la Scene : apres un repli il est deja revenu au launcher, et un jeu lance hors Scene ne le
    /// concerne pas. Peut etre appele plusieurs fois pour la meme tache (filet de `GameGuard`) : seul le premier appel compte.
    pub fn run_finished(&self) {
        let code = lock(&self.core.exit_code).take();
        match self.core.end_launch() {
            Ending::Staged => {
                if let Some(h) = self.host.get() {
                    h.run_finished();
                }
                self.note(&match code {
                    Some(c) => format!("fin du jeu (code de sortie {c}) : retour au launcher"),
                    None => "fin de la tache de jeu : retour au launcher".to_string(),
                });
            }
            Ending::FellBack => self.note(&fell_back_exit_line(code)),
            Ending::Outside => {}
        }
        self.publish();
    }

    pub fn set_view(&self, game: bool) {
        if let Some(h) = self.live_host() {
            h.set_view(game);
            self.note(if game { "bascule de vue : jeu" } else { "bascule de vue : launcher" });
            self.publish();
        }
    }

    /// Test : installe un faux hote a la place de la zone de jeu native.
    #[cfg(all(test, windows))]
    pub(crate) fn install_test_host(&self, host: Box<dyn HostApi>, sink: EventSink) {
        let _ = self.core.sink.set(sink);
        assert!(self.host.set(host).is_ok(), "hote deja installe");
    }
}

/// Preference systeme « afficher les animations dans Windows » desactivee : le rideau ne doit pas s'animer. Illisible :
/// animations gardees (comportement par defaut de Windows).
#[cfg(windows)]
fn reduced_motion() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION};
    let mut animations: i32 = 1;
    // SAFETY : `SPI_GETCLIENTAREAANIMATION` ecrit un BOOL (i32) a l'adresse fournie, valide pendant l'appel.
    let ok = unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&mut animations as *mut i32).cast(), 0) };
    ok != 0 && animations == 0
}

#[cfg(not(any(windows, target_os = "linux")))]
fn reduced_motion() -> bool {
    false
}

/// Linux : reglage GNOME « animations » desactive (`gsettings get org.gnome.desktop.interface enable-animations` rend
/// `false`). Illisible (pas de gsettings, autre bureau) : animations gardees.
#[cfg(target_os = "linux")]
fn reduced_motion() -> bool {
    let out = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "enable-animations"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output();
    out.is_ok_and(|o| o.status.success() && animations_off(&String::from_utf8_lossy(&o.stdout)))
}

/// Pur : sortie de `gsettings get ... enable-animations` -> animations desactivees ?
#[cfg(any(target_os = "linux", test))]
fn animations_off(gsettings_out: &str) -> bool {
    gsettings_out.trim() == "false"
}

/// Linux : la session graphique effective est X11 (`GDK_BACKEND=x11` pose par `lib.rs` avant la fenetre, `DISPLAY` present),
/// condition pour adopter la fenetre du jeu.
#[cfg(target_os = "linux")]
pub fn x11_session() -> bool {
    x11_session_of(std::env::var("GDK_BACKEND").ok().as_deref(), std::env::var_os("DISPLAY").is_some_and(|d| !d.is_empty()))
}

/// Pur : `GDK_BACKEND` (si defini) et presence de `DISPLAY` -> session X11 ?
#[cfg(any(target_os = "linux", test))]
fn x11_session_of(gdk_backend: Option<&str>, display: bool) -> bool {
    display && gdk_backend.is_some_and(|b| b.split(',').next().is_some_and(|f| f.trim() == "x11"))
}

/// Identifiant natif de la fenetre du launcher : HWND (Windows).
#[cfg(windows)]
fn own_handle(window: &tauri::WebviewWindow) -> Option<isize> {
    window.hwnd().ok().map(|h| h.0 as isize)
}

/// Identifiant natif de la fenetre du launcher : XID (Linux, fenetre X11).
#[cfg(target_os = "linux")]
fn own_handle(window: &tauri::WebviewWindow) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Xlib(h) => Some(h.window as isize),
        RawWindowHandle::Xcb(h) => Some(h.window.get() as isize),
        _ => None,
    }
}

/// Pur : la Scene existe-t-elle sur cette plateforme ? Windows toujours ; Linux seulement en session X11 effective.
fn stage_supported_on(windows: bool, linux: bool, x11: bool) -> bool {
    windows || (linux && x11)
}

/// Les jeux lances dans la Scene doivent etre forces en X11 (voir `launch_run::RunOptions::force_x11`).
impl Stage {
    pub fn force_x11(&self) -> bool {
        cfg!(target_os = "linux") && self.live_host().is_some()
    }
}

/// Bascule entre le jeu et le launcher (bouton de la barre de titre), avec l'animation de rideau.
#[tauri::command(rename_all = "snake_case")]
pub fn stage_view(state: State<'_, Arc<AppState>>, game: bool) {
    state.stage.set_view(game);
}

#[tauri::command(rename_all = "snake_case")]
pub fn stage_state(state: State<'_, Arc<AppState>>) -> StageState {
    state.stage.state()
}

/// Zone de jeu mesuree par l'interface (element reserve du DOM), en pixels physiques dans la zone cliente de la fenetre.
#[tauri::command(rename_all = "snake_case")]
pub fn stage_layout(state: State<'_, Arc<AppState>>, window: tauri::WebviewWindow, x: i32, y: i32, w: i32, h: i32, panel_w: i32) {
    state.stage.set_layout(&window, x, y, w, h, panel_w);
}

/// La zone de jeu integree existe sur cette plateforme (l'interface n'affiche le reglage « Jeu integre » que dans ce cas).
#[tauri::command(rename_all = "snake_case")]
pub fn stage_supported() -> bool {
    #[cfg(target_os = "linux")]
    let x11 = x11_session();
    #[cfg(not(target_os = "linux"))]
    let x11 = false;
    stage_supported_on(cfg!(windows), cfg!(target_os = "linux"), x11)
}

/// Ouvre (`true`) ou ferme (`false`) le panneau « en jeu » (Echap, bouton Fermer).
#[tauri::command(rename_all = "snake_case")]
pub fn stage_panel(state: State<'_, Arc<AppState>>, window: tauri::WebviewWindow, open: bool) {
    state.stage.set_panel(&window, open);
}

/// Faux hote pour les tests : aucune fenetre, mais le VRAI deroulement du rideau (`cubelink_host::flow::Machine`, la machine
/// pure que pilote `GameHost`), avec la meme correspondance appel -> evenement que `host/src/win/mod.rs` et une horloge
/// simulee. Ce qu'il prouve : apres chaque chemin du launcher, le rideau finit retire.
#[cfg(all(test, windows))]
pub(crate) mod fake {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_linux_seulement_en_session_x11() {
        assert!(stage_supported_on(true, false, false));
        assert!(stage_supported_on(false, true, true));
        assert!(!stage_supported_on(false, true, false));
        assert!(!stage_supported_on(false, false, true));
        assert!(x11_session_of(Some("x11"), true));
        assert!(x11_session_of(Some("x11,wayland"), true));
        assert!(!x11_session_of(Some("wayland"), true));
        assert!(!x11_session_of(Some("x11"), false));
        assert!(!x11_session_of(None, true));
    }

    #[test]
    fn animations_gnome() {
        assert!(animations_off("false\n"));
        assert!(!animations_off("true\n"));
        assert!(!animations_off(""));
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
        assert!(lines[7].contains("[launcher] Scene desactivee jusqu'au redemarrage de Cubelink : 2 replis de suite (hook_failed, puis no_window)"), "{text}");
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
}
