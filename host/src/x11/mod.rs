//! Implementation X11 de la Scene (Linux : X11 ou XWayland ; voir la doc du crate).
//!
//! Meme role et meme API que `win` : une couche MINCE qui traduit ce qui se passe en [`Event`], les passe a la machine
//! [`crate::flow`] sous le verrou d'etat, puis execute les [`Effect`] rendus une fois le verrou relache.
//!
//! Difference de fond : sous X11 la fenetre du jeu est REPARENTEE (`XReparentWindow`) dans une sous-fenetre (« scene ») du
//! launcher. Une fille X11 peut avoir le focus clavier (`XSetInputFocus`), et le gestionnaire de fenetres ne la voit plus : pas de
//! cadre, pas de bouton dans la barre des taches, calage exact. Le panneau « en jeu » reduit la scene (le jeu garde sa taille,
//! rogne) ; le rideau est une fenetre ARGB `override-redirect`, traversee par la souris. Repli si le reparentage est refuse :
//! `WM_TRANSIENT_FOR` + `_MOTIF_WM_HINTS` sans decoration + `SKIP_TASKBAR`.
//!
//! * `mod.rs`            : `Backend` de la Scene commune (`crate::scene`, ou vit l'API publique), etat propre a X11, execution des effets, repli, gardes.
//! * `embedded.rs`       : scene et rideau (fenetres), adoption, calage, pose, plein ecran, restitution au bureau.
//! * `watch.rs`          : fil des evenements X11 (detection des fenetres du jeu), balayage de secours.
//! * `curtain_window.rs` : contenu, rendu et minuteur du rideau.
//! * `util.rs`           : atomes, lectures de proprietes.
//!
//! Verrou d'etat : pris UNE fois par operation, jamais tenu pendant un appel a l'observateur ([`crate::health`]). Les requetes X
//! passent par UNE connexion partagee (`RustConnection` est `Send + Sync`) : un fil attend les evenements, un autre bat la cadence
//! du rideau, les appels de l'API partent de leur propre fil. Toute erreur X sur une fenetre disparue est ignoree.
//!
//! Repli : quand la Scene ne peut pas tenir le jeu (rattachement refuse, aucune fenetre dans le delai de garde, ou demande de
//! l'appelant), `fallback` rend sa fenetre au jeu, rouvre le rideau sur le launcher et annonce [`HostEvent::Fallback`]. Le jeu
//! n'est jamais tue et ne reste jamais invisible.

mod curtain_window;
mod embedded;
mod util;
mod watch;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::rust_connection::RustConnection;

use crate::flow::{CAPTION_TO_LAUNCHER, Cadence, Effect, Event};
use crate::guard::{self, Watched, ADOPTION_RETRY};
use crate::health::{FallbackReason, HostEvent, Observer};
use crate::scene::{Backend, Common};
use embedded::Origin;
use util::Atoms;


/// Journal de diagnostic (`CUBELINK_STAGE_DEBUG`) : stderr, horodate.
fn dbg_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("CUBELINK_STAGE_DEBUG").is_some())
}

fn dbg(m: &str) {
    if dbg_on() {
        static T0: OnceLock<Instant> = OnceLock::new();
        eprintln!("[scene x11 +{:>7} ms] {m}", T0.get_or_init(Instant::now).elapsed().as_millis());
    }
}

// ---------------------------------------------------------------------------------------------------- etat partage

/// Rattachement refuse : une fenetre n'est pas dans la scene apres une tentative terminee.
#[derive(Clone, Copy, Debug)]
struct Refused {
    /// Premier refus constate.
    since: Instant,
    /// Tentatives terminees sur un refus.
    attempts: u32,
    /// Derniere nouvelle tentative demandee.
    asked: Instant,
    /// Dernier constat de refus : la fenetre reste « refusee » tant qu'elle n'a pas tenu un moment dans la scene (le jeu defait le
    /// rattachement de facon asynchrone, une tentative qui « reussit » n'est qu'un repit).
    bad_at: Instant,
}

