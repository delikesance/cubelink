//! Implementation Win32 de la Scene (voir la doc du crate).
//!
//! Couche MINCE : elle traduit ce qui se passe en [`Event`], les passe a la machine [`crate::flow`] sous le verrou d'etat, puis
//! execute les [`Effect`] rendus une fois le verrou relache.
//!
//! * `mod.rs`            : API publique ([`GameHost`]), etat partage, execution des effets, repli.
//! * `embedded.rs`       : zone de jeu et rideau dans la fenetre du launcher ; adoption et calage de la fenetre du jeu.
//! * `game_window.rs`    : fil unique des appels synchrones vers la fenetre du jeu (adoption, styles, decoupe, restitution).
//! * `watch.rs`          : surveillance des fenetres du processus de jeu (hooks WinEvent, balayage de secours).
//! * `curtain_window.rs` : contenu, rendu et minuteur du rideau.
//! * `debug.rs`, `util.rs` : journal de diagnostic et mesures ; petits outils Win32.
//!
//! Verrou d'etat : pris UNE fois par operation, jamais tenu pendant un appel Win32 qui peut attendre un autre fil, ni pendant un
//! appel a l'observateur ([`crate::health`]).
//!
//! Repli : quand la Scene ne peut pas tenir le jeu (hooks refuses, rattachement refuse, aucune fenetre dans le delai de garde, ou
//! demande de l'appelant), `fallback` rend sa fenetre au jeu, rouvre le rideau sur le launcher et annonce
//! [`HostEvent::Fallback`]. Le jeu n'est jamais tue et ne reste jamais invisible.

mod curtain_window;
mod debug;
mod embedded;
mod game_window;
mod util;
mod watch;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::flow::{Bar, Content, Effect, Event, Machine, View};
use crate::guard::{self, Guards, Watched, ADOPTION_RETRY};
use crate::health::{FallbackReason, HostEvent, Observer};
use crate::progress::{Closer, Profile, Tracker};
use debug::dbg;
use game_window::{Origin, Returned};
use util::hw;

/// Le rideau doit etre affiche / remonte et anime (envoye a la zone de jeu, execute sur le fil de l'interface).
const WM_APP_CURTAIN_ON: u32 = WM_APP + 2;
/// Ouvrir le rideau (`wparam` = 1 : la fermeture de l'ancien jeu est abandonnee).
const WM_APP_REVEAL: u32 = WM_APP + 3;
/// Recaler le jeu / le rideau (poste par les evenements Windows, execute sur le fil de l'interface).
const WM_APP_SYNC: u32 = WM_APP + 5;
/// Une fenetre du jeu vient d'apparaitre (lparam = son HWND) : le rideau remonte juste au-dessus d'elle.
const WM_APP_RAISE: u32 = WM_APP + 6;
/// Fil de surveillance : (re)pose les hooks sur le processus `wparam` (0 = les retire).
const WM_APP_HOOK: u32 = WM_APP + 7;

const CAPTION_SWITCH: &str = "Changement de modpack";
const CAPTION_TO_LAUNCHER: &str = "Retour au launcher";
const CAPTION_TO_GAME: &str = "Retour au jeu";

// ---------------------------------------------------------------------------------------------------- etat partage

/// Rattachement refuse : une fenetre n'est pas possedee par celle du launcher apres une tentative terminee.
#[derive(Clone, Copy, Debug)]
struct Refused {
    /// Premier refus constate.
    since: Instant,
    /// Tentatives terminees sur un refus.
    attempts: u32,
    /// Derniere nouvelle tentative demandee.
    asked: Instant,
}

