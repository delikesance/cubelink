//! Implementation Win32 de la fenetre hote (voir la doc du crate).
//!
//! Couche MINCE : elle traduit ce qui se passe en [`Event`], les passe a la machine [`crate::flow`] sous le verrou d'etat, puis
//! execute les [`Effect`] rendus une fois le verrou relache.
//!
//! * `mod.rs`            : API publique ([`GameHost`]), etat partage, execution des effets.
//! * `game_window.rs`    : fil unique des appels synchrones vers la fenetre du jeu (adoption, styles, decoupe).
//! * `watch.rs`          : surveillance des fenetres du processus de jeu (hooks WinEvent, balayage de secours).
//! * `curtain_window.rs` : fenetre du rideau (rendu, minuteur).
//! * `embedded.rs`       : mode integre (production) : le jeu est une fenetre sans cadre POSSEDEE par celle du launcher.
//! * `standalone.rs`     : mode autonome (binaire `host-proto`) : fenetre hote propre, jeu en fenetre fille.
//! * `debug.rs`, `util.rs` : journal de diagnostic et mesures ; petits outils Win32.
//!
//! Verrou d'etat : pris UNE fois par operation, jamais tenu pendant un appel Win32 qui peut attendre un autre fil.

mod curtain_window;
mod debug;
mod embedded;
mod game_window;
mod standalone;
mod util;
mod watch;

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::flow::{Bar, Content, Effect, Event, Machine, View, OPEN_DELAY_MS, OPEN_MS};
use crate::progress::{Closer, Profile, Tracker};
use debug::dbg;
use util::hw;

pub use util::write_bmp;

/// Le rideau doit etre affiche / remonte et anime (envoye a la zone de jeu, execute sur le fil de l'interface).
const WM_APP_CURTAIN_ON: u32 = WM_APP + 2;
/// Ouvrir le rideau (`wparam` = 1 : la fermeture de l'ancien jeu est abandonnee).
const WM_APP_REVEAL: u32 = WM_APP + 3;
/// Mode integre : recaler le jeu / le rideau (poste par les evenements Windows, execute sur le fil de l'interface).
const WM_APP_SYNC: u32 = WM_APP + 5;
/// Une fenetre du jeu vient d'apparaitre (lparam = son HWND) : le rideau remonte juste au-dessus d'elle.
const WM_APP_RAISE: u32 = WM_APP + 6;
/// Fil de surveillance : (re)pose les hooks sur le processus `wparam` (0 = les retire).
const WM_APP_HOOK: u32 = WM_APP + 7;

const CAPTION_SWITCH: &str = "Changement de modpack";
const CAPTION_LAUNCH: &str = "Lancement";
const CAPTION_TO_LAUNCHER: &str = "Retour au launcher";
const CAPTION_TO_GAME: &str = "Retour au jeu";

// ---------------------------------------------------------------------------------------------------- API publique

/// Reglages de la fenetre hote.
#[derive(Clone)]
pub struct HostConfig {
    pub title: String,
    pub width: i32,
    pub height: i32,
}

/// Jeu a lancer : programme + ligne d'arguments brute (Windows), repertoire, variables d'environnement.
#[derive(Clone, Debug)]
pub struct GameSpec {
    pub label: String,
    /// Nom affiche sur le rideau (nom du modpack).
    pub title: String,
    /// Dossier des mods (compte les jars : dimensionne les etapes « mods »). `None` = jeu sans mods.
    pub mods_dir: Option<PathBuf>,
    /// Fichier d'historique des durees d'etapes (estimation apprise, ecrit apres chaque lancement reussi).
    pub profile_path: Option<PathBuf>,
    /// Copie de la sortie du jeu (journal) ecrite par l'hote.
    pub log_path: Option<PathBuf>,
    pub program: PathBuf,
    pub raw_args: String,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
}

/// Quand le rideau peut s'estomper apres l'adoption de la fenetre du jeu.
#[derive(Clone, Debug)]
pub enum Ready {
    /// Delai fixe apres l'affichage de la fenetre.
    Delay(Duration),
    /// Fichier cree par l'agent (menu ou monde affiche), avec delai maximal.
    File(PathBuf, Duration),
    /// Pret quand l'agent l'annonce (`[Cubelink] READY` dans la sortie) ou, a defaut, quand les textures sont assemblees et que
    /// le jeu se tait depuis `quiet` ; delai maximal `max`.
    Auto { quiet: Duration, max: Duration },
}

