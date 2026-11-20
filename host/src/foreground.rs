//! Amène la fenêtre du jeu INVITÉ au premier plan (Windows, et Linux via X11/XWayland).
//!
//! Pourquoi : le mod Dynamic FPS (livré dans de nombreux packs, p. ex. Fabulously Optimized) bride le rendu du jeu tant
//! que sa fenêtre n'est pas au premier plan ; le `--quickPlayMultiplayer` n'est alors traité qu'à ce rythme (mesuré :
//! 15 s à plus de 7 min entre « Overlay is removed » et « Connecting to »). Quand l'ami clique sur « Rejoindre », la
//! fenêtre du jeu doit donc passer devant.
//!
//! Périmètre : uniquement les fenêtres de haut niveau visibles du PROCESSUS du jeu (celui lancé par Cubelink) et notre
//! propre fenêtre. Aucune autre fenêtre n'est touchée, aucune capture d'écran. Une seule saisie simulée existe, en tout
//! dernier recours (`focus_own` : appui sur Alt si Windows refuse deux fois le premier plan) ; elle est journalisée.

#[cfg(windows)]
mod imp {
    use std::time::{Duration, Instant};
    use windows_sys::core::BOOL;
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{keybd_event, SetFocus, KEYEVENTF_KEYUP, VK_MENU};
    use windows_sys::Win32::UI::WindowsAndMessaging::ClipCursor;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, EnumWindows, GetForegroundWindow, GetWindow, GetWindowTextLengthW, GetWindowThreadProcessId,
        IsIconic, IsWindowVisible, SetForegroundWindow, ShowWindow, GW_OWNER, SW_RESTORE,
    };

    struct Search {
        pid: u32,
        found: HWND,
    }

    unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let s = &mut *(lparam as *mut Search);
        let mut owner_pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut owner_pid);
        if owner_pid == s.pid && IsWindowVisible(hwnd) != 0 && GetWindow(hwnd, GW_OWNER).is_null() && GetWindowTextLengthW(hwnd) > 0 {
            s.found = hwnd;
            return 0; // trouvée : on arrête l'énumération
        }
        1
    }

    fn game_window(pid: u32) -> Option<HWND> {
        let mut s = Search { pid, found: std::ptr::null_mut() };
        unsafe { EnumWindows(Some(each), &mut s as *mut Search as LPARAM) };
        (!s.found.is_null()).then_some(s.found)
    }

    /// Restaure puis passe la fenêtre au premier plan. Windows n'autorise `SetForegroundWindow` qu'à un processus qui
    /// a le focus ou vient de recevoir une saisie : on rattache brièvement notre file de saisie à celle de la fenêtre
    /// au premier plan (méthode documentée, sans saisie simulée). Renvoie `true` si la fenêtre est bien devant.
    fn front(hwnd: HWND) -> bool {
        unsafe {
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            }
            let fg = GetForegroundWindow();
            let me = GetCurrentThreadId();
            let fg_thread = if fg.is_null() { 0 } else { GetWindowThreadProcessId(fg, std::ptr::null_mut()) };
            let attached = fg_thread != 0 && fg_thread != me && AttachThreadInput(me, fg_thread, 1) != 0;
            BringWindowToTop(hwnd);
            let ok = SetForegroundWindow(hwnd) != 0;
            SetFocus(hwnd);
            if attached {
                AttachThreadInput(me, fg_thread, 0);
            }
            ok && GetForegroundWindow() == hwnd
        }
    }

    /// Prend le premier plan pour NOTRE fenêtre `hwnd` (panneau en jeu) et libère le curseur : `AttachThreadInput` (voir
    /// `front`), puis, en dernier recours, la touche Alt qui lève le verrou de premier plan. Mesuré en réel (2026-11-12) : une
    /// demande simple `SetForegroundWindow` est refusée à chaque ouverture du panneau, `AttachThreadInput` est donc nécessaire ;
    /// le repli sur Alt est journalisé (jamais déclenché pendant cet essai) pour savoir s'il peut être supprimé.
    pub fn focus_own(hwnd: isize) -> bool {
        let hwnd = hwnd as HWND;
        if hwnd.is_null() {
            return false;
        }
        let mut ok = front(hwnd);
        if !ok {
            eprintln!("[Cubelink] premier plan : AttachThreadInput refusé, repli sur la touche Alt");
            unsafe {
                keybd_event(VK_MENU as u8, 0, 0, 0);
                keybd_event(VK_MENU as u8, 0, KEYEVENTF_KEYUP, 0);
            }
            ok = front(hwnd);
        }
        unsafe { ClipCursor(std::ptr::null()) };
        ok
    }

    pub fn bring_to_front(pid: u32) {
        if pid == 0 {
            return;
        }
        std::thread::spawn(move || {
            let end = Instant::now() + Duration::from_secs(300);
            let mut tries = 0;
            while Instant::now() < end {
                std::thread::sleep(Duration::from_millis(500));
                let Some(hwnd) = game_window(pid) else { continue };
                tries += 1;
                let ok = front(hwnd);
                eprintln!("[Cubelink] fenêtre du jeu (pid {pid}) amenée au premier plan : {}", if ok { "oui" } else { "refusé par Windows" });
                if ok || tries >= 5 {
                    return;
                }
            }
        });
    }
}

