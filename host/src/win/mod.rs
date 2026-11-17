//! Implementation Win32 de la Scene (voir la doc du crate).
//!
//! Couche MINCE : elle traduit ce qui se passe en [`Event`], les passe a la machine [`crate::flow`] sous le verrou d'etat, puis
//! execute les [`Effect`] rendus une fois le verrou relache.
//!
//! * `mod.rs`            : `Backend` de la Scene commune (`crate::scene`, ou vit l'API publique), etat propre a Windows, execution des effets, repli.
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
use std::time::Instant;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::flow::{CAPTION_TO_LAUNCHER, Effect, Event};
use crate::guard::{self, Watched, ADOPTION_RETRY};
use crate::health::{FallbackReason, HostEvent, Observer};
use crate::scene::{Backend, Common};
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
pub struct State {
    /// Part commune aux deux systemes (voir `crate::scene`), accessible directement par `Deref`.
    c: Common<isize>,
    /// Fenetres en cours d'adoption (evite les doublons entre les evenements et le balayage lent).
    adopting: HashSet<isize>,
    /// Toute fenetre du jeu qu'on a touchee (garee, adoptee), telle qu'elle etait avant : de quoi la rendre. Une fenetre absente
    /// d'ici n'est plus (ou pas) a nous : le fil de la fenetre du jeu ignore les commandes qui la visent.
    origin: HashMap<isize, Origin>,
    /// (largeur, hauteur, bande) de la decoupe actuellement appliquee a la fenetre du jeu ; (0, 0, 0) : aucune. Bande 0 : region
    /// pleine (le jeu en porte toujours une une fois pose, voir `game_window::apply_region`).
    region: (i32, i32, i32),
    /// Fenetres dont le rattachement est refuse, tant qu'elles peuvent etre LA fenetre du jeu (`current` inconnue, ou elle-meme).
    refused: HashMap<isize, Refused>,
    /// Fenetres secondaires non rattachees deja signalees au journal (une ligne par fenetre).
    shrugged: HashSet<isize>,
    /// Fenetres que le jeu a AFFICHEES avant la fin de leur adoption (voir `embedded::adopt`).
    shown_early: HashSet<isize>,
    /// Fenetres constatees visibles a l'ecran sans etre rattachees (voir `embedded::cover_exposed`).
    exposed: HashMap<isize, Exposed>,
    /// Dernier changement de `fullscreen` (voir `embedded::note_fullscreen`).
    fullscreen_at: Option<Instant>,
    /// Le focus est a rendre au jeu apres la bascule plein ecran du launcher (voir `embedded::refocus`).
    refocus: bool,
}

impl std::ops::Deref for State {
    type Target = Common<isize>;
    fn deref(&self) -> &Common<isize> {
        &self.c
    }
}

impl std::ops::DerefMut for State {
    fn deref_mut(&mut self) -> &mut Common<isize> {
        &mut self.c
    }
}

pub struct Shared {
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
pub struct Todo {
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
            // `st.region` reste la decoupe REELLEMENT posee : c'est le fil du jeu qui la referme (en glissant) et la met a jour
            Effect::ClearRegion => adopted = st.adopted.clone(),
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
                st.fullscreen_at = None;
                st.refocus = false;
                st.fullscreen = false; // le launcher le quitte de lui-meme quand le jeu n'est plus rattache
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

/// La Scene Windows : l'API commune (`crate::scene`) sur ce systeme.
pub type GameHost = crate::scene::GameHost<Shared>;

impl Backend for Shared {
    type Win = isize;
    type State = State;
    type Todo = Todo;

    fn install(parent: isize) -> Result<Arc<Self>, String> {
        let s = Shared::install(parent)?;
        unsafe { embedded::create(&s)? };
        watch::start()?;
        Ok(s)
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
        unsafe { embedded::sync(self) };
    }

    fn reset_for_watch(st: &mut State) {
        st.adopting.clear();
        st.origin.clear();
        st.refused.clear();
        st.shrugged.clear();
        st.exposed.clear();
        st.shown_early.clear();
    }

    fn start_watch(&self, pid: u32) {
        if !watch::set_hooks(pid) {
            fallback(self, FallbackReason::HookFailed, "fil de surveillance des fenetres injoignable".into());
        }
        curtain_window::request_sync(self); // relance le minuteur (filet de securite, gardes) s'il etait arrete
    }

    fn reveal(&self, cancel: bool) {
        // l'ouverture se fait sur le fil de l'interface (qui possede le rideau)
        unsafe { SendMessageW(self.host(), WM_APP_REVEAL, usize::from(cancel), 0) };
    }

    fn ask_close(&self, w: isize) {
        unsafe { PostMessageW(hw(w), WM_CLOSE, 0, 0) };
    }

    fn fallback(&self, reason: FallbackReason, detail: String) -> bool {
        fallback(self, reason, detail)
    }
}
