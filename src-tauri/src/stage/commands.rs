//! Commandes Tauri de la Scene (`stage_*`, voir `docs/contrats.md`).

use std::sync::Arc;

use tauri::State;

use super::platform::*;
use super::StageState;
use crate::state::AppState;

/// Bascule entre le jeu et le launcher (bouton de la barre de titre), avec l'animation de rideau.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub fn stage_view(state: State<'_, Arc<AppState>>, game: bool) {
    state.stage.set_view(game);
}

/// État de la zone de jeu intégrée (rattachée, jeu visible, panneau ouvert).
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub fn stage_state(state: State<'_, Arc<AppState>>) -> StageState {
    state.stage.state()
}

/// Zone de jeu mesuree par l'interface (element reserve du DOM), en pixels physiques dans la zone cliente de la fenetre.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub fn stage_layout(state: State<'_, Arc<AppState>>, window: tauri::WebviewWindow, x: i32, y: i32, w: i32, h: i32, panel_w: i32) {
    state.stage.set_layout(&window, x, y, w, h, panel_w);
}

/// La zone de jeu integree existe sur cette plateforme (l'interface n'affiche le reglage « Jeu integre » que dans ce cas).
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub fn stage_supported() -> bool {
    #[cfg(target_os = "linux")]
    let x11 = x11_reachable();
    #[cfg(not(target_os = "linux"))]
    let x11 = false;
    stage_supported_on(cfg!(windows), cfg!(target_os = "linux"), x11)
}

/// Ouvre (`true`) ou ferme (`false`) le panneau « en jeu » (Echap, bouton Fermer).
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub fn stage_panel(state: State<'_, Arc<AppState>>, window: tauri::WebviewWindow, open: bool) {
    state.stage.set_panel(&window, open);
}
