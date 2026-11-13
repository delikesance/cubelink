//! Commandes d'authentification : comptes multiples (Microsoft par code d'appareil, hors ligne libres), persistés chiffrés.

use std::sync::Arc;

use core_lib::auth::device::DeviceCodeClient;
use core_lib::auth::minecraft::MinecraftClient;
use core_lib::auth::offline;
use core_lib::auth::refresh::refresh_tokens;
use core_lib::auth::accounts::{AccountEntry, AccountStore, AccountsFile, AccountsState};
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

/// Sérialise les lectures-modifications-écritures du fichier des comptes (jamais tenu pendant un `await`).
static ACCOUNTS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

impl AppState {
    /// Ancien stockage à compte unique (lu uniquement pour la migration).
    fn token_store(&self) -> TokenStore<FileKeyProvider> {
        TokenStore::new(&self.paths.tokens_file, FileKeyProvider::new(&self.paths.key_file))
    }

    fn account_store(&self) -> AccountStore<FileKeyProvider> {
        AccountStore::new(&self.paths.accounts_file, FileKeyProvider::new(&self.paths.key_file))
    }

    /// Charge les comptes ; migre `tokens.bin` (compte unique) vers `accounts.bin` s'il n'y a pas encore de fichier
    /// de comptes, et ne supprime l'ancien fichier qu'après l'écriture réussie. `Err` : fichier illisible.
    fn load_accounts(&self) -> Result<AccountsFile, Error> {
        let store = self.account_store();
        if let Some(f) = store.load()? {
            return Ok(f);
        }
        let legacy = self.token_store();
        match legacy.load() {
            Ok(Some(t)) => {
                let entry = AccountEntry::from_stored(t);
                let file = AccountsFile { active: Some(entry.id.clone()), accounts: vec![entry], ..Default::default() };
                store.save(&file)?;
                let _ = legacy.clear();
                Ok(file)
            }
            _ => Ok(AccountsFile::default()),
        }
    }

    /// Lecture sans réseau ; un fichier illisible équivaut à aucun compte (il sera remplacé à la prochaine écriture).
    fn read_accounts(&self) -> AccountsFile {
        let _g = ACCOUNTS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        self.load_accounts().unwrap_or_else(|e| {
            eprintln!("[Cubelink] comptes illisibles : {}", user_msg(&e));
            AccountsFile::default()
        })
    }

    /// Lecture-modification-écriture atomique des comptes. `f` renvoie `Err` pour annuler (rien n'est écrit).
    fn update_accounts<R>(&self, f: impl FnOnce(&mut AccountsFile) -> Result<R, String>) -> Result<(R, AccountsFile), String> {
        let _g = ACCOUNTS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut file = self.load_accounts().unwrap_or_else(|e| {
            eprintln!("[Cubelink] comptes illisibles, réinitialisés : {}", user_msg(&e));
            AccountsFile::default()
        });
        let r = f(&mut file)?;
        self.account_store().save(&file).map_err(|e| user_msg(&e))?;
        Ok((r, file))
    }

    /// Met le compte actif en mémoire et prévient l'interface (`auth://accounts`, et `auth://state` si le compte actif change).
    fn publish(&self, file: &AccountsFile, active_changed: bool) {
        let p = file.active_profile();
        *lock(&self.profile) = p.clone();
        self.emit_event("auth://accounts", &file.state());
        if active_changed {
            self.emit_event("auth://state", &p);
        }
    }

    /// Renouvelle la session du compte actif (refresh_token Microsoft puis Xbox, XSTS, Minecraft) et la réenregistre.
    /// Si les jetons sont déjà frais (renouvelés entre-temps), les renvoie tels quels. Messages sans secret.
    pub(crate) async fn refresh_with(&self, ms: &DeviceCodeClient, xbox: &XboxClient, mc: &MinecraftClient) -> Result<StoredTokens, RefreshFailure> {
        let _guard = REFRESH_LOCK.lock().await;
        let file = {
            let _g = ACCOUNTS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            self.load_accounts().map_err(|e| RefreshFailure::Revoked(user_msg(&e)))?
        };
        let Some(entry) = file.active_entry() else { return Err(RefreshFailure::NoSession) };
        let Some(old) = entry.to_stored() else { return Err(RefreshFailure::NoSession) };
        let id = entry.id.clone();
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
        // Le compte a pu changer ou disparaître pendant l'appel réseau : on met à jour celui de cet id, s'il existe.
        let (changed, file) = self
            .update_accounts(|f| {
                let Some(slot) = f.accounts.iter_mut().find(|a| a.id == id) else { return Ok(false) };
                let changed = slot.profile != new.profile;
                *slot = AccountEntry { id: id.clone(), ..AccountEntry::from_stored(new.clone()) };
                Ok(changed)
            })
            .map_err(RefreshFailure::Transient)?;
        if file.active.as_deref() == Some(id.as_str()) {
            if changed {
                self.publish(&file, true);
            } else {
                *lock(&self.profile) = Some(new.profile.clone());
            }
        }
        Ok(new)
    }

