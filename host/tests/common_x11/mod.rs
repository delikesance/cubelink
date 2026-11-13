//! Outils communs aux tests d'integration X11 de la Scene (`tests/x11_*.rs`).
//!
//! Chaque fichier de `tests/` est un processus a part avec UN test : il cree une fenetre « launcher » sur sa propre connexion X11,
//! y installe l'hote, lance un faux jeu (`src/bin/fake-game.rs`, variante X11) et verifie le resultat par des LECTURES X11
//! (parent, visibilite, rectangle ecran, focus). Aucune entree souris / clavier n'est envoyee ni simulee.
//!
//! Ces tests visent un serveur X11 NU (`xvfb-run`), sans gestionnaire de fenetres : sans lui, aucune decoration, aucun placement
//! ni reordonnancement ne vient fausser les lectures. Ils se mettent donc en retrait (le test passe, avec un message) quand
//! `DISPLAY` est absent OU quand un gestionnaire de fenetres tourne (`_NET_SUPPORTING_WM_CHECK`) : jamais de fenetre de test sur le
//! bureau de l'utilisateur, sauf `CUBELINK_X11_TESTS=1`. Ce que ces tests NE peuvent PAS verifier : le cadre du gestionnaire de
//! fenetres, la barre des taches, `_NET_ACTIVE_WINDOW`, la transparence du rideau (pas de compositeur), le comportement de Mutter ou
//! de XWayland, ni celui d'un vrai GLFW / SDL.

#![allow(dead_code)] // chaque scenario n'utilise qu'une partie des outils

use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use cubelink_host::{FallbackReason, HostEvent, Observer};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

static X: OnceLock<(RustConnection, u32)> = OnceLock::new();

/// Connexion X11 du test et fenetre racine.
pub fn x() -> &'static (RustConnection, u32) {
    X.get_or_init(|| {
        // Xvfb se reinitialise quand le dernier client part (fin d'un binaire de test) : la connexion qui suit peut etre
        // refusee un instant, on reessaie.
        let mut tries = 0;
        let (c, n) = loop {
            match RustConnection::connect(None) {
                Ok(ok) => break ok,
                Err(e) if tries < 40 => {
                    tries += 1;
                    let _ = e;
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(e) => panic!("connexion X11: {e:?}"),
            }
        };
        let root = c.setup().roots[n].root;
        (c, root)
    })
}

fn atom(name: &str) -> u32 {
    x().0.intern_atom(false, name.as_bytes()).unwrap().reply().unwrap().atom
}

/// Le test peut tourner ici : un serveur X11 nu est joignable. Sinon, affiche pourquoi et rend faux (le test doit alors rendre la main).
pub fn available() -> bool {
    if std::env::var_os("DISPLAY").is_none() {
        eprintln!("test X11 ignore : DISPLAY absent (lancer sous `xvfb-run -a`)");
        return false;
    }
    let Ok((c, n)) = RustConnection::connect(None) else {
        eprintln!("test X11 ignore : serveur X11 injoignable");
        return false;
    };
    if std::env::var_os("CUBELINK_X11_TESTS").is_some() {
        return true;
    }
    let root = c.setup().roots[n].root;
    let wm = c.intern_atom(false, b"_NET_SUPPORTING_WM_CHECK").unwrap().reply().unwrap().atom;
    let has_wm = c.get_property(false, root, wm, AtomEnum::WINDOW, 0, 1).unwrap().reply().is_ok_and(|r| r.value_len > 0);
    if has_wm {
        eprintln!("test X11 ignore : un gestionnaire de fenetres tourne (bureau reel ?) ; `xvfb-run -a` ou CUBELINK_X11_TESTS=1");
    }
    !has_wm
}

// ------------------------------------------------------------------------------------------------ fenetre « launcher »

/// Fenetre de premier niveau du test, dans le role de celle du launcher. Detruite a la fin du test, meme en cas d'echec.
pub struct Parent {
    pub xid: u32,
}