/// Fenetre du jeu visible a l'ecran alors qu'elle n'est pas rattachee (le jeu l'a affichee et ne repond pas).
#[derive(Clone, Copy, Debug)]
struct Exposed {
    since: Instant,
    /// Deja signalee au journal permanent (une ligne par fenetre).
    logged: bool,
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
    /// Toute fenetre du jeu qu'on a touchee (garee, adoptee), telle qu'elle etait avant : de quoi la rendre. Une fenetre absente
    /// d'ici n'est plus (ou pas) a nous : le fil de la fenetre du jeu ignore les commandes qui la visent.
    origin: HashMap<isize, Origin>,
    /// (largeur, hauteur, bande) de la decoupe actuellement appliquee a la fenetre du jeu.
    region: (i32, i32, i32),
    /// Zone de jeu en coordonnees client de la fenetre du launcher (x, y, largeur, hauteur).
    view: (i32, i32, i32, i32),
    /// Debut de la surveillance du jeu courant (`watch`).
    started: Option<Instant>,
    /// La Scene a renonce pour ce lancement (repli) ; remis a faux par `begin`.
    fell_back: bool,
    /// `HostEvent::Adopted` deja emis pour le jeu surveille.
    announced: bool,
    /// Fenetres dont le rattachement est refuse, tant qu'elles peuvent etre LA fenetre du jeu (`current` inconnue, ou elle-meme).
    refused: HashMap<isize, Refused>,
    /// Fenetres secondaires non rattachees deja signalees au journal (une ligne par fenetre).
    shrugged: HashSet<isize>,
    /// Lancement CACHE (`GameHost::expect_hidden`) : le jeu n'affichera pas sa fenetre lui-meme ; c'est l'hote qui la montre, a la
    /// pose ou, quoi qu'il arrive, en la lui rendant. Vaut pour le lancement en cours ; remis a faux par `begin`.
    expect_hidden: bool,
    /// La premiere pose de LA fenetre du jeu attend (`guard::pose_waits`) : elle reste garee hors ecran, rien ne la cale.
    pose_pending: bool,
    /// Dernier cadre remis par le jeu sur une fenetre pas encore posee.
    reframed_at: Option<Instant>,
    /// Instant ou chaque fenetre adoptee l'a ete.
    adopted_at: HashMap<isize, Instant>,
    /// Numero du lancement surveille (change a chaque `watch` et a chaque fin de jeu) : ce qui a ete mis en route pour un
    /// lancement (guet d'une fenetre cachee) s'arrete quand il n'est plus le courant.
    gen: u64,
    /// Fenetres que le jeu a AFFICHEES avant la fin de leur adoption (voir `embedded::adopt`).
    shown_early: HashSet<isize>,
    /// Fenetres constatees visibles a l'ecran sans etre rattachees (voir `embedded::cover_exposed`).
    exposed: HashMap<isize, Exposed>,
    /// `request_close` a ete demande au jeu surveille : sa fenetre se defait, on ne la corrige plus.
    close_asked: bool,
    /// Fenetres rendues au jeu par le dernier repli : `request_close` peut encore leur demander de se fermer.
    released: Vec<isize>,
    guards: Guards,
}

struct Shared {
    /// Zone de jeu : cible des messages de l'API.
    host: AtomicIsize,
    curtain: AtomicIsize,
    /// Fenetre du launcher.
    parent: isize,
    state: Mutex<State>,
    /// Observateur de l'appelant. Verrou distinct de l'etat : il n'est tenu que le temps de cloner le pointeur.
    observer: Mutex<Option<Observer>>,
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
    fn install(parent: isize) -> Result<Arc<Shared>, String> {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared {
            host: AtomicIsize::new(0),
            curtain: AtomicIsize::new(0),
            parent,
            state: Mutex::new(State::default()),
            observer: Mutex::new(None),
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

    /// Annonce `ev` a l'observateur. JAMAIS sous le verrou d'etat : l'observateur est du code de l'appelant.
    fn emit(&self, ev: HostEvent) {
        let observer = self.observer.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(o) = observer {
            o(&ev);
        }
    }

    /// Etape cle : journal permanent (observateur) et, avec `CUBELINK_STAGE_DEBUG`, dans le fil horodate du diagnostic.
    fn log(&self, m: impl Into<String>) {
        let m = m.into();
        dbg(&m);
        self.emit(HostEvent::Log(m));
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
                st.adopting.clear();
                st.origin.clear();
                st.current = None;
                st.shown_at = None;
                st.started = None;
                st.refused.clear();
                st.shrugged.clear();
                st.exposed.clear();
                st.shown_early.clear();
                st.adopted_at.clear();
                st.pose_pending = false;
                st.reframed_at = None;
                st.close_asked = false;
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
                        embedded::show_game(s, hw(g));
                    }
                }
                Effect::HideGame => {
                    if let Some(g) = todo.current {
                        s.log("fenetre du jeu retiree de la zone de jeu (retour au launcher)");
                        embedded::hide_game(s, hw(g));
                    }
                }
                Effect::ClearRegion => game_window::clear_region(s, todo.adopted.clone()),
                Effect::FocusGame => {
                    if let Some(g) = todo.current {
                        game_window::foreground(s, g);
                    }
                }
                Effect::SyncGame => embedded::sync(s),
                Effect::Detach => {
                    watch::set_hooks(0);
                }
                Effect::CloseBarDone | Effect::Timer(_) => {}
            }
        }
    }
}

