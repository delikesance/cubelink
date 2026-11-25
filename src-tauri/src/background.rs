//! Cubelink en fond : fermer la fenêtre la masque (icône dans la zone de notification) au lieu de quitter, pour que les
//! amis restent joignables ; démarrage avec la session (réglage `start_with_os`, lancement masqué) ; une seule instance.
//!
//! Quitter vraiment : menu de l'icône « Quitter Cubelink ». Un jeu de la zone intégrée est alors arrêté comme avant
//! (`quit_requested` / `quit_game`). Fermer la fenêtre pendant qu'un jeu y est intégré l'arrête de la même façon (le
//! masquer cacherait aussi le jeu), puis masque Cubelink au lieu de le quitter.
//!
//! Exceptions, où fermer quitte comme avant : le mode test (les E2E ferment l'application par WM_CLOSE) et l'absence
//! d'icône visible (Linux sans bibliothèque d'icône, ou sans hôte StatusNotifier comme GNOME sans extension
//! AppIndicator : la fenêtre masquée serait introuvable). Dans ce cas le démarrage `--background` ne masque pas non plus.
//! La présence de l'hôte est re-testée à chaque fois (démarrage, fermeture) : une extension activée plus tard est prise
//! en compte à la fermeture suivante.
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

use crate::hearth::StopMode;
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
/// L'icône de notification a été construite. Elle n'est utile que si un hôte la montre (voir [`tray_host_present`]) :
/// sans icône visible, masquer la fenêtre la rendrait introuvable et fermer quitte alors.
static TRAY_BUILT: AtomicBool = AtomicBool::new(false);

/// Noms D-Bus du StatusNotifierWatcher : les extensions GNOME et KDE publient le nom `kde`, la spécification le nom
/// `freedesktop`.
#[cfg(any(target_os = "linux", test))]
const WATCHER_NAMES: [&str; 2] = ["org.kde.StatusNotifierWatcher", "org.freedesktop.StatusNotifierWatcher"];

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
    crate::app_log::flush();
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
    if crate::test_hooks::hooks().test_mode || !tray_ok() {
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
            // Un serveur de groupe encore actif est arrêté proprement (monde publié, bail relâché).
            st.hearth_stop_all_with(stop_mode()).await;
            crate::app_log::flush();
            app.exit(0);
        }
    });
}

/// Pourquoi Cubelink se ferme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCause {
    /// Menu « Quitter » ou fermeture voulue : tout s'arrête sans hâte, le monde du groupe est publié.
    Quit,
    /// La machine ou la session s'arrête (fin de session, SIGTERM) : délais courts, rien n'est publié (§ 7.3).
    Shutdown,
}

impl ExitCause {
    /// Délai laissé au jeu intégré pour se fermer proprement.
    fn game_grace(self) -> Duration {
        match self {
            ExitCause::Quit => Duration::from_secs(60),
            ExitCause::Shutdown => Duration::from_secs(15),
        }
    }
}

/// Un arrêt de la machine a été demandé : même si la fermeture est déjà en cours pour une autre raison, la suite
/// (arrêt des serveurs de groupe) se fait en mode court.
static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

fn stop_mode() -> StopMode {
    if SHUTDOWN_REQUESTED.load(Ordering::SeqCst) {
        StopMode::Shutdown
    } else {
        StopMode::Normal
    }
}

/// Une fermeture est-elle en cours (menu, signal ou fin de session) ?
pub fn closing() -> bool {
    CLOSING.load(Ordering::SeqCst)
}

/// Quitter vraiment (menu de l'icône) : arrête un jeu intégré comme avant, puis sort.
pub fn quit_app(app: &AppHandle, st: &Arc<AppState>) {
    graceful_exit(app, st, ExitCause::Quit);
}

