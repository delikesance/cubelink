//! Partie COMMUNE de la Scene : l'API publique [`GameHost`] et l'etat du lancement que Windows et X11 partagent.
//!
//! Chaque systeme fournit un [`Backend`] (son `Shared` : fenetres, fils, verrou d'etat) ; tout ce qui ne depend pas du systeme
//! (deroulement du rideau, suivi des etapes, fermeture, panneau, contrat avec le launcher) est ecrit ICI une seule fois. Avant cette
//! factorisation, les deux `GameHost` etaient des copies qui avaient deja diverge (filtre du bruit repete absent sous Windows).
//!
//! L'etat de chaque systeme contient un [`Common`] et y donne acces par `Deref` : le code propre au systeme continue d'ecrire
//! `st.current`, `st.pids`... sans savoir quels champs sont communs.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::ops::DerefMut;
use std::path::Path;
use std::sync::{Arc, MutexGuard};
use std::time::{Duration, Instant};

use crate::flow::{Bar, CAPTION_SWITCH, CAPTION_TO_GAME, CAPTION_TO_LAUNCHER, Content, Event, Machine, View};
use crate::guard::Guards;
use crate::health::{FallbackReason, Observer};
use crate::progress::{count_jars, feed_tracker, line_body, Closer, Profile, Tracker};

/// Etat du lancement commun aux deux systemes (une fenetre est un `W` : HWND sous Windows, XID sous X11).
pub struct Common<W> {
    /// Deroulement de la vue (rideau, pose de la fenetre du jeu, panneau).
    pub(crate) machine: Machine,
    pub(crate) pids: HashSet<u32>,
    pub(crate) adopted: Vec<W>,
    /// LA fenetre du jeu (adoptee puis affichee par le jeu).
    pub(crate) current: Option<W>,
    pub(crate) shown_at: Option<Instant>,
    pub(crate) title: String,
    pub(crate) caption: String,
    pub(crate) tracker: Option<Tracker>,
    pub(crate) closer: Option<Closer>,
    /// Apparition du rideau (horloge de ses animations).
    pub(crate) curtain_t0: Option<Instant>,
    /// Zone de jeu en coordonnees client de la fenetre du launcher (x, y, largeur, hauteur).
    pub(crate) view: (i32, i32, i32, i32),
    /// Debut de la surveillance du jeu courant (`watch`).
    pub(crate) started: Option<Instant>,
    /// La Scene a renonce pour ce lancement (repli) ; remis a faux par `begin`.
    pub(crate) fell_back: bool,
    /// `HostEvent::Adopted` deja emis pour le jeu surveille.
    pub(crate) announced: bool,
    /// Lancement CACHE (`GameHost::expect_hidden`) : le jeu n'affichera pas sa fenetre lui-meme ; c'est l'hote qui la montre, a la
    /// pose ou, quoi qu'il arrive, en la lui rendant. Vaut pour le lancement en cours ; remis a faux par `begin`.
    pub(crate) expect_hidden: bool,
    /// La premiere pose de LA fenetre du jeu attend (`guard::pose_waits`) : elle reste garee, rien ne la cale.
    pub(crate) pose_pending: bool,
    /// Dernier cadre remis par le jeu sur une fenetre pas encore posee.
    pub(crate) reframed_at: Option<Instant>,
    /// Instant ou chaque fenetre adoptee l'a ete.
    pub(crate) adopted_at: HashMap<W, Instant>,
    /// Numero du lancement surveille (change a chaque `watch` et a chaque fin de jeu) : ce qui a ete mis en route pour un
    /// lancement (guet d'une fenetre cachee) s'arrete quand il n'est plus le courant.
    pub(crate) gen: u64,
    /// `request_close` a ete demande au jeu surveille : sa fenetre se defait, on ne la corrige plus.
    pub(crate) close_asked: bool,
    /// Fenetres rendues au jeu par le dernier repli : `request_close` peut encore leur demander de se fermer.
    pub(crate) released: Vec<W>,
    /// LA fenetre du jeu occupe l'ecran (derniere valeur annoncee par `HostEvent::Fullscreen`).
    pub(crate) fullscreen: bool,
    pub(crate) guards: Guards,
    /// Corps (sans horodatage) de la derniere ligne du jeu : un bruit repete ne compte pas pour la quietude (`is_noise`).
    pub(crate) last_body: Option<String>,
    /// Lignes ecartees comme bruit depuis le debut de la surveillance (journalisees a l'ouverture forcee).
    pub(crate) noise_lines: u64,
    /// L'ouverture forcee par le maximum de `is_ready` a deja ete journalisee pour ce lancement.
    pub(crate) forced_logged: bool,
}