/// Une fenetre de premier niveau d'un des processus de jeu (sonde de test : doit rester vide a l'ecran).
#[derive(Clone, Debug)]
pub struct TopLevel {
    pub class: String,
    pub title: String,
    pub visible: bool,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Delai maximal laisse a un jeu pour se fermer proprement (gros mondes / gros modpacks) avant de renoncer SANS le tuer.
pub const CLOSE_TIMEOUT: Duration = Duration::from_secs(180);

/// Resultat d'une fermeture propre.
#[derive(Debug, Clone)]
pub struct CloseReport {
    pub exit_code: Option<i32>,
    pub took_ms: u128,
    /// Le jeu a annonce l'enregistrement d'un monde.
    pub saved_world: bool,
}

/// Mesures d'un changement de jeu.
#[derive(Debug, Clone)]
pub struct SwitchReport {
    pub pid: u32,
    pub spawn_ms: u128,
    pub window_ms: u128,
    pub ready_ms: u128,
    pub had_game: bool,
    /// Fermeture propre de l'ancien jeu, s'il y en avait un.
    pub closed: Option<CloseReport>,
}

// ---------------------------------------------------------------------------------------------------- etat partage

/// Comment la fenetre du jeu est rattachee. Tout ce qui differe entre les deux modes passe par les methodes de `Mode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// Fenetre hote propre (prototype) : le jeu est une fenetre fille (`SetParent`).
    Standalone,
    /// Dans la fenetre du launcher : le jeu est une fenetre de premier niveau sans cadre, possedee par elle.
    Embedded,
}

#[derive(Default)]
struct State {
    /// Deroulement de la vue (rideau, pose de la fenetre du jeu, panneau).
    machine: Machine,
    pids: HashSet<u32>,
    adopted: Vec<isize>,
    /// LA fenetre du jeu (adoptee puis affichee par le jeu).
    current: Option<isize>,
    shown_at: Option<Instant>,
    title: String,
    caption: String,
    tracker: Option<Tracker>,
    closer: Option<Closer>,
    /// Apparition du rideau (horloge de ses animations).
    curtain_t0: Option<Instant>,
    /// Fenetres en cours d'adoption (evite les doublons entre les evenements et le balayage lent).
    adopting: HashSet<isize>,
    /// (largeur, hauteur, bande) de la decoupe actuellement appliquee a la fenetre du jeu.
    region: (i32, i32, i32),
    /// Mode integre : zone de jeu en coordonnees client de la fenetre du launcher (x, y, largeur, hauteur).
    view: (i32, i32, i32, i32),
    started: Option<Instant>,
    adopt_log: Vec<String>,
}

struct Shared {
    mode: Mode,
    /// Zone de jeu (integre) ou fenetre hote (autonome) : cible des messages de l'API.
    host: AtomicIsize,
    curtain: AtomicIsize,
    titlebar: AtomicIsize,
    /// Mode integre : fenetre du launcher.
    parent: isize,
    state: Mutex<State>,
    cv: Condvar,
    /// File du fil des appels synchrones vers la fenetre du jeu.
    game: mpsc::Sender<game_window::Cmd>,
}

static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();

/// Etat partage, s'il existe : les procedures de fenetre et les hooks ne paniquent jamais (un panic y arreterait le processus).
fn shared() -> Option<&'static Arc<Shared>> {
    SHARED.get()
}

impl Shared {
    /// Cree l'etat partage (un seul hote par processus) et demarre le fil de la fenetre du jeu.
    fn install(mode: Mode, parent: isize) -> Result<Arc<Shared>, String> {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared {
            mode,
            host: AtomicIsize::new(0),
            curtain: AtomicIsize::new(0),
            titlebar: AtomicIsize::new(0),
            parent,
            state: Mutex::new(State::default()),
            cv: Condvar::new(),
            game: tx,
        });
        SHARED.set(shared.clone()).map_err(|_| "hote deja demarre")?;
        game_window::spawn(rx)?;
        Ok(shared)
    }

    /// Verrou d'etat, tolerant a l'empoisonnement (l'etat reste coherent : chaque operation le met a jour d'un bloc).
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn host(&self) -> HWND {
        hw(self.host.load(Ordering::SeqCst))
    }

    fn curtain(&self) -> HWND {
        hw(self.curtain.load(Ordering::SeqCst))
    }
}

