//! Tâche A3 : jeton Minecraft, possession du jeu et profil.

use serde::Deserialize;

use crate::types::{Error, Profile, Result};

pub const DEFAULT_BASE_URL: &str = "https://api.minecraftservices.com";

/// Résultat de la connexion Minecraft : profil et jeton d'accès.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinecraftSession {
    pub profile: Profile,
    pub access_token: String,
    pub expires_in: u64,
}

#[derive(Debug, Deserialize)]
struct LoginResponse {
    access_token: String,
    expires_in: u64,
}

#[derive(Debug, Deserialize)]
struct Entitlements {
    #[serde(default)]
    items: Vec<EntitlementItem>,
}

#[derive(Debug, Deserialize)]
struct EntitlementItem {
    name: String,
}

#[derive(Debug, Deserialize)]
struct ProfileResponse {
    id: String,
    name: String,
}

pub struct MinecraftClient {
    http: reqwest::Client,
    base_url: String,
}

impl Default for MinecraftClient {
    fn default() -> Self {
        Self::new()
    }
}

impl MinecraftClient {
    pub fn new() -> Self {
        Self::with_base_url(DEFAULT_BASE_URL)
    }

    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent(concat!("cubelink/", env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap_or_default(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
        }
    }

    /// Enchaîne connexion Xbox, vérification de licence et lecture du profil.
    pub async fn login(&self, uhs: &str, xsts_token: &str) -> Result<MinecraftSession> {
        let login = self.login_with_xbox(uhs, xsts_token).await?;
        self.check_ownership(&login.access_token).await?;
        let profile = self.fetch_profile(&login.access_token).await?;
        Ok(MinecraftSession {
            profile,
            access_token: login.access_token,
            expires_in: login.expires_in,
        })
    }

    async fn send(&self, req: reqwest::RequestBuilder) -> Result<(u16, String)> {
        let resp = req.send().await.map_err(|e| Error::Network(e.to_string()))?;
        let status = resp.status().as_u16();
        let text = resp.text().await.map_err(|e| Error::Network(e.to_string()))?;
        Ok((status, text))
    }

    async fn login_with_xbox(&self, uhs: &str, xsts_token: &str) -> Result<LoginResponse> {
        let body = serde_json::json!({ "identityToken": format!("XBL3.0 x={uhs};{xsts_token}") });
        let (status, text) = self
            .send(
                self.http
                    .post(format!("{}/authentication/login_with_xbox", self.base_url))
                    .header("Content-Type", "application/json")
                    .body(body.to_string()),
            )
            .await?;
        if status == 403 {
            return Err(Error::Auth(
                "connexion Minecraft : jeton refusé (HTTP 403). L'application Azure (CUBELINK_MS_CLIENT_ID) \
                 n'est probablement pas autorisée à utiliser l'API Minecraft : demande l'approbation via le \
                 formulaire Mojang (aka.ms/AppRegInfo) ou utilise un client_id approuvé."
                    .into(),
            ));
        }
        check_status(status, "connexion Minecraft")?;
        serde_json::from_str(&text).map_err(|e| Error::Auth(format!("réponse login_with_xbox invalide : {e}")))
    }

    async fn check_ownership(&self, token: &str) -> Result<()> {
        let (status, text) = self
            .send(
                self.http
                    .get(format!("{}/entitlements/mcstore", self.base_url))
                    .bearer_auth(token),
            )
            .await?;
        check_status(status, "vérification de la licence")?;
        let ent: Entitlements =
            serde_json::from_str(&text).map_err(|e| Error::Auth(format!("réponse entitlements invalide : {e}")))?;
        if ent
            .items
            .iter()
            .any(|i| i.name == "game_minecraft" || i.name == "product_minecraft")
        {
            Ok(())
        } else {
            Err(Error::Auth("ce compte ne possède pas Minecraft Java".into()))
        }
    }

    async fn fetch_profile(&self, token: &str) -> Result<Profile> {
        let (status, text) = self
            .send(
                self.http
                    .get(format!("{}/minecraft/profile", self.base_url))
                    .bearer_auth(token),
            )
            .await?;
        if status == 404 {
            return Err(Error::Auth("aucun profil Minecraft".into()));
        }
        check_status(status, "lecture du profil")?;
        let p: ProfileResponse =
            serde_json::from_str(&text).map_err(|e| Error::Auth(format!("réponse profil invalide : {e}")))?;
        Ok(Profile::Online {
            name: p.name,
            uuid: dashed_uuid(&p.id)?,
        })
    }
}

fn check_status(status: u16, what: &str) -> Result<()> {
    match status {
        200..=299 => Ok(()),
        401 | 403 => Err(Error::Auth(format!("{what} : jeton refusé (HTTP {status})"))),
        _ => Err(Error::Network(format!("{what} : HTTP {status}"))),
    }
}

/// Convertit un UUID de 32 caractères hexadécimaux en forme 8-4-4-4-12.
fn dashed_uuid(id: &str) -> Result<String> {
    let id = id.replace('-', "").to_ascii_lowercase();
    if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::Auth(format!("UUID de profil invalide : {id}")));
    }
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &id[..8],
        &id[8..12],
        &id[12..16],
        &id[16..20],
        &id[20..]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ID: &str = "069a79f444e94726a5befca90e38aaf5";

    async fn mount_login(s: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/authentication/login_with_xbox"))
            .and(body_json(json!({"identityToken":"XBL3.0 x=uhs1;xsts1"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token":"mc-token","expires_in":86400,"token_type":"Bearer"
            })))
            .mount(s)
            .await;
    }

    async fn mount_get(s: &MockServer, p: &str, status: u16, body: serde_json::Value) {
        Mock::given(method("GET"))
            .and(path(p))
            .and(header("authorization", "Bearer mc-token"))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .mount(s)
            .await;
    }

    async fn run(s: &MockServer) -> Result<MinecraftSession> {
        MinecraftClient::with_base_url(s.uri()).login("uhs1", "xsts1").await
    }

    #[tokio::test]
    async fn success() {
        let s = MockServer::start().await;
        mount_login(&s).await;
        mount_get(&s, "/entitlements/mcstore", 200, json!({"items":[{"name":"product_minecraft"},{"name":"game_minecraft"}]})).await;
        mount_get(&s, "/minecraft/profile", 200, json!({"id":ID,"name":"Steve"})).await;
        let r = run(&s).await.unwrap();
        assert_eq!(r.access_token, "mc-token");
        assert_eq!(r.expires_in, 86400);
        assert_eq!(
            r.profile,
            Profile::Online {
                name: "Steve".into(),
                uuid: "069a79f4-44e9-4726-a5be-fca90e38aaf5".into()
            }
        );
    }

    #[tokio::test]
    async fn no_license() {
        let s = MockServer::start().await;
        mount_login(&s).await;
        mount_get(&s, "/entitlements/mcstore", 200, json!({"items":[]})).await;
        let e = run(&s).await.unwrap_err();
        assert!(matches!(e, Error::Auth(ref m) if m.contains("ne possède pas Minecraft Java")));
    }

    #[tokio::test]
    async fn no_profile() {
        let s = MockServer::start().await;
        mount_login(&s).await;
        mount_get(&s, "/entitlements/mcstore", 200, json!({"items":[{"name":"game_minecraft"}]})).await;
        mount_get(&s, "/minecraft/profile", 404, json!({"error":"NOT_FOUND"})).await;
        let e = run(&s).await.unwrap_err();
        assert!(matches!(e, Error::Auth(ref m) if m.contains("aucun profil Minecraft")));
    }

    #[tokio::test]
    async fn token_rejected() {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/authentication/login_with_xbox"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({"error":"UNAUTHORIZED"})))
            .mount(&s)
            .await;
        let e = run(&s).await.unwrap_err();
        assert!(matches!(e, Error::Auth(ref m) if m.contains("refusé")));
    }

    #[tokio::test]
    async fn malformed_response() {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/authentication/login_with_xbox"))
            .respond_with(ResponseTemplate::new(200).set_body_string("pas du json"))
            .mount(&s)
            .await;
        assert!(matches!(run(&s).await, Err(Error::Auth(_))));
    }

    #[tokio::test]
    async fn malformed_profile_uuid() {
        let s = MockServer::start().await;
        mount_login(&s).await;
        mount_get(&s, "/entitlements/mcstore", 200, json!({"items":[{"name":"game_minecraft"}]})).await;
        mount_get(&s, "/minecraft/profile", 200, json!({"id":"zz","name":"Steve"})).await;
        assert!(matches!(run(&s).await, Err(Error::Auth(_))));
    }

    #[test]
    fn dashed_uuid_accepts_dashed_input() {
        assert_eq!(
            dashed_uuid("069a79f4-44e9-4726-a5be-fca90e38aaf5").unwrap(),
            "069a79f4-44e9-4726-a5be-fca90e38aaf5"
        );
    }
}
