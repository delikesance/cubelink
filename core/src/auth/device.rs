//! Tâche A1 : flux « device code » Microsoft.

use std::time::Duration;

use serde::Deserialize;

use crate::types::{Error, Result};

pub const DEFAULT_BASE_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0";
pub const SCOPE: &str = "XboxLive.signin offline_access";

/// Réponse de `/devicecode` : à afficher à l'utilisateur.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DeviceCode {
    pub user_code: String,
    pub verification_uri: String,
    pub device_code: String,
    pub expires_in: u64,
    #[serde(default = "default_interval")]
    pub interval: u64,
}

fn default_interval() -> u64 {
    5
}

/// Jeton Microsoft.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct MsToken {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    pub expires_in: u64,
}

#[derive(Debug, Deserialize)]
struct OAuthError {
    error: String,
    #[serde(default)]
    error_description: String,
}

pub struct DeviceCodeClient {
    http: reqwest::Client,
    base_url: String,
    client_id: String,
    /// Multiplicateur de l'intervalle (secondes -> durée). Injectable pour les tests.
    time_unit: Duration,
}

impl DeviceCodeClient {
    pub fn new(client_id: impl Into<String>) -> Self {
        Self::with_base_url(client_id, DEFAULT_BASE_URL)
    }

    pub fn with_base_url(client_id: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            client_id: client_id.into(),
            time_unit: Duration::from_secs(1),
        }
    }

    /// Durée d'une « seconde » d'intervalle (tests : quelques millisecondes).
    pub fn with_time_unit(mut self, unit: Duration) -> Self {
        self.time_unit = unit;
        self
    }

    async fn post_form(&self, path: &str, fields: &[(&str, &str)]) -> Result<(u16, String)> {
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(fields)
            .finish();
        let resp = self
            .http
            .post(format!("{}{}", self.base_url, path))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .map_err(|e| Error::Network(e.to_string()))?;
        let status = resp.status().as_u16();
        let text = resp.text().await.map_err(|e| Error::Network(e.to_string()))?;
        Ok((status, text))
    }

    /// Démarre le flux : renvoie le code à montrer à l'utilisateur.
    pub async fn begin(&self) -> Result<DeviceCode> {
        let (status, text) = self
            .post_form("/devicecode", &[("client_id", &self.client_id), ("scope", SCOPE)])
            .await?;
        if status != 200 {
            return Err(oauth_error(status, &text));
        }
        serde_json::from_str(&text).map_err(|e| Error::Auth(format!("réponse devicecode invalide : {e}")))
    }

    /// Interroge `/token` jusqu'à validation, refus ou expiration.
    pub async fn poll(&self, code: &DeviceCode) -> Result<MsToken> {
        let mut interval = code.interval.max(1);
        let deadline = tokio::time::Instant::now() + self.time_unit * code.expires_in.min(u32::MAX as u64) as u32;
        loop {
            tokio::time::sleep(self.time_unit * interval as u32).await;
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::Auth("code d'appareil expiré".into()));
            }
            let (status, text) = self
                .post_form(
                    "/token",
                    &[
                        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                        ("client_id", &self.client_id),
                        ("device_code", &code.device_code),
                    ],
                )
                .await?;
            if status == 200 {
                return serde_json::from_str(&text)
                    .map_err(|e| Error::Auth(format!("réponse token invalide : {e}")));
            }
            let err: OAuthError = serde_json::from_str(&text)
                .map_err(|_| Error::Network(format!("réponse inattendue (HTTP {status})")))?;
            match err.error.as_str() {
                "authorization_pending" => {}
                "slow_down" => interval += 5,
                "expired_token" | "code_expired" => return Err(Error::Auth("code d'appareil expiré".into())),
                "authorization_declined" | "access_denied" => {
                    return Err(Error::Auth("connexion refusée par l'utilisateur".into()))
                }
                _ => return Err(oauth_error(status, &text)),
            }
        }
    }
}

