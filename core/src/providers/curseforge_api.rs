//! Client de l'API officielle CurseForge (recherche, fichiers d'un projet) avec la clé de développeur de Cubelink.
//!
//! Sans clé (`CUBELINK_CF_API_KEY` absente), tout appel échoue avec un message clair : le reste de
//! l'application (import de modpack par redirection sans clé) continue de fonctionner.

use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::providers::curseforge::{developer_key, API_BASE_URL};
use crate::types::{Error, Loader, Result};

/// `gameId` de Minecraft.
pub const GAME_ID: u32 = 432;
pub const CLASS_MODPACK: u32 = 4471;
pub const CLASS_MOD: u32 = 6;
pub const CLASS_RESOURCEPACK: u32 = 12;
pub const CLASS_SHADER: u32 = 6552;
/// `relationType` d'une dépendance obligatoire.
pub const REL_REQUIRED: u32 = 3;

/// `modLoaderType` CurseForge : 1 Forge, 4 Fabric, 5 Quilt, 6 NeoForge.
pub fn loader_type(l: Loader) -> Option<u32> {
    match l {
        Loader::Vanilla => None,
        Loader::Forge => Some(1),
        Loader::Fabric => Some(4),
        Loader::Quilt => Some(5),
        Loader::NeoForge => Some(6),
    }
}