impl Parent {
    pub fn new() -> Parent {
        let (c, root) = x();
        let xid = c.generate_id().unwrap();
        let aux = CreateWindowAux::new().background_pixel(c.setup().roots[0].white_pixel);
        c.create_window(0, xid, *root, 120, 80, 960, 640, 0, WindowClass::INPUT_OUTPUT, 0, &aux).unwrap();
        let cls = b"cubelink-test-parent\0CubelinkTestParent\0";
        c.change_property8(PropMode::REPLACE, xid, AtomEnum::WM_CLASS, AtomEnum::STRING, cls).unwrap();
        c.map_window(xid).unwrap();
        c.flush().unwrap();
        wait_until("fenetre parente affichee", Duration::from_secs(5), || visible(xid));
        Parent { xid }
    }

    pub fn id(&self) -> isize {
        self.xid as isize
    }

    pub fn client_size(&self) -> (i32, i32) {
        let g = x().0.get_geometry(self.xid).unwrap().reply().unwrap();
        (g.width as i32, g.height as i32)
    }

    /// Rectangle ecran d'une zone `(x, y, largeur, hauteur)` donnee en coordonnees client.
    pub fn to_screen(&self, zone: (i32, i32, i32, i32)) -> (i32, i32, i32, i32) {
        let t = x().0.translate_coordinates(self.xid, x().1, zone.0 as i16, zone.1 as i16).unwrap().reply().unwrap();
        (t.dst_x as i32, t.dst_y as i32, zone.2, zone.3)
    }
}

impl Drop for Parent {
    fn drop(&mut self) {
        let _ = x().0.destroy_window(self.xid);
        let _ = x().0.flush();
    }
}

pub fn pump_for(d: Duration) {
    std::thread::sleep(d);
}

/// Attend que `cond` soit vraie ; echec du test (avec `what`) si elle ne l'est pas dans `timeout`.
#[track_caller]
pub fn wait_until(what: &str, timeout: Duration, mut cond: impl FnMut() -> bool) {
    let end = Instant::now() + timeout;
    loop {
        if cond() {
            return;
        }
        assert!(Instant::now() < end, "delai depasse ({} s) : {what}", timeout.as_secs());
        std::thread::sleep(Duration::from_millis(2));
    }
}

// ------------------------------------------------------------------------------------------------ lectures X11

pub fn exists(w: u32) -> bool {
    x().0.get_window_attributes(w).ok().and_then(|c| c.reply().ok()).is_some()
}

/// Affichee ET tous ses ancetres aussi : ce que le joueur voit.
pub fn visible(w: u32) -> bool {
    x().0.get_window_attributes(w).ok().and_then(|c| c.reply().ok()).is_some_and(|a| a.map_state == MapState::VIEWABLE)
}

/// Son propre drapeau « affichee » (un ancetre cache la rend invisible sans changer cela).
pub fn mapped(w: u32) -> bool {
    x().0.get_window_attributes(w).ok().and_then(|c| c.reply().ok()).is_some_and(|a| a.map_state != MapState::UNMAPPED)
}

pub fn parent_of(w: u32) -> u32 {
    x().0.query_tree(w).ok().and_then(|c| c.reply().ok()).map_or(0, |t| t.parent)
}

/// Rectangle ecran (x, y, largeur, hauteur).
pub fn rect(w: u32) -> (i32, i32, i32, i32) {
    let (c, root) = x();
    let g = c.get_geometry(w).unwrap().reply().unwrap();
    let t = c.translate_coordinates(w, *root, 0, 0).unwrap().reply().unwrap();
    (t.dst_x as i32, t.dst_y as i32, g.width as i32, g.height as i32)
}

/// Taille propre (largeur, hauteur), sans position.
pub fn size(w: u32) -> (i32, i32) {
    let g = x().0.get_geometry(w).unwrap().reply().unwrap();
    (g.width as i32, g.height as i32)
}

