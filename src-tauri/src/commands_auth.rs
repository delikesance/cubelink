//! Commandes d'authentification Microsoft (code d'appareil) et profil hors ligne de test.

use std::sync::Arc;

use core_lib::auth::device::DeviceCodeClient;
use core_lib::auth::minecraft::MinecraftClient;
use core_lib::auth::offline;
use core_lib::auth::refresh::refresh_tokens;
use core_lib::auth::tokens::{FileKeyProvider, StoredTokens, TokenStore};
use core_lib::auth::xbox::XboxClient;
use core_lib::types::{Error, Profile};
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

/// Identifiant d'application Azure public par défaut (n'est pas un secret).
const DEFAULT_MS_CLIENT_ID: &str = "d5b2e079-2c2c-4ade-9dab-1c1eb204162a";

/// Identifiant d'application Azure : variable d'exécution, sinon valeur de compilation, sinon défaut.
fn ms_client_id() -> Result<String, String> {
    Ok(std::env::var("CUBELINK_MS_CLIENT_ID")
        .ok()
        .or_else(|| option_env!("CUBELINK_MS_CLIENT_ID").map(str::to_string))
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_MS_CLIENT_ID.to_string()))
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

/// Durée maximale d'un renouvellement de session (chaîne Microsoft + Xbox + Minecraft).
const REFRESH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Un seul renouvellement à la fois : le `refresh_token` peut tourner (usage unique) côté Microsoft.
static REFRESH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Échec d'un renouvellement de session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshFailure {
    /// Aucune session Microsoft stockée.
    NoSession,
    /// Échec transitoire (réseau, délai, service indisponible) : la session stockée reste valable pour un autre essai.
    Transient(String),
    /// Jeton révoqué ou expiré : il faut refaire la connexion.
    Revoked(String),
}

impl AppState {
    fn token_store(&self) -> TokenStore<FileKeyProvider> {
        TokenStore::new(&self.paths.tokens_file, FileKeyProvider::new(&self.paths.key_file))
    }

    fn set_profile(&self, p: Option<Profile>) {
        *lock(&self.profile) = p.clone();
        self.emit_event("auth://state", &p);
    }

    /// Renouvelle la session stockée (refresh_token Microsoft puis Xbox, XSTS, Minecraft) et la réenregistre.
    /// Si les jetons sont déjà frais (renouvelés entre-temps), les renvoie tels quels. Messages sans secret.
    pub(crate) async fn refresh_with(&self, ms: &DeviceCodeClient, xbox: &XboxClient, mc: &MinecraftClient) -> Result<StoredTokens, RefreshFailure> {
        let _guard = REFRESH_LOCK.lock().await;
        let old = match self.token_store().load() {
            Ok(Some(t)) => t,
            Ok(None) => return Err(RefreshFailure::NoSession),
            Err(e) => return Err(RefreshFailure::Revoked(user_msg(&e))),
        };
        let now = now_secs();
        if !old.needs_refresh(now, 60) {
            return Ok(old);
        }
        let new = match tokio::time::timeout(REFRESH_TIMEOUT, refresh_tokens(ms, xbox, mc, &old, now)).await {
            Err(_) => return Err(RefreshFailure::Transient("délai dépassé".into())),
            Ok(Err(Error::Network(_))) => return Err(RefreshFailure::Transient("connexion Internet indisponible".into())),
            Ok(Err(e)) => return Err(RefreshFailure::Revoked(user_msg(&e))),
            Ok(Ok(t)) => t,
        };
        self.token_store().save(&new).map_err(|e| RefreshFailure::Transient(user_msg(&e)))?;
        *lock(&self.profile) = Some(new.profile.clone());
        Ok(new)
    }

    async fn refresh_session(&self) -> Result<StoredTokens, RefreshFailure> {
        let ms = ms_client(ms_client_id().map_err(RefreshFailure::Revoked)?);
        self.refresh_with(&ms, &xbox_client(), &mc_client()).await
    }

    /// Profil courant avec renouvellement transparent : mémoire, sinon jetons valides, sinon refresh. `None` seulement
    /// si aucune session n'existe ou si le renouvellement est refusé (reconnexion nécessaire) ; en cas d'échec
    /// transitoire (hors ligne), le profil stocké est renvoyé et le lancement retentera le renouvellement.
    pub async fn auth_status_refresh(&self) -> Option<Profile> {
        if let Some(p) = lock(&self.profile).clone() {
            return Some(p);
        }
        let stored = match self.token_store().load() {
            Ok(Some(t)) => t,
            _ => return None,
        };
        if !stored.needs_refresh(now_secs(), 60) {
            *lock(&self.profile) = Some(stored.profile.clone());
            return Some(stored.profile);
        }
        match self.refresh_session().await {
            Ok(t) => Some(t.profile),
            Err(RefreshFailure::Transient(m)) => {
                eprintln!("[Cubelink] session : renouvellement reporté ({m})");
                Some(stored.profile)
            }
            Err(_) => None,
        }
    }

