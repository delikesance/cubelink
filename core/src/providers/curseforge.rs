//! Résolution des fichiers d'un modpack CurseForge (`manifest.json`) SANS clé API.
//!
//! L'endpoint web `https://www.curseforge.com/api/v1/mods/{projectID}/files/{fileID}/download`
//! répond par une redirection (3xx) vers `https://edge.forgecdn.net/files/<a>/<b>/<Nom>?api-key=...`.
//! On ne suit pas la redirection automatiquement : on lit `Location` (l'appelant valide https + hôte
//! contre `CDN_HOSTS`), et le nom du fichier vient du chemin de l'URL (décodé).

use std::collections::HashMap;

use serde::Deserialize;

use crate::types::{Error, Result};

pub const API_BASE_URL: &str = "https://api.curseforge.com";
pub const WEB_BASE_URL: &str = "https://www.curseforge.com";
/// Hôtes de téléchargement CurseForge à autoriser pour l'import.
pub const CDN_HOSTS: &[&str] = &["edge.forgecdn.net", "mediafilez.forgecdn.net"];
/// Taille maximale d'un fichier téléchargé sans hash de référence.
pub const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;

/// Cible de téléchargement résolue : URL finale du CDN et nom du fichier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CfDownload {
    pub url: String,
    pub file_name: String,
}

/// Fichier résolu par l'API officielle (résolution groupée).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CfApiFile {
    pub url: String,
    pub file_name: String,
    pub sha1: String,
    pub size: u64,
}

#[derive(Deserialize)]
struct FilesResponse {
    data: Vec<RawFile>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawFile {
    id: u64,
    file_name: String,
    #[serde(default)]
    download_url: Option<String>,
    #[serde(default)]
    file_length: u64,
    #[serde(default)]
    hashes: Vec<RawHash>,
}

#[derive(Deserialize)]
struct RawHash {
    value: String,
    algo: u32,
}

const BATCH: usize = 500;

/// Clé API de développeur (celle de Cubelink, pas de l'utilisateur) : variable d'exécution, sinon compilation.
pub fn developer_key() -> Option<String> {
    std::env::var("CUBELINK_CF_API_KEY")
        .ok()
        .or_else(|| option_env!("CUBELINK_CF_API_KEY").map(str::to_string))
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
}

pub struct CfResolver {
    http: reqwest::Client,
    base_url: String,
    api_base: String,
    api_key: Option<String>,
}

impl Default for CfResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl CfResolver {
    pub fn new() -> Self {
        let mut r = Self::with_base_url(WEB_BASE_URL);
        r.api_key = developer_key();
        r
    }

    /// Active la résolution groupée via l'API officielle (utile surtout aux tests).
    pub fn with_api(mut self, api_base: impl Into<String>, key: impl Into<String>) -> Self {
        self.api_base = api_base.into().trim_end_matches('/').to_string();
        self.api_key = Some(key.into());
        self
    }

    pub fn has_api_key(&self) -> bool {
        self.api_key.is_some()
    }

    /// Résout tous les fichiers en quelques requêtes (500 par lot). Les fichiers inconnus ou sans URL de
    /// distribution (téléchargement tiers désactivé par l'auteur) sont absents du résultat.
    pub async fn resolve_batch(&self, file_ids: &[u64]) -> Result<HashMap<u64, CfApiFile>> {
        let Some(key) = &self.api_key else { return Ok(HashMap::new()) };
        let mut out = HashMap::new();
        for chunk in file_ids.chunks(BATCH) {
            let resp = self
                .http
                .post(format!("{}/v1/mods/files", self.api_base))
                .header("x-api-key", key)
                .header("Accept", "application/json")
                .json(&serde_json::json!({ "fileIds": chunk }))
                .send()
                .await
                .map_err(|e| Error::Network(e.without_url().to_string()))?;
            let status = resp.status().as_u16();
            if !(200..300).contains(&status) {
                return Err(Error::Network(format!("API CurseForge : HTTP {status}")));
            }
            let body: FilesResponse = resp.json().await.map_err(|e| Error::Network(format!("réponse CurseForge invalide : {e}")))?;
            for f in body.data {
                let Some(url) = f.download_url.filter(|u| !u.is_empty()) else { continue };
                let sha1 = f.hashes.iter().find(|h| h.algo == 1).map(|h| h.value.to_ascii_lowercase()).unwrap_or_default();
                out.insert(f.id, CfApiFile { url, file_name: f.file_name, sha1, size: f.file_length });
            }
        }
        Ok(out)
    }

    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent(concat!("cubelink/", env!("CARGO_PKG_VERSION")))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap_or_default(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_base: API_BASE_URL.to_string(),
            api_key: None,
        }
    }

