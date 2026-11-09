//! Commandes d'authentification Microsoft (code d'appareil) et profil hors ligne de test.

use std::sync::Arc;

use core_lib::auth::device::DeviceCodeClient;
use core_lib::auth::minecraft::MinecraftClient;
use core_lib::auth::offline;
use core_lib::auth::tokens::{FileKeyProvider, StoredTokens, TokenStore};
use core_lib::auth::xbox::XboxClient;
use core_lib::types::Profile;
use serde::Serialize;
use tauri::State;

use crate::error::user_msg;
use crate::state::{lock, now_secs, AppState};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DeviceCodeInfo {
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
}

/// Identifiant d'application Azure : variable d'exécution, sinon valeur de compilation.
fn ms_client_id() -> Result<String, String> {
    std::env::var("CUBELINK_MS_CLIENT_ID")
        .ok()
        .or_else(|| option_env!("CUBELINK_MS_CLIENT_ID").map(str::to_string))
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "la connexion Microsoft n'est pas configurée (identifiant d'application manquant)".to_string())
}

fn ms_client(client_id: String) -> DeviceCodeClient {
    match &crate::test_hooks::hooks().auth_ms {
        Some(u) => DeviceCodeClient::with_base_url(client_id, u.clone()),
        None => DeviceCodeClient::new(client_id),
    }
}

fn xbox_client() -> XboxClient {
    let h = crate::test_hooks::hooks();
    if h.auth_xbox.is_none() && h.auth_xsts.is_none() {
        return XboxClient::new();
    }
    XboxClient::with_base_urls(
        h.auth_xbox.clone().unwrap_or_else(|| core_lib::auth::xbox::DEFAULT_USER_URL.to_string()),
        h.auth_xsts.clone().unwrap_or_else(|| core_lib::auth::xbox::DEFAULT_XSTS_URL.to_string()),
    )
}

fn mc_client() -> MinecraftClient {
    match &crate::test_hooks::hooks().auth_mc {
        Some(u) => MinecraftClient::with_base_url(u.clone()),
        None => MinecraftClient::new(),
    }
}

impl AppState {
    fn token_store(&self) -> TokenStore<FileKeyProvider> {
        TokenStore::new(&self.paths.tokens_file, FileKeyProvider::new(&self.paths.key_file))
    }

    fn set_profile(&self, p: Option<Profile>) {
        *lock(&self.profile) = p.clone();
        self.emit_event("auth://state", &p);
    }

    /// Profil courant : mémoire, sinon jetons stockés encore valides.
    pub fn auth_status(&self) -> Option<Profile> {
        if let Some(p) = lock(&self.profile).clone() {
            return Some(p);
        }
        match self.token_store().load() {
            Ok(Some(t)) if !t.needs_refresh(now_secs(), 60) => {
                *lock(&self.profile) = Some(t.profile.clone());
                Some(t.profile)
            }
            // TODO : rafraîchissement via le jeton Microsoft (pas d'API de refresh dans core::auth::device).
            _ => None,
        }
    }

    pub async fn auth_begin_login(&self) -> Result<DeviceCodeInfo, String> {
        let client = ms_client(ms_client_id()?);
        let code = client.begin().await.map_err(|e| user_msg(&e))?;
        let info = DeviceCodeInfo {
            user_code: code.user_code.clone(),
            verification_uri: code.verification_uri.clone(),
            expires_in: code.expires_in,
        };
        *lock(&self.pending_login) = Some(code);
        Ok(info)
    }

    pub async fn auth_poll_login(&self) -> Result<Profile, String> {
        let code = lock(&self.pending_login).clone().ok_or("aucune connexion en cours : demandez d'abord un code")?;
        let ms = ms_client(ms_client_id()?).poll(&code).await.map_err(|e| user_msg(&e))?;
        let xsts = xbox_client().authenticate(&ms.access_token).await.map_err(|e| user_msg(&e))?;
        let session = mc_client().login(&xsts.uhs, &xsts.token).await.map_err(|e| user_msg(&e))?;
        let now = now_secs();
        let stored = StoredTokens {
            ms_refresh_token: ms.refresh_token,
            ms_expires_at: now + ms.expires_in as i64,
            minecraft_token: session.access_token,
            minecraft_expires_at: now + session.expires_in as i64,
            profile: session.profile.clone(),
        };
        self.token_store().save(&stored).map_err(|e| user_msg(&e))?;
        *lock(&self.pending_login) = None;
        self.set_profile(Some(session.profile.clone()));
        Ok(session.profile)
    }

    pub fn auth_logout(&self) -> Result<(), String> {
        self.token_store().clear().map_err(|e| user_msg(&e))?;
        *lock(&self.pending_login) = None;
        self.set_profile(None);
        Ok(())
    }

    /// `enabled` = mode test actif (`CUBELINK_TEST_MODE=1`).
    pub fn auth_offline_login(&self, name: &str, enabled: bool) -> Result<Profile, String> {
        if !enabled {
            return Err("la connexion hors ligne n'est pas disponible".into());
        }
        let p = offline::offline_profile(name).map_err(|e| user_msg(&e))?;
        self.set_profile(Some(p.clone()));
        Ok(p)
    }

    /// Jeton Minecraft valide pour un profil en ligne.
    pub fn online_token(&self) -> Result<String, String> {
        match self.token_store().load() {
            Ok(Some(t)) if t.minecraft_expires_at > now_secs() + 30 => Ok(t.minecraft_token),
            _ => Err("session expirée : reconnectez-vous".into()),
        }
    }
}

#[tauri::command(rename_all = "snake_case")]
pub fn auth_status(state: State<'_, Arc<AppState>>) -> Result<Option<Profile>, String> {
    Ok(state.auth_status())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn auth_begin_login(state: State<'_, Arc<AppState>>) -> Result<DeviceCodeInfo, String> {
    state.auth_begin_login().await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn auth_poll_login(state: State<'_, Arc<AppState>>) -> Result<Profile, String> {
    state.auth_poll_login().await
}

#[tauri::command(rename_all = "snake_case")]
pub fn auth_logout(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    state.auth_logout()
}

#[tauri::command(rename_all = "snake_case")]
pub fn auth_offline_login(state: State<'_, Arc<AppState>>, name: String) -> Result<Profile, String> {
    state.auth_offline_login(&name, offline::test_mode_enabled())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::test_state;

    #[test]
    fn offline_refuse_sans_drapeau() {
        let dir = tempfile::tempdir().unwrap();
        let (st, ev) = test_state(dir.path());
        assert!(st.auth_offline_login("Notch", false).is_err());
        assert!(st.auth_status().is_none());
        assert!(ev.lock().unwrap().is_empty());
    }

    #[test]
    fn offline_accepte_avec_drapeau_et_emet() {
        let dir = tempfile::tempdir().unwrap();
        let (st, ev) = test_state(dir.path());
        let p = st.auth_offline_login("Notch", true).unwrap();
        assert!(matches!(p, Profile::Offline { ref name, .. } if name == "Notch"));
        assert_eq!(st.auth_status(), Some(p));
        assert_eq!(ev.lock().unwrap()[0].0, "auth://state");
        assert!(st.auth_offline_login("x", true).is_err(), "pseudo trop court");
        st.auth_logout().unwrap();
        assert!(st.auth_status().is_none());
    }

    #[tokio::test]
    async fn poll_sans_begin() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        assert!(st.auth_poll_login().await.unwrap_err().contains("aucune connexion"));
    }
}
