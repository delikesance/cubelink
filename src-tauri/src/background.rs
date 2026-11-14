//! Cubelink en fond : fermer la fenêtre la masque (icône dans la zone de notification) au lieu de quitter, pour que les
//! amis restent joignables ; démarrage avec la session (réglage `start_with_os`, lancement masqué) ; une seule instance.
//!
//! Quitter vraiment : menu de l'icône « Quitter Cubelink ». Un jeu de la zone intégrée est alors arrêté comme avant
//! (`quit_requested` / `quit_game`). Fermer la fenêtre pendant qu'un jeu y est intégré l'arrête de la même façon (le
//! masquer cacherait aussi le jeu), puis masque Cubelink au lieu de le quitter.
//!
//! Exceptions, où fermer quitte comme avant : le mode test (les E2E ferment l'application par WM_CLOSE) et l'absence
//! d'icône (Linux sans bibliothèque d'icône : la fenêtre masquée serait introuvable).
//!
//! Rien ici ne doit empêcher l'application de démarrer : une icône ou un démarrage automatique impossible est seulement
//! noté dans la console.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

use crate::state::AppState;

/// Argument passé par le démarrage automatique : Cubelink démarre sans montrer sa fenêtre.
pub const BACKGROUND_ARG: &str = "--background";

/// Fichier témoin (dans le dossier de données) : l'avis « Cubelink continue en arrière-plan » a déjà été montré.
const NOTICE_MARKER: &str = "background-notice-shown";

const MENU_OPEN: &str = "cubelink-open";
const MENU_QUIT: &str = "cubelink-quit";

/// Une sortie (ou un masquage après arrêt du jeu) est déjà en cours : un second clic ne relance rien.
static CLOSING: AtomicBool = AtomicBool::new(false);
/// « Quitter Cubelink » demandé pendant un masquage en cours : la sortie a lieu à la fin de celui-ci.
static EXIT_PENDING: AtomicBool = AtomicBool::new(false);
/// L'icône de notification existe. Sans elle, masquer la fenêtre la rendrait introuvable : fermer quitte alors.
static TRAY_OK: AtomicBool = AtomicBool::new(false);

/// Attente maximale de la fin d'un lancement abandonné (préparation, création du processus) avant de rouvrir les lancements.
const SETTLE_MAX: Duration = Duration::from_secs(30 * 60);

/// Pur : Cubelink a-t-il été lancé par le démarrage automatique ?
pub fn started_in_background<I: IntoIterator<Item = S>, S: AsRef<str>>(args: I) -> bool {
    args.into_iter().any(|a| a.as_ref() == BACKGROUND_ARG)
}

/// Pur : faut-il montrer l'avis du premier masquage ? Vrai une seule fois (crée le témoin).
pub fn take_first_notice(root: &Path) -> bool {
    let marker = root.join(NOTICE_MARKER);
    if marker.exists() {
        return false;
    }
    std::fs::write(&marker, b"1").is_ok()
}

/// Montre la fenêtre principale (restaurée et au premier plan).
pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn hide_main(app: &AppHandle, st: &AppState) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }
    st.stage.flush_log();
    if !crate::test_hooks::hooks().test_mode && take_first_notice(&st.paths.root) {
        let _ = app
            .notification()
            .builder()
            .title("Cubelink reste ouvert en arrière-plan")
            .body("Tes amis peuvent toujours te rejoindre. Pour quitter : clic droit sur l'icône Cubelink, puis « Quitter Cubelink ».")
            .show();
    }
}

/// Fermeture de la fenêtre (croix, Alt+F4, barre de titre) : masque au lieu de quitter. Appelée sur `CloseRequested`,
/// après `prevent_close`. En mode test (les E2E ferment l'application par WM_CLOSE) ou sans icône : quitte, comme avant.
pub fn on_close_requested(app: &AppHandle, st: &Arc<AppState>) {
    if crate::test_hooks::hooks().test_mode || !TRAY_OK.load(Ordering::SeqCst) {
        return quit_app(app, st);
    }
    if CLOSING.load(Ordering::SeqCst) {
        return;
    }
    // Rien dans la zone intégrée (ni préparation, ni démarrage, ni jeu) : masquer suffit, sans toucher à l'état de sortie.
    if !st.stage.staged() && !st.quit.spawning() {
        return hide_main(app, st);
    }
    // Un jeu est intégré à la fenêtre, y démarre ou s'y prépare : masquer cacherait aussi le jeu. On l'arrête comme une
    // sortie (abandon du lancement, ou fermeture propre), puis on masque au lieu de quitter.
    CLOSING.store(true, Ordering::SeqCst);
    hide_main(app, st);
    let (st, app) = (st.clone(), app.clone());
    tauri::async_runtime::spawn(async move {
        if st.quit_requested() {
            st.quit_game(Duration::from_secs(60)).await;
        }
        // Le lancement abandonné doit être terminé (plus de création de processus, zone libérée) avant de rouvrir les
        // lancements : sinon il passerait le contrôle de sortie et démarrerait un jeu dans une fenêtre masquée.
        let deadline = tokio::time::Instant::now() + SETTLE_MAX;
        while (st.quit.spawning() || st.stage.staged()) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        st.cancel_quit();
        CLOSING.store(false, Ordering::SeqCst);
        if EXIT_PENDING.load(Ordering::SeqCst) {
            app.exit(0);
        }
    });
}

