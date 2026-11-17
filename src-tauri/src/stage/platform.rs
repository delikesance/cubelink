//! Ce qui depend du systeme : identifiant natif de la fenetre du launcher, serveur X11 joignable, Scene possible.

use super::Stage;

/// Linux : la Scene peut servir sur ce poste. Un serveur X11 est joignable (`DISPLAY`, XWayland compris) et rien ne la coupe
/// (`CUBELINK_NO_STAGE`, mode test sans `CUBELINK_STAGE_TEST`). Le launcher n'est pas forcement en X11 a cet instant : avec
/// « Jeu integre » coupe au demarrage, il garde le Wayland natif (voir `lib.rs`), et le reglage doit rester visible pour etre
/// reactive (pris en compte au prochain demarrage).
#[cfg(target_os = "linux")]
pub fn x11_reachable() -> bool {
    let allowed = !crate::test_hooks::hooks().test_mode || std::env::var_os("CUBELINK_STAGE_TEST").is_some();
    std::env::var_os("DISPLAY").is_some_and(|d| !d.is_empty()) && std::env::var_os("CUBELINK_NO_STAGE").is_none() && allowed
}

/// Identifiant natif de la fenetre du launcher : HWND (Windows).
#[cfg(windows)]
pub(super) fn own_handle(window: &tauri::WebviewWindow) -> Option<isize> {
    window.hwnd().ok().map(|h| h.0 as isize)
}

/// Identifiant natif de la fenetre du launcher : XID (Linux, fenetre X11).
#[cfg(target_os = "linux")]
pub(super) fn own_handle(window: &tauri::WebviewWindow) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Xlib(h) => Some(h.window as isize),
        RawWindowHandle::Xcb(h) => Some(h.window.get() as isize),
        _ => None,
    }
}

/// Pur : la Scene existe-t-elle sur cette plateforme ? Windows toujours ; Linux seulement avec un serveur X11 joignable.
pub(super) fn stage_supported_on(windows: bool, linux: bool, x11: bool) -> bool {
    windows || (linux && x11)
}

/// Les jeux lances dans la Scene doivent etre forces en X11 (voir `launch_run::RunOptions::force_x11`).
impl Stage {
    pub fn force_x11(&self) -> bool {
        cfg!(target_os = "linux") && self.live_host().is_some()
    }
}