/// La fenetre touche l'ecran (une fenetre garee en -32000 n'y touche pas).
pub fn on_screen(w: u32) -> bool {
    let (x0, y0, w0, h0) = rect(w);
    let g = x().0.get_geometry(x().1).unwrap().reply().unwrap();
    x0 < g.width as i32 && y0 < g.height as i32 && x0 + w0 > 0 && y0 + h0 > 0
}

fn prop_len(w: u32, name: &str) -> usize {
    x().0.get_property(false, w, atom(name), AtomEnum::ANY, 0, 8).unwrap().reply().map_or(0, |r| r.value_len as usize)
}

pub fn class_of(w: u32) -> String {
    let r = x().0.get_property(false, w, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 64).ok().and_then(|c| c.reply().ok());
    r.map(|r| String::from_utf8_lossy(&r.value).split('\0').nth(1).unwrap_or("").to_string()).unwrap_or_default()
}

fn pid_prop(w: u32) -> Option<u32> {
    let r = x().0.get_property(false, w, atom("_NET_WM_PID"), AtomEnum::CARDINAL, 0, 1).ok()?.reply().ok()?;
    let v = r.value32()?.next();
    v
}

/// Fenetres (a tout niveau de l'arbre : une fenetre adoptee n'est plus une fille de la racine) de classe `class` du processus `pid`.
pub fn windows_of(pid: u32, class: &str) -> Vec<u32> {
    fn walk(w: u32, out: &mut Vec<u32>) {
        if let Some(t) = x().0.query_tree(w).ok().and_then(|c| c.reply().ok()) {
            for c in t.children {
                out.push(c);
                walk(c, out);
            }
        }
    }
    let mut all = Vec::new();
    walk(x().1, &mut all);
    all.into_iter().filter(|w| pid_prop(*w) == Some(pid) && class_of(*w) == class).collect()
}

/// La scene (sous-fenetre du launcher) et le rideau de l'hote de CE processus.
pub fn stage() -> u32 {
    *windows_of(std::process::id(), "CubelinkStage").first().expect("scene introuvable")
}

pub fn curtain() -> u32 {
    *windows_of(std::process::id(), "CubelinkCurtain").first().expect("fenetre du rideau introuvable")
}

/// Fenetre qui a le focus clavier (0 : aucune, 1 : `PointerRoot`).
pub fn focus() -> u32 {
    x().0.get_input_focus().unwrap().reply().unwrap().focus
}

/// La fenetre est telle qu'un joueur la retrouve apres un repli : fenetre du bureau (fille de la racine), affichee, a l'ecran, sans
/// marque de repli transitoire. (Le cadre et la barre des taches sont l'affaire du gestionnaire de fenetres : sans lui, non verifiables.)
#[track_caller]
pub fn assert_given_back(w: u32) {
    assert!(exists(w), "la fenetre rendue n'existe plus");
    assert_eq!(parent_of(w), x().1, "fenetre rendue pas sur le bureau");
    assert!(visible(w), "fenetre rendue invisible");
    assert!(on_screen(w), "fenetre rendue hors ecran : {:?}", rect(w));
    assert_eq!(prop_len(w, "WM_TRANSIENT_FOR"), 0, "fenetre rendue encore transitoire");
    assert_eq!(prop_len(w, "_MOTIF_WM_HINTS"), 0, "fenetre rendue encore sans decoration");
}

// ------------------------------------------------------------------------------------------------ faux jeu

/// Un faux jeu en cours d'execution. TUE a la fin du test, meme en cas d'echec.
pub struct FakeGame {
    child: Child,
}

impl FakeGame {
    /// Arguments : voir `src/bin/fake-game.rs`.
    pub fn spawn(args: &[&str]) -> FakeGame {
        let child = Command::new(env!("CARGO_BIN_EXE_fake-game")).args(args).spawn().expect("lancement du faux jeu");
        FakeGame { child }
    }