/// LA fenetre du jeu vient d'apparaitre (affichee par le jeu apres son adoption, ou deja visible a l'adoption).
fn mark_shown(st: &mut State, win: isize) {
    st.current = Some(win);
    st.shown_at = Some(Instant::now());
    if let Some(t) = st.tracker.as_mut() {
        t.window_shown();
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
        if !cancel && st.pose_pending {
            // ouverture demandee pendant l'attente de la premiere pose : on pose tout de suite (le rideau ne s'ouvre pas sur du vide)
            drop(st);
            embedded::settle_pose(s, true);
            st = s.lock();
        }
        if cancel {
            st.close_asked = false; // le jeu ne s'est pas ferme : il reste dans la Scene, corrections comprises
        }
        dispatch(&mut st, if cancel { Event::CancelClose } else { Event::Reveal })
    };
    run(s, &todo);
}

/// Fenetres a qui demander la fermeture : LA fenetre du jeu seulement (les fenetres d'aide adoptees n'ont pas a recevoir WM_CLOSE) ;
/// a defaut (pas encore affichee), toutes les fenetres adoptees ; apres un repli, celles qu'on a rendues au jeu.
fn close_targets(st: &State) -> Vec<isize> {
    match st.current {
        Some(c) => vec![c],
        None if !st.adopted.is_empty() => st.adopted.clone(),
        None => st.released.clone(),
    }
}