#[derive(Default)]
pub struct State {
    /// Part commune aux deux systemes (voir `crate::scene`), accessible directement par `Deref`.
    c: Common<u32>,
    /// Toute fenetre du jeu qu'on a touchee, telle qu'elle etait avant : de quoi la rendre.
    origin: HashMap<u32, Origin>,
    /// Fenetres dont le rattachement est refuse, tant qu'elles peuvent etre LA fenetre du jeu.
    refused: HashMap<u32, Refused>,
    /// C'est NOUS qui avons mis la fenetre du launcher en plein ecran (`_NET_WM_STATE_FULLSCREEN`) : a nous de la rendre.
    launcher_fs: bool,
    /// Le plein ecran vient d'un message `_NET_WM_STATE` : seul un message contraire (ou la fin du jeu) le quitte, pas une taille.
    fullscreen_msg: bool,
    /// Fenetres du bureau presentes au debut de la surveillance : jamais prises pour le jeu sur leur seule classe.
    known_before: HashSet<u32>,
    /// Rectangle ecran applique au rideau, a la scene (repere du launcher) et au jeu (largeur, hauteur dans la scene).
    curtain_rect: Option<(i32, i32, i32, i32)>,
    stage_rect: Option<(i32, i32, i32, i32)>,
    game_rect: Option<(u32, i32, i32)>,
    /// Largeur de la bande rognee a droite du jeu transitoire (panneau ouvert), deja appliquee par XShape.
    game_clip: Option<(u32, i32)>,
    stage_mapped: bool,
    /// Dernier `ConfigureNotify` synthetique envoye au jeu (fenetre, rectangle absolu en coordonnees racine).
    notified: Option<(u32, (i32, i32, i32, i32))>,
    /// Dernier journal « le jeu a deplace sa fenetre » (limite le bruit) et nombre de corrections depuis.
    fight_logged: Option<Instant>,
    fights: u32,
    /// Dernier balayage de secours.
    scanned_at: Option<Instant>,
}

impl std::ops::Deref for State {
    type Target = Common<u32>;
    fn deref(&self) -> &Common<u32> {
        &self.c
    }
}

impl std::ops::DerefMut for State {
    fn deref_mut(&mut self) -> &mut Common<u32> {
        &mut self.c
    }
}

pub struct Shared {
    conn: RustConnection,
    atoms: Atoms,
    root: u32,
    /// Fenetre du launcher.
    parent: u32,
    /// Scene : sous-fenetre du launcher qui porte la fenetre du jeu.
    stage: u32,
    curtain: u32,
    gc: u32,
    /// Profondeur du rideau (32 : ARGB, sinon celle de la racine).
    depth: u8,
    /// Un gestionnaire de fenetres tourne (lu a l'installation) : le focus et le premier plan ont alors un sens.
    wm_present: bool,
    scale: f32,
    state: Mutex<State>,
    /// Observateur de l'appelant. Verrou distinct de l'etat : il n'est tenu que le temps de cloner le pointeur.
    observer: Mutex<Option<Observer>>,
    render: Mutex<curtain_window::Render>,
    curtain_mapped: AtomicBool,
    tick: AtomicU8,
    stop: AtomicBool,
    ticker: Mutex<Option<std::thread::Thread>>,
}

impl Shared {
    /// Ouvre la connexion X11, cree la scene et le rideau, demarre les fils de l'hote.
    fn install(parent: u32) -> Result<Arc<Shared>, String> {
        let (conn, n) = RustConnection::connect(None).map_err(|e| format!("X11 indisponible : {e}"))?;
        let atoms = Atoms::new(&conn).map_err(|e| e.to_string())?.reply().map_err(|e| e.to_string())?;
        let screen = conn.setup().roots[n].clone();
        if !util::exists(&conn, parent) {
            return Err(format!("fenetre du launcher {parent:#x} introuvable"));
        }
        let made = embedded::create(&conn, &atoms, &screen, parent)?;
        let scale = std::env::var("GDK_SCALE").ok().and_then(|v| v.parse::<f32>().ok()).filter(|v| *v >= 1.0).unwrap_or(1.0);
        let wm_present = util::has_wm(&conn, &atoms, screen.root);
        let s = Arc::new(Shared {
            conn,
            atoms,
            root: screen.root,
            parent,
            stage: made.stage,
            curtain: made.curtain,
            gc: made.gc,
            depth: made.depth,
            wm_present,
            scale,
            state: Mutex::new(State::default()),
            observer: Mutex::new(None),
            render: Mutex::new(curtain_window::Render::new()),
            curtain_mapped: AtomicBool::new(false),
            tick: AtomicU8::new(Cadence::Off as u8),
            stop: AtomicBool::new(false),
            ticker: Mutex::new(None),
        });
        watch::start(&s)?;
        curtain_window::start_ticker(&s)?;
        Ok(s)
    }

    /// Verrou d'etat, tolerant a l'empoisonnement (l'etat reste coherent : chaque operation le met a jour d'un bloc).
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Annonce `ev` a l'observateur. JAMAIS sous le verrou d'etat : l'observateur est du code de l'appelant.
    fn emit(&self, ev: HostEvent) {
        let observer = self.observer.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(o) = observer {
            o(&ev);
        }
    }