/// Aiguillage des deux modes : le seul endroit ou l'on choisit entre `embedded` et `standalone`.
impl Mode {
    /// Rattache une fenetre de jeu (sur le fil de la fenetre du jeu).
    unsafe fn adopt(self, s: &Shared, win: isize) {
        match self {
            Mode::Embedded => embedded::adopt(s, win),
            Mode::Standalone => standalone::adopt(s, win),
        }
    }

    /// `hwnd` est deja rattachee a nous.
    unsafe fn is_attached(self, s: &Shared, hwnd: HWND) -> bool {
        match self {
            Mode::Embedded => embedded::is_attached(s, hwnd),
            Mode::Standalone => standalone::is_attached(s, hwnd),
        }
    }

    /// Une fenetre du jeu, pas encore adoptee, vient d'etre detectee.
    unsafe fn on_detected(self, s: &Shared, win: isize) {
        match self {
            Mode::Embedded => embedded::on_detected(s, win),
            Mode::Standalone => {}
        }
    }

    /// LA fenetre du jeu vient de s'afficher.
    unsafe fn on_game_shown(self, s: &Shared, hwnd: HWND) {
        match self {
            Mode::Embedded => {}
            Mode::Standalone => standalone::on_game_shown(s, hwnd),
        }
    }

    /// Une fenetre du processus de jeu a bouge.
    unsafe fn on_moved(self, s: &Shared, hwnd: HWND) {
        match self {
            Mode::Embedded => embedded::on_moved(s, hwnd),
            Mode::Standalone => standalone::on_moved(s, hwnd),
        }
    }

    /// Recale le rideau et la fenetre du jeu sur la zone de jeu.
    unsafe fn sync(self, s: &Shared) {
        match self {
            Mode::Embedded => embedded::sync(s),
            Mode::Standalone => {} // la fenetre hote recale ses filles elle-meme (WM_SIZE)
        }
    }

    unsafe fn place_game(self, s: &Shared, game: isize) {
        match self {
            Mode::Embedded => embedded::show_game(s, hw(game)),
            Mode::Standalone => {} // fenetre fille : toujours en place sous le rideau
        }
    }

    unsafe fn hide_game(self, s: &Shared, game: isize) {
        match self {
            Mode::Embedded => embedded::hide_game(s, hw(game)),
            Mode::Standalone => {}
        }
    }
}

/// Effets d'une transition, avec ce qu'il faut pour les executer une fois le verrou relache.
struct Todo {
    effects: Vec<Effect>,
    /// LA fenetre du jeu au moment de la transition.
    current: Option<isize>,
    /// Fenetres adoptees (seulement si une decoupe est a retirer).
    adopted: Vec<isize>,
}

impl Todo {
    fn has(&self, e: impl Fn(&Effect) -> bool) -> bool {
        self.effects.iter().any(e)
    }

    /// Le rideau a ete (re)ferme par cette transition.
    fn shows_curtain(&self) -> bool {
        self.has(|e| matches!(e, Effect::ShowCurtain { .. }))
    }
}

/// Passe `ev` a la machine et applique, sous le verrou, la part des effets qui ne touche que l'etat.
fn dispatch(st: &mut State, ev: Event) -> Todo {
    let now = Instant::now();
    let effects = st.machine.apply(ev, now);
    let current = st.current;
    let mut adopted = Vec::new();
    for e in &effects {
        match *e {
            Effect::ShowCurtain { fresh: true } => st.curtain_t0 = Some(now),
            Effect::ClearRegion => {
                st.region = (0, 0, 0);
                adopted = st.adopted.clone();
            }
            Effect::Detach => {
                st.tracker = None;
                st.pids.clear();
                st.adopted.clear();
                st.current = None;
            }
            Effect::CloseBarDone => {
                if let Some(c) = st.closer.as_mut() {
                    c.mark_exited();
                }
            }
            _ => {}
        }
    }
    if !st.machine.closing() {
        st.closer = None; // fermeture terminee, annulee ou oubliee (rideau rouvert)
    }
    Todo { effects, current, adopted }
}

/// Execute, verrou relache, les effets qui touchent les fenetres. Le minuteur est regle par le battement lui-meme.
fn run(s: &Shared, todo: &Todo) {
    run_if(s, todo, |_| true);
}