/// REPLI : la Scene renonce pour le lancement en cours. Toute fenetre du jeu deja touchee lui est rendue (sur le fil de la fenetre
/// du jeu), la surveillance s'arrete, le rideau se rouvre sur le launcher, puis `HostEvent::Fallback` est emis, une seule fois.
/// Faux (sans effet) s'il n'y a pas de jeu surveille ou si le repli a deja eu lieu. Appelable de n'importe quel fil.
fn fallback(s: &Shared, reason: FallbackReason, detail: String) -> bool {
    // Lancement cache : avant de conclure, balayage des fenetres du jeu, CACHEES comprises. Une fenetre principale que la
    // surveillance n'aurait jamais vue (hooks refuses, fenetre creee avant eux) est rendue, donc montree, comme les autres.
    let unseen: Vec<isize> = {
        let pids = {
            let st = s.lock();
            (st.expect_hidden && !st.fell_back && st.current.is_none()).then(|| st.pids.clone())
        };
        pids.map(|p| watch::main_candidates(&p).into_iter().filter(|(_, visible)| !visible).map(|(h, _)| h).collect()).unwrap_or_default()
    };
    let (todo, wins, rescue) = {
        let mut st = s.lock();
        if st.fell_back || st.pids.is_empty() {
            return false;
        }
        st.fell_back = true;
        let current = st.current;
        // Lancement cache : le jeu n'affichera jamais sa fenetre lui-meme. Tant que LA fenetre du jeu n'est pas connue, toute
        // candidate principale est donc rendue VISIBLE (hors de ce mode, une fenetre pas encore affichee reste cachee : le jeu
        // s'en charge). Jamais de jeu invisible.
        let hidden = st.expect_hidden;
        let is_main = |win: isize, o: &Origin| current == Some(win) || (hidden && current.is_none() && guard::main_sized(o.rect.2, o.rect.3) && unsafe { IsWindow(hw(win)) } != 0);
        let mut wins: Vec<Returned> = st.origin.drain().map(|(win, origin)| Returned { win, origin, main: is_main(win, &origin) }).collect();
        if hidden && current.is_none() {
            for h in unseen {
                if !wins.iter().any(|r| r.win == h) {
                    wins.push(Returned { win: h, origin: unsafe { Origin::of(hw(h)) }, main: true });
                }
            }
        }
        // ... et s'il n'y a aucune fenetre principale a rendre (pas encore creee, ou jamais vue faute de hooks), on la guette
        let rescue = (hidden && !wins.iter().any(|r| r.main)).then(|| (st.pids.clone(), st.gen));
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
        (todo, wins, rescue)
    };
    s.log(format!("repli decide ({}) : {detail}", reason.as_str()));
    run(s, &todo); // retire les hooks (`Detach`), referme puis rouvre le rideau sur le launcher
    game_window::release(s, wins, reason, detail);
    if let Some((pids, gen)) = rescue {
        s.log("lancement cache : aucune fenetre de jeu a rendre pour l'instant, elle sera montree des qu'elle existera");
        watch::rescue_hidden(pids, gen);
    }
    true
}

/// Lancement cache : la candidate principale ADOPTEE, restee cachee et seule depuis `guard::HIDDEN_SETTLE`, devient LA fenetre du
/// jeu sans evenement d'affichage (il n'y en aura pas). Sa pose l'affichera. Si le jeu l'affiche quand meme lui-meme, le chemin
/// normal l'a deja reconnue (`current`) et ceci ne fait rien : jamais de double traitement. Lectures locales.
fn promote_hidden(st: &mut State, parent: isize, now: Instant) -> Option<isize> {
    if !st.expect_hidden || st.current.is_some() {
        return None;
    }
    // candidates principales encore en vie, adoptees ou non : s'il y en a plusieurs, on ne choisit pas (le delai de garde
    // « aucune fenetre » finira par les rendre toutes, visibles)
    let mut mains = st.origin.iter().filter(|(w, o)| guard::main_sized(o.rect.2, o.rect.3) && unsafe { IsWindow(hw(**w)) } != 0).map(|(w, _)| *w);
    let (win, None) = (mains.next()?, mains.next()) else { return None };
    let settled = st.adopted_at.get(&win).is_some_and(|t| now.saturating_duration_since(*t) >= guard::HIDDEN_SETTLE);
    let ready = settled && unsafe { GetWindow(hw(win), GW_OWNER) as isize == parent && IsWindowVisible(hw(win)) == 0 };
    if !ready {
        return None;
    }
    mark_shown(st, win);
    Some(win)
}

