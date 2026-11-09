//! Tâche A2 : échange du jeton Microsoft contre un jeton Xbox Live puis XSTS.

use serde::Deserialize;
use serde_json::json;

use crate::types::{Error, Result};

pub const DEFAULT_USER_URL: &str = "https://user.auth.xboxlive.com";
pub const DEFAULT_XSTS_URL: &str = "https://xsts.auth.xboxlive.com";

/// Jeton XSTS et identifiant utilisateur (`uhs`) pour les services Minecraft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XstsToken {
    pub token: String,
    pub uhs: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct XblResponse {
    token: String,
    display_claims: Option<DisplayClaims>,
}

#[derive(Debug, Deserialize)]
struct DisplayClaims {
    xui: Vec<Xui>,
}

#[derive(Debug, Deserialize)]
struct Xui {
    uhs: String,
}

#[derive(Debug, Deserialize)]
struct XstsError {
    #[serde(rename = "XErr")]
    xerr: Option<u64>,
}

pub struct XboxClient {
    http: reqwest::Client,
    user_url: String,
    xsts_url: String,
}

impl Default for XboxClient {
    fn default() -> Self {
        Self::new()
    }
}

impl XboxClient {
    pub fn new() -> Self {
        Self::with_base_urls(DEFAULT_USER_URL, DEFAULT_XSTS_URL)
    }

    pub fn with_base_urls(user_url: impl Into<String>, xsts_url: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            user_url: user_url.into().trim_end_matches('/').to_string(),
            xsts_url: xsts_url.into().trim_end_matches('/').to_string(),
        }
    }

    async fn post(&self, url: String, body: serde_json::Value) -> Result<(u16, String)> {
        let resp = self
            .http
            .post(url)
            .header("Accept", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Network(e.to_string()))?;
        let status = resp.status().as_u16();
        let text = resp.text().await.map_err(|e| Error::Network(e.to_string()))?;
        Ok((status, text))
    }

    /// Échange le `access_token` Microsoft contre un jeton XSTS Minecraft.
    pub async fn authenticate(&self, ms_access_token: &str) -> Result<XstsToken> {
        let xbl = self.xbl_token(ms_access_token).await?;
        self.xsts_token(&xbl).await
    }

    async fn xbl_token(&self, ms_access_token: &str) -> Result<String> {
        let (status, text) = self
            .post(
                format!("{}/user/authenticate", self.user_url),
                json!({
                    "Properties": {
                        "AuthMethod": "RPS",
                        "SiteName": "user.auth.xboxlive.com",
                        "RpsTicket": format!("d={ms_access_token}"),
                    },
                    "RelyingParty": "http://auth.xboxlive.com",
                    "TokenType": "JWT",
                }),
            )
            .await?;
        if status != 200 {
            return Err(http_error("Xbox Live", status, &text));
        }
        let r: XblResponse = serde_json::from_str(&text)
            .map_err(|e| Error::Auth(format!("réponse Xbox Live invalide : {e}")))?;
        Ok(r.token)
    }

    async fn xsts_token(&self, xbl_token: &str) -> Result<XstsToken> {
        let (status, text) = self
            .post(
                format!("{}/xsts/authorize", self.xsts_url),
                json!({
                    "Properties": { "SandboxId": "RETAIL", "UserTokens": [xbl_token] },
                    "RelyingParty": "rp://api.minecraftservices.com/",
                    "TokenType": "JWT",
                }),
            )
            .await?;
        if status != 200 {
            return Err(http_error("XSTS", status, &text));
        }
        let r: XblResponse = serde_json::from_str(&text)
            .map_err(|e| Error::Auth(format!("réponse XSTS invalide : {e}")))?;
        let uhs = r
            .display_claims
            .and_then(|c| c.xui.into_iter().next())
            .map(|x| x.uhs)
            .ok_or_else(|| Error::Auth("réponse XSTS invalide : uhs absent".into()))?;
        Ok(XstsToken { token: r.token, uhs })
    }
}