    /// Tue le faux jeu (fin brutale, sa fenetre disparait).
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Le processus s'est arrete.
    pub fn exited(&mut self) -> bool {
        self.child.try_wait().ok().flatten().is_some()
    }

    /// La fenetre de classe `class` du faux jeu, si elle existe.
    pub fn window(&self, class: &str) -> Option<u32> {
        windows_of(self.pid(), class).into_iter().next()
    }
}

impl Drop for FakeGame {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn ask_game(win: u32, name: &str) {
    let (c, _) = x();
    let acks = atom("_FAKE_GAME_ACKS");
    let count = |w: u32| c.get_property(false, w, acks, AtomEnum::CARDINAL, 0, 1).ok().and_then(|k| k.reply().ok()).and_then(|r| r.value32().and_then(|mut v| v.next())).unwrap_or(0);
    let before = count(win);
    let ev = ClientMessageEvent::new(32, win, atom(name), [0u32; 5]);
    c.send_event(false, win, EventMask::NO_EVENT, ev).unwrap();
    c.flush().unwrap();
    wait_until("le faux jeu a obei", Duration::from_secs(5), || count(win) > before);
}

/// Demande au faux jeu d'occuper de lui-meme tout l'ecran avec sa fenetre (message client, pas une entree) ; attend qu'il l'ait fait.
#[track_caller]
pub fn game_grabs_screen(win: u32) {
    ask_game(win, "_FAKE_GAME_GRAB");
}

/// Le faux jeu quitte son plein ecran de lui-meme.
#[track_caller]
pub fn game_leaves_screen(win: u32) {
    ask_game(win, "_FAKE_GAME_UNGRAB");
}

/// Le faux jeu demande le plein ecran comme GLFW : message client `_NET_WM_STATE` a la racine. `action` : 0 retire, 1 ajoute, 2 bascule.
#[track_caller]
pub fn game_sends_net_wm_state(win: u32, action: u32) {
    ask_game(win, ["_FAKE_GAME_EWMH_REMOVE", "_FAKE_GAME_EWMH_ADD", "_FAKE_GAME_EWMH_TOGGLE"][action as usize]);
}

/// Atomes de `_NET_WM_STATE` d'une fenetre (vide : propriete absente).
pub fn net_wm_state(win: u32) -> Vec<u32> {
    let r = x().0.get_property(false, win, atom("_NET_WM_STATE"), AtomEnum::ATOM, 0, 64).unwrap().reply().unwrap();
    r.value32().map(|v| v.collect()).unwrap_or_default()
}

pub fn fullscreen_atom() -> u32 {
    atom("_NET_WM_STATE_FULLSCREEN")
}

/// Faux gestionnaire de fenetres : SEUL a choisir `SubstructureRedirect` sur la racine (le serveur nu n'en a pas), il note les messages
/// clients `_NET_WM_STATE` (fenetre, action, atome) que le launcher lui adresse, sans rien en faire (sans vrai gestionnaire la
/// fenetre ne change pas de taille). Releve ses selections a l'arret : plus rien ne passe alors par lui (`MapRequest`...).
pub struct FakeWm {
    seen: Arc<Mutex<Vec<(u32, u32, u32)>>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl FakeWm {
    pub fn start() -> FakeWm {
        let (seen, stop) = (Arc::new(Mutex::new(Vec::new())), Arc::new(AtomicBool::new(false)));
        let (sink, flag) = (seen.clone(), stop.clone());
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let (c, n) = RustConnection::connect(None).expect("connexion du faux gestionnaire");
            let root = c.setup().roots[n].root;
            let net_state = c.intern_atom(false, b"_NET_WM_STATE").unwrap().reply().unwrap().atom;
            c.change_window_attributes(root, &ChangeWindowAttributesAux::new().event_mask(EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY)).unwrap().check().expect("SubstructureRedirect deja pris : un gestionnaire tourne");
            ready_tx.send(()).unwrap();
            while !flag.load(Ordering::Relaxed) {
                match c.poll_for_event() {
                    Ok(Some(x11rb::protocol::Event::ClientMessage(m))) if m.type_ == net_state && m.format == 32 => {
                        let d = m.data.as_data32();
                        sink.lock().unwrap().push((m.window, d[0], d[1]));
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => std::thread::sleep(Duration::from_millis(2)),
                    Err(_) => break,
                }
            }
            let _ = c.change_window_attributes(root, &ChangeWindowAttributesAux::new().event_mask(EventMask::NO_EVENT));
            let _ = c.flush();
        });
        ready_rx.recv().unwrap();
        FakeWm { seen, stop, thread: Some(thread) }
    }

