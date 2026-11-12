//! Zone de jeu integree au launcher (Windows) : la fenetre native du jeu est adoptee DANS la fenetre du launcher, sous la barre
//! de titre, et un rideau (deux panneaux, barre de chargement) masque les lancements et les changements de modpack. Voir le crate
//! `cubelink-host`. Hors Windows (ou en mode test) tout est inerte : le jeu garde sa propre fenetre.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::State;

use crate::state::{lock, AppState, EventSink};

/// Geometrie de secours (pixels CSS), utilisee seulement tant que l'interface n'a pas publie la sienne (`stage_layout`) :
/// la source de verite est le DOM (`--titlebar-h`, `--panel-w`).
const FALLBACK_TITLEBAR_CSS_PX: f64 = 36.0;
const FALLBACK_PANEL_CSS_PX: f64 = 380.0;
/// Le jeu est « pret » quand il ne dit plus rien depuis ce delai (une fois textures et fenetre la).
pub const READY_QUIET: Duration = Duration::from_millis(2500);
/// Au-dela, on ouvre le rideau quoi qu'il arrive.
pub const READY_MAX: Duration = Duration::from_secs(90);

/// Etat publie a l'interface (`stage://state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
pub struct StageState {
    /// Un jeu est rattache a la fenetre du launcher.
    pub attached: bool,
    /// La zone du jeu est celle qu'on voit (sinon : le launcher).
    pub game: bool,
    /// Le panneau « en jeu » est ouvert (bande de droite du jeu).
    pub panel: bool,
}

#[derive(Default)]
pub struct Stage {
    #[cfg(windows)]
    host: std::sync::OnceLock<cubelink_host::GameHost>,
    sink: std::sync::OnceLock<EventSink>,
    last: Mutex<StageState>,
    /// Derniere geometrie publiee par l'interface.
    layout: Mutex<Option<Layout>>,
}

/// Zone de jeu publiee par l'interface, retenue en MARGES par rapport a la zone cliente (pixels physiques) : un redimensionnement
/// s'applique ainsi tout de suite, sans attendre la publication suivante.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub panel_w: i32,
}

impl Layout {
    /// Pur : marges deduites d'un rectangle `(x, y, w, h)` dans une zone cliente `(cw, ch)`.
    pub fn from_rect(x: i32, y: i32, w: i32, h: i32, panel_w: i32, cw: i32, ch: i32) -> Layout {
        // L'interface arrondit x, y, w, h separement : a une echelle fractionnaire (125 %, 150 %) le bord oppose peut tomber a
        // 1 px du bord de la fenetre. Une marge de 1 px est donc un artefact d'arrondi, pas une marge voulue.
        let snap = |m: i32| if m <= 1 { 0 } else { m };
        Layout { left: x.max(0), top: y.max(0), right: snap(cw - x - w), bottom: snap(ch - y - h), panel_w: panel_w.max(0) }
    }

    /// Pur : rectangle `(x, y, w, h)` de la zone de jeu pour une zone cliente `(cw, ch)`.
    pub fn rect(&self, cw: i32, ch: i32) -> (i32, i32, i32, i32) {
        (self.left, self.top, (cw - self.left - self.right).max(1), (ch - self.top - self.bottom).max(1))
    }
}

impl Stage {
    fn publish(&self) {
        #[cfg(windows)]
        if let Some(h) = self.host.get() {
            let (attached, game) = h.view();
            let now = StageState { attached, game: attached && game, panel: attached && h.panel() };
            let mut last = lock(&self.last);
            if *last != now {
                *last = now;
                if let (Some(sink), Ok(v)) = (self.sink.get(), serde_json::to_value(now)) {
                    sink("stage://state", v);
                }
            }
        }
    }

    pub fn state(&self) -> StageState {
        *lock(&self.last)
    }
}