/// Champ de tri : 1 mis en avant (pertinence), 3 dernière mise à jour, 6 téléchargements totaux.
pub fn sort_field(sort: Option<&str>) -> u32 {
    match sort {
        Some("downloads") => 6,
        Some("updated") => 3,
        _ => 1,
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CfAuthor {
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CfLogo {
    #[serde(default)]
    pub thumbnail_url: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CfLinks {
    #[serde(default)]
    pub website_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CfScreenshot {
    #[serde(default)]
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CfMod {
    pub id: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub slug: String,
    #[serde(default)]
    pub links: Option<CfLinks>,
    #[serde(default)]
    pub authors: Vec<CfAuthor>,
    #[serde(default)]
    pub logo: Option<CfLogo>,
    #[serde(default)]
    pub download_count: f64,
    /// `false` : l'auteur interdit la distribution par des applications tierces.
    #[serde(default)]
    pub allow_mod_distribution: Option<bool>,
    #[serde(default)]
    pub screenshots: Vec<CfScreenshot>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CfPagination {
    #[serde(default)]
    pub total_count: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CfSearch {
    pub data: Vec<CfMod>,
    #[serde(default)]
    pub pagination: Option<CfPagination>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CfHash {
    pub value: String,
    pub algo: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CfDependency {
    pub mod_id: u64,
    pub relation_type: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CfFile {
    pub id: u64,
    #[serde(default)]
    pub mod_id: u64,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub file_name: String,
    #[serde(default)]
    pub file_date: String,
    #[serde(default)]
    pub file_length: u64,
    #[serde(default)]
    pub download_url: Option<String>,
    #[serde(default)]
    pub hashes: Vec<CfHash>,
    #[serde(default)]
    pub game_versions: Vec<String>,
    #[serde(default)]
    pub dependencies: Vec<CfDependency>,
    /// Empreinte CurseForge (murmur2) du fichier ; 0 si absente.
    #[serde(default)]
    pub file_fingerprint: u64,
}

/// Correspondance exacte d'une empreinte (`POST /v1/fingerprints/432`).
#[derive(Debug, Clone, Deserialize)]
pub struct CfFingerprintMatch {
    /// Identifiant du projet.
    pub id: u64,
    pub file: CfFile,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfFingerprints {
    #[serde(default)]
    exact_matches: Vec<CfFingerprintMatch>,
}

/// Empreinte CurseForge d'un fichier : MurmurHash2 32 bits, graine 1, calculé après suppression des octets
/// blancs (tabulation, LF, CR, espace) ; la longueur utilisée est celle du contenu filtré.
pub fn murmur2_fingerprint(bytes: &[u8]) -> u32 {
    const M: u32 = 0x5bd1e995;
    let data: Vec<u8> = bytes.iter().copied().filter(|b| !matches!(b, 9 | 10 | 13 | 32)).collect();
    let mut h: u32 = 1 ^ (data.len() as u32);
    let (chunks, tail) = data.as_chunks::<4>();
    for c in chunks {
        let mut k = u32::from_le_bytes(*c);
        k = k.wrapping_mul(M);
        k ^= k >> 24;
        k = k.wrapping_mul(M);
        h = h.wrapping_mul(M);
        h ^= k;
    }
    if !tail.is_empty() {
        if tail.len() >= 3 {
            h ^= u32::from(tail[2]) << 16;
        }
        if tail.len() >= 2 {
            h ^= u32::from(tail[1]) << 8;
        }
        h ^= u32::from(tail[0]);
        h = h.wrapping_mul(M);
    }
    h ^= h >> 13;
    h = h.wrapping_mul(M);
    h ^= h >> 15;
    h
}

impl CfFile {
    pub fn sha1(&self) -> String {
        self.hashes.iter().find(|h| h.algo == 1).map(|h| h.value.to_ascii_lowercase()).unwrap_or_default()
    }

    pub fn url(&self) -> Option<&str> {
        self.download_url.as_deref().filter(|u| !u.is_empty())
    }

    /// Versions de Minecraft (« 1.21.1 »…), sans les loaders ni « Client » / « Server ».
    pub fn minecraft_versions(&self) -> Vec<String> {
        self.game_versions.iter().filter(|v| v.starts_with(|c: char| c.is_ascii_digit())).cloned().collect()
    }

    /// Loaders en minuscules (fabric, forge, neoforge, quilt).
    pub fn loaders(&self) -> Vec<String> {
        self.game_versions
            .iter()
            .map(|v| v.to_ascii_lowercase())
            .filter(|v| matches!(v.as_str(), "fabric" | "forge" | "neoforge" | "quilt"))
            .collect()
    }
}

#[derive(Deserialize)]
struct Data<T> {
    data: T,
}

pub struct CfApi {
    http: reqwest::Client,
    base: String,
    key: Option<String>,
}

impl Default for CfApi {
    fn default() -> Self {
        Self::new()
    }
}

impl CfApi {
    /// API de production avec la clé de développeur (variable d'exécution ou de compilation).
    pub fn new() -> Self {
        let mut c = Self::with_base(API_BASE_URL, "");
        c.key = developer_key();
        c
    }

    /// Base personnalisée (tests) et clé donnée. Une clé vide équivaut à « pas de clé ».
    pub fn with_base(base: impl Into<String>, key: impl Into<String>) -> Self {
        let key: String = key.into();
        Self {
            http: reqwest::Client::builder()
                .user_agent(concat!("cubelink/", env!("CARGO_PKG_VERSION")))
                .connect_timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_default(),
            base: base.into().trim_end_matches('/').to_string(),
            key: Some(key).filter(|k| !k.is_empty()),
        }
    }

    pub fn has_key(&self) -> bool {
        self.key.is_some()
    }

    async fn get<T: DeserializeOwned>(&self, path: &str, params: &[(&str, String)]) -> Result<T> {
        let Some(key) = &self.key else {
            return Err(Error::Invalid("CurseForge n'est pas disponible : clé API de Cubelink absente".into()));
        };
        let resp = self
            .http
            .get(format!("{}{path}", self.base))
            .header("x-api-key", key)
            .header("Accept", "application/json")
            .query(params)
            .send()
            .await
            .map_err(|e| Error::Network(e.without_url().to_string()))?;
        match resp.status().as_u16() {
            200..=299 => {}
            404 => return Err(Error::NotFound("projet ou fichier CurseForge introuvable".into())),
            401 | 403 => return Err(Error::Network("CurseForge a refusé la clé API (HTTP 403)".into())),
            429 => return Err(Error::Network("CurseForge limite le débit (HTTP 429), réessaie dans un instant".into())),
            c => return Err(Error::Network(format!("API CurseForge : HTTP {c}"))),
        }
        let bytes = resp.bytes().await.map_err(|e| Error::Network(e.without_url().to_string()))?;
        serde_json::from_slice(&bytes).map_err(|e| Error::Network(format!("réponse CurseForge invalide : {e}")))
    }

    async fn post<T: DeserializeOwned>(&self, path: &str, body: &serde_json::Value) -> Result<T> {
        let Some(key) = &self.key else {
            return Err(Error::Invalid("CurseForge n'est pas disponible : clé API de Cubelink absente".into()));
        };
        let resp = self
            .http
            .post(format!("{}{path}", self.base))
            .header("x-api-key", key)
            .header("Accept", "application/json")
            .json(body)
            .send()
            .await
            .map_err(|e| Error::Network(e.without_url().to_string()))?;
        match resp.status().as_u16() {
            200..=299 => {}
            404 => return Err(Error::NotFound("projet ou fichier CurseForge introuvable".into())),
            401 | 403 => return Err(Error::Network("CurseForge a refusé la clé API (HTTP 403)".into())),
            429 => return Err(Error::Network("CurseForge limite le débit (HTTP 429), réessaie dans un instant".into())),
            c => return Err(Error::Network(format!("API CurseForge : HTTP {c}"))),
        }
        let bytes = resp.bytes().await.map_err(|e| Error::Network(e.without_url().to_string()))?;
        serde_json::from_slice(&bytes).map_err(|e| Error::Network(format!("réponse CurseForge invalide : {e}")))
    }

    /// Reconnaissance par lot d'empreintes (`murmur2_fingerprint`) : seules les empreintes connues sont renvoyées.
    pub async fn fingerprints(&self, fps: &[u32]) -> Result<Vec<CfFingerprintMatch>> {
        if fps.is_empty() {
            return Ok(Vec::new());
        }
        let r: Data<CfFingerprints> = self.post(&format!("/v1/fingerprints/{GAME_ID}"), &serde_json::json!({ "fingerprints": fps })).await?;
        Ok(r.data.exact_matches)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn search(
        &self,
        class_id: u32,
        query: &str,
        game_version: Option<&str>,
        loader: Option<u32>,
        sort_field: u32,
        index: u32,
        page_size: u32,
    ) -> Result<CfSearch> {
        let mut p: Vec<(&str, String)> = vec![
            ("gameId", GAME_ID.to_string()),
            ("classId", class_id.to_string()),
            ("searchFilter", query.to_string()),
            ("sortField", sort_field.to_string()),
            ("sortOrder", "desc".into()),
            ("index", index.to_string()),
            ("pageSize", page_size.to_string()),
        ];
        if let Some(v) = game_version {
            p.push(("gameVersion", v.to_string()));
        }
        if let Some(l) = loader {
            p.push(("modLoaderType", l.to_string()));
        }
        self.get("/v1/mods/search", &p).await
    }

    /// Fichiers d'un projet (première page de 50), du plus récent au plus ancien.
    pub async fn mod_files(&self, mod_id: u64, game_version: Option<&str>, loader: Option<u32>) -> Result<Vec<CfFile>> {
        let mut p: Vec<(&str, String)> = vec![("pageSize", "50".into())];
        if let Some(v) = game_version {
            p.push(("gameVersion", v.to_string()));
        }
        if let Some(l) = loader {
            p.push(("modLoaderType", l.to_string()));
        }
        let mut files: Vec<CfFile> = self.get::<Data<Vec<CfFile>>>(&format!("/v1/mods/{mod_id}/files"), &p).await?.data;
        files.sort_by(|a, b| b.file_date.cmp(&a.file_date).then(b.id.cmp(&a.id)));
        Ok(files)
    }

    /// Fiche complète d'un projet (logo, captures, résumé, auteurs).
    pub async fn mod_info(&self, mod_id: u64) -> Result<CfMod> {
        Ok(self.get::<Data<CfMod>>(&format!("/v1/mods/{mod_id}"), &[]).await?.data)
    }

    /// Description longue d'un projet (HTML).
    pub async fn mod_description(&self, mod_id: u64) -> Result<String> {
        Ok(self.get::<Data<String>>(&format!("/v1/mods/{mod_id}/description"), &[]).await?.data)
    }

    pub async fn file(&self, mod_id: u64, file_id: u64) -> Result<CfFile> {
        Ok(self.get::<Data<CfFile>>(&format!("/v1/mods/{mod_id}/files/{file_id}"), &[]).await?.data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn search_sends_filters_and_parses() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/mods/search"))
            .and(header("x-api-key", "k"))
            .and(query_param("gameId", "432"))
            .and(query_param("classId", "6"))
            .and(query_param("searchFilter", "jei"))
            .and(query_param("gameVersion", "1.20.1"))
            .and(query_param("modLoaderType", "4"))
            .and(query_param("sortField", "6"))
            .and(query_param("sortOrder", "desc"))
            .and(query_param("index", "20"))
            .and(query_param("pageSize", "10"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{"id": 5, "name": "JEI", "summary": "s", "slug": "jei",
                          "links": {"websiteUrl": "https://www.curseforge.com/minecraft/mc-mods/jei"},
                          "authors": [{"name": "mezz"}], "logo": {"thumbnailUrl": "https://media.forgecdn.net/t.png"},
                          "downloadCount": 12345.0, "allowModDistribution": false}],
                "pagination": {"index": 20, "pageSize": 10, "resultCount": 1, "totalCount": 99}
            })))
            .expect(1)
            .mount(&s)
            .await;
        let c = CfApi::with_base(s.uri(), "k");
        let r = c.search(CLASS_MOD, "jei", Some("1.20.1"), Some(4), sort_field(Some("downloads")), 20, 10).await.unwrap();
        assert_eq!(r.data[0].id, 5);
        assert_eq!(r.data[0].authors[0].name, "mezz");
        assert_eq!(r.data[0].allow_mod_distribution, Some(false));
        assert_eq!(r.pagination.unwrap().total_count, 99);
    }

    #[tokio::test]
    async fn files_sorted_newest_first_and_helpers() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/mods/7/files"))
            .and(query_param("gameVersion", "1.20.1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": [
                {"id": 1, "modId": 7, "displayName": "old", "fileName": "a-1.jar", "fileDate": "2023-01-01T00:00:00Z", "downloadUrl": null,
                 "gameVersions": ["1.20.1", "Fabric", "Client"], "hashes": []},
                {"id": 2, "modId": 7, "displayName": "new", "fileName": "a-2.jar", "fileDate": "2024-01-01T00:00:00Z",
                 "downloadUrl": "https://edge.forgecdn.net/files/1/2/a-2.jar", "fileLength": 9,
                 "gameVersions": ["1.20.1", "NeoForge"], "hashes": [{"value": "AB", "algo": 1}, {"value": "cd", "algo": 2}],
                 "dependencies": [{"modId": 9, "relationType": 3}, {"modId": 10, "relationType": 2}]}
            ]})))
            .mount(&s)
            .await;
        let f = CfApi::with_base(s.uri(), "k").mod_files(7, Some("1.20.1"), None).await.unwrap();
        assert_eq!(f[0].id, 2);
        assert_eq!(f[0].sha1(), "ab");
        assert_eq!(f[0].loaders(), vec!["neoforge"]);
        assert_eq!(f[0].minecraft_versions(), vec!["1.20.1"]);
        assert_eq!(f[0].dependencies.iter().filter(|d| d.relation_type == REL_REQUIRED).count(), 1);
        assert!(f[1].url().is_none());
        assert_eq!(f[1].loaders(), vec!["fabric"]);
    }

    #[tokio::test]
    async fn errors_without_key_or_on_status() {
        let s = MockServer::start().await;
        Mock::given(method("GET")).and(path("/v1/mods/1/files/2")).respond_with(ResponseTemplate::new(404)).mount(&s).await;
        Mock::given(method("GET")).and(path("/v1/mods/search")).respond_with(ResponseTemplate::new(403)).mount(&s).await;
        let nokey = CfApi::with_base(s.uri(), "");
        assert!(!nokey.has_key());
        match nokey.search(CLASS_MOD, "", None, None, 1, 0, 5).await.unwrap_err() {
            Error::Invalid(m) => assert!(m.contains("clé"), "{m}"),
            e => panic!("{e:?}"),
        }
        let c = CfApi::with_base(s.uri(), "k");
        assert!(matches!(c.file(1, 2).await, Err(Error::NotFound(_))));
        assert!(matches!(c.search(CLASS_MOD, "", None, None, 1, 0, 5).await, Err(Error::Network(_))));
    }

    #[test]
    fn murmur2_ignores_whitespace_and_is_stable() {
        let a = murmur2_fingerprint(b"abc def\r\n\tghi");
        assert_eq!(a, murmur2_fingerprint(b"abcdefghi"));
        assert_ne!(a, murmur2_fingerprint(b"abcdefghj"));
        // Toutes les longueurs de queue (0 à 3) fonctionnent.
        let hs: std::collections::HashSet<u32> = (0..9).map(|n| murmur2_fingerprint(&vec![b'x'; n])).collect();
        assert_eq!(hs.len(), 9);
    }

    #[tokio::test]
    async fn fingerprints_post_and_parse() {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/fingerprints/432"))
            .and(header("x-api-key", "k"))
            .and(wiremock::matchers::body_json(json!({"fingerprints": [1, 2]})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {"exactMatches": [
                {"id": 306612, "file": {"id": 9, "modId": 306612, "fileName": "a.jar", "fileFingerprint": 2}}
            ], "exactFingerprints": [2]}})))
            .expect(1)
            .mount(&s)
            .await;
        let c = CfApi::with_base(s.uri(), "k");
        let m = c.fingerprints(&[1, 2]).await.unwrap();
        assert_eq!((m[0].id, m[0].file.file_fingerprint), (306612, 2));
        assert!(c.fingerprints(&[]).await.unwrap().is_empty());
    }

    #[test]
    fn loader_and_sort_mapping() {
        assert_eq!(loader_type(Loader::Fabric), Some(4));
        assert_eq!(loader_type(Loader::NeoForge), Some(6));
        assert_eq!(loader_type(Loader::Quilt), Some(5));
        assert_eq!(loader_type(Loader::Forge), Some(1));
        assert_eq!(loader_type(Loader::Vanilla), None);
        assert_eq!((sort_field(None), sort_field(Some("downloads")), sort_field(Some("updated"))), (1, 6, 3));
    }
}