/// Comme `run`, pour les seuls effets retenus par `keep`.
fn run_if(s: &Shared, todo: &Todo, keep: impl Fn(&Effect) -> bool) {
    for e in todo.effects.iter().filter(|e| keep(e)) {
        unsafe {
            match *e {
                Effect::ShowCurtain { .. } => {
                    SendMessageW(s.host(), WM_APP_CURTAIN_ON, 0, 0);
                }
                Effect::HideCurtain => {
                    ShowWindow(s.curtain(), SW_HIDE);
                }
                Effect::PlaceGame => {
                    if let Some(g) = todo.current {
                        dbg("fenetre du jeu placee sur la zone de jeu");
                        s.mode.place_game(s, g);
                    }
                }
                Effect::HideGame => {
                    if let Some(g) = todo.current {
                        dbg("fenetre du jeu cachee (retour launcher)");
                        s.mode.hide_game(s, g);
                    }
                }
                Effect::ClearRegion => game_window::clear_region(s, todo.adopted.clone()),
                Effect::FocusGame => {
                    if let Some(g) = todo.current {
                        game_window::foreground(s, g);
                    }
                }
                Effect::SyncGame => s.mode.sync(s),
                Effect::Detach => watch::set_hooks(0),
                Effect::CloseBarDone | Effect::Timer(_) => {}
            }
        }
    }
}

/// LA fenetre du jeu vient d'apparaitre (affichee par le jeu apres son adoption, ou deja visible a l'adoption).
fn mark_shown(st: &mut State, win: isize, class: Option<&str>) {
    st.current = Some(win);
    st.shown_at = Some(Instant::now());
    if let Some(t) = st.tracker.as_mut() {
        t.window_shown();
    }
    if let Some(class) = class {
        let t = st.started.map_or(0, |x| x.elapsed().as_millis());
        st.adopt_log.push(format!("+{t} ms : fenetre du jeu affichee ({class})"));
    }
    dispatch(st, Event::WindowShown); // aucun effet : la pose se fait au battement suivant, sur le fil de l'interface
}

/// Ouverture du rideau demandee (sur le fil de l'interface).
unsafe fn on_reveal(s: &Shared, host: HWND, cancel: bool) {
    let visible = debug::dbg_on() && IsWindowVisible(host) != 0;
    let todo = {
        let mut st = s.lock();
        if debug::dbg_on() {
            dbg(&format!("ouverture demandee : zone visible={visible} vue={:?} adoptees={} courante={:?}", st.machine.view(), st.adopted.len(), st.current));
        }
        dispatch(&mut st, if cancel { Event::CancelClose } else { Event::Reveal })
    };
    run(s, &todo);
}

/// Fenetres a qui demander la fermeture : LA fenetre du jeu seulement (les fenetres d'aide adoptees n'ont pas a recevoir WM_CLOSE) ;
/// a defaut (pas encore affichee), toutes les fenetres adoptees.
fn close_targets(st: &State) -> Vec<isize> {
    match st.current {
        Some(c) => vec![c],
        None => st.adopted.clone(),
    }
}

/// La fenetre hote et son processus de jeu courant.
pub struct GameHost {
    s: Arc<Shared>,
    running: Mutex<Option<Child>>,
    current_title: Mutex<String>,
}

impl GameHost {
    /// Cree la fenetre hote (fil dedie avec boucle de messages) et installe la surveillance des fenetres de jeu.
    pub fn start(cfg: HostConfig) -> Result<GameHost, String> {
        let s = Shared::install(Mode::Standalone, 0)?;
        standalone::start(cfg)?;
        if debug::dbg_on() {
            std::thread::spawn(debug::mouse_diag);
        }
        watch::start()?;
        Ok(GameHost { s, running: Mutex::new(None), current_title: Mutex::new(String::new()) })
    }

    pub fn hwnd(&self) -> isize {
        self.s.host.load(Ordering::SeqCst)
    }

    /// Pids des jeux surveilles.
    pub fn pids(&self) -> HashSet<u32> {
        self.s.lock().pids.clone()
    }

    /// Journal d'adoption (horodate depuis le lancement).
    pub fn adopt_log(&self) -> Vec<String> {
        self.s.lock().adopt_log.clone()
    }

    /// Fenetres de premier niveau (visibles ou non) appartenant aux jeux surveilles.
    pub fn toplevel_windows(&self) -> Vec<TopLevel> {
        util::toplevel_of(&self.pids())
    }

