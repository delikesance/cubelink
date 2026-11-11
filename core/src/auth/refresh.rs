//! Renouvellement silencieux de la session : jeton Microsoft (`refresh_token`) puis Xbox Live, XSTS et Minecraft.

use super::device::DeviceCodeClient;
use super::minecraft::MinecraftClient;
use super::tokens::StoredTokens;
use super::xbox::XboxClient;
use crate::types::Result;

/// Rejoue la chaîne d'authentification à partir du `refresh_token` de `old` et renvoie les nouveaux jetons.
/// Si Microsoft ne renvoie pas de nouveau `refresh_token`, l'ancien est conservé. Rien n'est écrit sur le disque ici.
/// `Error::Network` = échec transitoire ; toute autre erreur = il faut se reconnecter (voir `DeviceCodeClient::refresh`).
pub async fn refresh_tokens(
    ms: &DeviceCodeClient,
    xbox: &XboxClient,
    mc: &MinecraftClient,
    old: &StoredTokens,
    now: i64,
) -> Result<StoredTokens> {
    let token = ms.refresh(&old.ms_refresh_token).await?;
    let xsts = xbox.authenticate(&token.access_token).await?;
    let session = mc.login(&xsts.uhs, &xsts.token).await?;
    Ok(StoredTokens {
        ms_refresh_token: if token.refresh_token.is_empty() { old.ms_refresh_token.clone() } else { token.refresh_token },
        ms_expires_at: now + token.expires_in as i64,
        minecraft_token: session.access_token,
        minecraft_expires_at: now + session.expires_in as i64,
        profile: session.profile,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Error, Profile};
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ID: &str = "069a79f444e94726a5befca90e38aaf5";

    fn old() -> StoredTokens {
        StoredTokens {
            ms_refresh_token: "old-rt".into(),
            ms_expires_at: 10,
            minecraft_token: "old-mc".into(),
            minecraft_expires_at: 10,
            profile: Profile::Online { name: "Steve".into(), uuid: "069a79f4-44e9-4726-a5be-fca90e38aaf5".into() },
        }
    }

    async fn mount_chain(s: &MockServer) {
        for (m, p, body) in [
            ("POST", "/user/authenticate", json!({"Token":"xbl","DisplayClaims":{"xui":[{"uhs":"h1"}]}})),
            ("POST", "/xsts/authorize", json!({"Token":"xsts","DisplayClaims":{"xui":[{"uhs":"h2"}]}})),
            ("POST", "/authentication/login_with_xbox", json!({"access_token":"new-mc","expires_in":86400})),
            ("GET", "/entitlements/mcstore", json!({"items":[{"name":"game_minecraft"}]})),
            ("GET", "/minecraft/profile", json!({"id":ID,"name":"Steve"})),
        ] {
            Mock::given(method(m)).and(path(p)).respond_with(ResponseTemplate::new(200).set_body_json(body)).mount(s).await;
        }
    }

    fn clients(s: &MockServer) -> (DeviceCodeClient, XboxClient, MinecraftClient) {
        (DeviceCodeClient::with_base_url("cid", s.uri()), XboxClient::with_base_urls(s.uri(), s.uri()), MinecraftClient::with_base_url(s.uri()))
    }

    async fn token(s: &MockServer, status: u16, body: serde_json::Value) {
        Mock::given(method("POST")).and(path("/token")).respond_with(ResponseTemplate::new(status).set_body_json(body)).mount(s).await;
    }

    #[tokio::test]
    async fn refresh_complet_met_a_jour_tous_les_jetons() {
        let s = MockServer::start().await;
        token(&s, 200, json!({"access_token":"ms-at","refresh_token":"new-rt","expires_in":3600})).await;
        mount_chain(&s).await;
        let (a, b, c) = clients(&s);
        let t = refresh_tokens(&a, &b, &c, &old(), 1000).await.unwrap();
        assert_eq!(t.ms_refresh_token, "new-rt");
        assert_eq!(t.minecraft_token, "new-mc");
        assert_eq!((t.ms_expires_at, t.minecraft_expires_at), (4600, 87400));
        assert!(!t.needs_refresh(1000, 60));
    }

    #[tokio::test]
    async fn refresh_sans_nouveau_refresh_token_garde_l_ancien() {
        let s = MockServer::start().await;
        token(&s, 200, json!({"access_token":"ms-at","expires_in":3600})).await;
        mount_chain(&s).await;
        let (a, b, c) = clients(&s);
        assert_eq!(refresh_tokens(&a, &b, &c, &old(), 0).await.unwrap().ms_refresh_token, "old-rt");
    }

    #[tokio::test]
    async fn refresh_refuse_ne_touche_a_rien_et_renvoie_une_erreur_auth() {
        let s = MockServer::start().await;
        token(&s, 400, json!({"error":"invalid_grant"})).await;
        let (a, b, c) = clients(&s);
        let e = refresh_tokens(&a, &b, &c, &old(), 0).await.unwrap_err();
        assert!(matches!(e, Error::Auth(_)));
    }
}