/// Routine de sortie commune : menu « Quitter », mise à jour (`commands_update`), SIGTERM, fin de session. Arrête le jeu
/// intégré, puis les serveurs de groupe, puis sort. Idempotente : un second appel (deuxième signal, menu après un signal)
/// ne relance rien ; un arrêt de la machine demandé pendant une fermeture normale la fait passer en mode court.
pub fn graceful_exit(app: &AppHandle, st: &Arc<AppState>, cause: ExitCause) {
    if cause == ExitCause::Shutdown {
        SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
    }
    if CLOSING.swap(true, Ordering::SeqCst) {
        // Un masquage arrête déjà le jeu : sortir à sa fin.
        EXIT_PENDING.store(true, Ordering::SeqCst);
        return;
    }
    let (st, app) = (st.clone(), app.clone());
    tauri::async_runtime::spawn(async move {
        let stop_game = async {
            if st.quit_requested() {
                st.quit_game(cause.game_grace()).await;
            } else {
                st.stage.flush_log();
            }
        };
        if cause == ExitCause::Shutdown {
            // Le système ne laisse que quelques secondes : le jeu et les serveurs s'arrêtent en même temps, sinon un jeu
            // lent à fermer mangerait le délai du serveur (tué avant d'avoir enregistré).
            tokio::join!(stop_game, st.hearth_stop_all_with(StopMode::Shutdown));
        } else {
            stop_game.await;
            // Un serveur de groupe encore actif est arrêté proprement (monde publié, bail relâché ; arrêt de la machine
            // demandé entre-temps : voir `StopMode::Shutdown`).
            st.hearth_stop_all_with(stop_mode()).await;
        }
        crate::app_log::flush();
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
    TRAY_BUILT.store(true, Ordering::SeqCst);
    if !tray_host_present() {
        tracing::warn!("aucun hote StatusNotifier (GNOME sans extension AppIndicator ?) : fermer la fenetre quittera Cubelink");
    }
    Ok(())
}

/// L'icône de notification existe ET un hôte la montre : la fenêtre masquée peut être rouverte depuis elle. Évalué à
/// chaque appel (démarrage, fermeture), pas figé : un hôte apparu ou disparu depuis est pris en compte.
pub fn tray_ok() -> bool {
    TRAY_BUILT.load(Ordering::SeqCst) && tray_host_present()
}

/// Pur : un hôte de zone de notification est-il présent ? `probe(nom)` renvoie `None` si le nom D-Bus n'a pas de
/// propriétaire, sinon `Some(hôte_enregistré)`. Un watcher sans hôte enregistré (démarrage de KDE, watcher orphelin)
/// n'affiche rien : il compte comme absent.
#[cfg(any(target_os = "linux", test))]
fn watcher_with_host<F: Fn(&str) -> Option<bool>>(probe: F) -> bool {
    WATCHER_NAMES.iter().any(|n| probe(n) == Some(true))
}

/// Sonde réelle d'un nom (bus de session), bornée à 1 s dans un thread à part (jamais bloquant dans un contexte async).
/// Toute erreur (pas de bus de session, refus du bac à sable) vaut « absent » : fermer quitte, ce qui est sûr.
#[cfg(target_os = "linux")]
fn dbus_probe(name: &str) -> Option<bool> {
    let (tx, rx) = std::sync::mpsc::channel();
    let owned = name.to_string();
    std::thread::spawn(move || {
        let _ = tx.send(dbus_probe_blocking(&owned));
    });
    rx.recv_timeout(Duration::from_secs(1)).ok().flatten()
}

#[cfg(target_os = "linux")]
fn dbus_probe_blocking(name: &str) -> Option<bool> {
    let conn = zbus::blocking::Connection::session().ok()?;
    let bus_name = zbus::names::BusName::try_from(name).ok()?;
    if !zbus::blocking::fdo::DBusProxy::new(&conn).ok()?.name_has_owner(bus_name).ok()? {
        return None;
    }
    let proxy = zbus::blocking::Proxy::new(&conn, name.to_string(), "/StatusNotifierWatcher", name.to_string()).ok()?;
    // Propriété illisible alors que le nom est possédé : un watcher ancien, supposé muni d'un hôte.
    Some(proxy.get_property::<bool>("IsStatusNotifierHostRegistered").unwrap_or(true))
}

#[cfg(target_os = "linux")]
fn tray_host_present() -> bool {
    watcher_with_host(dbus_probe)
}

/// Hors Linux (Windows : zone de notification toujours là) : inchangé.
#[cfg(not(target_os = "linux"))]
fn tray_host_present() -> bool {
    true
}

/// Comme [`setup_tray`], mais une bibliothèque d'icône absente (Linux sans `libayatana-appindicator`) fait paniquer le
/// crate système au lieu de renvoyer une erreur : rattrapé ici, Cubelink démarre sans icône (fermer quitte alors).
pub fn try_setup_tray(app: &AppHandle, st: Arc<AppState>) {
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| setup_tray(app, st)));
    match res {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!("icone de la zone de notification : {e}"),
        Err(_) => tracing::warn!("icone de la zone de notification indisponible : fermer la fenetre quittera Cubelink"),
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
        tracing::warn!("démarrage avec la session : {e}");
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

    /// Sonde factice : `(nom, hôte_enregistré)` des noms possédés ; les autres n'ont pas de propriétaire.
    fn probe<'a>(owned: &'a [(&'a str, bool)]) -> impl Fn(&str) -> Option<bool> + 'a {
        move |n| owned.iter().find(|(o, _)| *o == n).map(|(_, h)| *h)
    }

    #[test]
    fn hote_absent_sans_watcher() {
        assert!(!watcher_with_host(probe(&[])));
    }

    #[test]
    fn hote_present_avec_watcher_kde_ou_freedesktop() {
        assert!(watcher_with_host(probe(&[("org.kde.StatusNotifierWatcher", true)])));
        assert!(watcher_with_host(probe(&[("org.freedesktop.StatusNotifierWatcher", true)])));
    }

    #[test]
    fn watcher_sans_hote_enregistre_compte_comme_absent() {
        assert!(!watcher_with_host(probe(&[("org.kde.StatusNotifierWatcher", false)])));
        // Un autre watcher avec hôte suffit.
        assert!(watcher_with_host(probe(&[
            ("org.kde.StatusNotifierWatcher", false),
            ("org.freedesktop.StatusNotifierWatcher", true),
        ])));
    }

    /// Réel : vrai bus de session, faux watcher. Lancé sous `dbus-run-session` (voir la PR) ; ignoré sinon.
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "exige un bus de session D-Bus : dbus-run-session -- cargo test -p cubelink --lib sonde_reelle -- --ignored"]
    fn sonde_reelle_du_bus_de_session() {
        // Sans watcher : absent.
        assert!(!tray_host_present());
        // Faux watcher avec hôte enregistré.
        struct W(bool);
        #[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
        impl W {
            #[zbus(property)]
            fn is_status_notifier_host_registered(&self) -> bool {
                self.0
            }
        }
        for (registered, want) in [(false, false), (true, true)] {
            let conn = zbus::blocking::connection::Builder::session()
                .unwrap()
                .name("org.kde.StatusNotifierWatcher")
                .unwrap()
                .serve_at("/StatusNotifierWatcher", W(registered))
                .unwrap()
                .build()
                .unwrap();
            assert_eq!(tray_host_present(), want, "hôte enregistré = {registered}");
            drop(conn);
        }
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