impl<W> Default for Common<W> {
    fn default() -> Self {
        Self {
            machine: Machine::default(),
            pids: HashSet::new(),
            adopted: Vec::new(),
            current: None,
            shown_at: None,
            title: String::new(),
            caption: String::new(),
            tracker: None,
            closer: None,
            curtain_t0: None,
            view: (0, 0, 0, 0),
            started: None,
            fell_back: false,
            announced: false,
            expect_hidden: false,
            pose_pending: false,
            reframed_at: None,
            adopted_at: HashMap::new(),
            gen: 0,
            close_asked: false,
            released: Vec::new(),
            fullscreen: false,
            guards: Guards::default(),
            last_body: None,
            noise_lines: 0,
            forced_logged: false,
        }
    }
}

impl<W: Copy + Eq + Hash> Common<W> {
    /// Remise a zero de la part commune pour un nouveau jeu surveille (`watch`).
    fn reset_for_watch(&mut self, pid: u32) {
        self.pids.clear();
        self.pids.insert(pid);
        self.adopted.clear();
        self.released.clear();
        self.current = None;
        self.shown_at = None;
        self.adopted_at.clear();
        self.pose_pending = false;
        self.reframed_at = None;
        self.close_asked = false;
        self.announced = false;
        self.last_body = None;
        self.noise_lines = 0;
        self.forced_logged = false;
        self.gen += 1;
        self.started = Some(Instant::now());
    }

    /// Fenetres a qui demander la fermeture : LA fenetre du jeu seulement (les fenetres d'aide adoptees n'ont pas a la recevoir) ;
    /// a defaut (pas encore affichee), toutes les fenetres adoptees ; apres un repli, celles qu'on a rendues au jeu.
    pub(crate) fn close_targets(&self) -> Vec<W> {
        match self.current {
            Some(c) => vec![c],
            None if !self.adopted.is_empty() => self.adopted.clone(),
            None => self.released.clone(),
        }
    }
}

/// Ce qu'un systeme fournit a la Scene commune. Les appels venant de [`GameHost`] partent du fil de l'appelant ; le verrou d'etat
/// n'est jamais tenu pendant `run`, `reveal`, `ask_close`, `fallback` ni `log`.
pub trait Backend: Send + Sync + 'static {
    /// Identifiant d'une fenetre (HWND, XID).
    type Win: Copy + Eq + Hash;
    /// Etat du systeme, qui contient la part commune.
    type State: DerefMut<Target = Common<Self::Win>>;
    /// Effets d'une transition de la machine, a executer verrou relache.
    type Todo;

    /// Cree la zone du jeu dans la fenetre `parent` du launcher et demarre les fils de l'hote.
    fn install(parent: isize) -> Result<Arc<Self>, String>;
    fn lock(&self) -> MutexGuard<'_, Self::State>;
    /// Journal de l'hote (`HostEvent::Log`).
    fn log(&self, m: String);
    fn set_observer(&self, observer: Observer);
    /// Passe `ev` a la machine et applique sous le verrou la part des effets qui ne touche que l'etat.
    fn dispatch(st: &mut Self::State, ev: Event) -> Self::Todo;
    /// Le rideau a ete (re)ferme par cette transition.
    fn shows_curtain(todo: &Self::Todo) -> bool;
    /// La transition a produit au moins un effet.
    fn has_effects(todo: &Self::Todo) -> bool;
    /// Execute les effets qui touchent les fenetres.
    fn run(&self, todo: &Self::Todo);
    /// Recale la zone du jeu et le rideau sur `view`.
    fn sync(&self);
    /// Echelle d'affichage du launcher (1.0 = 100 %), pour le rendu du rideau. Par defaut sans effet : Windows relit le DPI de sa
    /// fenetre a chaque rendu.
    fn set_scale(&self, _scale: f32) {}
    /// Remise a zero de la part propre au systeme pour un nouveau jeu surveille (sous le verrou).
    fn reset_for_watch(st: &mut Self::State);
    /// Commence a guetter les fenetres du jeu `pid` (verrou relache), et relance le minuteur du rideau.
    fn start_watch(&self, pid: u32);
    /// Ouverture du rideau (`cancel` : la fermeture du jeu est abandonnee, il reste dans la Scene).
    fn reveal(&self, cancel: bool);
    /// Demande a la fenetre `w` de se fermer comme le ferait le joueur (croix de la fenetre).
    fn ask_close(&self, w: Self::Win);
    /// REPLI : la Scene renonce pour le lancement en cours (voir chaque systeme). Faux s'il n'y avait rien a rendre.
    fn fallback(&self, reason: FallbackReason, detail: String) -> bool;
    /// Fin de l'hote (`Drop` du [`GameHost`]) : rendre le jeu et liberer ce qui doit l'etre.
    fn shutdown(&self) {}
}

