//! Tâche P1m : client de l'API Modrinth v2 (recherche, versions, reconnaissance par hash).

use std::collections::HashMap;

use reqwest::{RequestBuilder, Response};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::json;

use crate::types::{Error, Loader, Result};

pub const DEFAULT_BASE_URL: &str = "https://api.modrinth.com/v2";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchFilters {
    pub minecraft_version: Option<String>,
    pub loader: Option<Loader>,
    /// Ex. "mod", "modpack", "resourcepack", "shader".
    pub project_type: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SearchHit {
    pub project_id: String,
    pub slug: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub icon_url: Option<String>,
    #[serde(default)]
    pub author: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SearchResult {
    pub hits: Vec<SearchHit>,
    #[serde(default)]
    pub total_hits: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FileHashes {
    #[serde(default)]
    pub sha1: String,
    #[serde(default)]
    pub sha512: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct VersionFile {
    pub url: String,
    pub filename: String,
    #[serde(default)]
    pub primary: bool,
    #[serde(default)]
    pub size: u64,
    pub hashes: FileHashes,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Dependency {
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub version_id: Option<String>,
    /// "required", "optional", "incompatible" ou "embedded".
    pub dependency_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Version {
    pub id: String,
    pub project_id: String,
    pub version_number: String,
    #[serde(default)]
    pub game_versions: Vec<String>,
    #[serde(default)]
    pub loaders: Vec<String>,
    #[serde(default)]
    pub files: Vec<VersionFile>,
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
}

pub struct ModrinthClient {
    http: reqwest::Client,
    base_url: String,
    user_agent: String,
}

impl ModrinthClient {
    /// `user_agent` : ex. « cubelink/0.1 (contact) », fourni par l'appelant (exigé par Modrinth).
    pub fn new(user_agent: impl Into<String>) -> Self {
        Self::with_base_url(DEFAULT_BASE_URL, user_agent)
    }

    pub fn with_base_url(base_url: impl Into<String>, user_agent: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            user_agent: user_agent.into(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    fn get(&self, path: &str) -> RequestBuilder {
        self.http.get(self.url(path)).header("User-Agent", &self.user_agent)
    }

    async fn send(&self, req: RequestBuilder, what: &str) -> Result<Response> {
        let resp = req.send().await.map_err(|e| Error::Network(e.to_string()))?;
        let status = resp.status();
        match status.as_u16() {
            200..=299 => Ok(resp),
            404 => Err(Error::NotFound(what.to_string())),
            429 => {
                let wait = resp
                    .headers()
                    .get("Retry-After")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| format!("réessayer dans {s} s"))
                    .unwrap_or_else(|| "délai d'attente inconnu".to_string());
                Err(Error::Network(format!("Modrinth limite le débit (HTTP 429) : {wait}")))
            }
            code => Err(Error::Network(format!("HTTP {code} pour {what}"))),
        }
    }

    async fn json<T: DeserializeOwned>(&self, req: RequestBuilder, what: &str) -> Result<T> {
        let resp = self.send(req, what).await?;
        let bytes = resp.bytes().await.map_err(|e| Error::Network(e.to_string()))?;
        serde_json::from_slice(&bytes).map_err(|e| Error::Network(format!("réponse Modrinth invalide ({what}) : {e}")))
    }

    pub async fn search(&self, query: &str, filters: &SearchFilters) -> Result<SearchResult> {
        let mut params: Vec<(&str, String)> = vec![("query", query.to_string())];
        if let Some(f) = facets(filters) {
            params.push(("facets", f));
        }
        if let Some(l) = filters.limit {
            params.push(("limit", l.to_string()));
        }
        if let Some(o) = filters.offset {
            params.push(("offset", o.to_string()));
        }
        self.json(self.get("/search").query(&params), "recherche Modrinth").await
    }

    pub async fn project_versions(
        &self,
        project_id: &str,
        minecraft_version: Option<&str>,
        loader: Option<Loader>,
    ) -> Result<Vec<Version>> {
        let mut params: Vec<(&str, String)> = Vec::new();
        if let Some(v) = minecraft_version {
            params.push(("game_versions", json!([v]).to_string()));
        }
        if let Some(l) = loader.and_then(loader_name) {
            params.push(("loaders", json!([l]).to_string()));
        }
        let path = format!("/project/{}/version", encode_segment(project_id));
        self.json(self.get(&path).query(&params), &format!("projet Modrinth {project_id}")).await
    }

    /// Version correspondant à un sha1 de fichier ; `Ok(None)` si Modrinth ne le connaît pas.
    pub async fn version_from_hash(&self, sha1: &str) -> Result<Option<Version>> {
        let path = format!("/version_file/{}", encode_segment(sha1));
        let req = self.get(&path).query(&[("algorithm", "sha1")]);
        match self.json(req, &format!("hash {sha1}")).await {
            Ok(v) => Ok(Some(v)),
            Err(Error::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Reconnaissance par lot : map hash (sha1) -> version. Les hashes inconnus sont absents.
    pub async fn versions_from_hashes(&self, hashes: &[String]) -> Result<HashMap<String, Version>> {
        if hashes.is_empty() {
            return Ok(HashMap::new());
        }
        let req = self
            .http
            .post(self.url("/version_files"))
            .header("User-Agent", &self.user_agent)
            .json(&json!({"hashes": hashes, "algorithm": "sha1"}));
        self.json(req, "lot de hashes Modrinth").await
    }
}

fn loader_name(l: Loader) -> Option<&'static str> {
    match l {
        Loader::Vanilla => None,
        Loader::NeoForge => Some("neoforge"),
        Loader::Fabric => Some("fabric"),
        Loader::Quilt => Some("quilt"),
        Loader::Forge => Some("forge"),
    }
}

/// Segment de chemin : protège `/`, `?`, `#` et `%`.
fn encode_segment(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `[["versions:1.21.1"],["categories:neoforge"],["project_type:mod"]]` (ET entre groupes).
fn facets(f: &SearchFilters) -> Option<String> {
    let mut groups: Vec<[String; 1]> = Vec::new();
    if let Some(v) = &f.minecraft_version {
        groups.push([format!("versions:{v}")]);
    }
    if let Some(l) = f.loader.and_then(loader_name) {
        groups.push([format!("categories:{l}")]);
    }
    if let Some(t) = &f.project_type {
        groups.push([format!("project_type:{t}")]);
    }
    if groups.is_empty() {
        None
    } else {
        Some(serde_json::to_string(&groups).expect("facets sérialisables"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_json, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const UA: &str = "cubelink/0.1 (test)";

    fn client(s: &MockServer) -> ModrinthClient {
        ModrinthClient::with_base_url(s.uri(), UA)
    }

    fn version_json() -> serde_json::Value {
        json!({
            "id": "v1", "project_id": "p1", "version_number": "1.2.3",
            "game_versions": ["1.21.1"], "loaders": ["neoforge"],
            "files": [{"url":"https://cdn/x.jar","filename":"x.jar","primary":true,"size":42,
                       "hashes":{"sha1":"aa","sha512":"bb"}}],
            "dependencies": [{"project_id":"p2","version_id":null,"dependency_type":"required"}]
        })
    }

    #[tokio::test]
    async fn search_sends_exact_facets_and_parses() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .and(query_param("query", "create"))
            .and(query_param("facets", r#"[["versions:1.21.1"],["categories:neoforge"],["project_type:mod"]]"#))
            .and(query_param("limit", "5"))
            .and(query_param("offset", "10"))
            .and(wiremock::matchers::header("User-Agent", UA))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "hits": [{"project_id":"p1","slug":"create","title":"Create","description":"d",
                          "downloads":9,"icon_url":null,"author":"simibubi"}],
                "total_hits": 1, "offset": 10, "limit": 5
            })))
            .expect(1)
            .mount(&s)
            .await;
        let f = SearchFilters {
            minecraft_version: Some("1.21.1".into()),
            loader: Some(Loader::NeoForge),
            project_type: Some("mod".into()),
            limit: Some(5),
            offset: Some(10),
        };
        let r = client(&s).search("create", &f).await.unwrap();
        assert_eq!(r.total_hits, 1);
        assert_eq!(r.hits[0].slug, "create");
        assert_eq!(r.hits[0].downloads, 9);
        assert!(r.hits[0].icon_url.is_none());
        assert_eq!(r.hits[0].author, "simibubi");
    }

    #[tokio::test]
    async fn project_versions_uses_json_arrays() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/project/p1/version"))
            .and(query_param("game_versions", r#"["1.21.1"]"#))
            .and(query_param("loaders", r#"["neoforge"]"#))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([version_json()])))
            .expect(1)
            .mount(&s)
            .await;
        let v = client(&s).project_versions("p1", Some("1.21.1"), Some(Loader::NeoForge)).await.unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].version_number, "1.2.3");
        assert_eq!(v[0].files[0].hashes.sha512, "bb");
        assert!(v[0].files[0].primary);
        assert_eq!(v[0].dependencies[0].dependency_type, "required");
        assert_eq!(v[0].dependencies[0].version_id, None);
    }

    #[tokio::test]
    async fn project_404_is_not_found() {
        let s = MockServer::start().await;
        let err = client(&s).project_versions("nope", None, None).await.unwrap_err();
        assert!(matches!(err, Error::NotFound(_)), "{err:?}");
    }

    #[tokio::test]
    async fn version_from_hash_found() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/version_file/aa"))
            .and(query_param("algorithm", "sha1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(version_json()))
            .mount(&s)
            .await;
        let v = client(&s).version_from_hash("aa").await.unwrap().unwrap();
        assert_eq!(v.project_id, "p1");
    }

    #[tokio::test]
    async fn version_from_hash_absent_is_none() {
        let s = MockServer::start().await;
        assert_eq!(client(&s).version_from_hash("zz").await.unwrap(), None);
    }

    #[tokio::test]
    async fn versions_from_hashes_batch() {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/version_files"))
            .and(body_json(json!({"hashes": ["aa", "zz"], "algorithm": "sha1"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"aa": version_json()})))
            .expect(1)
            .mount(&s)
            .await;
        let m = client(&s).versions_from_hashes(&["aa".to_string(), "zz".to_string()]).await.unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m["aa"].id, "v1");
        assert!(!m.contains_key("zz"));
        assert!(client(&s).versions_from_hashes(&[]).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn rate_limit_mentions_wait() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "17"))
            .mount(&s)
            .await;
        match client(&s).search("x", &SearchFilters::default()).await.unwrap_err() {
            Error::Network(m) => assert!(m.contains("17"), "{m}"),
            e => panic!("{e:?}"),
        }
    }

    #[tokio::test]
    async fn server_error_is_network() {
        let s = MockServer::start().await;
        Mock::given(method("GET")).and(path("/search")).respond_with(ResponseTemplate::new(500)).mount(&s).await;
        let err = client(&s).search("x", &SearchFilters::default()).await.unwrap_err();
        assert!(matches!(err, Error::Network(_)), "{err:?}");
    }

    #[tokio::test]
    async fn invalid_json_is_network() {
        let s = MockServer::start().await;
        Mock::given(method("GET")).and(path("/search")).respond_with(ResponseTemplate::new(200).set_body_string("{oops")).mount(&s).await;
        let err = client(&s).search("x", &SearchFilters::default()).await.unwrap_err();
        assert!(matches!(err, Error::Network(_)), "{err:?}");
    }

    #[test]
    fn facets_empty_and_vanilla() {
        assert_eq!(facets(&SearchFilters::default()), None);
        let f = SearchFilters { loader: Some(Loader::Vanilla), ..Default::default() };
        assert_eq!(facets(&f), None);
    }
}