    /// `(fenetre, action, atome)` des messages `_NET_WM_STATE` recus pour `win`, dans l'ordre (ceux du faux jeu, adresses a la meme
    /// racine, sont aussi vus : on filtre sur la fenetre).
    pub fn requests_for(&self, win: u32) -> Vec<(u32, u32, u32)> {
        self.seen.lock().unwrap().iter().copied().filter(|r| r.0 == win).collect()
    }

    /// Rend la racine (attend que la selection soit relevee).
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for FakeWm {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

// ------------------------------------------------------------------------------------------------ evenements de l'hote

/// Evenements recus de l'hote (l'observateur est appele depuis ses fils).
#[derive(Clone, Default)]
pub struct Events(Arc<Mutex<Vec<HostEvent>>>);

impl Events {
    pub fn observer(&self) -> Observer {
        let sink = self.0.clone();
        Arc::new(move |ev: &HostEvent| sink.lock().unwrap().push(ev.clone()))
    }

    pub fn all(&self) -> Vec<HostEvent> {
        self.0.lock().unwrap().clone()
    }

    pub fn fallbacks(&self) -> Vec<(FallbackReason, String)> {
        self.all().into_iter().filter_map(|e| if let HostEvent::Fallback { reason, detail } = e { Some((reason, detail)) } else { None }).collect()
    }

    /// Classes des fenetres annoncees par `HostEvent::Adopted`, dans l'ordre.
    pub fn adopted(&self) -> Vec<String> {
        self.all().into_iter().filter_map(|e| if let HostEvent::Adopted { class, .. } = e { Some(class) } else { None }).collect()
    }

    /// Valeurs de `HostEvent::Fullscreen`, dans l'ordre.
    pub fn fullscreens(&self) -> Vec<bool> {
        self.all().into_iter().filter_map(|e| if let HostEvent::Fullscreen(on) = e { Some(on) } else { None }).collect()
    }

    pub fn logs(&self) -> Vec<String> {
        self.all().into_iter().filter_map(|e| if let HostEvent::Log(m) = e { Some(m) } else { None }).collect()
    }

    /// Une ligne du journal contient `needle`.
    pub fn logged(&self, needle: &str) -> bool {
        self.logs().iter().any(|l| l.contains(needle))
    }

    /// Echec du test si le journal ne contient pas `needle` (affiche alors tout le journal).
    #[track_caller]
    pub fn assert_logged(&self, needle: &str) {
        assert!(self.logged(needle), "journal sans {needle:?} :\n{}", self.logs().join("\n"));
    }
}

// ------------------------------------------------------------------------------------------------ echantillonnage

/// Echantillonne en continu (toutes les ~0,5 ms, sur un fil et une connexion a part) les fenetres du processus `pid` qui sont des
/// enfants DIRECTS de la racine et de classe `class` : tout instant ou l'une d'elles est VISIBLE est une fenetre de jeu autonome
/// que le joueur ne doit jamais apercevoir tant que le jeu est dans la Scene.
pub struct Sampler {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<(u64, Vec<String>)>>,
}

impl Sampler {
    pub fn start(pid: u32, class: &str) -> Sampler {
        let stop = Arc::new(AtomicBool::new(false));
        let (flag, class) = (stop.clone(), class.to_string());
        let thread = std::thread::spawn(move || {
            let (c, n) = RustConnection::connect(None).expect("connexion de l'echantillonneur");
            let root = c.setup().roots[n].root;
            let pid_atom = c.intern_atom(false, b"_NET_WM_PID").unwrap().reply().unwrap().atom;
            let t0 = Instant::now();
            let (mut samples, mut seen) = (0u64, Vec::new());
            while !flag.load(Ordering::Relaxed) {
                let kids = c.query_tree(root).unwrap().reply().unwrap().children;
                for w in kids {
                    let Some(p) = c.get_property(false, w, pid_atom, AtomEnum::CARDINAL, 0, 1).ok().and_then(|k| k.reply().ok()).and_then(|r| r.value32().and_then(|mut v| v.next())) else { continue };
                    let cls = c.get_property(false, w, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 64).ok().and_then(|k| k.reply().ok()).map(|r| String::from_utf8_lossy(&r.value).split('\0').nth(1).unwrap_or("").to_string());
                    if p != pid || cls.as_deref() != Some(class.as_str()) {
                        continue;
                    }
                    if c.get_window_attributes(w).ok().and_then(|k| k.reply().ok()).is_some_and(|a| a.map_state == MapState::VIEWABLE) {
                        seen.push(format!("+{} ms : fenetre {w:#x} visible sur la racine", t0.elapsed().as_millis()));
                    }
                }
                samples += 1;
                std::thread::sleep(Duration::from_micros(500));
            }
            (samples, seen)
        });
        Sampler { stop, thread: Some(thread) }
    }

    /// Arrete l'echantillonnage : (nombre d'echantillons, apparitions fautives).
    pub fn stop(mut self) -> (u64, Vec<String>) {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().map(|t| t.join().expect("fil d'echantillonnage")).unwrap_or_default()
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Valeurs 32 bits d'une propriete de `win` (vide : absente).
pub fn card_prop(win: u32, name: &str) -> Vec<u32> {
    let r = x().0.get_property(false, win, atom(name), AtomEnum::CARDINAL, 0, 16).unwrap().reply().unwrap();
    r.value32().map(|v| v.collect()).unwrap_or_default()
}

/// Premiere valeur d'une propriete `CARDINAL` (0 : absente).
pub fn card1(win: u32, name: &str) -> u32 {
    card_prop(win, name).first().copied().unwrap_or(0)
}

/// Deplace la fenetre du launcher (sans gestionnaire de fenetres : la requete est executee telle quelle).
pub fn move_window(win: u32, xy: (i32, i32)) {
    let (c, _) = x();
    c.configure_window(win, &ConfigureWindowAux::new().x(xy.0).y(xy.1)).unwrap();
    c.flush().unwrap();
}

/// Rectangles de la region d'ENTREE (`SHAPE`) d'une fenetre.
pub fn input_shape(win: u32) -> Vec<Rectangle> {
    use x11rb::protocol::shape;
    shape::get_rectangles(&x().0, win, shape::SK::INPUT).unwrap().reply().unwrap().rectangles
}

/// Fenetre (fille directe de la racine) sous le pointeur apres l'avoir place en `(px, py)` (coordonnees ecran) : ce que la souris
/// atteindrait. Deplace le pointeur du serveur X de test (nu), jamais celui d'un bureau reel.
pub fn window_under_pointer(px: i16, py: i16) -> u32 {
    let (c, root) = x();
    c.warp_pointer(0u32, *root, 0, 0, 0, 0, px, py).unwrap();
    c.flush().unwrap();
    c.query_pointer(*root).unwrap().reply().unwrap().child
}

/// Comme `card_prop`, pour une propriete d'un type nomme (`WM_STATE`).
pub fn card_prop_typed(win: u32, name: &str, ty: &str) -> Vec<u32> {
    let r = x().0.get_property(false, win, atom(name), atom(ty), 0, 16).unwrap().reply().unwrap();
    r.value32().map(|v| v.collect()).unwrap_or_default()
}