/// Gardes du lancement en cours, a chaque battement du rideau (fil de l'interface) : relance le rattachement s'il a ete refuse,
/// declenche le repli quand `crate::guard::verdict` le dit. Lectures locales seulement.
fn guard_tick(s: &Shared) {
    let (verdict, retry, detail, promoted) = {
        let mut st = s.lock();
        if st.fell_back || st.pids.is_empty() {
            return;
        }
        let now = Instant::now();
        let promoted = promote_hidden(&mut st, s.parent, now);
        // Ne restent que les refus qui comptent : fenetre toujours la, et qui est (ou peut encore etre) LA fenetre du jeu. Des que
        // celle-ci est connue, les refus des autres fenetres sont oublies : ils ne declenchent plus jamais de repli.
        let current = st.current;
        st.refused.retain(|w, _| unsafe { IsWindow(hw(*w)) } != 0 && (current.is_none() || current == Some(*w)));
        // Seule une fenetre que le jeu a AFFICHEE peut faire conclure a un echec (c'est elle que le joueur attend, garee hors
        // ecran) ; une fenetre cachee refusee est seulement retentee. La plus avancee vers le repli decide.
        let refused = st
            .refused
            .iter()
            .filter(|(w, _)| unsafe { IsWindowVisible(hw(**w)) } != 0 || (st.expect_hidden && st.origin.get(*w).is_some_and(|o| guard::main_sized(o.rect.2, o.rect.3))))
            .map(|(_, r)| (now.saturating_duration_since(r.since), r.attempts))
            .max_by_key(|(since, attempts)| (*attempts >= guard::ADOPTION_MIN_ATTEMPTS, *since));
        let watched = Watched { active: true, since_watch: st.started.map(|t| now.saturating_duration_since(t)), window_shown: current.is_some(), refused };
        let verdict = guard::verdict(&st.guards, &watched);
        let detail = match verdict {
            Some(FallbackReason::AdoptionFailed) => {
                let (since, attempts) = watched.refused.unwrap_or_default();
                format!("fenetre du jeu toujours pas possedee par le launcher apres {attempts} tentatives en {} s", since.as_secs())
            }
            Some(_) => format!("aucune fenetre de jeu affichee {} s apres le lancement", watched.since_watch.unwrap_or_default().as_secs()),
            None => String::new(),
        };
        // une tentative a la fois (les corrections en file sont fusionnees) : la fenetre qui attend depuis le plus longtemps
        let retry = if verdict.is_none() {
            st.refused.iter_mut().filter(|(_, r)| now.saturating_duration_since(r.asked) >= ADOPTION_RETRY).min_by_key(|(_, r)| r.asked).map(|(w, r)| {
                r.asked = now;
                *w
            })
        } else {
            None
        };
        (verdict, retry, detail, promoted)
    };
    if let Some(win) = promoted {
        s.log(format!("lancement cache : fenetre du jeu ({}) tenue pour affichee (le jeu ne la montrera pas, l'hote le fera a la pose)", unsafe { util::class_name(hw(win)) }));
        game_window::announce(s, win);
    }
    if let Some(win) = retry {
        game_window::request_fix(s, win, s.parent, None);
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
    /// Cree la zone du jeu (fenetre fille de `parent`, un HWND de la fenetre du launcher) et le rideau, et demarre les fils de
    /// l'hote. A appeler DEPUIS LE FIL qui possede `parent` (sa boucle de messages anime le rideau). Un seul hote par processus.
    pub fn embed(parent: isize) -> Result<GameHost, String> {
        let s = Shared::install(parent)?;
        unsafe { embedded::create(&s)? };
        watch::start()?;
        Ok(GameHost { s })
    }

    /// Place la zone du jeu et le rideau (coordonnees physiques dans la zone cliente de la fenetre parente).
    pub fn relayout(&self, x: i32, y: i32, w: i32, h: i32) {
        self.s.lock().view = (x, y, w.max(1), h.max(1));
        unsafe { embedded::sync(&self.s) };
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
            st.adopting.clear();
            st.origin.clear();
            st.pids.clear();
            st.pids.insert(pid);
            st.adopted.clear();
            st.released.clear();
            st.current = None;
            st.shown_at = None;
            st.refused.clear();
            st.shrugged.clear();
            st.exposed.clear();
            st.shown_early.clear();
            st.adopted_at.clear();
            st.pose_pending = false;
            st.reframed_at = None;
            st.close_asked = false;
            st.announced = false;
            st.gen += 1;
            st.started = Some(Instant::now());
            todo
        };
        run(&self.s, &todo);
        self.s.log(format!("surveillance des fenetres du processus {pid}"));
        if !watch::set_hooks(pid) {
            fallback(&self.s, FallbackReason::HookFailed, "fil de surveillance des fenetres injoignable".into());
        }
        curtain_window::request_sync(&self.s); // relance le minuteur (filet de securite, gardes) s'il etait arrete
    }

    /// Une ligne de la sortie du jeu (alimente l'avancement du lancement ou de la fermeture).
    pub fn feed_line(&self, line: &str) {
        let mut st = self.s.lock();
        if let Some(t) = st.tracker.as_mut() {
            t.feed_line(line);
        }
        if let Some(c) = st.closer.as_mut() {
            c.feed_line(line);
        }
    }

    /// Le jeu a l'air pret (textures assemblees, fenetre affichee, plus de bruit depuis `quiet`) ou la fenetre est affichee depuis
    /// trop longtemps (`max`) : on ne laisse jamais le rideau bloque. Apres un repli il n'y a plus rien a attendre de la Scene :
    /// vrai aussi (le rideau est deja rouvert, `mark_ready` et `reveal` sont alors sans effet). Si aucune fenetre n'apparait, c'est
    /// le delai de garde (`crate::guard::NO_WINDOW_GUARD`) qui declenche le repli.
    pub fn is_ready(&self, quiet: Duration, max: Duration) -> bool {
        let st = self.s.lock();
        if st.fell_back || st.tracker.as_ref().is_some_and(|t| t.looks_ready(quiet)) {
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

    /// Ouvre le rideau (les panneaux s'ecartent et decouvrent le jeu).
    pub fn reveal(&self) {
        unsafe { SendMessageW(self.s.host(), WM_APP_REVEAL, 0, 0) };
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

    /// Demande au jeu de se fermer comme le ferait le joueur (croix de la fenetre) : il quitte le monde, enregistre, s'eteint.
    /// Vaut aussi pour un jeu rendu par un repli (sa fenetre est connue jusqu'au prochain `watch`).
    pub fn request_close(&self) {
        let windows = {
            let mut st = self.s.lock();
            st.close_asked = true;
            close_targets(&st)
        };
        self.s.log(format!("fermeture demandee au jeu ({} fenetre(s))", windows.len()));
        for w in windows {
            unsafe { PostMessageW(hw(w), WM_CLOSE, 0, 0) };
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
        unsafe { SendMessageW(self.s.host(), WM_APP_REVEAL, 1, 0) };
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

    /// Rend au jeu sa fenetre (cadre, barre des taches, position visible, plus de proprietaire), rouvre le rideau sur le launcher
    /// et arrete la surveillance pour CE lancement ; emet `HostEvent::Fallback { reason: Requested, .. }`. Sans jeu : sans effet.
    pub fn release(&self, detail: &str) {
        fallback(&self.s, FallbackReason::Requested, detail.to_string());
    }

    /// Lancement CACHE : le launcher a demarre le jeu avec la consigne « fenetre cachee » (`STARTUPINFO` : `STARTF_USESHOWWINDOW`
    /// et `SW_HIDE`), le jeu n'affichera donc pas sa fenetre lui-meme. A appeler apres `begin` et avant `watch` ; vaut pour CE
    /// lancement, `begin` le remet a faux. L'hote tient alors la fenetre adoptee pour celle du jeu sans attendre qu'il l'affiche,
    /// l'affiche lui-meme a la pose, et GARANTIT qu'elle soit montree si la Scene renonce (tout repli) : jamais de jeu invisible.
    /// Un jeu qui ignore la consigne et s'affiche lui-meme suit le chemin habituel.
    pub fn expect_hidden(&self, hidden: bool) {
        self.s.lock().expect_hidden = hidden;
    }

    /// Le lancement en cours est toujours dans la Scene (faux apres un repli, jusqu'au prochain `begin`).
    pub fn staged(&self) -> bool {
        !self.s.lock().fell_back
    }
}

fn count_jars(dir: &std::path::Path) -> u32 {
    std::fs::read_dir(dir).map_or(0, |d| d.filter_map(|e| e.ok()).filter(|e| e.path().extension().is_some_and(|x| x == "jar")).count() as u32)
}
