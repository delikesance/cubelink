//! Tâche M1 : manifeste des versions Mojang et JSON de version (sha1 vérifié).

use serde::Deserialize;

use crate::types::{Error, Result};

pub const DEFAULT_MANIFEST_URL: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LatestVersions {
    pub release: String,
    pub snapshot: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct VersionEntry {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    pub sha1: String,
    #[serde(rename = "releaseTime")]
    pub release_time: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct VersionManifest {
    pub latest: LatestVersions,
    pub versions: Vec<VersionEntry>,
}

impl VersionManifest {
    /// Cherche une version par identifiant.
    pub fn find(&self, id: &str) -> Result<&VersionEntry> {
        self.versions
            .iter()
            .find(|v| v.id == id)
            .ok_or_else(|| Error::NotFound(format!("version Minecraft {id}")))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AssetIndexRef {
    pub id: String,
    pub url: String,
    pub sha1: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DownloadInfo {
    pub url: String,
    pub sha1: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct VersionDownloads {
    pub client: DownloadInfo,
    /// Mappings ProGuard Mojang du client (absents des versions non obfusquées, 26.x).
    #[serde(default)]
    pub client_mappings: Option<DownloadInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct JavaVersion {
    #[serde(rename = "majorVersion")]
    pub major_version: u32,
}

/// Champs utiles du JSON de version. `libraries` garde le JSON brut (tâche M2).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct VersionJson {
    pub id: String,
    #[serde(rename = "mainClass")]
    pub main_class: String,
    #[serde(rename = "assetIndex")]
    pub asset_index: AssetIndexRef,
    pub downloads: VersionDownloads,
    #[serde(default)]
    pub libraries: Vec<serde_json::Value>,
    /// Format moderne (>= 1.13) : `{ "game": [...], "jvm": [...] }` brut.
    #[serde(default)]
    pub arguments: Option<serde_json::Value>,
    /// Format ancien : une chaîne d'arguments de jeu.
    #[serde(default, rename = "minecraftArguments")]
    pub minecraft_arguments: Option<String>,
    #[serde(default, rename = "javaVersion")]
    pub java_version: Option<JavaVersion>,
}

/// Identifiants des versions « release », de la plus récente à la plus ancienne (`releaseTime`).
pub fn release_ids(manifest: &VersionManifest) -> Vec<String> {
    let mut rel: Vec<&VersionEntry> = manifest.versions.iter().filter(|v| v.kind == "release").collect();
    // Les dates ISO 8601 en UTC (même fuseau) se comparent lexicographiquement.
    rel.sort_by(|a, b| b.release_time.cmp(&a.release_time));
    rel.into_iter().map(|v| v.id.clone()).collect()
}

pub struct VersionsClient {
    http: reqwest::Client,
    manifest_url: String,
}

impl Default for VersionsClient {
    fn default() -> Self {
        Self::new()
    }
}

impl VersionsClient {
    pub fn new() -> Self {
        Self::with_manifest_url(DEFAULT_MANIFEST_URL)
    }

    pub fn with_manifest_url(url: impl Into<String>) -> Self {
        Self { http: reqwest::Client::new(), manifest_url: url.into() }
    }

    async fn get_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let resp = self.http.get(url).send().await.map_err(|e| Error::Network(e.to_string()))?;
        let status = resp.status();
        if status.as_u16() == 404 {
            return Err(Error::NotFound(url.to_string()));
        }
        if !status.is_success() {
            return Err(Error::Network(format!("HTTP {} pour {url}", status.as_u16())));
        }
        let bytes = resp.bytes().await.map_err(|e| Error::Network(e.to_string()))?;
        Ok(bytes.to_vec())
    }

    pub async fn fetch_manifest(&self) -> Result<VersionManifest> {
        let bytes = self.get_bytes(&self.manifest_url).await.map_err(|e| match e {
            Error::NotFound(u) => Error::Network(format!("manifeste indisponible (HTTP 404) : {u}")),
            other => other,
        })?;
        serde_json::from_slice(&bytes).map_err(|e| Error::Network(format!("manifeste invalide : {e}")))
    }

    /// Télécharge le JSON de version, vérifie son sha1 puis le décode.
    pub async fn fetch_version_json(&self, entry: &VersionEntry) -> Result<VersionJson> {
        let bytes = self.get_bytes(&entry.url).await?;
        let actual = sha1_hex(&bytes);
        if !actual.eq_ignore_ascii_case(&entry.sha1) {
            return Err(Error::Integrity { expected: entry.sha1.to_lowercase(), actual });
        }
        serde_json::from_slice(&bytes).map_err(|e| Error::Network(format!("JSON de version {} invalide : {e}", entry.id)))
    }
}

/// Manifeste depuis l'URL officielle.
pub async fn fetch_manifest() -> Result<VersionManifest> {
    VersionsClient::new().fetch_manifest().await
}

/// JSON de version via l'URL de l'entrée (client par défaut).
pub async fn fetch_version_json(entry: &VersionEntry) -> Result<VersionJson> {
    VersionsClient::new().fetch_version_json(entry).await
}

/// SHA-1 minimal (RFC 3174), hexadécimal minuscule. Utilisé uniquement pour l'intégrité Mojang.
pub fn sha1_hex(data: &[u8]) -> String {
    let mut h: [u32; 5] = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for chunk in msg.as_chunks::<64>().0 {
        let mut w = [0u32; 80];
        for (i, b) in chunk.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*b);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6u32),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn version_body() -> String {
        json!({
            "id": "1.21.1",
            "mainClass": "net.minecraft.client.main.Main",
            "assetIndex": {"id":"17","url":"https://x/17.json","sha1":"aa","size":10,"totalSize":99},
            "downloads": {"client": {"url":"https://x/client.jar","sha1":"bb","size":20}},
            "libraries": [{"name":"a:b:1","rules":[{"action":"allow"}]}, {"name":"c:d:2"}],
            "arguments": {"game": ["--demo"], "jvm": ["-Xss1M"]},
            "javaVersion": {"component":"java-runtime-delta","majorVersion":21}
        })
        .to_string()
    }

    fn manifest_json(server: &MockServer, sha1: &str) -> serde_json::Value {
        json!({
            "latest": {"release":"1.21.1","snapshot":"24w40a"},
            "versions": [
                {"id":"1.20.6","type":"release","url":format!("{}/v/1.20.6.json", server.uri()),"sha1":"0","releaseTime":"2024-04-29T10:00:00+00:00"},
                {"id":"24w40a","type":"snapshot","url":"u","sha1":"0","releaseTime":"2024-10-01T10:00:00+00:00"},
                {"id":"1.21.1","type":"release","url":format!("{}/v/1.21.1.json", server.uri()),"sha1":sha1,"releaseTime":"2024-08-08T12:00:00+00:00"},
                {"id":"1.19","type":"release","url":"u","sha1":"0","releaseTime":"2022-06-07T10:00:00+00:00"}
            ]
        })
    }

    async fn setup(sha1_in_manifest: &str) -> (MockServer, VersionsClient) {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/manifest.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(manifest_json(&s, sha1_in_manifest)))
            .mount(&s)
            .await;
        let c = VersionsClient::with_manifest_url(format!("{}/manifest.json", s.uri()));
        (s, c)
    }

    #[test]
    fn sha1_known_vectors() {
        assert_eq!(sha1_hex(b""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(
            sha1_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
        assert_eq!(sha1_hex(&vec![b'a'; 1_000_000]), "34aa973cd4c4daa4f61eeb2bdbad27316534016f");
    }

    #[tokio::test]
    async fn manifest_is_parsed() {
        let (_s, c) = setup("0").await;
        let m = c.fetch_manifest().await.unwrap();
        assert_eq!(m.latest.release, "1.21.1");
        assert_eq!(m.latest.snapshot, "24w40a");
        assert_eq!(m.versions.len(), 4);
        assert_eq!(m.versions[0].kind, "release");
    }

    #[tokio::test]
    async fn releases_sorted_newest_first() {
        let (_s, c) = setup("0").await;
        let m = c.fetch_manifest().await.unwrap();
        assert_eq!(release_ids(&m), vec!["1.21.1", "1.20.6", "1.19"]);
    }

    #[tokio::test]
    async fn manifest_404_is_network_error() {
        let s = MockServer::start().await;
        let c = VersionsClient::with_manifest_url(format!("{}/manifest.json", s.uri()));
        assert!(matches!(c.fetch_manifest().await, Err(Error::Network(_))));
    }

    #[tokio::test]
    async fn version_found_and_verified() {
        let body = version_body();
        let (s, c) = setup(&sha1_hex(body.as_bytes())).await;
        Mock::given(method("GET")).and(path("/v/1.21.1.json")).respond_with(ResponseTemplate::new(200).set_body_string(body)).mount(&s).await;
        let m = c.fetch_manifest().await.unwrap();
        let v = c.fetch_version_json(m.find("1.21.1").unwrap()).await.unwrap();
        assert_eq!(v.id, "1.21.1");
        assert_eq!(v.main_class, "net.minecraft.client.main.Main");
        assert_eq!(v.asset_index.id, "17");
        assert_eq!(v.asset_index.size, 10);
        assert_eq!(v.downloads.client.url, "https://x/client.jar");
        assert_eq!(v.libraries.len(), 2);
        assert_eq!(v.libraries[0]["name"], "a:b:1");
        assert!(v.libraries[0]["rules"].is_array());
        assert!(v.arguments.is_some());
        assert!(v.minecraft_arguments.is_none());
        assert_eq!(v.java_version.unwrap().major_version, 21);
    }

    #[tokio::test]
    async fn wrong_sha1_is_rejected() {
        let (s, c) = setup(&"0".repeat(40)).await;
        Mock::given(method("GET")).and(path("/v/1.21.1.json")).respond_with(ResponseTemplate::new(200).set_body_string(version_body())).mount(&s).await;
        let m = c.fetch_manifest().await.unwrap();
        let err = c.fetch_version_json(m.find("1.21.1").unwrap()).await.unwrap_err();
        assert!(matches!(err, Error::Integrity { .. }), "{err:?}");
    }

    #[tokio::test]
    async fn unknown_version_is_not_found() {
        let (_s, c) = setup("0").await;
        let m = c.fetch_manifest().await.unwrap();
        assert!(matches!(m.find("9.9.9"), Err(Error::NotFound(_))));
    }

    #[tokio::test]
    async fn version_json_404_is_not_found() {
        let (_s, c) = setup("0").await;
        let m = c.fetch_manifest().await.unwrap();
        // aucun mock pour /v/1.20.6.json : wiremock répond 404
        let err = c.fetch_version_json(m.find("1.20.6").unwrap()).await.unwrap_err();
        assert!(matches!(err, Error::NotFound(_)), "{err:?}");
    }

    #[tokio::test]
    async fn legacy_minecraft_arguments() {
        let body = json!({
            "id":"1.8.9","mainClass":"net.minecraft.client.main.Main",
            "assetIndex":{"id":"1.8","url":"u","sha1":"s","size":1},
            "downloads":{"client":{"url":"u","sha1":"s","size":1}},
            "minecraftArguments":"--username ${auth_player_name}"
        })
        .to_string();
        let s = MockServer::start().await;
        Mock::given(method("GET")).and(path("/v.json")).respond_with(ResponseTemplate::new(200).set_body_string(body.clone())).mount(&s).await;
        let entry = VersionEntry {
            id: "1.8.9".into(), kind: "release".into(), url: format!("{}/v.json", s.uri()),
            sha1: sha1_hex(body.as_bytes()).to_uppercase(), release_time: String::new(),
        };
        let v = VersionsClient::new().fetch_version_json(&entry).await.unwrap();
        assert_eq!(v.minecraft_arguments.as_deref(), Some("--username ${auth_player_name}"));
        assert!(v.java_version.is_none());
    }
}
