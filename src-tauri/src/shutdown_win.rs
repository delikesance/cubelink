//! Fenêtre cachée de premier niveau qui reçoit la fin de session de Windows (docs/hearth.md § 7.3).
//!
//! Pas `HWND_MESSAGE` : une fenêtre de messages ne reçoit pas les diffusions `WM_QUERYENDSESSION` / `WM_ENDSESSION`, et
//! Tao n'expose pas ces messages. Elle vit sur un fil à elle, avec sa boucle de messages. Windows n'exerce ce code qu'à
//! l'arrêt réel d'une session (déconnexion, extinction, redémarrage, mise à jour MSIX via `ENDSESSION_CLOSEAPP`).

use std::ptr::{null, null_mut};
use std::sync::{Arc, OnceLock};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Shutdown::{ShutdownBlockReasonCreate, ShutdownBlockReasonDestroy};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassW, TranslateMessage, MSG, WM_ENDSESSION,
    WM_QUERYENDSESSION, WNDCLASSW, WS_OVERLAPPED,
};

use super::{query_action, QueryAction, SessionEnd, BLOCK_REASON, END_SESSION_WAIT};

static HANDLER: OnceLock<Arc<dyn SessionEnd>> = OnceLock::new();

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Démarre le fil de la fenêtre cachée. Un échec est seulement noté : Cubelink fonctionne alors comme avant.
pub fn spawn_session_watcher(handler: Arc<dyn SessionEnd>) {
    if HANDLER.set(handler).is_err() {
        return;
    }
    let spawned = std::thread::Builder::new().name("cubelink-session-end".into()).spawn(|| {
        // SAFETY: appels Win32 ordinaires, sur le fil qui possède la fenêtre et sa boucle de messages.
        unsafe { message_loop() }
    });
    if let Err(e) = spawned {
        eprintln!("[Cubelink] fin de session Windows non écoutée : {e}");
    }
}

unsafe fn message_loop() {
    let class = wide("CubelinkSessionEnd");
    let title = wide("Cubelink");
    let hinstance = GetModuleHandleW(null());
    let mut wc: WNDCLASSW = std::mem::zeroed();
    wc.lpfnWndProc = Some(wnd_proc);
    wc.hInstance = hinstance;
    wc.lpszClassName = class.as_ptr();
    if RegisterClassW(&wc) == 0 {
        eprintln!("[Cubelink] fin de session Windows : classe de fenêtre refusée");
        return;
    }
    // Jamais affichée (pas de WS_VISIBLE), de premier niveau (pas de parent, pas HWND_MESSAGE).
    let hwnd = CreateWindowExW(0, class.as_ptr(), title.as_ptr(), WS_OVERLAPPED, 0, 0, 0, 0, null_mut(), null_mut(), hinstance, null());
    if hwnd.is_null() {
        eprintln!("[Cubelink] fin de session Windows : fenêtre refusée");
        return;
    }
    let mut msg: MSG = std::mem::zeroed();
    while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_QUERYENDSESSION => {
            let Some(h) = HANDLER.get() else { return 1 };
            match query_action(h.busy()) {
                QueryAction::Allow => 1,
                QueryAction::BlockAndStop => {
                    let reason = wide(BLOCK_REASON);
                    ShutdownBlockReasonCreate(hwnd, reason.as_ptr());
                    h.begin();
                    // FALSE : l'arrêt attend la fin de notre routine (l'utilisateur garde « Arrêter quand même »).
                    0
                }
            }
        }
        // `wparam` vrai : la session se termine pour de bon, le processus peut être tué dès le retour. `lparam` porte
        // `ENDSESSION_CLOSEAPP` (fermeture demandée par le Store ou le Restart Manager) : même traitement.
        WM_ENDSESSION => {
            if wparam != 0 {
                if let Some(h) = HANDLER.get() {
                    h.begin();
                    h.wait(END_SESSION_WAIT);
                }
                ShutdownBlockReasonDestroy(hwnd);
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
