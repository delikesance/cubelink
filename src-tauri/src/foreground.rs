//! Amène la fenêtre du jeu INVITÉ au premier plan (Windows seulement).
//!
//! Pourquoi : le mod Dynamic FPS (livré dans de nombreux packs, p. ex. Fabulously Optimized) bride le rendu du jeu tant
//! que sa fenêtre n'est pas au premier plan ; le `--quickPlayMultiplayer` n'est alors traité qu'à ce rythme (mesuré :
//! 15 s à plus de 7 min entre « Overlay is removed » et « Connecting to »). Quand l'ami clique sur « Rejoindre », la
//! fenêtre du jeu doit donc passer devant.
//!
//! Périmètre : uniquement les fenêtres de haut niveau visibles du PROCESSUS du jeu (celui lancé par Cubelink). Aucune
//! autre fenêtre n'est touchée, aucune capture d'écran, aucune saisie simulée.

#[cfg(windows)]
mod imp {
    use std::time::{Duration, Instant};
    use windows_sys::core::BOOL;
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, EnumWindows, GetForegroundWindow, GetWindow, GetWindowTextLengthW, GetWindowThreadProcessId,
        IsIconic, IsWindowVisible, SetForegroundWindow, ShowWindow, GW_OWNER, SW_RESTORE,
    };
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowLongW, GetWindowRect, GWL_STYLE, WS_CAPTION};

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

    /// Vrai si la fenêtre du jeu couvre exactement son moniteur sans bordure ni titre (plein écran / borderless).
    pub fn fullscreen(pid: u32) -> bool {
        let Some(hwnd) = game_window(pid) else { return false };
        unsafe {
            let mut r = std::mem::zeroed();
            if GetWindowRect(hwnd, &mut r) == 0 {
                return false;
            }
            let mut mi: MONITORINFO = std::mem::zeroed();
            mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut mi) == 0 {
                return false;
            }
            let m = mi.rcMonitor;
            let has_caption = (GetWindowLongW(hwnd, GWL_STYLE) as u32) & WS_CAPTION == WS_CAPTION;
            super::covers_monitor((r.left, r.top, r.right, r.bottom), (m.left, m.top, m.right, m.bottom), has_caption)
        }
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

/// Lance (en arrière-plan) l'amenée au premier plan de la fenêtre du jeu `pid`. Sans effet hors Windows.
pub fn bring_game_to_front(pid: u32) {
    #[cfg(windows)]
    imp::bring_to_front(pid);
    #[cfg(not(windows))]
    let _ = pid;
}

/// Pur : la fenêtre `(gauche, haut, droite, bas)` couvre-t-elle exactement le moniteur, sans barre de titre ?
pub fn covers_monitor(win: (i32, i32, i32, i32), mon: (i32, i32, i32, i32), has_caption: bool) -> bool {
    !has_caption && win == mon
}

/// Best effort : le jeu `pid` est-il en plein écran (fenêtre sans cadre couvrant le moniteur) ? Un overlay
/// toujours au-dessus peut alors rester masqué. Toujours `false` hors Windows ou si la fenêtre est introuvable.
pub fn game_fullscreen(pid: u32) -> bool {
    #[cfg(windows)]
    return pid != 0 && imp::fullscreen(pid);
    #[cfg(not(windows))]
    {
        let _ = pid;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::covers_monitor;

    #[test]
    fn plein_ecran_seulement_sans_titre_et_pile_sur_le_moniteur() {
        let m = (0, 0, 1920, 1080);
        assert!(covers_monitor(m, m, false));
        assert!(!covers_monitor(m, m, true), "fenêtre maximisée avec titre");
        assert!(!covers_monitor((0, 0, 1280, 720), m, false), "fenêtré");
        assert!(!covers_monitor((-8, -8, 1928, 1088), m, false), "débordement d'une fenêtre maximisée");
    }
}
