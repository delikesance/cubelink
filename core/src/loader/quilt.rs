//! Tâche L3 : installation du loader Quilt (même flux que `fabric.rs`).
//!
//! Flux : `list_loader_versions` -> `fetch_profile` -> `install` -> `merge_with_vanilla`.
//! Les types `LoaderVersion`, `MergedVersion` et `InstalledProfile` sont ceux de Fabric.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

pub use super::fabric::{InstalledProfile, LoaderVersion, MergedVersion};
use crate::mojang::libraries::maven_path;
use crate::mojang::versions::{sha1_hex, VersionJson};
use crate::types::{Error, Result};

pub const DEFAULT_META_URL: &str = "https://meta.quiltmc.org";
const DEFAULT_MAVEN_URL: &str = "https://maven.quiltmc.org/repository/release/";

/// Version stable la plus récente (liste triée de la plus récente à la plus ancienne).
pub fn latest_stable(versions: &[LoaderVersion]) -> Option<&LoaderVersion> {
    versions.iter().find(|v| v.stable)
}

pub struct QuiltClient {
    http: reqwest::Client,
    meta_url: String,
}

impl Default for QuiltClient {
    fn default() -> Self {
        Self::new()
    }
}

fn encode_segment(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

impl QuiltClient {
    pub fn new() -> Self {
        Self::with_meta_url(DEFAULT_META_URL)
    }

    /// `meta_url` : base sans `/v3` (ex. `https://meta.quiltmc.org`).
    pub fn with_meta_url(url: impl Into<String>) -> Self {
        Self { http: reqwest::Client::new(), meta_url: url.into().trim_end_matches('/').to_string() }
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

    /// Versions du loader compatibles avec `minecraft`, de la plus récente à la plus ancienne.
    pub async fn list_loader_versions(&self, minecraft: &str) -> Result<Vec<LoaderVersion>> {
        #[derive(Deserialize)]
        struct Entry {
            loader: Inner,
        }
        #[derive(Deserialize)]
        struct Inner {
            version: String,
        }
        let url = format!("{}/v3/versions/loader/{}", self.meta_url, encode_segment(minecraft));
        let bytes = self.get_bytes(&url).await.map_err(|e| match e {
            Error::NotFound(_) => Error::NotFound(format!("Minecraft {minecraft} pour Quilt")),
            other => other,
        })?;
        let entries: Vec<Entry> =
            serde_json::from_slice(&bytes).map_err(|e| Error::Network(format!("liste Quilt invalide : {e}")))?;
        // L'API Quilt n'a pas de drapeau `stable` : un suffixe `-beta.N`/`-rc.N` marque une préversion.
        Ok(entries
            .into_iter()
            .map(|e| LoaderVersion { stable: !e.loader.version.contains('-'), version: e.loader.version })
            .collect())
    }

    /// JSON de profil de version (`quilt-loader-<loader>-<minecraft>`).
    pub async fn fetch_profile(&self, minecraft: &str, loader_version: &str) -> Result<Value> {
        let url = format!(
            "{}/v3/versions/loader/{}/{}/profile/json",
            self.meta_url,
            encode_segment(minecraft),
            encode_segment(loader_version)
        );
        let bytes = self.get_bytes(&url).await.map_err(|e| match e {
            Error::NotFound(_) => Error::NotFound(format!("Quilt {loader_version} pour Minecraft {minecraft}")),
            other => other,
        })?;
        let v: Value =
            serde_json::from_slice(&bytes).map_err(|e| Error::Network(format!("profil Quilt invalide : {e}")))?;
        if v.get("id").and_then(Value::as_str).is_none() {
            return Err(Error::Network("profil Quilt sans `id`".into()));
        }
        Ok(v)
    }

    /// Écrit `<versions_dir>/<id>/<id>.json` (atomique) et télécharge les libraries manquantes ou invalides.
    pub async fn install(&self, profile: &Value, libs_dir: &Path, versions_dir: &Path) -> Result<InstalledProfile> {
        let id = profile
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Invalid("profil sans `id`".into()))?;
        if id.is_empty() || id.contains(['/', '\\']) || id.contains("..") {
            return Err(Error::Invalid(format!("identifiant de profil invalide « {id} »")));
        }
        let libraries = profile.get("libraries").and_then(Value::as_array).cloned().unwrap_or_default();

        // Libraries d'abord : le JSON n'apparaît que si tout est présent.
        for lib in &libraries {
            self.ensure_library(lib, libs_dir).await?;
        }

        let dir = versions_dir.join(id);
        tokio::fs::create_dir_all(&dir).await?;
        let json_path = dir.join(format!("{id}.json"));
        let body = serde_json::to_vec_pretty(profile).map_err(|e| Error::Other(e.to_string()))?;
        write_atomic(&json_path, &body).await?;
        Ok(InstalledProfile { id: id.to_string(), json_path })
    }

    async fn ensure_library(&self, lib: &Value, libs_dir: &Path) -> Result<()> {
        let name = lib
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Invalid("library sans `name`".into()))?;
        let rel = maven_path(name)?;
        if rel.split('/').any(|s| s == "..") {
            return Err(Error::Invalid(format!("chemin de library invalide « {name} »")));
        }
        let sha1 = lib.get("sha1").and_then(Value::as_str).filter(|s| !s.is_empty());
        let dest = libs_dir.join(&rel);

        if is_valid(&dest, sha1).await {
            return Ok(());
        }

        let base = match lib.get("url").and_then(Value::as_str) {
            Some(u) if !u.is_empty() => u.to_string(),
            _ => DEFAULT_MAVEN_URL.to_string(),
        };
        let base = if base.ends_with('/') { base } else { format!("{base}/") };
        let bytes = self.get_bytes(&format!("{base}{rel}")).await?;
        if bytes.is_empty() {
            return Err(Error::Network(format!("fichier vide pour {name}")));
        }
        if let Some(expected) = sha1 {
            let actual = sha1_hex(&bytes);
            if !actual.eq_ignore_ascii_case(expected) {
                return Err(Error::Integrity { expected: expected.to_lowercase(), actual });
            }
        }
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        write_atomic(&dest, &bytes).await
    }
}

