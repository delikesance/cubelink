//! Panneau superposé au jeu : fenêtre `overlay`, raccourci global, notification d'invitation.
//!
//! Le panneau n'existe que pendant qu'un jeu tourne (écoute de `launch://state`). Le raccourci global n'est
//! enregistré que dans ce cas (il ne vole donc pas `Ctrl+Shift+C` aux autres applications le reste du temps).
//! Rien ici ne doit empêcher l'application de démarrer : toute erreur est journalisée puis ignorée.

use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Listener, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use tauri_plugin_notification::NotificationExt;

use crate::state::{lock, AppState};

pub const LABEL: &str = "overlay";
const WIDTH: f64 = 420.0;
const HEIGHT: f64 = 640.0;
const MARGIN: i32 = 16;

/// Charge utile de la commande `overlay_state` et de l'événement `overlay://state`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OverlayState {
    pub available: bool,
    pub game_running: bool,
    pub hotkey: String,
    pub fullscreen_exclusive: bool,
}

/// État partagé (géré par Tauri).
#[derive(Default)]
pub struct OverlayCtl {
    game_running: AtomicBool,
    registered: Mutex<Option<Shortcut>>,
}

/// Pur : le panneau n'est utilisable que s'il est activé ET qu'un jeu tourne.
pub fn overlay_available(enabled: bool, game_running: bool) -> bool {
    enabled && game_running
}

/// Pur : `launch://state` -> le jeu tourne-t-il ? `None` si l'état ne change rien (préparation).
pub fn running_after(state: &str) -> Option<bool> {
    match state {
        "running" => Some(true),
        "exited" | "failed" => Some(false),
        _ => None,
    }
}

/// Valide un raccourci (format du plugin) : erreur française claire sinon.
pub fn validate_hotkey(h: &str) -> Result<(), String> {
    let sc = Shortcut::from_str(h.trim())
        .map_err(|_| format!("raccourci invalide « {h} » : utilisez par exemple Ctrl+Shift+C ou Alt+F9"))?;
    let is_fkey = {
        let k = format!("{:?}", sc.key);
        k.len() >= 2 && k.starts_with('F') && k[1..].chars().all(|c| c.is_ascii_digit())
    };
    if sc.mods.is_empty() && !is_fkey {
        return Err("le raccourci doit contenir au moins une touche Ctrl, Alt, Maj ou Windows (ou être une touche F1 à F24)".into());
    }
    Ok(())
}

/// Pur : coin haut-gauche (px physiques) d'une fenêtre collée au bord droit du moniteur, centrée en hauteur.
pub fn place_right(mon: (i32, i32, i32, i32), win_w: i32, win_h: i32, margin: i32) -> (i32, i32) {
    let (mx, my, mw, mh) = mon;
    let x = (mx + mw - win_w - margin).max(mx);
    let y = (my + (mh - win_h) / 2).max(my);
    (x, y)
}

/// Pur : titre et corps de la notification d'invitation (`None` si la charge utile est inutilisable).
pub fn invite_notification(payload: &Value, hotkey: &str) -> Option<(String, String)> {
    let clip = |s: &str| s.chars().filter(|c| !c.is_control()).take(48).collect::<String>();
    let name = clip(payload.get("name")?.as_str()?);
    if name.trim().is_empty() {
        return None;
    }
    let world = payload.get("world_name").and_then(Value::as_str).map(clip).filter(|w| !w.trim().is_empty());
    let mut body = match world {
        Some(w) => format!("Monde : {w}. "),
        None => String::new(),
    };
    body.push_str(&format!("Ouvre le panneau avec {hotkey} pour répondre."));
    Some((format!("{name} t'invite à rejoindre son monde"), body))
}

fn settings_of(app: &AppHandle) -> (bool, String) {
    match app.try_state::<Arc<AppState>>() {
        Some(st) => {
            let s = st.settings_get();
            (s.overlay_enabled, s.overlay_hotkey)
        }
        None => (true, core_lib::types::DEFAULT_OVERLAY_HOTKEY.to_string()),
    }
}