fn http_error(service: &str, status: u16, text: &str) -> Error {
    if status == 401 {
        if let Ok(XstsError { xerr: Some(code) }) = serde_json::from_str(text) {
            let msg = match code {
                2148916233 => "ce compte Microsoft n'a pas de compte Xbox : créez-en un sur xbox.com",
                2148916238 => "compte enfant : un adulte doit l'ajouter à un groupe Famille Microsoft",
                2148916235 => "Xbox Live n'est pas disponible dans votre pays ou région",
                _ => return Error::Auth(format!("{service} : refus (XErr {code})")),
            };
            return Error::Auth(msg.into());
        }
        return Error::Auth(format!("{service} : jeton Microsoft refusé (HTTP 401)"));
    }
    if status >= 500 {
        Error::Network(format!("{service} : erreur serveur (HTTP {status})"))
    } else {
        Error::Auth(format!("{service} : HTTP {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client(s: &MockServer) -> XboxClient {
        XboxClient::with_base_urls(s.uri(), s.uri())
    }

    async fn mount_xbl(s: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/user/authenticate"))
            .and(body_partial_json(json!({
                "Properties": {"RpsTicket": "d=mstoken", "AuthMethod": "RPS"},
                "RelyingParty": "http://auth.xboxlive.com", "TokenType": "JWT"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "Token": "xbl", "DisplayClaims": {"xui": [{"uhs": "h1"}]}
            })))
            .mount(s)
            .await;
    }

    async fn mount_xsts(s: &MockServer, status: u16, body: serde_json::Value) {
        Mock::given(method("POST"))
            .and(path("/xsts/authorize"))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .mount(s)
            .await;
    }

    #[tokio::test]
    async fn success() {
        let s = MockServer::start().await;
        mount_xbl(&s).await;
        Mock::given(method("POST"))
            .and(path("/xsts/authorize"))
            .and(body_partial_json(json!({
                "Properties": {"SandboxId": "RETAIL", "UserTokens": ["xbl"]},
                "RelyingParty": "rp://api.minecraftservices.com/"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "Token": "xsts", "DisplayClaims": {"xui": [{"uhs": "h2"}]}
            })))
            .mount(&s)
            .await;
        let t = client(&s).authenticate("mstoken").await.unwrap();
        assert_eq!(t, XstsToken { token: "xsts".into(), uhs: "h2".into() });
    }

    async fn xerr(code: u64) -> Error {
        let s = MockServer::start().await;
        mount_xbl(&s).await;
        mount_xsts(&s, 401, json!({"Identity":"0","XErr":code,"Message":"","Redirect":"x"})).await;
        client(&s).authenticate("mstoken").await.unwrap_err()
    }

    #[tokio::test]
    async fn xerr_no_xbox_account() {
        assert!(matches!(xerr(2148916233).await, Error::Auth(m) if m.contains("pas de compte Xbox")));
    }

    #[tokio::test]
    async fn xerr_child_account() {
        assert!(matches!(xerr(2148916238).await, Error::Auth(m) if m.contains("Famille")));
    }

    #[tokio::test]
    async fn xerr_country() {
        assert!(matches!(xerr(2148916235).await, Error::Auth(m) if m.contains("pays")));
    }

    #[tokio::test]
    async fn xerr_unknown() {
        assert!(matches!(xerr(1).await, Error::Auth(m) if m.contains("XErr 1")));
    }

    #[tokio::test]
    async fn malformed_response() {
        let s = MockServer::start().await;
        mount_xbl(&s).await;
        mount_xsts(&s, 200, json!({"Token": "t"})).await;
        assert!(matches!(client(&s).authenticate("mstoken").await, Err(Error::Auth(_))));
        let s2 = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string("pas du json"))
            .mount(&s2)
            .await;
        assert!(matches!(client(&s2).authenticate("mstoken").await, Err(Error::Auth(_))));
    }

    #[tokio::test]
    async fn server_error_500() {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&s)
            .await;
        assert!(matches!(client(&s).authenticate("mstoken").await, Err(Error::Network(_))));
    }

    #[tokio::test]
    async fn network_error() {
        let c = XboxClient::with_base_urls("http://127.0.0.1:1", "http://127.0.0.1:1");
        assert!(matches!(c.authenticate("x").await, Err(Error::Network(_))));
    }
}
