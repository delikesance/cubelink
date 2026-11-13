//! Raccourci global du panneau « en jeu » (amis, invitations, modpacks), même quand le jeu a le clavier ; ne fait rien hors du jeu. Notification d'invitation d'un ami. Rien ici ne doit empêcher l'application de démarrer.
//!
//! Le raccourci n'est enregistré que pendant qu'un jeu tourne (il ne vole donc pas `Ctrl+Maj+C` aux autres applications).

use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tauri::{AppHandle, Listener, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use tauri_plugin_notification::NotificationExt;

use crate::state::{lock, AppState};

/// État partagé (géré par Tauri).
#[derive(Default)]
pub struct HotkeyCtl {
    game_running: AtomicBool,
    registered: Mutex<Option<Shortcut>>,
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

/// Pur : titre et corps de la notification d'invitation (`None` si la charge utile est inutilisable). `hotkey` : `None` quand
/// le raccourci n'existera pas (pas de Scene : le jeu a sa propre fenetre), le texte renvoie alors vers le launcher.
pub fn invite_notification(payload: &Value, hotkey: Option<&str>) -> Option<(String, String)> {
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
    match hotkey {
        Some(hotkey) => body.push_str(&format!("Appuie sur {hotkey} en jeu pour répondre.")),
        None => body.push_str("Ouvre Cubelink pour répondre."),
    }
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

/// Ouvre ou ferme le panneau « en jeu ». Sans effet hors du jeu (chargement, rideau, launcher visible).
pub fn toggle(app: &AppHandle) {
    let (Some(st), Some(w)) = (app.try_state::<Arc<AppState>>(), app.get_webview_window("main")) else { return };
    let open = !st.stage.state().panel;
    st.stage.set_panel(&w, open);
}

/// Aligne l'enregistrement du raccourci sur l'état (réglages + jeu en cours + jeu affiché dans la fenêtre du launcher).
pub fn apply_settings(app: &AppHandle) {
    let Some(ctl) = app.try_state::<Arc<HotkeyCtl>>() else { return };
    let (enabled, hotkey) = settings_of(app);
    let staged = app.try_state::<Arc<AppState>>().is_some_and(|s| s.stage.staged());
    let available = enabled && staged && ctl.game_running.load(Ordering::SeqCst);
    let gs = app.global_shortcut();
    let mut reg = lock(&ctl.registered);
    if let Some(old) = reg.take() {
        let _ = gs.unregister(old);
    }
    if !available {
        return;
    }
    match Shortcut::from_str(hotkey.trim()) {
        Ok(sc) => {
            let h = app.clone();
            let res = gs.on_shortcut(sc, move |_, _, ev| {
                if ev.state == ShortcutState::Pressed {
                    let h = h.clone();
                    std::thread::spawn(move || toggle(&h));
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

/// Écoute le jeu et les invitations. Appelée une fois depuis `setup` ; n'échoue jamais.
pub fn setup(app: &AppHandle, test_mode: bool) {
    app.manage(Arc::new(HotkeyCtl::default()));
    let h = app.clone();
    app.listen("launch://state", move |ev| {
        let state = serde_json::from_str::<Value>(ev.payload())
            .ok()
            .and_then(|v| v.get("state").and_then(Value::as_str).and_then(running_after));
        let Some(now) = state else { return };
        let Some(ctl) = h.try_state::<Arc<HotkeyCtl>>() else { return };
        if ctl.game_running.swap(now, Ordering::SeqCst) != now {
            let h = h.clone();
            // Le gestionnaire d'événement ne doit pas bloquer l'émetteur.
            std::thread::spawn(move || apply_settings(&h));
        }
    });
    // Repli : le jeu a repris sa propre fenêtre, le panneau « en jeu » n'existe plus pour lui ; le raccourci est rendu
    // aux autres applications.
    let h = app.clone();
    app.listen("stage://fallback", move |_| {
        let h = h.clone();
        std::thread::spawn(move || apply_settings(&h));
    });
    let h = app.clone();
    app.listen("friend://invite", move |ev| {
        if test_mode {
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(ev.payload()) else { return };
        let (_, hotkey) = settings_of(&h);
        // Linux : le raccourci n'existe que si le prochain jeu ira dans la Scene (X11) ; sinon la notification n'en parle pas.
        let scene = h.try_state::<Arc<AppState>>().is_some_and(|s| s.stage.available());
        let hotkey = (!cfg!(target_os = "linux") || scene).then_some(hotkey.as_str());
        if let Some((title, body)) = invite_notification(&v, hotkey) {
            if let Err(e) = h.notification().builder().title(title).body(body).show() {
                eprintln!("[Cubelink] notification d'invitation : {e}");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn notification_d_invitation() {
        let v = serde_json::json!({"name": "Alex", "world_name": "Survie"});
        let (t, b) = invite_notification(&v, Some("Ctrl+Shift+C")).unwrap();
        assert_eq!(t, "Alex t'invite à rejoindre son monde");
        assert!(b.contains("Survie") && b.contains("Ctrl+Shift+C"));
        let (_, b) = invite_notification(&v, None).unwrap();
        assert!(b.contains("Survie") && !b.contains("Ctrl") && b.contains("Ouvre Cubelink"));
        assert!(invite_notification(&serde_json::json!({"world_name": "x"}), Some("F9")).is_none());
        assert!(invite_notification(&serde_json::json!({"name": "  "}), Some("F9")).is_none());
        let long = serde_json::json!({"name": "a".repeat(500)});
        assert!(invite_notification(&long, Some("F9")).unwrap().0.len() < 100);
    }
}