/// La Scene : zone de jeu dans la fenetre du launcher, rideau, et jeu courant. Meme API sous Windows et sous X11.
pub struct GameHost<B: Backend> {
    s: Arc<B>,
}

impl<B: Backend> GameHost<B> {
    /// Cree la zone du jeu dans `parent` (HWND de la fenetre du launcher sous Windows, XID sous X11) et le rideau, et demarre les
    /// fils de l'hote. Windows : a appeler DEPUIS LE FIL qui possede `parent` (sa boucle de messages anime le rideau). X11 : `Err`
    /// si aucun serveur X11 n'est joignable (`DISPLAY`), le jeu garde alors sa propre fenetre. Un seul hote par processus.
    pub fn embed(parent: isize) -> Result<Self, String> {
        Ok(Self { s: B::install(parent)? })
    }

    /// Place la zone du jeu et le rideau (coordonnees physiques dans la zone cliente de la fenetre parente).
    pub fn relayout(&self, x: i32, y: i32, w: i32, h: i32) {
        self.s.lock().view = (x, y, w.max(1), h.max(1));
        self.s.sync();
    }

    /// Un lancement commence : les panneaux se ferment sur le launcher (ou sur le jeu en cours) puis le chargement s'affiche.
    /// Le lancement est (de nouveau) dans la Scene : `staged` redevient vrai.
    pub fn begin(&self, title: &str, caption: &str, mods_dir: Option<&Path>, profile_path: Option<&Path>) {
        // lectures disque hors du verrou
        let n = mods_dir.map_or(0, count_jars);
        let prev = profile_path.and_then(Profile::load);
        let todo = {
            let mut st = self.s.lock();
            let todo = B::dispatch(&mut st, Event::BeginLaunch);
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
        self.s.run(&todo);
    }

    /// Surveille les fenetres du processus `pid` (le nouveau jeu) : elles seront adoptees des leur creation.
    pub fn watch(&self, pid: u32) {
        let todo = {
            let mut st = self.s.lock();
            let todo = B::dispatch(&mut st, Event::Watch);
            B::reset_for_watch(&mut st);
            st.reset_for_watch(pid);
            todo
        };
        self.s.run(&todo);
        self.s.log(format!("surveillance des fenetres du processus {pid}"));
        self.s.start_watch(pid);
    }

    /// Une ligne de la sortie du jeu (alimente l'avancement du lancement ou de la fermeture). Le bruit repete (voir `is_noise`) n'est
    /// pas compte comme de l'activite : un jeu qui se plaint en boucle est « silencieux » pour la quietude du `is_ready`.
    pub fn feed_line(&self, line: &str) {
        let mut guard = self.s.lock();
        let st: &mut Common<B::Win> = &mut guard;
        if let Some(t) = st.tracker.as_mut() {
            if feed_tracker(t, &mut st.last_body, line) {
                st.noise_lines += 1;
            }
        } else {
            st.last_body = Some(line_body(line).to_string());
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
    pub fn mark_ready(&self, profile_path: Option<&Path>) {
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
        self.s.reveal(false);
    }

    /// Debut d'un changement de modpack : les panneaux se ferment sur le jeu en cours et la barre suit sa fermeture (0-50 %).
    pub fn begin_close(&self, title: &str) {
        let todo = {
            let mut st = self.s.lock();
            let todo = B::dispatch(&mut st, Event::BeginClose);
            st.closer = Some(Closer::new());
            st.tracker = None;
            st.title = title.to_string();
            st.caption = CAPTION_SWITCH.to_string();
            todo
        };
        self.s.log("changement de modpack : rideau ferme sur le jeu en cours".into());
        self.s.run(&todo);
    }

    /// Demande au jeu de se fermer comme le ferait le joueur (croix de la fenetre : `WM_CLOSE`, `WM_DELETE_WINDOW`) : il quitte le
    /// monde, enregistre, s'eteint. Vaut aussi pour un jeu rendu par un repli (sa fenetre est connue jusqu'au prochain `watch`).
    pub fn request_close(&self) {
        let windows = {
            let mut st = self.s.lock();
            st.close_asked = true;
            st.close_targets()
        };
        self.s.log(format!("fermeture demandee au jeu ({} fenetre(s))", windows.len()));
        for w in windows {
            self.s.ask_close(w);
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
                let todo = B::dispatch(&mut st, Event::GameExited);
                if B::shows_curtain(&todo) {
                    st.title.clear();
                    st.caption = CAPTION_TO_LAUNCHER.to_string();
                }
                Some(todo)
            }
        };
        self.s.log(if todo.is_some() { "jeu termine" } else { "jeu termine (il avait ete rendu : rien a faire)" }.into());
        if let Some(todo) = todo {
            self.s.run(&todo);
        }
    }

    /// Le jeu ne s'est pas ferme dans le delai : on le laisse tranquille (jamais force) et on rouvre le rideau sur lui.
    pub fn cancel_close(&self) {
        self.s.reveal(true);
    }

    /// Bascule entre le jeu (`true`) et le launcher (`false`) avec l'animation de rideau, sans arreter le jeu.
    pub fn set_view(&self, game: bool) {
        let todo = {
            let mut st = self.s.lock();
            let todo = B::dispatch(&mut st, Event::SetView(if game { View::Game } else { View::Launcher }));
            if B::shows_curtain(&todo) {
                st.title.clear();
                st.caption = if game { CAPTION_TO_GAME } else { CAPTION_TO_LAUNCHER }.to_string();
            }
            todo
        };
        self.s.run(&todo);
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
        let todo = B::dispatch(&mut self.s.lock(), ev);
        self.s.run(&todo);
        B::has_effects(&todo)
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
impl<B: Backend> GameHost<B> {
    /// Installe l'observateur des evenements de sante (remplace le precedent).
    pub fn set_observer(&self, observer: Observer) {
        self.s.set_observer(observer);
    }

    /// Echelle d'affichage du launcher (facteur d'echelle de sa fenetre), a transmettre a chaque recalage : le rideau (logo, titre,
    /// barre) est dessine a cette echelle et redessine tout de suite si elle change. Valeur non finie ou < 1 : ramenee a 1.
    pub fn set_scale(&self, scale: f32) {
        self.s.set_scale(scale);
    }

    /// Mouvement reduit (preference systeme) : le rideau ne s'anime plus (panneaux poses / retires sans glissement, logo fige).
    pub fn set_calm(&self, calm: bool) {
        self.s.lock().machine.set_calm(calm);
    }

    /// Rend au jeu sa fenetre (cadre, position visible, plus de proprietaire ni de scene), rouvre le rideau sur le launcher et
    /// arrete la surveillance pour CE lancement ; emet `HostEvent::Fallback { reason: Requested, .. }`. Sans jeu : sans effet.
    pub fn release(&self, detail: &str) {
        self.s.fallback(FallbackReason::Requested, detail.to_string());
    }

    /// Lancement CACHE : le jeu n'affichera pas sa fenetre lui-meme (Windows : `STARTUPINFO` avec `SW_HIDE` ; X11 : fenetre creee
    /// invisible). A appeler apres `begin` et avant `watch` ; vaut pour CE lancement, `begin` le remet a faux. L'hote tient alors la
    /// fenetre adoptee pour celle du jeu sans attendre qu'il l'affiche, l'affiche lui-meme a la pose, et GARANTIT qu'elle soit
    /// montree si la Scene renonce (tout repli) : jamais de jeu invisible. Un jeu qui s'affiche lui-meme suit le chemin habituel.
    pub fn expect_hidden(&self, hidden: bool) {
        self.s.lock().expect_hidden = hidden;
    }

    /// Le lancement en cours est toujours dans la Scene (faux apres un repli, jusqu'au prochain `begin`).
    pub fn staged(&self) -> bool {
        !self.s.lock().fell_back
    }
}

impl<B: Backend> Drop for GameHost<B> {
    fn drop(&mut self) {
        self.s.shutdown();
    }
}