/// Fichier présent, non vide et, si `sha1` est fourni, de même empreinte.
async fn is_valid(path: &Path, sha1: Option<&str>) -> bool {
    let Ok(data) = tokio::fs::read(path).await else { return false };
    if data.is_empty() {
        return false;
    }
    match sha1 {
        Some(expected) => sha1_hex(&data).eq_ignore_ascii_case(expected),
        None => true,
    }
}

async fn write_atomic(dest: &Path, data: &[u8]) -> Result<()> {
    let mut tmp = dest.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    tokio::fs::write(&tmp, data).await?;
    if let Err(e) = tokio::fs::rename(&tmp, dest).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e.into());
    }
    Ok(())
}

/// Liste des versions du loader (client par défaut).
pub async fn list_loader_versions(minecraft: &str) -> Result<Vec<LoaderVersion>> {
    QuiltClient::new().list_loader_versions(minecraft).await
}

/// Profil de version (client par défaut).
pub async fn fetch_profile(minecraft: &str, loader_version: &str) -> Result<Value> {
    QuiltClient::new().fetch_profile(minecraft, loader_version).await
}

/// Installation (client par défaut).
pub async fn install(profile: &Value, libs_dir: &Path, versions_dir: &Path) -> Result<InstalledProfile> {
    QuiltClient::new().install(profile, libs_dir, versions_dir).await
}