    async fn refresh_session(&self) -> Result<StoredTokens, RefreshFailure> {
        let ms = ms_client(ms_client_id().map_err(RefreshFailure::Revoked)?);
        self.refresh_with(&ms, &xbox_client(), &mc_client()).await
    }

    /// Profil du compte actif avec renouvellement transparent : mémoire, sinon jetons valides (ou compte hors ligne),
    /// sinon refresh. `None` seulement si aucun compte n'est actif ou si le renouvellement est refusé (reconnexion
    /// nécessaire) ; en cas d'échec transitoire (hors ligne), le profil stocké est renvoyé et le lancement retentera.
    pub async fn auth_status_refresh(&self) -> Option<Profile> {
        if let Some(p) = lock(&self.profile).clone() {
            return Some(p);
        }
        let file = self.read_accounts();
        let entry = file.active_entry()?;
        let Some(stored) = entry.to_stored() else {
            *lock(&self.profile) = Some(entry.profile.clone());
            return Some(entry.profile.clone());
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

    /// Profil actif déjà en mémoire, sinon compte actif hors ligne ou aux jetons encore valides (sans réseau).
    pub fn auth_status(&self) -> Option<Profile> {
        if let Some(p) = lock(&self.profile).clone() {
            return Some(p);
        }
        let file = self.read_accounts();
        let entry = file.active_entry()?;
        let usable = entry.to_stored().is_none_or(|t| !t.needs_refresh(now_secs(), 60));
        if !usable {
            return None;
        }
        *lock(&self.profile) = Some(entry.profile.clone());
        Some(entry.profile.clone())
    }

    /// Liste des comptes et compte actif, sans réseau.
    pub fn auth_accounts(&self) -> AccountsState {
        self.read_accounts().state()
    }

    /// Change de compte actif (sans réseau ; le renouvellement éventuel se fait au besoin).
    pub fn auth_switch(&self, id: &str) -> Result<Profile, String> {
        let (before, file) = self.update_accounts(|f| {
            if f.get(id).is_none() {
                return Err("compte introuvable".to_string());
            }
            let before = f.active.clone();
            f.active = Some(id.to_string());
            Ok(before)
        })?;
        self.publish(&file, before.as_deref() != Some(id));
        file.active_profile().ok_or_else(|| "compte introuvable".to_string())
    }

    /// Retire un compte (sans erreur s'il n'existe pas) ; s'il était actif, le premier restant le devient.
    pub fn auth_remove(&self, id: &str) -> Result<(), String> {
        let ((removed, before), file) = self.update_accounts(|f| {
            let before = f.active.clone();
            Ok((f.remove(id), before))
        })?;
        if removed {
            self.publish(&file, before != file.active);
        }
        Ok(())
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

    /// Termine la connexion Microsoft : ajoute le compte (ou met à jour celui de même UUID) et le rend actif.
    pub async fn auth_poll_login(&self) -> Result<Profile, String> {
        let code = lock(&self.pending_login).clone().ok_or("aucune connexion en cours : demandez d'abord un code")?;
        let ms = ms_client(ms_client_id()?).poll(&code).await.map_err(|e| user_msg(&e))?;
        let xsts = xbox_client().authenticate(&ms.access_token).await.map_err(|e| user_msg(&e))?;
        let session = mc_client().login(&xsts.uhs, &xsts.token).await.map_err(|e| user_msg(&e))?;
        let now = now_secs();
        let entry = AccountEntry::from_stored(StoredTokens {
            ms_refresh_token: ms.refresh_token,
            ms_expires_at: now + ms.expires_in as i64,
            minecraft_token: session.access_token,
            minecraft_expires_at: now + session.expires_in as i64,
            profile: session.profile.clone(),
        });
        let id = entry.id.clone();
        let ((), file) = self.update_accounts(|f| {
            f.upsert(entry);
            f.active = Some(id);
            Ok(())
        })?;
        *lock(&self.pending_login) = None;
        self.publish(&file, true);
        Ok(session.profile)
    }

    /// Retire le compte actif (le premier restant devient actif).
    pub fn auth_logout(&self) -> Result<(), String> {
        *lock(&self.pending_login) = None;
        match self.read_accounts().active {
            Some(id) => self.auth_remove(&id),
            None => {
                self.publish(&AccountsFile::default(), true);
                Ok(())
            }
        }
    }

    /// Ajoute un compte hors ligne libre (même pseudo = même compte) et le rend actif.
    pub fn auth_offline_login(&self, name: &str) -> Result<Profile, String> {
        let p = offline::offline_profile(name).map_err(|e| user_msg(&e))?;
        let entry = AccountEntry::offline(p.clone());
        let ((), file) = self.update_accounts(|f| {
            if f.get(&entry.id).is_none() {
                f.accounts.push(entry.clone());
            }
            f.active = Some(entry.id.clone());
            Ok(())
        })?;
        self.publish(&file, true);
        Ok(p)
    }

    /// Jeton Minecraft valide pour le compte actif (en ligne).
    pub async fn online_token(&self) -> Result<String, String> {
        let file = self.read_accounts();
        let Some(entry) = file.active_entry() else { return Err("connectez-vous avant de lancer le jeu".into()) };
        let Some(t) = entry.to_stored() else {
            return Err("ce compte est hors ligne : connectez-vous avec un compte Microsoft pour jouer en ligne".into());
        };
        if t.minecraft_expires_at > now_secs() + 30 {
            return Ok(t.minecraft_token);
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
pub fn auth_accounts(state: State<'_, Arc<AppState>>) -> Result<AccountsState, String> {
    Ok(state.auth_accounts())
}

/// Tête du skin (PNG 8×8 en data URL) d'un compte Microsoft, `None` si elle est indisponible (hors ligne, pas de
/// skin). Mise en cache pour la session : un seul appel réseau par compte.
#[tauri::command(rename_all = "snake_case")]
pub async fn auth_skin_head(uuid: String) -> Result<Option<String>, String> {
    use base64::Engine;
    use std::collections::HashMap;
    use std::sync::Mutex;
    static CACHE: Mutex<Option<HashMap<String, Option<String>>>> = Mutex::new(None);
    let Some(id) = core_lib::skin::normalize_uuid(&uuid) else { return Ok(None) };
    if let Some(hit) = lock(&CACHE).as_ref().and_then(|m| m.get(&id).cloned()) {
        return Ok(hit);
    }
    let head = match core_lib::skin::head_png(&id).await {
        Ok(png) => Some(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png))),
        // Réseau indisponible : pas mis en cache, on réessaiera.
        Err(Error::Network(_)) => return Ok(None),
        Err(_) => None,
    };
    lock(&CACHE).get_or_insert_with(HashMap::new).insert(id, head.clone());
    Ok(head)
}

/// Tête du skin (data URL PNG 8×8) du joueur au pseudo `name`, pour la liste d'amis. `None` si le pseudo est inconnu
/// de Mojang (compte hors ligne) ou le réseau indisponible. Mise en cache pour la session.
#[tauri::command(rename_all = "snake_case")]
pub async fn skin_head_by_name(name: String) -> Result<Option<String>, String> {
    use base64::Engine;
    use std::collections::HashMap;
    use std::sync::Mutex;
    static CACHE: Mutex<Option<HashMap<String, Option<String>>>> = Mutex::new(None);
    if !core_lib::skin::valid_name(&name) {
        return Ok(None);
    }
    let key = name.to_ascii_lowercase();
    if let Some(hit) = lock(&CACHE).as_ref().and_then(|m| m.get(&key).cloned()) {
        return Ok(hit);
    }
    let head = match core_lib::skin::head_png_by_name(&name).await {
        Ok(png) => Some(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png))),
        Err(Error::Network(_)) => return Ok(None),
        Err(_) => None,
    };
    lock(&CACHE).get_or_insert_with(HashMap::new).insert(key, head.clone());
    Ok(head)
}

#[tauri::command(rename_all = "snake_case")]
pub fn auth_switch(state: State<'_, Arc<AppState>>, id: String) -> Result<Profile, String> {
    state.auth_switch(&id)
}

#[tauri::command(rename_all = "snake_case")]
pub fn auth_remove(state: State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
    state.auth_remove(&id)
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
    state.auth_offline_login(&name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::{test_state, Events};
    use core_lib::auth::accounts::{profile_name, profile_uuid, AccountKind};
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ID: &str = "069a79f444e94726a5befca90e38aaf5";
    const UUID: &str = "069a79f4-44e9-4726-a5be-fca90e38aaf5";

    fn online(name: &str, uuid: &str, rt: &str, expires: i64) -> StoredTokens {
        StoredTokens {
            ms_refresh_token: rt.into(),
            ms_expires_at: expires,
            minecraft_token: format!("mc-{rt}"),
            minecraft_expires_at: expires,
            profile: Profile::Online { name: name.into(), uuid: uuid.into() },
        }
    }

    fn expired(now: i64) -> StoredTokens {
        online("Steve", UUID, "old-rt", now - 10)
    }

    /// Ajoute un compte en ligne comme le fait `auth_poll_login` (sans réseau, sans toucher la mémoire ni les événements).
    fn seed(st: &AppState, t: StoredTokens, make_active: bool) {
        let e = AccountEntry::from_stored(t);
        st.update_accounts(|f| {
            if make_active || f.active.is_none() {
                f.active = Some(e.id.clone());
            }
            f.upsert(e);
            Ok(())
        })
        .unwrap();
    }

    fn entry_of(st: &AppState, id: &str) -> AccountEntry {
        st.account_store().load().unwrap().unwrap().get(id).cloned().unwrap()
    }

    fn names(ev: &Events) -> Vec<String> {
        ev.lock().unwrap().iter().map(|(n, _)| n.clone()).collect()
    }

    #[test]
    fn offline_accepte_sans_drapeau_et_emet() {
        let dir = tempfile::tempdir().unwrap();
        let (st, ev) = test_state(dir.path());
        let p = st.auth_offline_login("Notch").unwrap();
        assert!(matches!(p, Profile::Offline { ref name, .. } if name == "Notch"));
        assert_eq!(st.auth_status(), Some(p.clone()));
        assert_eq!(names(&ev), ["auth://accounts", "auth://state"]);
        assert_eq!(ev.lock().unwrap()[1].1, serde_json::to_value(Some(&p)).unwrap());
        st.auth_logout().unwrap();
        assert!(st.auth_status().is_none());
        assert!(st.auth_accounts().accounts.is_empty());
    }

    #[test]
    fn pseudo_invalide_refuse_sans_effet() {
        let dir = tempfile::tempdir().unwrap();
        let (st, ev) = test_state(dir.path());
        for n in ["x", "", "pseudo trop long !!", "é_é_é", "a-b-c"] {
            assert!(st.auth_offline_login(n).is_err(), "{n}");
        }
        assert!(st.auth_accounts().accounts.is_empty());
        assert!(ev.lock().unwrap().is_empty());
        assert!(!st.paths.accounts_file.exists());
    }

    #[test]
    fn meme_pseudo_meme_compte() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        st.auth_offline_login("Notch").unwrap();
        st.auth_offline_login("Alex").unwrap();
        st.auth_offline_login("Notch").unwrap();
        let s = st.auth_accounts();
        assert_eq!(s.accounts.len(), 2);
        assert_eq!(s.active.as_deref(), Some(core_lib::auth::offline::offline_uuid("Notch").as_str()));
    }

    #[test]
    fn deux_en_ligne_un_hors_ligne_switch_remove_et_persistance() {
        let dir = tempfile::tempdir().unwrap();
        let (st, ev) = test_state(dir.path());
        let far = now_secs() + 100_000;
        seed(&st, online("Alice", "u-a", "rt-a", far), true);
        seed(&st, online("Bob", "u-b", "rt-b", far), true);
        let off = st.auth_offline_login("Crack").unwrap();
        let s = st.auth_accounts();
        assert_eq!(s.accounts.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["Alice", "Bob", "Crack"]);
        assert_eq!(s.accounts[2].kind, AccountKind::Offline);
        assert_eq!(s.active.as_deref(), Some(profile_uuid(&off)));
        assert_eq!(st.auth_status(), Some(off));

        // switch vers un compte en ligne : profil, mémoire (lu par l'onglet amis) et événements
        ev.lock().unwrap().clear();
        let p = st.auth_switch("u-a").unwrap();
        assert_eq!(p, Profile::Online { name: "Alice".into(), uuid: "u-a".into() });
        assert_eq!(st.profile_name().as_deref(), Some("Alice"));
        assert_eq!(names(&ev), ["auth://accounts", "auth://state"]);
        // même compte : seule la liste est republiée
        ev.lock().unwrap().clear();
        st.auth_switch("u-a").unwrap();
        assert_eq!(names(&ev), ["auth://accounts"]);
        assert!(st.auth_switch("inconnu").unwrap_err().contains("introuvable"));

        // « redémarrage » : nouvel état sur le même dossier
        drop(st);
        let (st, ev) = test_state(dir.path());
        assert_eq!(st.auth_accounts().accounts.len(), 3);
        assert_eq!(st.auth_accounts().active.as_deref(), Some("u-a"));
        assert_eq!(st.auth_status(), Some(Profile::Online { name: "Alice".into(), uuid: "u-a".into() }));

        // retrait d'un compte non actif : l'actif ne change pas, un seul événement
        st.auth_remove("u-b").unwrap();
        assert_eq!(st.auth_accounts().active.as_deref(), Some("u-a"));
        assert_eq!(names(&ev), ["auth://accounts"]);
        st.auth_remove("u-b").unwrap();
        assert_eq!(names(&ev).len(), 1, "retrait d'un compte absent : sans effet");

        // retrait de l'actif : bascule sur le premier restant (Crack), puis plus rien
        ev.lock().unwrap().clear();
        st.auth_remove("u-a").unwrap();
        let s = st.auth_accounts();
        assert_eq!(s.accounts.len(), 1);
        assert_eq!(s.active.as_deref(), Some(s.accounts[0].id.as_str()));
        assert_eq!(st.auth_status().map(|p| profile_name(&p).to_string()), Some("Crack".into()));
        assert_eq!(names(&ev), ["auth://accounts", "auth://state"]);
        st.auth_logout().unwrap();
        assert_eq!(st.auth_accounts(), AccountsState::default());
        assert!(st.auth_status().is_none());
        let last = ev.lock().unwrap().last().cloned().unwrap();
        assert_eq!(last, ("auth://state".to_string(), serde_json::Value::Null));
    }

    #[test]
    fn migration_de_tokens_bin() {
        let dir = tempfile::tempdir().unwrap();
        let (st, ev) = test_state(dir.path());
        let far = now_secs() + 100_000;
        let old = online("Steve", UUID, "rt-old", far);
        st.token_store().save(&old).unwrap();
        assert!(st.paths.tokens_file.exists() && !st.paths.accounts_file.exists());

        let s = st.auth_accounts();
        assert_eq!(s.active.as_deref(), Some(UUID));
        assert_eq!(s.accounts.len(), 1);
        assert_eq!(s.accounts[0].kind, AccountKind::Online);
        assert!(st.paths.accounts_file.exists());
        assert!(!st.paths.tokens_file.exists(), "ancien fichier supprimé après écriture réussie");
        assert_eq!(entry_of(&st, UUID).to_stored(), Some(old.clone()));
        assert_eq!(st.auth_status(), Some(old.profile.clone()));
        assert!(ev.lock().unwrap().is_empty(), "lecture seule : aucun événement");
        // idempotent
        assert_eq!(st.auth_accounts(), s);
    }

    #[test]
    fn migration_tokens_illisible_laisse_le_fichier() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        std::fs::write(&st.paths.tokens_file, b"corrompu").unwrap();
        assert!(st.auth_accounts().accounts.is_empty());
        assert!(st.paths.tokens_file.exists());
        assert!(!st.paths.accounts_file.exists());
    }

    #[tokio::test]
    async fn poll_sans_begin() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        assert!(st.auth_poll_login().await.unwrap_err().contains("aucune connexion"));
    }