    /// Etape cle : journal permanent (observateur) et, avec `CUBELINK_STAGE_DEBUG`, sur stderr.
    fn log(&self, m: impl Into<String>) {
        let m = m.into();
        dbg(&m);
        self.emit(HostEvent::Log(m));
    }

    fn flush(&self) {
        let _ = self.conn.flush();
    }

    /// Fenetre a nous (scene, rideau) : jamais prise pour une fenetre de jeu.
    fn own(&self, win: u32) -> bool {
        win == self.stage || win == self.curtain || win == self.parent
    }
}

/// Effets d'une transition, avec ce qu'il faut pour les executer une fois le verrou relache.
pub struct Todo {
    effects: Vec<Effect>,
    /// LA fenetre du jeu au moment de la transition.
    current: Option<u32>,
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
    for e in &effects {
        match *e {
            Effect::ShowCurtain { fresh: true } => st.curtain_t0 = Some(now),
            Effect::Detach => {
                st.tracker = None;
                st.pids.clear();
                st.adopted.clear();
                st.origin.clear();
                st.current = None;
                st.shown_at = None;
                st.started = None;
                st.refused.clear();
                st.adopted_at.clear();
                st.pose_pending = false;
                st.reframed_at = None;
                st.close_asked = false;
                // `fullscreen` et `launcher_fs` restent : `embedded::end_fullscreen` (effet `Detach`) les solde et rend le launcher
                st.game_rect = None;
                st.stage_mapped = false;
                st.stage_rect = None;
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
    Todo { effects, current }
}

/// Execute, verrou relache, les effets qui touchent les fenetres. Le minuteur est regle par le battement lui-meme.
fn run(s: &Shared, todo: &Todo) {
    run_if(s, todo, |_| true);
}

/// Comme `run`, pour les seuls effets retenus par `keep`.
fn run_if(s: &Shared, todo: &Todo, keep: impl Fn(&Effect) -> bool) {
    for e in todo.effects.iter().filter(|e| keep(e)) {
        match *e {
            Effect::ShowCurtain { .. } => embedded::curtain_on(s),
            Effect::HideCurtain => embedded::curtain_off(s),
            Effect::PlaceGame => {
                if let Some(g) = todo.current {
                    embedded::show_game(s, g);
                }
            }
            Effect::HideGame => {
                if let Some(g) = todo.current {
                    s.log("fenetre du jeu retiree de la zone de jeu (retour au launcher)");
                    embedded::hide_game(s, g);
                }
            }
            Effect::ClearRegion | Effect::SyncGame => embedded::sync(s),
            Effect::FocusGame => {
                if let Some(g) = todo.current {
                    embedded::foreground(s, g);
                }
            }
            Effect::Detach => {
                embedded::end_fullscreen(s, None);
                embedded::unstage(s)
            } // la scene vide ne reste pas affichee apres la fin du jeu
            Effect::CloseBarDone | Effect::Timer(_) => {}
        }
    }
    s.flush();
}

/// LA fenetre du jeu vient d'apparaitre (affichee par le jeu apres son adoption, ou deja visible a l'adoption).
fn mark_shown(st: &mut State, win: u32) {
    st.current = Some(win);
    st.shown_at = Some(Instant::now());
    if let Some(t) = st.tracker.as_mut() {
        t.window_shown();
    }
    dispatch(st, Event::WindowShown); // aucun effet : la pose se fait au battement suivant
}

/// Ouverture du rideau demandee.
fn on_reveal(s: &Shared, cancel: bool) {
    let pending = !cancel && s.lock().pose_pending;
    if pending {
        // ouverture demandee pendant l'attente de la premiere pose : on pose tout de suite (le rideau ne s'ouvre pas sur du vide)
        embedded::settle_pose(s, true);
    }
    let todo = {
        let mut st = s.lock();
        if cancel {
            st.close_asked = false; // le jeu ne s'est pas ferme : il reste dans la Scene, corrections comprises
        }
        dbg(&format!("ouverture demandee : vue={:?} adoptees={} courante={:?}", st.machine.view(), st.adopted.len(), st.current));
        dispatch(&mut st, if cancel { Event::CancelClose } else { Event::Reveal })
    };
    run(s, &todo);
}

/// REPLI : la Scene renonce pour le lancement en cours. Toute fenetre du jeu deja touchee lui est rendue, la surveillance
/// s'arrete, le rideau se rouvre sur le launcher, puis `HostEvent::Fallback` est emis, une seule fois (apres la restitution : a
/// l'instant de l'evenement, la fenetre EST rendue). Faux (sans effet) s'il n'y a pas de jeu surveille ou si le repli a deja eu lieu.
fn fallback(s: &Shared, reason: FallbackReason, detail: String) -> bool {
    let (todo, wins) = {
        let mut st = s.lock();
        if st.fell_back || st.pids.is_empty() {
            return false;
        }
        st.fell_back = true;
        let current = st.current;
        // Lancement cache : tant que LA fenetre du jeu n'est pas connue, toute candidate principale est rendue VISIBLE (hors de ce
        // mode, une fenetre pas encore affichee reste cachee : le jeu s'en charge). Jamais de jeu invisible.
        let hidden = st.expect_hidden;
        let mut wins: Vec<embedded::Returned> = st
            .origin
            .drain()
            .map(|(win, origin)| embedded::Returned { win, origin, main: current == Some(win) || (hidden && current.is_none() && guard::main_sized(origin.rect.2, origin.rect.3)) })
            .collect();
        wins.sort_by_key(|r| r.main); // LA fenetre du jeu en dernier : elle finit au-dessus des autres
        st.released = match current {
            Some(c) => vec![c],
            None => wins.iter().map(|r| r.win).collect(),
        };
        // le jeu n'est plus dans la Scene : meme chemin qu'un retour au launcher, sauf que sa fenetre lui est rendue (plus bas)
        let todo = dispatch(&mut st, Event::Leave);
        if todo.shows_curtain() {
            st.title.clear();
            st.caption = CAPTION_TO_LAUNCHER.to_string();
        }
        (todo, wins)
    };
    s.log(format!("repli decide ({}) : {detail}", reason.as_str()));
    // la fenetre est rendue AVANT que le rideau ne se rouvre : le joueur ne voit jamais la scene vide
    embedded::release(s, wins, reason, detail);
    run(s, &todo);
    true
}

/// Lancement cache : la candidate principale ADOPTEE, restee cachee et seule depuis `guard::HIDDEN_SETTLE`, devient LA fenetre du
/// jeu sans evenement d'affichage (il n'y en aura pas). Sa pose l'affichera.
fn promote_hidden(s: &Shared, now: Instant) -> Option<u32> {
    let win = {
        let st = s.lock();
        if !st.expect_hidden || st.current.is_some() {
            return None;
        }
        let mut mains = st.origin.iter().filter(|(_, o)| guard::main_sized(o.rect.2, o.rect.3)).map(|(w, _)| *w);
        let (win, None) = (mains.next()?, mains.next()) else { return None };
        let settled = st.adopted_at.get(&win).is_some_and(|t| now.saturating_duration_since(*t) >= guard::HIDDEN_SETTLE);
        if !settled {
            return None;
        }
        win
    };
    let ready = util::exists(&s.conn, win) && !util::is_mapped(&s.conn, win) && util::parent_of(&s.conn, win) == Some(s.stage);
    if !ready {
        return None;
    }
    let mut st = s.lock();
    if st.current.is_some() || !st.origin.contains_key(&win) {
        return None;
    }
    mark_shown(&mut st, win);
    Some(win)
}

/// Gardes du lancement en cours, a chaque battement du rideau : relance le rattachement s'il a ete refuse, declenche le repli
/// quand `crate::guard::verdict` le dit.
fn guard_tick(s: &Shared) {
    {
        let st = s.lock();
        if st.fell_back || st.pids.is_empty() {
            return;
        }
    }
    let now = Instant::now();
    let promoted = promote_hidden(s, now);
    // refus a oublier : fenetre disparue, ou tenue dans la scene depuis assez longtemps
    let gone: Vec<u32> = {
        let st = s.lock();
        let held = |w: &u32, bad_at: Instant| now.saturating_duration_since(bad_at) >= Duration::from_secs(1) && util::parent_of(&s.conn, *w) == Some(s.stage);
        st.refused.iter().filter(|(w, r)| !util::exists(&s.conn, **w) || held(w, r.bad_at)).map(|(w, _)| *w).collect()
    };
    let (verdict, retry, detail) = {
        let mut st = s.lock();
        // Ne restent que les refus qui comptent : fenetre toujours la, et qui est (ou peut encore etre) LA fenetre du jeu.
        let current = st.current;
        st.refused.retain(|w, _| !gone.contains(w) && (current.is_none() || current == Some(*w)));
        let refused = st.refused.values().map(|r| (now.saturating_duration_since(r.since), r.attempts)).max_by_key(|(since, attempts)| (*attempts >= guard::ADOPTION_MIN_ATTEMPTS, *since));
        let watched = Watched { active: true, since_watch: st.started.map(|t| now.saturating_duration_since(t)), window_shown: current.is_some(), refused };
        let verdict = guard::verdict(&st.guards, &watched);
        let detail = match verdict {
            Some(FallbackReason::AdoptionFailed) => {
                let (since, attempts) = watched.refused.unwrap_or_default();
                format!("fenetre du jeu toujours pas dans la scene du launcher apres {attempts} tentatives en {} s", since.as_secs())
            }
            Some(_) => format!("aucune fenetre de jeu affichee {} s apres le lancement", watched.since_watch.unwrap_or_default().as_secs()),
            None => String::new(),
        };
        // une tentative a la fois : la fenetre qui attend depuis le plus longtemps
        let retry = if verdict.is_none() {
            st.refused.iter_mut().filter(|(_, r)| now.saturating_duration_since(r.asked) >= ADOPTION_RETRY).min_by_key(|(_, r)| r.asked).map(|(w, r)| {
                r.asked = now;
                *w
            })
        } else {
            None
        };
        (verdict, retry, detail)
    };
    if let Some(win) = promoted {
        s.log(format!("lancement cache : fenetre du jeu ({}) tenue pour affichee (le jeu ne la montrera pas, l'hote le fera a la pose)", util::class_name(&s.conn, win)));
        embedded::announce(s, win);
    }
    if let Some(win) = retry {
        embedded::retry(s, win);
    }
    if let Some(reason) = verdict {
        fallback(s, reason, detail);
    }
}

// ---------------------------------------------------------------------------------------------------- API publique

/// La Scene X11 : l'API commune (`crate::scene`) sur ce systeme.
pub type GameHost = crate::scene::GameHost<Shared>;

impl Backend for Shared {
    type Win = u32;
    type State = State;
    type Todo = Todo;

    fn install(parent: isize) -> Result<Arc<Self>, String> {
        let parent = u32::try_from(parent).map_err(|_| "XID de fenetre invalide".to_string())?;
        Shared::install(parent)
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        Shared::lock(self)
    }

    fn log(&self, m: String) {
        Shared::log(self, m);
    }

    fn set_observer(&self, observer: Observer) {
        *self.observer.lock().unwrap_or_else(|e| e.into_inner()) = Some(observer);
    }

    fn dispatch(st: &mut State, ev: Event) -> Todo {
        dispatch(st, ev)
    }

    fn shows_curtain(todo: &Todo) -> bool {
        todo.shows_curtain()
    }

    fn has_effects(todo: &Todo) -> bool {
        !todo.effects.is_empty()
    }

    fn run(&self, todo: &Todo) {
        run(self, todo);
    }

    fn sync(&self) {
        embedded::sync(self);
        self.flush();
    }

    fn reset_for_watch(st: &mut State) {
        st.origin.clear();
        st.refused.clear();
        st.game_rect = None;
        st.scanned_at = None;
    }

    fn start_watch(&self, pid: u32) {
        self.log(format!("hooks poses sur le processus {pid} (evenements X11 de la racine)"));
        watch::scan(self, true); // fenetres deja creees avant `watch`
        curtain_window::request_sync(self); // relance le minuteur (filet de securite, gardes) s'il etait arrete
    }

    fn reveal(&self, cancel: bool) {
        on_reveal(self, cancel);
    }

    fn ask_close(&self, w: u32) {
        util::ask_close(&self.conn, &self.atoms, w);
    }

    fn fallback(&self, reason: FallbackReason, detail: String) -> bool {
        fallback(self, reason, detail)
    }

    /// Arrete les fils et detruit la scene et le rideau (les fenetres du jeu ne sont pas touchees : un jeu encore dans la scene
    /// est d'abord rendu au bureau, jamais detruit avec elle).
    fn shutdown(&self) {
        // un jeu encore dans la scene serait detruit avec elle (BadWindow) : on le rend d'abord au bureau (sans effet sans jeu surveille)
        fallback(self, FallbackReason::Requested, "hote arrete".into());
        embedded::end_fullscreen(self, None); // jamais de launcher laisse en plein ecran (sans effet si deja solde)
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.conn.destroy_window(self.curtain);
        let _ = self.conn.destroy_window(self.stage);
        let ev = ClientMessageEvent::new(32, self.parent, self.atoms._CUBELINK_HOST, [0u32; 5]);
        let _ = self.conn.send_event(false, self.parent, EventMask::STRUCTURE_NOTIFY, ev); // reveille le fil des evenements
        self.flush();
        if let Some(t) = self.ticker.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            t.unpark();
        }
    }
}