#[cfg(target_os = "linux")]
mod imp_x11 {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{
        AtomEnum, ClientMessageEvent, ConfigureWindowAux, ConnectionExt, EventMask, InputFocus, MapState, StackMode, Window,
    };
    use x11rb::rust_connection::RustConnection;
    use x11rb::CURRENT_TIME;

    fn connect() -> Option<(RustConnection, Window)> {
        let (c, n) = RustConnection::connect(None).ok()?;
        let root = c.setup().roots.get(n)?.root;
        Some((c, root))
    }

    fn atom(c: &RustConnection, name: &str) -> Option<u32> {
        Some(c.intern_atom(false, name.as_bytes()).ok()?.reply().ok()?.atom)
    }

    /// Fenetre (a la racine ou jusqu'a 3 niveaux plus bas : la Scene reparente le jeu) dont `_NET_WM_PID` est `pid` et qui est affichee.
    fn game_window(c: &RustConnection, root: Window, pid: u32) -> Option<Window> {
        let pid_atom = atom(c, "_NET_WM_PID")?;
        let mut level = vec![root];
        for _ in 0..4 {
            let mut next = Vec::new();
            for w in level {
                let Ok(Ok(tree)) = c.query_tree(w).map(|t| t.reply()) else { continue };
                for child in tree.children {
                    let has_pid = c
                        .get_property(false, child, pid_atom, AtomEnum::CARDINAL, 0, 1)
                        .ok()
                        .and_then(|r| r.reply().ok())
                        .and_then(|r| r.value32().and_then(|mut v| v.next()))
                        == Some(pid);
                    let viewable = c.get_window_attributes(child).ok().and_then(|r| r.reply().ok()).is_some_and(|a| a.map_state == MapState::VIEWABLE);
                    if has_pid && viewable {
                        return Some(child);
                    }
                    next.push(child);
                }
            }
            level = next;
        }
        None
    }

    /// Delai que `front` laisse au gestionnaire de fenetres (il repond de facon asynchrone) pour la tentative du fil `bring_to_front`.
    const WAIT_BRING: Duration = Duration::from_millis(300);
    /// Idem pour `focus_own`, appele sur le fil de commande : bref.
    const WAIT_OWN: Duration = Duration::from_millis(150);
    const POLL: Duration = Duration::from_millis(25);
    /// Profondeur maximale de remontee dans l'arbre des fenetres (cadre du gestionnaire, scene, fenetre du jeu).
    const DEPTH: usize = 4;

