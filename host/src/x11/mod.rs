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
//! * `mod.rs`            : API publique ([`GameHost`]), etat partage, execution des effets, repli, gardes.
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

use crate::flow::{Bar, Cadence, Content, Effect, Event, Machine, View};
use crate::guard::{self, Guards, Watched, ADOPTION_RETRY};
use crate::health::{FallbackReason, HostEvent, Observer};
use crate::progress::{Closer, Profile, Tracker};
use embedded::Origin;
use util::Atoms;

const CAPTION_SWITCH: &str = "Changement de modpack";
const CAPTION_TO_LAUNCHER: &str = "Retour au launcher";
const CAPTION_TO_GAME: &str = "Retour au jeu";

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
struct State {
    /// Deroulement de la vue (rideau, pose de la fenetre du jeu, panneau).
    machine: Machine,
    pids: HashSet<u32>,
    adopted: Vec<u32>,
    /// LA fenetre du jeu (adoptee puis affichee par le jeu).
    current: Option<u32>,
    shown_at: Option<Instant>,
    title: String,
    caption: String,
    tracker: Option<Tracker>,
    closer: Option<Closer>,
    /// Apparition du rideau (horloge de ses animations).
    curtain_t0: Option<Instant>,
    /// Toute fenetre du jeu qu'on a touchee, telle qu'elle etait avant : de quoi la rendre.
    origin: HashMap<u32, Origin>,
    /// Zone de jeu en coordonnees client de la fenetre du launcher (x, y, largeur, hauteur).
    view: (i32, i32, i32, i32),
    /// Debut de la surveillance du jeu courant (`watch`).
    started: Option<Instant>,
    /// La Scene a renonce pour ce lancement (repli) ; remis a faux par `begin`.
    fell_back: bool,
    /// `HostEvent::Adopted` deja emis pour le jeu surveille.
    announced: bool,
    /// Fenetres dont le rattachement est refuse, tant qu'elles peuvent etre LA fenetre du jeu.
    refused: HashMap<u32, Refused>,
    /// Lancement CACHE (`GameHost::expect_hidden`) : le jeu n'affichera pas sa fenetre lui-meme ; c'est l'hote qui la montre, a la
    /// pose ou, quoi qu'il arrive, en la lui rendant. Remis a faux par `begin`.
    expect_hidden: bool,
    /// La premiere pose de LA fenetre du jeu attend (`guard::pose_waits`).
    pose_pending: bool,
    /// Dernier cadre remis par le jeu sur une fenetre pas encore posee.
    reframed_at: Option<Instant>,
    /// Instant ou chaque fenetre adoptee l'a ete.
    adopted_at: HashMap<u32, Instant>,
    /// Numero du lancement surveille (change a chaque `watch` et a chaque fin de jeu).
    gen: u64,
    /// `request_close` a ete demande au jeu surveille : sa fenetre se defait, on ne la corrige plus.
    close_asked: bool,
    /// Fenetres rendues au jeu par le dernier repli : `request_close` peut encore leur demander de se fermer.
    released: Vec<u32>,
    /// LA fenetre du jeu occupe l'ecran (derniere valeur annoncee par `HostEvent::Fullscreen`).
    fullscreen: bool,
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
    guards: Guards,
    /// Corps (sans horodatage) de la derniere ligne du jeu : un bruit repete ne compte pas pour la quietude (`is_noise`).
    last_body: Option<String>,
    /// Lignes ecartees comme bruit depuis le debut de la surveillance (journalisees a l'ouverture forcee).
    noise_lines: u64,
    /// L'ouverture forcee par `READY_MAX` a deja ete journalisee pour ce lancement.
    forced_logged: bool,
}