fn game_pid(app: &AppHandle) -> u32 {
    app.try_state::<Arc<AppState>>()
        .and_then(|st| lock(&st.game_handle).as_ref().map(|(_, g)| g.pid))
        .unwrap_or(0)
}

fn running(app: &AppHandle) -> bool {
    app.try_state::<Arc<OverlayCtl>>().map(|c| c.game_running.load(Ordering::SeqCst)).unwrap_or(false)
}

pub fn snapshot(app: &AppHandle) -> OverlayState {
    let (enabled, hotkey) = settings_of(app);
    let game_running = running(app);
    let fullscreen_exclusive = game_running && crate::foreground::game_fullscreen(game_pid(app));
    OverlayState { available: overlay_available(enabled, game_running), game_running, hotkey, fullscreen_exclusive }
}

fn emit_state(app: &AppHandle) {
    let _ = app.emit("overlay://state", snapshot(app));
}

fn hide(app: &AppHandle, give_focus_back: bool) {
    if let Some(w) = app.get_webview_window(LABEL) {
        if w.is_visible().unwrap_or(false) {
            let _ = w.hide();
            if give_focus_back {
                crate::foreground::bring_game_to_front(game_pid(app));
            }
        }
    }
}

fn ensure_window(app: &AppHandle) -> Result<tauri::WebviewWindow, String> {
    if let Some(w) = app.get_webview_window(LABEL) {
        return Ok(w);
    }
    WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("index.html#/overlay".into()))
        .title("Cubelink")
        .inner_size(WIDTH, HEIGHT)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .visible(false)
        .build()
        .map_err(|e| format!("impossible de créer le panneau : {e}"))
}

fn place(app: &AppHandle, w: &tauri::WebviewWindow) {
    let Ok(Some(m)) = w.current_monitor().or_else(|_| app.primary_monitor()) else { return };
    let (p, s) = (m.position(), m.size());
    let scale = m.scale_factor();
    let (x, y) = place_right(
        (p.x, p.y, s.width as i32, s.height as i32),
        (WIDTH * scale) as i32,
        (HEIGHT * scale) as i32,
        (MARGIN as f64 * scale) as i32,
    );
    let _ = w.set_position(tauri::PhysicalPosition::new(x, y));
}

/// Affiche (avec focus) ou cache le panneau. À appeler hors du fil principal (création de fenêtre).
pub fn toggle(app: &AppHandle) -> Result<(), String> {
    if !snapshot(app).available {
        return Err("aucun jeu en cours : le panneau n'est disponible que pendant une partie".into());
    }
    let w = ensure_window(app)?;
    if w.is_visible().unwrap_or(false) {
        hide(app, true);
    } else {
        place(app, &w);
        let _ = w.show();
        let _ = w.set_focus();
    }
    emit_state(app);
    Ok(())
}

/// Aligne l'enregistrement du raccourci et la visibilité du panneau sur l'état (réglages + jeu en cours).
pub fn apply_settings(app: &AppHandle) {
    let Some(ctl) = app.try_state::<Arc<OverlayCtl>>() else { return };
    let (enabled, hotkey) = settings_of(app);
    let available = overlay_available(enabled, running(app));
    let gs = app.global_shortcut();
    let mut reg = lock(&ctl.registered);
    if let Some(old) = reg.take() {
        let _ = gs.unregister(old);
    }
    if available {
        match Shortcut::from_str(hotkey.trim()) {
            Ok(sc) => {
                let h = app.clone();
                let res = gs.on_shortcut(sc, move |_, _, ev| {
                    if ev.state == ShortcutState::Pressed {
                        let h = h.clone();
                        std::thread::spawn(move || {
                            if let Err(e) = toggle(&h) {
                                eprintln!("[Cubelink] raccourci du panneau : {e}");
                            }
                        });
                    }
                });
                match res {
                    Ok(()) => *reg = Some(sc),
                    Err(e) => eprintln!("[Cubelink] raccourci « {hotkey} » non enregistré : {e}"),
                }
            }
            Err(e) => eprintln!("[Cubelink] raccourci « {hotkey} » invalide : {e}"),
        }
    }
    drop(reg);
    if !available {
        hide(app, false);
    }
    emit_state(app);
}