/// Fusion profil Quilt + JSON vanilla (mêmes règles que Fabric).
pub fn merge_with_vanilla(vanilla: &VersionJson, profile: &Value) -> MergedVersion {
    super::fabric::merge_with_vanilla(vanilla, profile)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const A: &[u8] = b"jar-a";
    const B: &[u8] = b"jar-b";
    const C: &[u8] = b"jar-c";
    const ID: &str = "quilt-loader-0.26.4-1.21.1";

    /// Loader et hashed sur le Maven Quilt, intermediary sur le dépôt Fabric.
    fn profile(server: &MockServer) -> Value {
        json!({
            "id": ID,
            "inheritsFrom": "1.21.1",
            "mainClass": "org.quiltmc.loader.impl.launch.knot.KnotClient",
            "arguments": {"game": [], "jvm": ["-DQuiltMcEmu=x"]},
            "libraries": [
                {"name": "org.quiltmc:quilt-loader:0.26.4", "url": format!("{}/quilt/", server.uri()), "sha1": sha1_hex(A)},
                {"name": "org.quiltmc:hashed:1.21.1", "url": format!("{}/quilt", server.uri())},
                {"name": "net.fabricmc:intermediary:1.21.1", "url": format!("{}/fabric/", server.uri()), "sha1": sha1_hex(C)}
            ]
        })
    }

    fn vanilla() -> VersionJson {
        serde_json::from_value(json!({
            "id": "1.21.1",
            "mainClass": "net.minecraft.client.main.Main",
            "assetIndex": {"id":"17","url":"u","sha1":"aa","size":1},
            "downloads": {"client": {"url":"u","sha1":"bb","size":2}},
            "libraries": [{"name":"org.ow2.asm:asm:9.7"}, {"name":"com.google:guava:31"}],
            "arguments": {"game": ["--demo"], "jvm": ["-Xss1M"]},
            "javaVersion": {"component":"java-runtime-delta","majorVersion":21}
        }))
        .unwrap()
    }

    async fn serve(server: &MockServer, p: &str, body: &[u8], expect: u64) {
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.to_vec()))
            .expect(expect)
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn lists_versions_and_latest_stable() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v3/versions/loader/1.21.1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"loader": {"separator": "-beta.", "build": 1, "version": "0.27.0-beta.1"}, "hashed": {}},
                {"loader": {"separator": ".", "build": 4, "version": "0.26.4"}, "hashed": {}},
                {"loader": {"version": "0.26.3"}}
            ])))
            .mount(&s)
            .await;
        let v = QuiltClient::with_meta_url(s.uri()).list_loader_versions("1.21.1").await.unwrap();
        assert_eq!(v.len(), 3);
        assert!(!v[0].stable);
        assert_eq!(latest_stable(&v).unwrap().version, "0.26.4");
        assert!(latest_stable(&[]).is_none());
    }

    #[tokio::test]
    async fn fetches_profile() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v3/versions/loader/1.21.1/0.26.4/profile/json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(profile(&s)))
            .mount(&s)
            .await;
        let got = QuiltClient::with_meta_url(s.uri()).fetch_profile("1.21.1", "0.26.4").await.unwrap();
        assert_eq!(got["id"], ID);
        assert_eq!(got["libraries"].as_array().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn unknown_version_is_not_found_and_500_is_network() {
        let s = MockServer::start().await;
        let c = QuiltClient::with_meta_url(s.uri());
        assert!(matches!(c.fetch_profile("1.21.1", "9.9.9").await, Err(Error::NotFound(_))));
        assert!(matches!(c.list_loader_versions("0.0").await, Err(Error::NotFound(_))));
        let s2 = MockServer::start().await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(500)).mount(&s2).await;
        let c2 = QuiltClient::with_meta_url(s2.uri());
        assert!(matches!(c2.list_loader_versions("1.21.1").await, Err(Error::Network(_))));
    }

    #[tokio::test]
    async fn installs_three_libs_from_two_mavens_skipping_existing() {
        let s = MockServer::start().await;
        serve(&s, "/quilt/org/quiltmc/quilt-loader/0.26.4/quilt-loader-0.26.4.jar", A, 0).await;
        serve(&s, "/quilt/org/quiltmc/hashed/1.21.1/hashed-1.21.1.jar", B, 1).await;
        serve(&s, "/fabric/net/fabricmc/intermediary/1.21.1/intermediary-1.21.1.jar", C, 1).await;

        let tmp = tempfile::tempdir().unwrap();
        let (libs, vers) = (tmp.path().join("libraries"), tmp.path().join("versions"));
        let existing = libs.join("org/quiltmc/quilt-loader/0.26.4/quilt-loader-0.26.4.jar");
        std::fs::create_dir_all(existing.parent().unwrap()).unwrap();
        std::fs::write(&existing, A).unwrap();

        let inst = QuiltClient::with_meta_url(s.uri()).install(&profile(&s), &libs, &vers).await.unwrap();
        assert_eq!(inst.id, ID);
        assert_eq!(inst.json_path, vers.join(ID).join(format!("{ID}.json")));
        let written: Value = serde_json::from_slice(&std::fs::read(&inst.json_path).unwrap()).unwrap();
        assert_eq!(written["mainClass"], "org.quiltmc.loader.impl.launch.knot.KnotClient");
        assert_eq!(std::fs::read(libs.join("org/quiltmc/hashed/1.21.1/hashed-1.21.1.jar")).unwrap(), B);
        assert_eq!(std::fs::read(libs.join("net/fabricmc/intermediary/1.21.1/intermediary-1.21.1.jar")).unwrap(), C);
        assert!(!inst.json_path.with_extension("json.tmp").exists());
    }

    #[tokio::test]
    async fn bad_sha1_missing_lib_and_dead_host() {
        let s = MockServer::start().await;
        serve(&s, "/m/a/b/1/b-1.jar", B, 1).await;
        let tmp = tempfile::tempdir().unwrap();
        let (l, v) = (tmp.path().join("l"), tmp.path().join("v"));
        let c = QuiltClient::with_meta_url(s.uri());

        let bad = json!({"id": "x", "libraries": [{"name": "a:b:1", "url": format!("{}/m/", s.uri()), "sha1": "0".repeat(40)}]});
        let err = c.install(&bad, &l, &v).await.unwrap_err();
        assert!(matches!(err, Error::Integrity { .. }), "{err:?}");
        assert!(!v.join("x/x.json").exists());

        let missing = json!({"id": "x", "libraries": [{"name": "c:d:1", "url": format!("{}/m/", s.uri())}]});
        assert!(matches!(c.install(&missing, &l, &v).await, Err(Error::NotFound(_))));

        let dead = json!({"id": "x", "libraries": [{"name": "a:b:1", "url": "http://127.0.0.1:1/"}]});
        assert!(matches!(c.install(&dead, &l, &v).await, Err(Error::Network(_))));
    }

    #[tokio::test]
    async fn rejects_dangerous_profile_id() {
        let tmp = tempfile::tempdir().unwrap();
        let c = QuiltClient::new();
        let p = json!({"id": "../evil", "libraries": []});
        assert!(matches!(c.install(&p, tmp.path(), tmp.path()).await, Err(Error::Invalid(_))));
        let p = json!({"id": "a/b", "libraries": []});
        assert!(matches!(c.install(&p, tmp.path(), tmp.path()).await, Err(Error::Invalid(_))));
    }

    #[test]
    fn merges_with_vanilla() {
        let p = json!({
            "id": ID,
            "inheritsFrom": "1.21.1",
            "mainClass": "org.quiltmc.loader.impl.launch.knot.KnotClient",
            "arguments": {"game": ["--quilt"], "jvm": ["-DQuiltMcEmu=x"]},
            "libraries": [
                {"name": "org.quiltmc:quilt-loader:0.26.4", "url": "http://x"},
                {"name": "org.ow2.asm:asm:9.7", "url": "http://x"}
            ]
        });
        let m = merge_with_vanilla(&vanilla(), &p);
        assert_eq!(m.id, ID);
        assert_eq!(m.inherits_from, "1.21.1");
        assert_eq!(m.main_class, "org.quiltmc.loader.impl.launch.knot.KnotClient");
        let names: Vec<&str> = m.libraries.iter().map(|l| l["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["org.quiltmc:quilt-loader:0.26.4", "org.ow2.asm:asm:9.7", "com.google:guava:31"]);
        assert_eq!(m.jvm_args, vec![json!("-Xss1M"), json!("-DQuiltMcEmu=x")]);
        assert_eq!(m.game_args, vec![json!("--demo"), json!("--quilt")]);
        assert_eq!(m.java_version.unwrap().major_version, 21);
    }
}