    /// Ferme le rideau (deux panneaux qui se rejoignent sur le jeu) puis affiche titre + barre de chargement.
    pub fn show_curtain(&self, title: &str, caption: &str) {
        let todo = {
            let mut st = self.s.lock();
            st.title = title.to_string();
            st.caption = caption.to_string();
            dispatch(&mut st, Event::Hold)
        };
        run(&self.s, &todo);
    }

    /// Ouvre le rideau (les panneaux s'ecartent et decouvrent le jeu).
    pub fn reveal(&self) {
        unsafe { SendMessageW(self.s.host(), WM_APP_REVEAL, 0, 0) };
    }

    /// Lance `spec` : si un jeu tourne, le rideau fige sa derniere image, l'ancien processus est arrete, puis le nouveau est
    /// lance ; sa fenetre est adoptee a sa creation. Rend la main quand le jeu est affiche ET « pret », rideau retire.
    pub fn switch_to(&self, spec: &GameSpec, ready: &Ready) -> Result<SwitchReport, String> {
        let t0 = Instant::now();
        let had_game = self.running().is_some();
        let mut closed = None;
        if had_game {
            // Fermeture PROPRE de l'ancien jeu : monde quitte et enregistre, Java arrete par le jeu lui-meme. En cas d'echec ou
            // de delai depasse, rien n'est force : l'ancien jeu reste en place et l'appelant decide.
            let title = spec.title.clone(); // le rideau montre d'emblee le modpack qui arrive
            match self.close_gracefully(&title, CLOSE_TIMEOUT) {
                Ok(r) => closed = Some(r),
                Err(e) => {
                    self.reveal();
                    return Err(format!("fermeture propre impossible, l'ancien jeu est conserve : {e}"));
                }
            }
        }
        // la fermeture est terminee : le rideau passe au lancement
        self.begin(&spec.title, CAPTION_LAUNCH, spec.mods_dir.as_deref(), spec.profile_path.as_deref());
        {
            let mut st = self.s.lock();
            dispatch(&mut st, Event::Watch); // la fenetre du nouveau jeu n'existe pas encore
            st.pids.clear();
            st.adopted.clear();
            st.current = None;
            st.shown_at = None;
            st.started = Some(Instant::now());
        }
        let mut cmd = Command::new(&spec.program);
        cmd.raw_arg(&spec.raw_args);
        if let Some(d) = &spec.cwd {
            cmd.current_dir(d);
        }
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| format!("lancement : {e}"))?;
        let pid = child.id();
        let log = spec.log_path.as_ref().and_then(|p| std::fs::File::create(p).ok()).map(|f| Arc::new(Mutex::new(f)));
        if let Some(o) = child.stdout.take() {
            pump_output(self.s.clone(), o, log.clone());
        }
        if let Some(e) = child.stderr.take() {
            pump_output(self.s.clone(), e, log);
        }
        self.s.lock().pids.insert(pid);
        watch::set_hooks(pid);
        *self.running() = Some(child);
        *self.current_title.lock().unwrap_or_else(|e| e.into_inner()) = spec.title.clone();
        let spawn_ms = t0.elapsed().as_millis();