fn oauth_error(status: u16, text: &str) -> Error {
    match serde_json::from_str::<OAuthError>(text) {
        Ok(e) => Error::Auth(format!("{} : {}", e.error, e.error_description)),
        Err(_) => Error::Network(format!("HTTP {status}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client(server: &MockServer) -> DeviceCodeClient {
        DeviceCodeClient::with_base_url("cid", server.uri()).with_time_unit(Duration::from_millis(10))
    }

    fn code() -> DeviceCode {
        DeviceCode {
            user_code: "ABCD".into(),
            verification_uri: "https://x/link".into(),
            device_code: "dev".into(),
            expires_in: 100,
            interval: 1,
        }
    }

    async fn mount_token(server: &MockServer, body: serde_json::Value, status: u16, times: Option<u64>) {
        let mut m = Mock::given(method("POST")).and(path("/token")).respond_with(ResponseTemplate::new(status).set_body_json(body));
        if let Some(n) = times {
            m = m.up_to_n_times(n);
        }
        m.mount(server).await;
    }

    fn ok_body() -> serde_json::Value {
        json!({"access_token":"at","refresh_token":"rt","expires_in":3600,"token_type":"Bearer"})
    }

    #[tokio::test]
    async fn begin_returns_code() {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/devicecode"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "user_code":"ABCD","device_code":"dev","verification_uri":"https://x/link",
                "expires_in":900,"interval":5,"message":"m"
            })))
            .mount(&s)
            .await;
        let c = client(&s).begin().await.unwrap();
        assert_eq!(c.user_code, "ABCD");
        assert_eq!(c.device_code, "dev");
        assert_eq!(c.interval, 5);
        assert_eq!(c.expires_in, 900);
    }

    #[tokio::test]
    async fn poll_success() {
        let s = MockServer::start().await;
        mount_token(&s, ok_body(), 200, None).await;
        let t = client(&s).poll(&code()).await.unwrap();
        assert_eq!(t.access_token, "at");
        assert_eq!(t.refresh_token, "rt");
        assert_eq!(t.expires_in, 3600);
    }

    #[tokio::test]
    async fn poll_pending_then_success() {
        let s = MockServer::start().await;
        mount_token(&s, json!({"error":"authorization_pending"}), 400, Some(2)).await;
        mount_token(&s, ok_body(), 200, None).await;
        assert_eq!(client(&s).poll(&code()).await.unwrap().access_token, "at");
        assert_eq!(s.received_requests().await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn poll_slow_down_then_success() {
        let s = MockServer::start().await;
        mount_token(&s, json!({"error":"slow_down"}), 400, Some(1)).await;
        mount_token(&s, ok_body(), 200, None).await;
        let start = std::time::Instant::now();
        assert!(client(&s).poll(&code()).await.is_ok());
        // 1 unité + (1 + 5) unités de 10 ms.
        assert!(start.elapsed() >= Duration::from_millis(70));
    }

    #[tokio::test]
    async fn poll_expired_token() {
        let s = MockServer::start().await;
        mount_token(&s, json!({"error":"expired_token"}), 400, None).await;
        assert!(matches!(client(&s).poll(&code()).await, Err(Error::Auth(_))));
    }

    #[tokio::test]
    async fn poll_declined() {
        let s = MockServer::start().await;
        mount_token(&s, json!({"error":"authorization_declined"}), 400, None).await;
        let e = client(&s).poll(&code()).await.unwrap_err();
        assert!(matches!(e, Error::Auth(ref m) if m.contains("refusée")));
    }

    #[tokio::test]
    async fn poll_local_deadline() {
        let s = MockServer::start().await;
        mount_token(&s, json!({"error":"authorization_pending"}), 400, None).await;
        let mut c = code();
        c.expires_in = 5;
        assert!(matches!(client(&s).poll(&c).await, Err(Error::Auth(_))));
    }

    #[tokio::test]
    async fn network_error() {
        let c = DeviceCodeClient::with_base_url("cid", "http://127.0.0.1:1");
        assert!(matches!(c.begin().await, Err(Error::Network(_))));
    }
}