    /// Profil courant déjà en mémoire, sinon jetons stockés encore valides (sans réseau ; voir `auth_status_refresh`).
    pub fn auth_status(&self) -> Option<Profile> {
        if let Some(p) = lock(&self.profile).clone() {
            return Some(p);
        }
        match self.token_store().load() {
            Ok(Some(t)) if !t.needs_refresh(now_secs(), 60) => {
                *lock(&self.profile) = Some(t.profile.clone());
                Some(t.profile)
            }
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
    pub async fn online_token(&self) -> Result<String, String> {
        if let Ok(Some(t)) = self.token_store().load() {
            if t.minecraft_expires_at > now_secs() + 30 {
                return Ok(t.minecraft_token);
            }
        }
        match self.refresh_session().await {
            Ok(t) => Ok(t.minecraft_token),
            Err(RefreshFailure::Transient(m)) => Err(format!("impossible de renouveler la session Minecraft ({m}) : vérifiez la connexion Internet")),
            Err(_) => Err("session expirée : reconnectez-vous".into()),
        }
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn auth_status(state: State<'_, Arc<AppState>>) -> Result<Option<Profile>, String> {
    Ok(state.auth_status_refresh().await)
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

    // --- Renouvellement de session (serveurs HTTP simulés, aucun vrai jeton) ---
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ID: &str = "069a79f444e94726a5befca90e38aaf5";

    fn expired(now: i64) -> StoredTokens {
        StoredTokens {
            ms_refresh_token: "old-rt".into(),
            ms_expires_at: now - 10,
            minecraft_token: "old-mc".into(),
            minecraft_expires_at: now - 10,
            profile: Profile::Online { name: "Steve".into(), uuid: "069a79f4-44e9-4726-a5be-fca90e38aaf5".into() },
        }
    }

    async fn mount(s: &MockServer, m: &str, p: &str, status: u16, body: serde_json::Value) {
        Mock::given(method(m)).and(path(p)).respond_with(ResponseTemplate::new(status).set_body_json(body)).mount(s).await;
    }

    async fn mount_chain(s: &MockServer) {
        mount(s, "POST", "/user/authenticate", 200, json!({"Token":"xbl","DisplayClaims":{"xui":[{"uhs":"h1"}]}})).await;
        mount(s, "POST", "/xsts/authorize", 200, json!({"Token":"xsts","DisplayClaims":{"xui":[{"uhs":"h2"}]}})).await;
        mount(s, "POST", "/authentication/login_with_xbox", 200, json!({"access_token":"new-mc","expires_in":86400})).await;
        mount(s, "GET", "/entitlements/mcstore", 200, json!({"items":[{"name":"game_minecraft"}]})).await;
        mount(s, "GET", "/minecraft/profile", 200, json!({"id":ID,"name":"Steve"})).await;
    }

    fn clients(s: &MockServer) -> (DeviceCodeClient, XboxClient, MinecraftClient) {
        (DeviceCodeClient::with_base_url("cid", s.uri()), XboxClient::with_base_urls(s.uri(), s.uri()), MinecraftClient::with_base_url(s.uri()))
    }

    #[tokio::test]
    async fn refresh_ok_persiste_le_nouveau_refresh_token() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        st.token_store().save(&expired(now_secs())).unwrap();
        let s = MockServer::start().await;
        mount(&s, "POST", "/token", 200, json!({"access_token":"ms-at","refresh_token":"new-rt","expires_in":3600})).await;
        mount_chain(&s).await;
        let (a, b, c) = clients(&s);
        let t = st.refresh_with(&a, &b, &c).await.unwrap();
        assert_eq!(t.minecraft_token, "new-mc");
        let saved = st.token_store().load().unwrap().unwrap();
        assert_eq!(saved.ms_refresh_token, "new-rt", "nouveau refresh_token réenregistré");
        assert_eq!(saved.minecraft_token, "new-mc");
        assert!(!saved.needs_refresh(now_secs(), 60));
        assert_eq!(st.auth_status(), Some(saved.profile), "profil en mémoire");
        // deuxième appel : jetons frais, aucun nouvel échange (le mock /token répondrait pareil, on vérifie l'état stable)
        assert_eq!(st.refresh_with(&a, &b, &c).await.unwrap().minecraft_token, "new-mc");
    }

    #[tokio::test]
    async fn refresh_refuse_ne_panique_pas_et_laisse_les_jetons_et_le_hors_ligne() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let old = expired(now_secs());
        st.token_store().save(&old).unwrap();
        let s = MockServer::start().await;
        mount(&s, "POST", "/token", 400, json!({"error":"invalid_grant","error_description":"expired"})).await;
        let (a, b, c) = clients(&s);
        let e = st.refresh_with(&a, &b, &c).await.unwrap_err();
        assert!(matches!(&e, RefreshFailure::Revoked(m) if m.contains("reconnectez-vous") && !m.contains("old-rt")), "{e:?}");
        assert_eq!(st.token_store().load().unwrap().unwrap(), old, "jetons inchangés");
        assert!(st.auth_status().is_none());
        // le profil hors ligne (mode test) n'est pas affecté
        let p = st.auth_offline_login("Notch", true).unwrap();
        assert_eq!(st.auth_status_refresh().await, Some(p));
    }

    #[tokio::test]
    async fn refresh_transitoire_et_sans_session() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let s = MockServer::start().await;
        let (a, b, c) = clients(&s);
        assert_eq!(st.refresh_with(&a, &b, &c).await.unwrap_err(), RefreshFailure::NoSession);
        st.token_store().save(&expired(now_secs())).unwrap();
        mount(&s, "POST", "/token", 503, json!({"error":"temporarily_unavailable"})).await;
        assert!(matches!(st.refresh_with(&a, &b, &c).await.unwrap_err(), RefreshFailure::Transient(_)));
        assert_eq!(st.token_store().load().unwrap().unwrap().ms_refresh_token, "old-rt");
    }
}