        // attente de la fenetre du jeu (adoptee a sa creation)
        let shown = {
            let mut st = self.s.lock();
            let deadline = Instant::now() + Duration::from_secs(180);
            while st.current.is_none() {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Err("la fenetre du jeu n'est jamais apparue (180 s)".into());
                }
                st = self.s.cv.wait_timeout(st, left.min(Duration::from_millis(200))).unwrap_or_else(|e| e.into_inner()).0;
                if let Some(c) = self.running().as_mut() {
                    if let Ok(Some(status)) = c.try_wait() {
                        return Err(format!("le jeu s'est arrete avant d'afficher sa fenetre ({status})"));
                    }
                }
            }
            st.shown_at.unwrap_or_else(Instant::now)
        };
        let window_ms = t0.elapsed().as_millis();
        match ready {
            Ready::Delay(d) => {
                let until = shown + *d;
                std::thread::sleep(until.saturating_duration_since(Instant::now()));
            }
            Ready::File(f, max) => {
                let until = Instant::now() + *max;
                while !f.exists() && Instant::now() < until {
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
            Ready::Auto { quiet, max } => {
                let until = Instant::now() + *max;
                while Instant::now() < until {
                    if self.s.lock().tracker.as_ref().is_some_and(|t| t.looks_ready(*quiet)) {
                        break;
                    }
                    if let Some(c) = self.running().as_mut() {
                        if let Ok(Some(status)) = c.try_wait() {
                            return Err(format!("le jeu s'est arrete pendant le chargement ({status})"));
                        }
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
        if let Some(t) = self.s.lock().tracker.as_mut() {
            t.mark_ready();
        }
        // laisse voir « 100 % / Pret » un instant avant le fondu
        std::thread::sleep(Duration::from_millis(450));
        if let Some(path) = &spec.profile_path {
            // l'ecriture du fichier se fait hors du verrou
            let profile = self.s.lock().tracker.as_ref().map(|t| t.finish_profile());
            if let Some(p) = profile {
                p.save(path);
            }
        }
        self.reveal();
        std::thread::sleep(Duration::from_millis((OPEN_DELAY_MS + OPEN_MS) as u64 + 80));
        Ok(SwitchReport { pid, spawn_ms, window_ms, ready_ms: t0.elapsed().as_millis(), had_game, closed })
    }

    fn running(&self) -> MutexGuard<'_, Option<Child>> {
        self.running.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ------------------------------------------------------------------------------------------------ mode integre

    /// Mode integre : cree la zone du jeu (fenetre fille de `parent`, un HWND de la fenetre du launcher) et le rideau.
    /// A appeler DEPUIS LE FIL qui possede `parent` (sa boucle de messages anime le rideau).
    pub fn embed(parent: isize) -> Result<GameHost, String> {
        let s = Shared::install(Mode::Embedded, parent)?;
        unsafe { embedded::create(&s)? };
        watch::start()?;
        Ok(GameHost { s, running: Mutex::new(None), current_title: Mutex::new(String::new()) })
    }

    /// Place la zone du jeu et le rideau (coordonnees physiques dans la zone cliente de la fenetre parente).
    pub fn relayout(&self, x: i32, y: i32, w: i32, h: i32) {
        self.s.lock().view = (x, y, w.max(1), h.max(1));
        unsafe { self.s.mode.sync(&self.s) };
    }

    /// Un lancement commence : les panneaux se ferment sur le launcher (ou sur le jeu en cours) puis le chargement s'affiche.
    pub fn begin(&self, title: &str, caption: &str, mods_dir: Option<&std::path::Path>, profile_path: Option<&std::path::Path>) {
        dbg("begin");
        // lectures disque hors du verrou
        let n = mods_dir.map_or(0, count_jars);
        let prev = profile_path.and_then(Profile::load);
        let todo = {
            let mut st = self.s.lock();
            let todo = dispatch(&mut st, Event::BeginLaunch);
            st.tracker = Some(Tracker::new(n, prev));
            st.title = title.to_string();
            // la fermeture de l'ancien jeu vient d'occuper la 1re moitie de la barre
            let second_half = st.machine.content() == Content::Launch(Bar::SecondHalf);
            st.caption = if second_half { CAPTION_SWITCH } else { caption }.to_string();
            todo
        };
        run(&self.s, &todo);
    }

    /// Surveille les fenetres du processus `pid` (le nouveau jeu) : elles seront adoptees des leur creation.
    pub fn watch(&self, pid: u32) {
        let todo = {
            let mut st = self.s.lock();
            let todo = dispatch(&mut st, Event::Watch);
            st.adopting.clear();
            st.pids.clear();
            st.pids.insert(pid);
            st.adopted.clear();
            st.current = None;
            st.shown_at = None;
            st.started = Some(Instant::now());
            todo
        };
        run(&self.s, &todo);
        watch::set_hooks(pid);
        curtain_window::request_sync(&self.s); // relance le minuteur (filet de securite) s'il etait arrete
    }

    /// Une ligne de la sortie du jeu (alimente l'avancement du lancement ou de la fermeture).
    pub fn feed_line(&self, line: &str) {
        feed(&mut self.s.lock(), line);
    }

    /// Le jeu a l'air pret (textures assemblees, fenetre affichee, plus de bruit depuis `quiet`) ou la fenetre est affichee depuis
    /// trop longtemps (`max`) : on ne laisse jamais le rideau bloque.
    pub fn is_ready(&self, quiet: Duration, max: Duration) -> bool {
        let st = self.s.lock();
        if st.tracker.as_ref().is_some_and(|t| t.looks_ready(quiet)) {
            return true;
        }
        st.shown_at.is_some_and(|t| t.elapsed() > max)
    }

    /// Le jeu est pret : la barre atteint 100 %, l'historique des durees est enregistre.
    pub fn mark_ready(&self, profile_path: Option<&std::path::Path>) {
        let profile = {
            let mut st = self.s.lock();
            st.tracker.as_mut().map(|t| {
                t.mark_ready();
                t.finish_profile()
            })
        };
        // ecriture du fichier hors du verrou
        if let (Some(profile), Some(path)) = (profile, profile_path) {
            profile.save(path);
        }
    }

    /// Debut d'un changement de modpack : les panneaux se ferment sur le jeu en cours et la barre suit sa fermeture (0-50 %).
    pub fn begin_close(&self, title: &str) {
        dbg("begin_close");
        let todo = {
            let mut st = self.s.lock();
            let todo = dispatch(&mut st, Event::BeginClose);
            st.closer = Some(Closer::new());
            st.tracker = None;
            st.title = title.to_string();
            st.caption = CAPTION_SWITCH.to_string();
            todo
        };
        run(&self.s, &todo);
    }

    /// Demande au jeu de se fermer comme le ferait le joueur (croix de la fenetre) : il quitte le monde, enregistre, s'eteint.
    pub fn request_close(&self) {
        let windows = close_targets(&self.s.lock());
        for w in windows {
            unsafe { PostMessageW(hw(w), WM_CLOSE, 0, 0) };
        }
    }

    /// La tache de jeu est terminee (arret du jeu, echec du lancement ou kill). Pendant un changement de modpack (fermeture
    /// demandee) : la barre atteint 50 % et rien d'autre ; sinon retour au launcher avec la meme animation de rideau.
    pub fn run_finished(&self) {
        dbg("run_finished");
        self.to_launcher(Event::GameExited);
    }

    /// Le jeu ne s'est pas ferme dans le delai : on le laisse tranquille (jamais force) et on rouvre le rideau sur lui.
    pub fn cancel_close(&self) {
        unsafe { SendMessageW(self.s.host(), WM_APP_REVEAL, 1, 0) };
    }

    /// Retour au launcher : panneaux fermes, zone du jeu masquee, panneaux rouverts.
    pub fn leave(&self) {
        dbg("leave");
        self.to_launcher(Event::Leave);
    }

    fn to_launcher(&self, ev: Event) {
        let todo = {
            let mut st = self.s.lock();
            let todo = dispatch(&mut st, ev);
            if todo.shows_curtain() {
                st.title.clear();
                st.caption = CAPTION_TO_LAUNCHER.to_string();
            }
            todo
        };
        run(&self.s, &todo);
    }

    /// Bascule entre le jeu (`true`) et le launcher (`false`) avec l'animation de rideau, sans arreter le jeu.
    pub fn set_view(&self, game: bool) {
        dbg("set_view");
        let todo = {
            let mut st = self.s.lock();
            let todo = dispatch(&mut st, Event::SetView(if game { View::Game } else { View::Launcher }));
            if todo.shows_curtain() {
                st.title.clear();
                st.caption = if game { CAPTION_TO_GAME } else { CAPTION_TO_LAUNCHER }.to_string();
            }
            todo
        };
        run(&self.s, &todo);
    }

    /// Le joueur est dans le jeu : fenetre du jeu affichee, rideau retire, aucun basculement en cours.
    pub fn in_game(&self) -> bool {
        self.s.lock().machine.in_game()
    }

    /// Panneau « en jeu » ouvert ?
    pub fn panel(&self) -> bool {
        self.s.lock().machine.panel_width() > 0
    }

    /// Ouvre (bande de `width_px` a droite du jeu, le launcher apparait dessous) ou ferme le panneau. A l'ouverture : refuse (faux)
    /// hors du jeu (chargement, rideau, launcher visible). A la fermeture : le jeu retrouve le premier plan. Vrai si l'etat a change.
    pub fn set_panel(&self, open: bool, width_px: i32) -> bool {
        let ev = if open { Event::PanelOpen { width: width_px.max(1) } } else { Event::PanelClose };
        let todo = dispatch(&mut self.s.lock(), ev);
        run(&self.s, &todo);
        !todo.effects.is_empty()
    }

    /// (un jeu est rattache, la zone du jeu est celle qu'on voit)
    pub fn view(&self) -> (bool, bool) {
        let st = self.s.lock();
        (!st.pids.is_empty(), st.machine.view() == View::Game)
    }

    /// Ferme le jeu courant COMME LE FERAIT LE JOUEUR (croix de la fenetre) : le jeu quitte le monde, arrete son serveur integre,
    /// enregistre, puis s'eteint. On attend sa fin reelle sans jamais le tuer ; au-dela de `timeout` on renonce (le jeu reste).
    pub fn close_gracefully(&self, title: &str, timeout: Duration) -> Result<CloseReport, String> {
        let t0 = Instant::now();
        if self.running().is_none() {
            return Ok(CloseReport { exit_code: None, took_ms: 0, saved_world: false });
        }
        let (todo, windows) = {
            let mut st = self.s.lock();
            let todo = dispatch(&mut st, Event::BeginClose);
            st.closer = Some(Closer::new());
            st.tracker = None;
            st.title = title.to_string();
            st.caption = CAPTION_SWITCH.to_string();
            let windows = close_targets(&st);
            (todo, windows)
        };
        run(&self.s, &todo);
        for w in &windows {
            unsafe { PostMessageW(hw(*w), WM_CLOSE, 0, 0) };
        }
        loop {
            let status = self.running().as_mut().and_then(|c| c.try_wait().ok().flatten());
            if let Some(status) = status {
                let saved = self.s.lock().closer.as_mut().map(|c| {
                    c.mark_exited();
                    c.saw_save()
                });
                std::thread::sleep(Duration::from_millis(700)); // laisse voir « fermé proprement »
                *self.running() = None;
                {
                    let mut st = self.s.lock();
                    st.pids.clear();
                    st.adopted.clear();
                    st.current = None;
                }
                watch::set_hooks(0);
                return Ok(CloseReport { exit_code: status.code(), took_ms: t0.elapsed().as_millis(), saved_world: saved.unwrap_or(false) });
            }
            if t0.elapsed() > timeout {
                dispatch(&mut self.s.lock(), Event::CloseAbandoned);
                return Err(format!("le jeu ne s'est pas ferme en {} s (il n'a PAS ete force)", timeout.as_secs()));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Arrete le jeu courant (sans rideau).
    pub fn stop_game(&self) {
        let child = self.running().take();
        if let Some(mut c) = child {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    /// Capture de la zone cliente de l'hote telle qu'a l'ecran : (largeur, hauteur, pixels BGRA).
    pub fn screenshot(&self) -> Option<(i32, i32, Vec<u8>)> {
        unsafe { util::capture_client(self.s.host()) }
    }

    /// Redimensionne la fenetre hote (taille exterieure).
    pub fn resize(&self, w: i32, h: i32) {
        unsafe { SetWindowPos(self.s.host(), null_mut(), 0, 0, w, h, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE) };
    }

    /// La fenetre hote existe encore (le joueur ne l'a pas fermee).
    pub fn is_open(&self) -> bool {
        unsafe { IsWindow(self.s.host()) != 0 }
    }

    /// Ferme la fenetre hote (et arrete le jeu).
    pub fn close(&self) {
        self.stop_game();
        unsafe { PostMessageW(self.s.host(), WM_CLOSE, 0, 0) };
    }
}

/// Une ligne de la sortie du jeu : avancement du lancement et de la fermeture.
fn feed(st: &mut State, line: &str) {
    if let Some(t) = st.tracker.as_mut() {
        t.feed_line(line);
    }
    if let Some(c) = st.closer.as_mut() {
        c.feed_line(line);
    }
}

fn count_jars(dir: &std::path::Path) -> u32 {
    std::fs::read_dir(dir).map_or(0, |d| d.filter_map(|e| e.ok()).filter(|e| e.path().extension().is_some_and(|x| x == "jar")).count() as u32)
}

/// Lit la sortie d'un jeu : alimente le suivi d'etapes (et une copie fichier) jusqu'a la fin du processus.
fn pump_output<R: std::io::Read + Send + 'static>(s: Arc<Shared>, r: R, log: Option<Arc<Mutex<std::fs::File>>>) {
    std::thread::spawn(move || {
        let mut br = BufReader::new(r);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match br.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if let Some(f) = &log {
                        let _ = f.lock().unwrap_or_else(|e| e.into_inner()).write_all(&buf);
                    }
                    let line = String::from_utf8_lossy(&buf).into_owned();
                    feed(&mut s.lock(), &line);
                }
            }
        }
    });
}