    #[tokio::test]
    async fn compte_hors_ligne_sans_jeton_en_ligne_mais_lancement_possible() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let p = st.auth_offline_login("Notch").unwrap();
        assert!(st.online_token().await.unwrap_err().contains("hors ligne"));
        assert_eq!(st.auth_status_refresh().await, Some(p), "profil hors ligne utilisable sans réseau");
        st.auth_logout().unwrap();
        assert!(st.online_token().await.unwrap_err().contains("connectez-vous"));
    }

    #[tokio::test]
    async fn online_token_du_compte_actif_seulement() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let far = now_secs() + 100_000;
        seed(&st, online("Alice", "u-a", "rt-a", far), true);
        seed(&st, online("Bob", "u-b", "rt-b", far), false);
        assert_eq!(st.online_token().await.unwrap(), "mc-rt-a");
        st.auth_switch("u-b").unwrap();
        assert_eq!(st.online_token().await.unwrap(), "mc-rt-b");
    }

    // --- Renouvellement de session (serveurs HTTP simulés, aucun vrai jeton) ---

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
        seed(&st, expired(now_secs()), true);
        let s = MockServer::start().await;
        mount(&s, "POST", "/token", 200, json!({"access_token":"ms-at","refresh_token":"new-rt","expires_in":3600})).await;
        mount_chain(&s).await;
        let (a, b, c) = clients(&s);
        let t = st.refresh_with(&a, &b, &c).await.unwrap();
        assert_eq!(t.minecraft_token, "new-mc");
        let saved = entry_of(&st, UUID).to_stored().unwrap();
        assert_eq!(saved.ms_refresh_token, "new-rt", "nouveau refresh_token réenregistré");
        assert_eq!(saved.minecraft_token, "new-mc");
        assert!(!saved.needs_refresh(now_secs(), 60));
        assert_eq!(st.auth_status(), Some(saved.profile), "profil en mémoire");
        // deuxième appel : jetons frais, aucun nouvel échange
        assert_eq!(st.refresh_with(&a, &b, &c).await.unwrap().minecraft_token, "new-mc");
    }

    #[tokio::test]
    async fn refresh_ne_touche_que_le_compte_actif() {
        let dir = tempfile::tempdir().unwrap();
        let (st, ev) = test_state(dir.path());
        let other = online("Bob", "u-b", "rt-b", now_secs() - 10);
        seed(&st, other.clone(), true);
        seed(&st, expired(now_secs()), true); // actif : Steve
        let s = MockServer::start().await;
        mount(&s, "POST", "/token", 200, json!({"access_token":"ms-at","refresh_token":"new-rt","expires_in":3600})).await;
        mount_chain(&s).await;
        let (a, b, c) = clients(&s);
        st.refresh_with(&a, &b, &c).await.unwrap();
        assert_eq!(entry_of(&st, UUID).to_stored().unwrap().ms_refresh_token, "new-rt");
        assert_eq!(entry_of(&st, "u-b").to_stored(), Some(other), "l'autre compte (expiré) reste intact");
        assert_eq!(st.auth_accounts().active.as_deref(), Some(UUID));
        assert!(ev.lock().unwrap().is_empty(), "profil inchangé : aucun événement");
        // basculer sur le compte expiré, puis le renouveler : seul lui change
        st.auth_switch("u-b").unwrap();
        let before = entry_of(&st, UUID);
        st.refresh_with(&a, &b, &c).await.unwrap();
        assert_eq!(entry_of(&st, UUID), before);
        assert_eq!(entry_of(&st, "u-b").to_stored().unwrap().minecraft_token, "new-mc");
    }

    #[tokio::test]
    async fn refresh_refuse_ne_panique_pas_et_laisse_les_jetons_et_le_hors_ligne() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let old = expired(now_secs());
        seed(&st, old.clone(), true);
        let s = MockServer::start().await;
        mount(&s, "POST", "/token", 400, json!({"error":"invalid_grant","error_description":"expired"})).await;
        let (a, b, c) = clients(&s);
        let e = st.refresh_with(&a, &b, &c).await.unwrap_err();
        assert!(matches!(&e, RefreshFailure::Revoked(m) if m.contains("reconnectez-vous") && !m.contains("old-rt")), "{e:?}");
        assert_eq!(entry_of(&st, UUID).to_stored(), Some(old), "jetons inchangés");
        assert!(st.auth_status().is_none());
        // un compte hors ligne reste utilisable ; le compte expiré reste dans la liste
        let p = st.auth_offline_login("Notch").unwrap();
        assert_eq!(st.auth_status_refresh().await, Some(p));
        assert_eq!(st.auth_accounts().accounts.len(), 2);
    }

    #[tokio::test]
    async fn refresh_transitoire_et_sans_session() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let s = MockServer::start().await;
        let (a, b, c) = clients(&s);
        assert_eq!(st.refresh_with(&a, &b, &c).await.unwrap_err(), RefreshFailure::NoSession);
        st.auth_offline_login("Notch").unwrap();
        assert_eq!(st.refresh_with(&a, &b, &c).await.unwrap_err(), RefreshFailure::NoSession, "compte hors ligne : pas de jeton");
        seed(&st, expired(now_secs()), true);
        mount(&s, "POST", "/token", 503, json!({"error":"temporarily_unavailable"})).await;
        assert!(matches!(st.refresh_with(&a, &b, &c).await.unwrap_err(), RefreshFailure::Transient(_)));
        assert_eq!(entry_of(&st, UUID).to_stored().unwrap().ms_refresh_token, "old-rt");
    }
}