pub(super) struct Shared {
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
struct Todo {
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

/// Fenetres a qui demander la fermeture : LA fenetre du jeu seulement ; a defaut (pas encore affichee), toutes les fenetres
/// adoptees ; apres un repli, celles qu'on a rendues au jeu.
fn close_targets(st: &State) -> Vec<u32> {
    match st.current {
        Some(c) => vec![c],
        None if !st.adopted.is_empty() => st.adopted.clone(),
        None => st.released.clone(),
    }
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

/// La Scene : zone de jeu dans la fenetre du launcher, rideau, et jeu courant.
pub struct GameHost {
    s: Arc<Shared>,
}

impl GameHost {
    /// Cree la scene (sous-fenetre de `parent`, un XID de la fenetre du launcher) et le rideau, et demarre les fils de l'hote.
    /// `Err` si aucun serveur X11 n'est joignable (`DISPLAY`) : le jeu garde alors sa propre fenetre.
    pub fn embed(parent: isize) -> Result<GameHost, String> {
        let parent = u32::try_from(parent).map_err(|_| "XID de fenetre invalide".to_string())?;
        Ok(GameHost { s: Shared::install(parent)? })
    }

    /// Place la zone du jeu et le rideau (coordonnees physiques dans la zone cliente de la fenetre parente).
    pub fn relayout(&self, x: i32, y: i32, w: i32, h: i32) {
        self.s.lock().view = (x, y, w.max(1), h.max(1));
        embedded::sync(&self.s);
        self.s.flush();
    }

    /// Un lancement commence : les panneaux se ferment sur le launcher (ou sur le jeu en cours) puis le chargement s'affiche.
    /// Le lancement est (de nouveau) dans la Scene : `staged` redevient vrai.
    pub fn begin(&self, title: &str, caption: &str, mods_dir: Option<&std::path::Path>, profile_path: Option<&std::path::Path>) {
        // lectures disque hors du verrou
        let n = mods_dir.map_or(0, count_jars);
        let prev = profile_path.and_then(Profile::load);
        let todo = {
            let mut st = self.s.lock();
            let todo = dispatch(&mut st, Event::BeginLaunch);
            st.fell_back = false;
            st.expect_hidden = false; // consigne du lancement precedent : a redonner pour celui-ci
            st.tracker = Some(Tracker::new(n, prev));
            st.title = title.to_string();
            // la fermeture de l'ancien jeu vient d'occuper la 1re moitie de la barre
            let second_half = st.machine.content() == Content::Launch(Bar::SecondHalf);
            st.caption = if second_half { CAPTION_SWITCH } else { caption }.to_string();
            todo
        };
        self.s.log(format!("lancement : rideau ferme ({n} mods)"));
        run(&self.s, &todo);
    }

    /// Surveille les fenetres du processus `pid` (le nouveau jeu) : elles seront adoptees des leur creation.
    pub fn watch(&self, pid: u32) {
        let todo = {
            let mut st = self.s.lock();
            let todo = dispatch(&mut st, Event::Watch);
            st.origin.clear();
            st.pids.clear();
            st.pids.insert(pid);
            st.adopted.clear();
            st.released.clear();
            st.current = None;
            st.shown_at = None;
            st.refused.clear();
            st.adopted_at.clear();
            st.pose_pending = false;
            st.reframed_at = None;
            st.close_asked = false;
            st.announced = false;
            st.game_rect = None;
            st.scanned_at = None;
            st.last_body = None;
            st.noise_lines = 0;
            st.forced_logged = false;
            st.gen += 1;
            st.started = Some(Instant::now());
            todo
        };
        run(&self.s, &todo);
        self.s.log(format!("surveillance des fenetres du processus {pid}"));
        self.s.log(format!("hooks poses sur le processus {pid} (evenements X11 de la racine)"));
        watch::scan(&self.s, true); // fenetres deja creees avant `watch`
        curtain_window::request_sync(&self.s); // relance le minuteur (filet de securite, gardes) s'il etait arrete
    }

    /// Une ligne de la sortie du jeu (alimente l'avancement du lancement ou de la fermeture). Le bruit repete (voir `is_noise`) n'est
    /// pas compte comme de l'activite : un jeu qui se plaint en boucle est « silencieux » pour la quietude du `is_ready`.
    pub fn feed_line(&self, line: &str) {
        let mut st = self.s.lock();
        let State { tracker, last_body, noise_lines, .. } = &mut *st;
        if let Some(t) = tracker.as_mut() {
            if feed_tracker(t, last_body, line) {
                *noise_lines += 1;
            }
        } else {
            *last_body = Some(line_body(line).to_string());
        }
        if let Some(c) = st.closer.as_mut() {
            c.feed_line(line);
        }
    }

    /// Le jeu a l'air pret (textures assemblees, fenetre affichee, plus de bruit utile depuis `quiet`) ou la fenetre est affichee
    /// depuis trop longtemps (`max`) : on ne laisse jamais le rideau bloque, meme si le jeu continue d'ecrire (l'ouverture forcee est
    /// journalisee avec sa raison). Apres un repli il n'y a plus rien a attendre de la Scene : vrai aussi (le rideau est deja
    /// rouvert, `mark_ready` et `reveal` sont alors sans effet). Si aucune fenetre n'apparait, c'est le delai de garde
    /// (`crate::guard::NO_WINDOW_GUARD`) qui declenche le repli.
    pub fn is_ready(&self, quiet: Duration, max: Duration) -> bool {
        let forced = {
            let mut st = self.s.lock();
            if st.fell_back || st.tracker.as_ref().is_some_and(|t| t.looks_ready(quiet)) {
                return true;
            }
            let Some(elapsed) = st.shown_at.map(|t| t.elapsed()).filter(|e| *e > max) else { return false };
            let first = !std::mem::replace(&mut st.forced_logged, true);
            let (phase, silent) = st.tracker.as_ref().map_or((None, Duration::ZERO), |t| (Some(t.phase()), t.silent_for()));
            first.then(|| format!("ouverture forcee du rideau : fenetre du jeu affichee depuis {} s (maximum {} s), etape {:?}, derniere ligne utile il y a {:.1} s, {} ligne(s) repetee(s) ecartee(s)", elapsed.as_secs(), max.as_secs(), phase, silent.as_secs_f32(), st.noise_lines))
        };
        if let Some(m) = forced {
            self.s.log(m);
        }
        true
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

    /// Ouvre le rideau (les panneaux s'ecartent et decouvrent le jeu).
    pub fn reveal(&self) {
        on_reveal(&self.s, false);
    }

    /// Debut d'un changement de modpack : les panneaux se ferment sur le jeu en cours et la barre suit sa fermeture (0-50 %).
    pub fn begin_close(&self, title: &str) {
        let todo = {
            let mut st = self.s.lock();
            let todo = dispatch(&mut st, Event::BeginClose);
            st.closer = Some(Closer::new());
            st.tracker = None;
            st.title = title.to_string();
            st.caption = CAPTION_SWITCH.to_string();
            todo
        };
        self.s.log("changement de modpack : rideau ferme sur le jeu en cours");
        run(&self.s, &todo);
    }

    /// Demande au jeu de se fermer comme le ferait le joueur (croix de la fenetre, `WM_DELETE_WINDOW`) : il quitte le monde,
    /// enregistre, s'eteint. Vaut aussi pour un jeu rendu par un repli (sa fenetre est connue jusqu'au prochain `watch`).
    pub fn request_close(&self) {
        let windows = {
            let mut st = self.s.lock();
            st.close_asked = true;
            close_targets(&st)
        };
        self.s.log(format!("fermeture demandee au jeu ({} fenetre(s))", windows.len()));
        for w in windows {
            util::ask_close(&self.s.conn, &self.s.atoms, w);
        }
    }

    /// La tache de jeu est terminee (arret du jeu, echec du lancement ou kill). Pendant un changement de modpack (fermeture
    /// demandee) : la barre atteint 50 % et rien d'autre ; sinon retour au launcher avec la meme animation de rideau. Apres un
    /// repli le rideau est deja rouvert sur le launcher : la fin du jeu rendu ne rejoue aucune animation.
    pub fn run_finished(&self) {
        let todo = {
            let mut st = self.s.lock();
            st.released.clear();
            st.gen += 1; // le jeu est parti : plus rien a guetter pour lui
            if st.fell_back && !st.machine.closing() && !st.machine.attached() {
                None
            } else {
                let todo = dispatch(&mut st, Event::GameExited);
                if todo.shows_curtain() {
                    st.title.clear();
                    st.caption = CAPTION_TO_LAUNCHER.to_string();
                }
                Some(todo)
            }
        };
        self.s.log(if todo.is_some() { "jeu termine" } else { "jeu termine (il avait ete rendu : rien a faire)" });
        if let Some(todo) = todo {
            run(&self.s, &todo);
        }
    }

    /// Le jeu ne s'est pas ferme dans le delai : on le laisse tranquille (jamais force) et on rouvre le rideau sur lui.
    pub fn cancel_close(&self) {
        on_reveal(&self.s, true);
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

    /// Delais de garde (voir `crate::guard`). Les valeurs par defaut sont celles de production ; les tests d'integration les
    /// raccourcissent.
    pub fn set_guards(&self, guards: Guards) {
        self.s.lock().guards = guards;
    }
}

/// CONTRAT avec le launcher (voir `crate::health`). Signatures figees.
impl GameHost {
    /// Installe l'observateur des evenements de sante (remplace le precedent).
    pub fn set_observer(&self, observer: Observer) {
        *self.s.observer.lock().unwrap_or_else(|e| e.into_inner()) = Some(observer);
    }

    /// Mouvement reduit (preference systeme) : le rideau ne s'anime plus (panneaux poses / retires sans glissement, logo fige).
    pub fn set_calm(&self, calm: bool) {
        self.s.lock().machine.set_calm(calm);
    }

    /// Rend au jeu sa fenetre (au bureau, avec le cadre du gestionnaire de fenetres, position visible, plus de scene), rouvre le
    /// rideau sur le launcher et arrete la surveillance pour CE lancement ; emet `HostEvent::Fallback { reason: Requested, .. }`.
    /// Sans jeu : sans effet.
    pub fn release(&self, detail: &str) {
        fallback(&self.s, FallbackReason::Requested, detail.to_string());
    }

    /// Lancement CACHE : le jeu n'affichera pas sa fenetre lui-meme (fenetre creee invisible, par exemple via l'agent Java). A
    /// appeler apres `begin` et avant `watch` ; vaut pour CE lancement, `begin` le remet a faux. L'hote tient alors la fenetre
    /// adoptee pour celle du jeu sans attendre qu'il l'affiche, l'affiche lui-meme a la pose, et GARANTIT qu'elle soit montree si
    /// la Scene renonce (tout repli) : jamais de jeu invisible.
    pub fn expect_hidden(&self, hidden: bool) {
        self.s.lock().expect_hidden = hidden;
    }

    /// Le lancement en cours est toujours dans la Scene (faux apres un repli, jusqu'au prochain `begin`).
    pub fn staged(&self) -> bool {
        !self.s.lock().fell_back
    }
}

impl Drop for GameHost {
    /// Arrete les fils et detruit la scene et le rideau (les fenetres du jeu ne sont pas touchees : un jeu encore dans la scene
    /// est d'abord rendu au bureau, jamais detruit avec elle).
    fn drop(&mut self) {
        // un jeu encore dans la scene serait detruit avec elle (BadWindow) : on le rend d'abord au bureau (sans effet sans jeu surveille)
        fallback(&self.s, FallbackReason::Requested, "hote arrete".into());
        embedded::end_fullscreen(&self.s, None); // jamais de launcher laisse en plein ecran (sans effet si deja solde)
        self.s.stop.store(true, Ordering::SeqCst);
        let s = &self.s;
        let _ = s.conn.destroy_window(s.curtain);
        let _ = s.conn.destroy_window(s.stage);
        let ev = ClientMessageEvent::new(32, s.parent, s.atoms._CUBELINK_HOST, [0u32; 5]);
        let _ = s.conn.send_event(false, s.parent, EventMask::STRUCTURE_NOTIFY, ev); // reveille le fil des evenements
        s.flush();
        if let Some(t) = s.ticker.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            t.unpark();
        }
    }
}

/// Ligne du jeu sans son horodatage `[hh:mm:ss] ` de tete.
fn line_body(line: &str) -> &str {
    let l = line.trim_end();
    let b = l.as_bytes();
    if b.len() > 11 && b[0] == b'[' && b[3] == b':' && b[6] == b':' && b[9] == b']' && b[10] == b' ' && [1, 2, 4, 5, 7, 8].iter().all(|i| b[*i].is_ascii_digit()) {
        &l[11..]
    } else {
        l
    }
}

/// Bruit repete qui ne doit pas compter comme de l'activite du jeu : l'avertissement SDL 3 « Failed to synchronize SDL window » (une
/// fois par image tant qu'une synchronisation de fenetre echoue), ou une ligne identique a la precedente, horodatage mis a part.
fn is_noise(body: &str, last: Option<&str>) -> bool {
    body.contains("Failed to synchronize SDL window") || last == Some(body)
}

/// Passe `line` au suivi du lancement ; vrai si c'etait du bruit (analysee mais sans compter comme activite).
fn feed_tracker(t: &mut Tracker, last_body: &mut Option<String>, line: &str) -> bool {
    let body = line_body(line);
    let noise = is_noise(body, last_body.as_deref());
    if last_body.as_deref() != Some(body) {
        *last_body = Some(body.to_string());
    }
    if noise {
        t.feed_line_quiet(line);
    } else {
        t.feed_line(line);
    }
    noise
}

fn count_jars(dir: &std::path::Path) -> u32 {
    std::fs::read_dir(dir).map_or(0, |d| d.filter_map(|e| e.ok()).filter(|e| e.path().extension().is_some_and(|x| x == "jar")).count() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    // lignes COPIEES du journal reel d'un lancement (Minecraft 26.3, LWJGL 3.4.3 / SDL 3, XWayland) ou l'avertissement SDL
    // revenait ~10 fois par seconde apres le dernier message utile
    const SPAM: &str = "[20:33:35] [Render thread/WARN]: Failed to synchronize SDL window: ";
    const SPAM_NEXT_SECOND: &str = "[20:33:36] [Render thread/WARN]: Failed to synchronize SDL window: ";
    const RELOAD: &str = "[20:33:32] [Render thread/INFO]: Reloading ResourceManager: vanilla, vanilla";
    const SOUND: &str = "[20:33:34] [Render thread/INFO]: Sound engine started";
    const ATLAS: &str = "[20:33:34] [Render thread/INFO]: Created: 1024x1024x0 minecraft:textures/atlas/gui.png-atlas";

    /// `GameHost::feed_line` sans hote : meme filtre, sur un `Tracker`.
    fn feed(t: &mut Tracker, last: &mut Option<String>, line: &str) -> bool {
        feed_tracker(t, last, line)
    }

    #[test]
    fn horodatage_retire() {
        assert_eq!(line_body(SPAM), "[Render thread/WARN]: Failed to synchronize SDL window:");
        assert_eq!(line_body("sans horodatage  "), "sans horodatage");
        assert_eq!(line_body("[Cubelink] READY"), "[Cubelink] READY");
    }

    #[test]
    fn le_bruit_sdl_repete_ne_repousse_pas_la_quietude() {
        let (mut t, mut last) = (Tracker::new(0, None), None);
        for l in [RELOAD, SOUND, ATLAS] {
            assert!(!feed(&mut t, &mut last, l), "ligne utile prise pour du bruit : {l}");
        }
        t.window_shown();
        std::thread::sleep(Duration::from_millis(300));
        // le jeu se plaint en boucle, une ligne « nouvelle » (horodatage) a chaque seconde : ce n'est pas de l'activite
        for l in [SPAM, SPAM, SPAM_NEXT_SECOND, SPAM_NEXT_SECOND] {
            assert!(feed(&mut t, &mut last, l));
        }
        assert!(t.looks_ready(Duration::from_millis(200)), "le bruit a repousse la quietude");
        assert!(t.silent_for() >= Duration::from_millis(300));
    }

    #[test]
    fn une_ligne_utile_repousse_la_quietude() {
        let (mut t, mut last) = (Tracker::new(0, None), None);
        for l in [RELOAD, SOUND, ATLAS] {
            feed(&mut t, &mut last, l);
        }
        t.window_shown();
        std::thread::sleep(Duration::from_millis(300));
        assert!(!feed(&mut t, &mut last, "[20:33:37] [Render thread/INFO]: Narrator loaded"), "une vraie ligne n'est pas du bruit");
        assert!(!t.looks_ready(Duration::from_millis(200)), "une ligne utile doit relancer la quietude");
    }

    #[test]
    fn une_ligne_identique_a_la_precedente_est_du_bruit_pas_la_premiere() {
        assert!(!is_noise("a", None));
        assert!(!is_noise("a", Some("b")));
        assert!(is_noise("a", Some("a")));
        assert!(is_noise("[Render thread/WARN]: Failed to synchronize SDL window:", None));
    }
}