    /// Résout (sans la suivre) la redirection de téléchargement d'un fichier.
    pub async fn resolve(&self, project_id: u64, file_id: u64) -> Result<CfDownload> {
        let endpoint = format!("{}/api/v1/mods/{project_id}/files/{file_id}/download", self.base_url);
        let resp = self.http.get(&endpoint).send().await.map_err(|e| Error::Network(e.without_url().to_string()))?;
        let status = resp.status();
        if !status.is_redirection() {
            return if status.as_u16() == 404 {
                Err(Error::NotFound(format!("fichier CurseForge {project_id}/{file_id} introuvable")))
            } else {
                Err(Error::Network(format!("CurseForge : HTTP {} (projet {project_id}, fichier {file_id})", status.as_u16())))
            };
        }
        let location = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| Error::Network("redirection CurseForge sans en-tête Location".into()))?;
        let base = url::Url::parse(&endpoint).map_err(|e| Error::Invalid(e.to_string()))?;
        let target = base.join(location).map_err(|e| Error::Invalid(format!("Location invalide : {e}")))?;
        let file_name = file_name_from_url(&target)?;
        // La requête `?api-key=...` du CDN est conservée telle quelle : elle fait partie de l'URL signée.
        Ok(CfDownload { url: target.to_string(), file_name })
    }
}

/// Dernier segment du chemin, décodé (pourcentage). Le contrôle de chemin se fait chez l'appelant.
pub fn file_name_from_url(u: &url::Url) -> Result<String> {
    let last = u.path_segments().and_then(|mut s| s.next_back()).unwrap_or("");
    let name = percent_decode(last).ok_or_else(|| Error::Invalid(format!("nom de fichier non UTF-8 : {last:?}")))?;
    if name.is_empty() {
        return Err(Error::Invalid("URL CurseForge sans nom de fichier".into()));
    }
    Ok(name)
}

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let h = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(h, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn batch_resolves_in_one_request_and_skips_restricted() {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/mods/files"))
            .and(wiremock::matchers::header("x-api-key", "k"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data":[
                {"id":1,"fileName":"a.jar","downloadUrl":"https://edge.forgecdn.net/files/0/1/a.jar","fileLength":3,"hashes":[{"value":"AB","algo":1}]},
                {"id":2,"fileName":"b.jar","downloadUrl":null,"fileLength":4,"hashes":[]}
            ]})))
            .expect(1)
            .mount(&s)
            .await;
        let r = CfResolver::with_base_url("http://127.0.0.1:1").with_api(s.uri(), "k");
        let m = r.resolve_batch(&[1, 2]).await.unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[&1].sha1, "ab");
        assert_eq!(m[&1].size, 3);
    }

    #[tokio::test]
    async fn batch_without_key_is_empty() {
        let r = CfResolver::with_base_url("http://127.0.0.1:1");
        assert!(r.resolve_batch(&[1]).await.unwrap().is_empty());
    }

    #[test]
    fn decodes_percent_names() {
        let u = url::Url::parse("https://edge.forgecdn.net/files/1/2/My%20Mod%20%5B1.20%5D+x.jar?api-key=k").unwrap();
        assert_eq!(file_name_from_url(&u).unwrap(), "My Mod [1.20]+x.jar");
        let u = url::Url::parse("https://edge.forgecdn.net/files/1/2/").unwrap();
        assert!(file_name_from_url(&u).is_err());
        let u = url::Url::parse("https://edge.forgecdn.net/files/1/2/a%2F..%2Fb.jar").unwrap();
        assert_eq!(file_name_from_url(&u).unwrap(), "a/../b.jar"); // refusé ensuite par validate_file_path
    }

    #[tokio::test]
    async fn resolves_redirect_without_following() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/mods/5/files/77/download"))
            .respond_with(ResponseTemplate::new(307).insert_header("Location", "https://edge.forgecdn.net/files/0/77/a%20b.jar?api-key=zz"))
            .mount(&s)
            .await;
        let r = CfResolver::with_base_url(s.uri()).resolve(5, 77).await.unwrap();
        assert_eq!(r.file_name, "a b.jar");
        assert_eq!(r.url, "https://edge.forgecdn.net/files/0/77/a%20b.jar?api-key=zz");
    }

    #[tokio::test]
    async fn not_found_and_blocked() {
        let s = MockServer::start().await;
        Mock::given(method("GET")).and(path("/api/v1/mods/1/files/1/download")).respond_with(ResponseTemplate::new(404)).mount(&s).await;
        Mock::given(method("GET")).and(path("/api/v1/mods/1/files/2/download")).respond_with(ResponseTemplate::new(403)).mount(&s).await;
        Mock::given(method("GET")).and(path("/api/v1/mods/1/files/3/download")).respond_with(ResponseTemplate::new(307)).mount(&s).await;
        let r = CfResolver::with_base_url(s.uri());
        assert!(matches!(r.resolve(1, 1).await, Err(Error::NotFound(_))));
        assert!(matches!(r.resolve(1, 2).await, Err(Error::Network(_))));
        assert!(matches!(r.resolve(1, 3).await, Err(Error::Network(_))));
    }
}