#[cfg(windows)]
impl Stage {
    /// Cree la zone du jeu dans la fenetre principale. A appeler depuis le fil principal (`setup`).
    pub fn attach(&self, window: &tauri::WebviewWindow, sink: EventSink) {
        let Ok(hwnd) = window.hwnd() else { return };
        match cubelink_host::GameHost::embed(hwnd.0 as isize) {
            Ok(h) => {
                let _ = self.host.set(h);
                let _ = self.sink.set(sink);
                self.relayout(window);
            }
            Err(e) => eprintln!("[Cubelink] zone de jeu integree indisponible : {e}"),
        }
    }

    /// Recale la zone du jeu apres un deplacement, un redimensionnement ou un changement de dpi : marges publiees par l'interface
    /// appliquees a la taille actuelle (a defaut : toute la zone sous la barre de titre).
    pub fn relayout(&self, window: &tauri::WebviewWindow) {
        let (Some(h), Ok(size)) = (self.host.get(), window.inner_size()) else { return };
        let (cw, ch) = (size.width as i32, size.height as i32);
        let published = *lock(&self.layout);
        let (x, y, w, hgt) = match published {
            Some(l) => l.rect(cw, ch),
            None => {
                let tb = (FALLBACK_TITLEBAR_CSS_PX * window.scale_factor().unwrap_or(1.0)).round() as i32;
                (0, tb, cw.max(1), (ch - tb).max(1))
            }
        };
        h.relayout(x, y, w, hgt);
    }

    /// L'interface publie la zone de jeu (rectangle de l'element reserve) et la largeur du panneau, en pixels physiques.
    pub fn set_layout(&self, window: &tauri::WebviewWindow, x: i32, y: i32, w: i32, h: i32, panel_w: i32) {
        let Ok(size) = window.inner_size() else { return };
        *lock(&self.layout) = Some(Layout::from_rect(x, y, w, h, panel_w, size.width as i32, size.height as i32));
        self.relayout(window);
    }

    pub fn enabled(&self) -> bool {
        self.host.get().is_some()
    }

    /// Ouvre/ferme le panneau « en jeu ». A l'ouverture, le focus passe au launcher (Echap le ferme) ; refuse hors du jeu.
    pub fn set_panel(&self, window: &tauri::WebviewWindow, open: bool) -> bool {
        let Some(h) = self.host.get() else { return false };
        let published = lock(&self.layout).map(|l| l.panel_w);
        let pw = match published {
            Some(pw) if pw > 0 => pw,
            _ => (FALLBACK_PANEL_CSS_PX * window.scale_factor().unwrap_or(1.0)).round() as i32,
        };
        let changed = h.set_panel(open, pw);
        if changed {
            if open {
                if let Ok(hwnd) = window.hwnd() {
                    crate::foreground::focus_own_window(hwnd.0 as isize);
                }
                let _ = window.set_focus();
            }
            self.publish();
        }
        changed
    }

    /// Un lancement commence : les panneaux se ferment, le chargement s'affiche.
    pub fn begin(&self, title: &str, mods_dir: &Path, profile_path: &Path) {
        if let Some(h) = self.host.get() {
            h.begin(title, "Lancement", Some(mods_dir), Some(profile_path));
            self.publish();
        }
    }

    pub fn watch(&self, pid: u32) {
        if let Some(h) = self.host.get() {
            h.watch(pid);
            self.publish();
        }
    }

    pub fn feed_line(&self, line: &str) {
        if let Some(h) = self.host.get() {
            h.feed_line(line);
        }
    }

    pub fn is_ready(&self) -> bool {
        self.host.get().is_some_and(|h| h.is_ready(READY_QUIET, READY_MAX))
    }

    pub fn mark_ready(&self, profile_path: &Path) {
        if let Some(h) = self.host.get() {
            h.mark_ready(Some(profile_path));
        }
    }

    pub fn reveal(&self) {
        if let Some(h) = self.host.get() {
            h.reveal();
            self.publish();
        }
    }

    pub fn begin_close(&self, title: &str) {
        if let Some(h) = self.host.get() {
            h.begin_close(title);
            self.publish();
        }
    }

    pub fn request_close(&self) {
        if let Some(h) = self.host.get() {
            h.request_close();
        }
    }

    pub fn cancel_close(&self) {
        if let Some(h) = self.host.get() {
            h.cancel_close();
        }
    }

