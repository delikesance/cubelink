//! Commandes `identity_export` / `identity_import` / `identity_restart` : sauvegarde chiffrée de l'identité
//! (clé `identity.key` + liste d'amis), dépôt d'une restauration et redémarrage pour l'appliquer.
//!
//! Aucun remplacement à chaud : l'identité sert au réseau et à Hearth tant que l'application tourne. L'import
//! dépose la restauration dans `p2p/import-pending/`, `AppState::new` l'applique au démarrage suivant.
//! La phrase de passe et les octets de clé ne sont jamais journalisés.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use p2p_lib::identity_backup::{self, BackupError};
use tauri::{AppHandle, State};

use crate::error::user_msg_any;
use crate::state::{lock, AppState};

const BUSY: &str = "Ferme Minecraft et arrête le partage avant de restaurer une identité.";

fn path_of(raw: &str) -> Result<PathBuf, String> {
    let p = raw.trim();
    if p.is_empty() {
        return Err("Choisis d'abord un fichier.".into());
    }
    Ok(PathBuf::from(p))
}

fn msg(e: BackupError) -> String {
    // Les messages de BackupError sont déjà en français et sans chemin.
    user_msg_any(&e)
}

impl AppState {
    /// Écrit la sauvegarde chiffrée dans `path` ; renvoie la clé publique sauvegardée.
    pub fn identity_export_to(&self, path: &Path, passphrase: &str) -> Result<String, String> {
        identity_backup::export_identity(&self.identity, &self.friends, &self.paths.p2p.join("tmp"), passphrase, path)
            .map_err(msg)
    }

    /// Valide la sauvegarde `path` et la dépose pour le prochain démarrage ; renvoie la clé publique à restaurer.
    pub fn identity_stage_import(&self, path: &Path, passphrase: &str) -> Result<String, String> {
        if lock(&self.game).is_some() || lock(&self.host).is_some() {
            return Err(BUSY.into());
        }
        identity_backup::stage_import(&self.paths.p2p, path, passphrase).map_err(msg)
    }

    /// Refuse le redémarrage s'il n'y a rien à appliquer ou si une partie est en cours.
    pub fn identity_check_restart(&self) -> Result<(), String> {
        if !identity_backup::has_pending_import(&self.paths.p2p) {
            return Err("Aucune restauration en attente.".into());
        }
        if lock(&self.game).is_some() || lock(&self.host).is_some() {
            return Err(BUSY.into());
        }
        Ok(())
    }
}

/// Sauvegarde l'identité et la liste d'amis dans un fichier `.cubelink-id` chiffré. Renvoie la clé publique.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn identity_export(st: State<'_, Arc<AppState>>, path: String, passphrase: String) -> Result<String, String> {
    let st = st.inner().clone();
    let path = path_of(&path)?;
    // Argon2id (64 Mio) : hors du fil asynchrone.
    tokio::task::spawn_blocking(move || st.identity_export_to(&path, &passphrase))
        .await
        .map_err(|_| "La sauvegarde a échoué.".to_string())?
}

/// Valide une sauvegarde et la prépare ; elle s'applique au redémarrage (`identity_restart`). Renvoie la clé publique.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn identity_import(st: State<'_, Arc<AppState>>, path: String, passphrase: String) -> Result<String, String> {
    let st = st.inner().clone();
    let path = path_of(&path)?;
    tokio::task::spawn_blocking(move || st.identity_stage_import(&path, &passphrase))
        .await
        .map_err(|_| "La restauration a échoué.".to_string())?
}

/// Redémarre Cubelink pour appliquer la restauration préparée par `identity_import`.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn identity_restart(app: AppHandle, st: State<'_, Arc<AppState>>) -> Result<(), String> {
    st.identity_check_restart()?;
    // Sous Windows, le nouveau processus ne peut pas renommer `friends.db` tant que celui-ci la tient ouverte : elle
    // est fermée avant le relancement (le nouveau processus réessaie de toute façon si l'ancien met du temps à sortir).
    st.friends.close();
    app.restart();
}
