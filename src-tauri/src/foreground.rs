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

    /// Demande au gestionnaire de fenetres d'activer `w` (EWMH, source « pager » : un gestionnaire refuse d'ordinaire une
    /// source « application » sans saisie recente) puis donne le focus clavier et remonte la fenetre. Sans effet de bord ailleurs.
    fn front(c: &RustConnection, root: Window, w: Window) -> bool {
        if let Some(active) = atom(c, "_NET_ACTIVE_WINDOW") {
            let ev = ClientMessageEvent::new(32, w, active, [2, CURRENT_TIME, 0, 0, 0]);
            let _ = c.send_event(false, root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, ev);
        }
        let _ = c.configure_window(w, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE));
        let _ = c.set_input_focus(InputFocus::PARENT, w, CURRENT_TIME);
        c.flush().is_ok() && c.get_input_focus().ok().and_then(|r| r.reply().ok()).is_some()
    }

    pub fn focus_own(xid: isize) -> bool {
        let Ok(w) = u32::try_from(xid) else { return false };
        let Some((c, root)) = connect() else { return false };
        front(&c, root, w)
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
                let ok = front(&c, root, w);
                eprintln!("[Cubelink] fenêtre du jeu (pid {pid}) amenée au premier plan : {}", if ok { "oui" } else { "refusé" });
                if ok || tries >= 5 {
                    return;
                }
            }
        });
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

/// Prend le premier plan pour notre propre fenêtre (HWND natif), `false` si Windows refuse ou hors Windows.
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