    /// `from` est `target` ou l'un de ses descendants (jusqu'a `DEPTH` niveaux), sans jamais monter au-dela de la racine.
    fn climbs_to(from: Window, target: Window, root: Window, parent_of: &mut impl FnMut(Window) -> Option<Window>) -> bool {
        let mut cur = from;
        for _ in 0..=DEPTH {
            if cur == target {
                return true;
            }
            if cur == root || cur == 0 {
                return false;
            }
            let Some(p) = parent_of(cur) else { return false };
            cur = p;
        }
        false
    }

    /// La fenetre `other` (focus ou fenetre active) designe `w` : c'est `w`, un descendant de `w` (le jeu reparente sous la Scene
    /// porte le focus a la place de la fenetre de premier niveau) ou un ancetre de `w` (cadre du gestionnaire). La racine, « aucune
    /// fenetre » (0) et `PointerRoot` (1) ne designent jamais `w`.
    fn related(w: Window, other: Window, root: Window, mut parent_of: impl FnMut(Window) -> Option<Window>) -> bool {
        if other <= 1 || other == root || w == root {
            return false;
        }
        climbs_to(other, w, root, &mut parent_of) || climbs_to(w, other, root, &mut parent_of)
    }

    fn parent_of(c: &RustConnection, w: Window) -> Option<Window> {
        Some(c.query_tree(w).ok()?.reply().ok()?.parent)
    }

    /// `w` est bien devant : le focus clavier ou la fenetre active annoncee par le gestionnaire (`_NET_ACTIVE_WINDOW`, absente sans
    /// gestionnaire) designe `w`. `None` : la connexion est coupee.
    fn is_front(c: &RustConnection, root: Window, w: Window, active_atom: Option<u32>) -> Option<bool> {
        let focus = c.get_input_focus().ok()?.reply().ok()?.focus; // aussi un aller-retour : le serveur a traite nos requetes
        if related(w, focus, root, |x| parent_of(c, x)) {
            return Some(true);
        }
        let Some(active_atom) = active_atom else { return Some(false) };
        let active = c
            .get_property(false, root, active_atom, AtomEnum::WINDOW, 0, 1)
            .ok()
            .and_then(|r| r.reply().ok())
            .and_then(|r| r.value32().and_then(|mut v| v.next()))
            .unwrap_or(0);
        Some(related(w, active, root, |x| parent_of(c, x)))
    }