/// Quitter vraiment (menu de l'icône) : arrête un jeu intégré comme avant, puis sort.
pub fn quit_app(app: &AppHandle, st: &Arc<AppState>) {
    if CLOSING.swap(true, Ordering::SeqCst) {
        // Un masquage arrête déjà le jeu : sortir à sa fin.
        EXIT_PENDING.store(true, Ordering::SeqCst);
        return;
    }
    let (st, app) = (st.clone(), app.clone());
    tauri::async_runtime::spawn(async move {
        if st.quit_requested() {
            st.quit_game(Duration::from_secs(60)).await;
        } else {
            st.stage.flush_log();
        }
        app.exit(0);
    });
}

/// Icône de la zone de notification : clic gauche = ouvrir, menu = Ouvrir / Quitter.
pub fn setup_tray(app: &AppHandle, st: Arc<AppState>) -> tauri::Result<()> {
    let open = MenuItemBuilder::with_id(MENU_OPEN, "Ouvrir Cubelink").build(app)?;
    let quit = MenuItemBuilder::with_id(MENU_QUIT, "Quitter Cubelink").build(app)?;
    let menu = MenuBuilder::new(app).items(&[&open, &quit]).build()?;
    let mut b = TrayIconBuilder::with_id("cubelink")
        .tooltip("Cubelink")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, ev| match ev.id().as_ref() {
            MENU_OPEN => show_main(app),
            MENU_QUIT => quit_app(app, &st),
            _ => {}
        })
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        b = b.icon(icon.clone());
    }
    b.build(app)?;
    TRAY_OK.store(true, Ordering::SeqCst);
    Ok(())
}

/// Comme [`setup_tray`], mais une bibliothèque d'icône absente (Linux sans `libayatana-appindicator`) fait paniquer le
/// crate système au lieu de renvoyer une erreur : rattrapé ici, Cubelink démarre sans icône (fermer quitte alors).
pub fn try_setup_tray(app: &AppHandle, st: Arc<AppState>) {
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| setup_tray(app, st)));
    match res {
        Ok(Ok(())) => {}
        Ok(Err(e)) => eprintln!("[Cubelink] icone de la zone de notification : {e}"),
        Err(_) => eprintln!("[Cubelink] icone de la zone de notification indisponible : fermer la fenetre quittera Cubelink"),
    }
}

/// Aligne le démarrage avec la session sur le réglage `start_with_os`. Jamais en mode test (pas d'entrée laissée sur
/// la machine de test).
pub fn apply_autostart(app: &AppHandle) {
    if crate::test_hooks::hooks().test_mode {
        return;
    }
    use tauri_plugin_autostart::ManagerExt;
    let Some(st) = app.try_state::<Arc<AppState>>() else { return };
    let want = st.settings_get().start_with_os;
    let al = app.autolaunch();
    let res = match (want, al.is_enabled().unwrap_or(false)) {
        (true, false) => al.enable(),
        (false, true) => al.disable(),
        _ => Ok(()),
    };
    if let Err(e) = res {
        eprintln!("[Cubelink] démarrage avec la session : {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argument_de_demarrage_en_fond() {
        assert!(started_in_background(["cubelink.exe", "--background"]));
        assert!(!started_in_background(["cubelink.exe"]));
        assert!(!started_in_background(["cubelink.exe", "--backgroundx"]));
    }

    #[test]
    fn avis_montre_une_seule_fois() {
        let dir = tempfile::tempdir().unwrap();
        assert!(take_first_notice(dir.path()));
        assert!(!take_first_notice(dir.path()));
        // Dossier illisible ou absent : pas d'avis plutôt qu'un avis à chaque fermeture.
        assert!(!take_first_notice(&dir.path().join("absent")));
    }
}
