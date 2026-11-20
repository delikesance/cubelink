//! Lien ami `cubelink://add/<code>` reçu du système (clic sur un lien, `xdg-open`, argument de lancement).
//!
//! Le plugin `deep-link` lit l'argument de lancement (démarrage à froid) et, avec la fonctionnalité `deep-link` de
//! l'instance unique, celui d'un second lancement (Cubelink déjà ouvert, éventuellement en fond). Ici : on garde le
//! lien en attente, on montre la fenêtre et on prévient l'interface (`friend://link`), qui le reprend par
//! `friend_take_pending_link`. Le lien n'est ni validé ni suivi d'un envoi : il préremplit « Ajouter un ami », la
//! demande part sur un geste de l'utilisateur et la comparaison du code de vérification reste obligatoire.

use std::sync::Arc;

use p2p_lib::friend_code::LINK_SCHEME;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_deep_link::DeepLinkExt;

use crate::state::{lock, AppState};

/// Événement « un lien attend » (sans charge utile) : l'interface appelle `friend_take_pending_link`.
pub const EVENT: &str = "friend://link";
/// Un code ami tient en moins de 1 Ko : au-delà, ce n'est pas un lien d'ami (borne aussi ce qu'on garde en mémoire).
const MAX_LINK_LEN: usize = 4096;

/// Pur : le premier candidat qui est un lien `cubelink://…` de taille raisonnable. Son contenu n'est pas jugé ici : un
/// lien mal formé arrive à l'interface, qui le montre dans le champ avec son message d'erreur habituel.
pub fn friend_link<I, S>(candidates: I) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let prefix = format!("{LINK_SCHEME}://");
    candidates
        .into_iter()
        .map(|c| c.as_ref().trim().to_string())
        .find(|c| c.len() <= MAX_LINK_LEN && c.get(..prefix.len()).is_some_and(|p| p.eq_ignore_ascii_case(&prefix)))
}

/// Met le lien en attente, montre la fenêtre (même si Cubelink tourne en fond) et prévient l'interface.
fn deliver(app: &AppHandle, st: &AppState, link: String) {
    *lock(&st.pending_link) = Some(link);
    crate::background::show_main(app);
    let _ = app.emit(EVENT, ());
}

/// Branche la réception des liens et l'enregistrement du schéma. À appeler dans `setup`, une fois l'état géré.
pub fn setup(app: &tauri::App, st: &Arc<AppState>, test_mode: bool) {
    let deep = app.deep_link();
    // Démarrage à froid : le lien est l'argument de lancement. L'interface n'écoute pas encore : elle le reprendra à son montage.
    if let Ok(Some(urls)) = deep.get_current() {
        if let Some(link) = friend_link(urls.iter().map(|u| u.as_str())) {
            *lock(&st.pending_link) = Some(link);
        }
    }
    // Cubelink déjà lancé : l'instance unique transmet l'argument du second lancement au plugin, qui émet ici.
    let (handle, st2) = (app.handle().clone(), st.clone());
    deep.on_open_url(move |e| {
        let urls = e.urls();
        if let Some(link) = friend_link(urls.iter().map(|u| u.as_str())) {
            deliver(&handle, &st2, link);
        }
    });
    if !test_mode {
        register(app.handle());
    }
}

/// Chemin de l'exécutable courant (vide si inconnu).
fn exe_path() -> String {
    std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Paquet MSIX : le lien est déclaré par le manifeste (`uap:Protocol`) ; écrire une clé de registre pointant vers
/// l'exécutable empaqueté (`WindowsApps`) risquerait de masquer l'activation du paquet.
fn is_msix_path(exe: &str) -> bool {
    exe.to_lowercase().contains("\\windowsapps\\")
}

/// Déclare Cubelink auprès du système comme gestionnaire de `cubelink://`. Les paquets (MSI, MSIX, .deb, .rpm) le font
/// déjà à l'installation ; ceci couvre ce qu'ils ne font pas : l'AppImage (son chemin change quand on la déplace, d'où
/// un réenregistrement à chaque démarrage, sur un fil à part car il lance `xdg-mime`) et un MSI posé avant ce correctif.
fn register(app: &AppHandle) {
    let handle = app.clone();
    if cfg!(target_os = "linux") && app.env().appimage.is_some() {
        std::thread::spawn(move || {
            if let Err(e) = handle.deep_link().register(LINK_SCHEME) {
                eprintln!("[Cubelink] enregistrement du lien cubelink:// impossible : {e}");
            }
        });
    } else if cfg!(windows) && !is_msix_path(&exe_path()) && !app.deep_link().is_registered(LINK_SCHEME).unwrap_or(false) {
        if let Err(e) = app.deep_link().register(LINK_SCHEME) {
            eprintln!("[Cubelink] enregistrement du lien cubelink:// impossible : {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_first_cubelink_link() {
        assert_eq!(friend_link(["cubelink.exe", "cubelink://add/mcl1-abcd"]).as_deref(), Some("cubelink://add/mcl1-abcd"));
        assert_eq!(friend_link(["--background", " CubeLink://add/MCL1-ABCD \n"]).as_deref(), Some("CubeLink://add/MCL1-ABCD"));
        assert_eq!(friend_link(["cubelink://add/a", "cubelink://add/b"]).as_deref(), Some("cubelink://add/a"));
    }

    #[test]
    fn ignores_everything_else() {
        let none: [&str; 0] = [];
        assert_eq!(friend_link(none), None);
        for other in ["--background", "https://add/mcl1-abcd", "launcher://add/mcl1-abcd", "cubelink:/add", "cubelink", "mcl1-aaaa-bbbb", "é"] {
            assert_eq!(friend_link([other]), None, "{other}");
        }
    }

    #[test]
    fn msix_path_is_not_registered_by_hand() {
        assert!(is_msix_path(r"C:\Program Files\WindowsApps\Cubelink_1.0_x64__abc\cubelink.exe"));
        assert!(!is_msix_path(r"C:\Program Files\Cubelink\cubelink.exe"));
        assert!(!is_msix_path("/usr/bin/cubelink"));
    }

    #[test]
    fn oversized_link_is_dropped() {
        let big = format!("cubelink://add/{}", "a".repeat(MAX_LINK_LEN));
        assert_eq!(friend_link([big.as_str()]), None);
        let ok = format!("cubelink://add/{}", "a".repeat(MAX_LINK_LEN - 15));
        assert_eq!(friend_link([ok.as_str()]).as_deref(), Some(ok.as_str()));
    }
}