    /// Demande au gestionnaire de fenetres d'activer `w` (EWMH, source « pager » : un gestionnaire refuse d'ordinaire une
    /// source « application » sans saisie recente) puis donne le focus clavier et remonte la fenetre. Le gestionnaire repond de
    /// facon asynchrone : on sonde jusqu'a `wait` et on rend le verdict REEL (`w` a-t-elle le focus ou est-elle la fenetre active ?),
    /// pas « le serveur a repondu ». `None` : connexion coupee.
    fn front(c: &RustConnection, root: Window, w: Window, wait: Duration) -> Option<bool> {
        let active_atom = atom(c, "_NET_ACTIVE_WINDOW");
        if let Some(active) = active_atom {
            let ev = ClientMessageEvent::new(32, w, active, [2, CURRENT_TIME, 0, 0, 0]);
            let _ = c.send_event(false, root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, ev);
        }
        let _ = c.configure_window(w, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE));
        let _ = c.set_input_focus(InputFocus::PARENT, w, CURRENT_TIME);
        c.flush().ok()?;
        let end = Instant::now() + wait;
        loop {
            if is_front(c, root, w, active_atom)? {
                return Some(true);
            }
            if Instant::now() >= end {
                return Some(false);
            }
            std::thread::sleep(POLL);
        }
    }

    /// Connexion de `focus_own`, ouverte a la premiere demande puis reutilisee (une par Ctrl+Maj+C serait un gaspillage).
    static OWN: Mutex<Option<(RustConnection, Window)>> = Mutex::new(None);

    /// Execute `f` sur la connexion partagee. Si elle est coupee (`f` rend `None`), la jette et recommence UNE fois sur une neuve.
    fn with_conn<R>(f: impl Fn(&RustConnection, Window) -> Option<R>) -> Option<R> {
        let mut guard = OWN.lock().unwrap_or_else(|e| e.into_inner());
        for _ in 0..2 {
            if guard.is_none() {
                *guard = connect();
            }
            let (c, root) = guard.as_ref()?;
            // les erreurs des requetes sans reponse (BadMatch sur une fenetre non affichee) arrivent comme des evenements : on les vide
            let alive = std::iter::from_fn(|| c.poll_for_event().transpose()).all(|e| e.is_ok());
            if alive {
                if let Some(r) = f(c, *root) {
                    return Some(r);
                }
            }
            *guard = None;
        }
        None
    }

    pub fn focus_own(xid: isize) -> bool {
        let Ok(w) = u32::try_from(xid) else { return false };
        with_conn(|c, root| front(c, root, w, WAIT_OWN)).unwrap_or(false)
    }

    pub fn bring_to_front(pid: u32) {
        if pid == 0 {
            return;
        }
        std::thread::spawn(move || {
            let Some((c, root)) = connect() else { return };
            let end = Instant::now() + Duration::from_secs(300);
            let mut tries = 0;
            while Instant::now() < end {
                std::thread::sleep(Duration::from_millis(500));
                let Some(w) = game_window(&c, root, pid) else { continue };
                tries += 1;
                let Some(ok) = front(&c, root, w, WAIT_BRING) else { return };
                let verdict = if ok { "oui".to_string() } else { format!("refusé par le gestionnaire de fenêtres (essai {tries}/5)") };
                eprintln!("[Cubelink] fenêtre du jeu (pid {pid}) amenée au premier plan : {verdict}");
                if ok || tries >= 5 {
                    return;
                }
            }
        });
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        const ROOT: Window = 10;

        /// Arbre : racine(10) > cadre(20) > launcher(30) > scene(40) > jeu(50) ; racine > autre(60).
        fn tree(w: Window) -> Option<Window> {
            match w {
                20 | 60 => Some(ROOT),
                30 => Some(20),
                40 => Some(30),
                50 => Some(40),
                _ => None,
            }
        }

        #[test]
        fn la_fenetre_elle_meme_ou_son_descendant_ou_son_cadre_comptent() {
            assert!(related(30, 30, ROOT, tree));
            assert!(related(30, 50, ROOT, tree), "le jeu reparente porte le focus");
            assert!(related(30, 20, ROOT, tree), "le cadre du gestionnaire est active");
        }

        #[test]
        fn une_autre_fenetre_ou_un_focus_neutre_ne_comptent_pas() {
            assert!(!related(30, 60, ROOT, tree));
            assert!(!related(30, 0, ROOT, tree), "aucune fenetre");
            assert!(!related(30, 1, ROOT, tree), "PointerRoot");
            assert!(!related(30, ROOT, ROOT, tree), "la racine est l'ancetre de tout : jamais un verdict");
            assert!(!related(999, 30, ROOT, tree), "fenetre inexistante");
        }
    }
}

/// Lance (en arrière-plan) l'amenée au premier plan de la fenêtre du jeu `pid`. Sans effet hors Windows et Linux.
pub fn bring_game_to_front(pid: u32) {
    #[cfg(windows)]
    imp::bring_to_front(pid);
    #[cfg(target_os = "linux")]
    imp_x11::bring_to_front(pid);
    #[cfg(not(any(windows, target_os = "linux")))]
    let _ = pid;
}

/// Prend le premier plan pour notre propre fenêtre (HWND natif ou XID X11) : `true` seulement si elle est bien devant (Windows :
/// `GetForegroundWindow`, Linux : focus clavier ou `_NET_ACTIVE_WINDOW`), `false` si le système refuse ou hors Windows et Linux.
pub fn focus_own_window(hwnd: isize) -> bool {
    #[cfg(windows)]
    return imp::focus_own(hwnd);
    #[cfg(target_os = "linux")]
    return imp_x11::focus_own(hwnd);
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = hwnd;
        false
    }
}