/// Écoute le jeu et les invitations. Appelée une fois depuis `setup` ; n'échoue jamais.
pub fn setup(app: &AppHandle, test_mode: bool) {
    app.manage(Arc::new(OverlayCtl::default()));
    let h = app.clone();
    app.listen("launch://state", move |ev| {
        let state = serde_json::from_str::<Value>(ev.payload())
            .ok()
            .and_then(|v| v.get("state").and_then(Value::as_str).and_then(running_after));
        let Some(now) = state else { return };
        let Some(ctl) = h.try_state::<Arc<OverlayCtl>>() else { return };
        if ctl.game_running.swap(now, Ordering::SeqCst) != now {
            let h = h.clone();
            // Le gestionnaire d'événement ne doit pas bloquer l'émetteur (enregistrement du raccourci, fenêtre).
            std::thread::spawn(move || apply_settings(&h));
        }
    });
    let h = app.clone();
    app.listen("friend://invite", move |ev| {
        if test_mode {
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(ev.payload()) else { return };
        let (_, hotkey) = settings_of(&h);
        if let Some((title, body)) = invite_notification(&v, &hotkey) {
            if let Err(e) = h.notification().builder().title(title).body(body).show() {
                eprintln!("[Cubelink] notification d'invitation : {e}");
            }
        }
    });
}

#[tauri::command(rename_all = "snake_case")]
pub async fn overlay_toggle(app: AppHandle) -> Result<(), String> {
    toggle(&app)
}

#[tauri::command(rename_all = "snake_case")]
pub fn overlay_state(app: AppHandle) -> Result<OverlayState, String> {
    Ok(snapshot(&app))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disponible_seulement_si_active_et_jeu_en_cours() {
        assert!(overlay_available(true, true));
        assert!(!overlay_available(true, false));
        assert!(!overlay_available(false, true));
        assert!(!overlay_available(false, false));
    }

    #[test]
    fn etat_du_jeu_depuis_launch_state() {
        assert_eq!(running_after("running"), Some(true));
        assert_eq!(running_after("exited"), Some(false));
        assert_eq!(running_after("failed"), Some(false));
        assert_eq!(running_after("preparing"), None);
    }

    #[test]
    fn raccourcis_valides_et_invalides() {
        for ok in ["Ctrl+Shift+C", "Alt+F9", "F8", "CommandOrControl+Shift+K", "Ctrl+Alt+Space"] {
            assert!(validate_hotkey(ok).is_ok(), "{ok}");
        }
        for bad in ["", "Ctrl+Shift+", "Ctrl+Pas", "C", "Espace", "Ctrl+A+B"] {
            let e = validate_hotkey(bad).unwrap_err();
            assert!(e.contains("raccourci"), "{bad}: {e}");
        }
    }

    #[test]
    fn placement_a_droite_et_centre() {
        assert_eq!(place_right((0, 0, 1920, 1080), 420, 640, 16), (1484, 220));
        // Moniteur secondaire décalé ; fenêtre plus grande que l'écran : reste dans l'écran.
        assert_eq!(place_right((1920, 0, 1920, 1080), 420, 640, 16), (3404, 220));
        assert_eq!(place_right((0, 0, 300, 400), 420, 640, 16), (0, 0));
    }

    #[test]
    fn notification_d_invitation() {
        let v = serde_json::json!({"name": "Alex", "world_name": "Survie"});
        let (t, b) = invite_notification(&v, "Ctrl+Shift+C").unwrap();
        assert_eq!(t, "Alex t'invite à rejoindre son monde");
        assert!(b.contains("Survie") && b.contains("Ctrl+Shift+C"));
        assert!(invite_notification(&serde_json::json!({"world_name": "x"}), "F9").is_none());
        assert!(invite_notification(&serde_json::json!({"name": "  "}), "F9").is_none());
        let long = serde_json::json!({"name": "a".repeat(500)});
        assert!(invite_notification(&long, "F9").unwrap().0.len() < 100);
    }
}