    pub fn run_finished(&self) {
        if let Some(h) = self.host.get() {
            h.run_finished();
            self.publish();
        }
    }

    pub fn set_view(&self, game: bool) {
        if let Some(h) = self.host.get() {
            h.set_view(game);
            self.publish();
        }
    }
}

#[cfg(not(windows))]
impl Stage {
    pub fn attach(&self, _window: &tauri::WebviewWindow, _sink: EventSink) {}
    pub fn relayout(&self, _window: &tauri::WebviewWindow) {}
    pub fn set_layout(&self, _window: &tauri::WebviewWindow, _x: i32, _y: i32, _w: i32, _h: i32, _panel_w: i32) {}
    pub fn enabled(&self) -> bool {
        false
    }
    pub fn set_panel(&self, _window: &tauri::WebviewWindow, _open: bool) -> bool {
        false
    }
    pub fn begin(&self, _title: &str, _mods_dir: &Path, _profile_path: &Path) {}
    pub fn watch(&self, _pid: u32) {}
    pub fn feed_line(&self, _line: &str) {}
    pub fn is_ready(&self) -> bool {
        false
    }
    pub fn mark_ready(&self, _profile_path: &Path) {}
    pub fn reveal(&self) {}
    pub fn begin_close(&self, _title: &str) {}
    pub fn request_close(&self) {}
    pub fn cancel_close(&self) {}
    pub fn run_finished(&self) {}
    pub fn set_view(&self, _game: bool) {}
}

/// Bascule entre le jeu et le launcher (bouton de la barre de titre), avec l'animation de rideau.
#[tauri::command(rename_all = "snake_case")]
pub fn stage_view(state: State<'_, Arc<AppState>>, game: bool) {
    state.stage.set_view(game);
}

#[tauri::command(rename_all = "snake_case")]
pub fn stage_state(state: State<'_, Arc<AppState>>) -> StageState {
    state.stage.state()
}

/// Zone de jeu mesuree par l'interface (element reserve du DOM), en pixels physiques dans la zone cliente de la fenetre.
#[tauri::command(rename_all = "snake_case")]
pub fn stage_layout(state: State<'_, Arc<AppState>>, window: tauri::WebviewWindow, x: i32, y: i32, w: i32, h: i32, panel_w: i32) {
    state.stage.set_layout(&window, x, y, w, h, panel_w);
}

/// Ouvre (`true`) ou ferme (`false`) le panneau « en jeu » (Echap, bouton Fermer).
#[tauri::command(rename_all = "snake_case")]
pub fn stage_panel(state: State<'_, Arc<AppState>>, window: tauri::WebviewWindow, open: bool) {
    state.stage.set_panel(&window, open);
}

#[cfg(test)]
mod tests {
    use super::Layout;

    #[test]
    fn marges_conservees_au_redimensionnement() {
        // zone sous une barre de titre de 36 px, fenetre 1280 x 720
        let l = Layout::from_rect(0, 36, 1280, 684, 380, 1280, 720);
        assert_eq!(l, Layout { left: 0, top: 36, right: 0, bottom: 0, panel_w: 380 });
        assert_eq!(l.rect(1280, 720), (0, 36, 1280, 684));
        assert_eq!(l.rect(1920, 1080), (0, 36, 1920, 1044), "la zone suit la fenetre sans nouvelle publication");
    }

    #[test]
    fn rectangle_jamais_vide_ni_negatif() {
        let l = Layout::from_rect(-5, -5, 5000, 5000, -3, 800, 600);
        assert_eq!(l, Layout { left: 0, top: 0, right: 0, bottom: 0, panel_w: 0 });
        // echelle 125 % : y = 45, h = 854 dans une zone cliente de 900 px -> 1 px d'arrondi en bas, pas une marge
        assert_eq!(Layout::from_rect(0, 45, 1600, 854, 475, 1600, 900).bottom, 0);
        let l = Layout::from_rect(10, 40, 100, 100, 0, 800, 600);
        assert_eq!(l.rect(20, 20), (10, 40, 1, 1));
    }
}
