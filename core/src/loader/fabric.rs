//! Tâche L2 : installation du loader Fabric (modèle pour Quilt/Forge/NeoForge).
//!
//! Flux : `list_loader_versions` -> `fetch_profile` -> `install` (écrit le JSON de profil et
//! télécharge les libraries) -> `merge_with_vanilla` (consommé par la construction de la ligne de commande).

use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use crate::mojang::libraries::maven_path;
use crate::mojang::versions::{sha1_hex, AssetIndexRef, JavaVersion, VersionDownloads, VersionJson};
use crate::types::{Error, Result};

pub const DEFAULT_META_URL: &str = "https://meta.fabricmc.net";
const DEFAULT_MAVEN_URL: &str = "https://maven.fabricmc.net/";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoaderVersion {
    pub version: String,
    pub stable: bool,
}

/// Version stable la plus récente (la liste de Fabric est triée de la plus récente à la plus ancienne).
pub fn latest_stable(versions: &[LoaderVersion]) -> Option<&LoaderVersion> {
    versions.iter().find(|v| v.stable)
}

/// Résultat de la fusion profil loader + JSON vanilla.
#[derive(Debug, Clone, PartialEq)]
pub struct MergedVersion {
    /// Identifiant du profil loader (ex. `fabric-loader-0.16.5-1.21.1`).
    pub id: String,
    /// Version vanilla parente (`inheritsFrom`, sinon l'id vanilla).
    pub inherits_from: String,
    /// `mainClass` du loader.
    pub main_class: String,
    pub asset_index: AssetIndexRef,
    pub downloads: VersionDownloads,
    /// JSON brut : libraries du loader d'abord, puis vanilla, sans doublon de `name` (première occurrence gardée).
    pub libraries: Vec<Value>,
    /// Arguments JVM : vanilla puis loader (entrées brutes, chaînes ou objets à règles).
    pub jvm_args: Vec<Value>,
    /// Arguments de jeu : vanilla puis loader.
    pub game_args: Vec<Value>,
    /// Ancien format (< 1.13) du JSON vanilla, conservé tel quel.
    pub minecraft_arguments: Option<String>,
    pub java_version: Option<JavaVersion>,
}

/// Profil installé sur disque.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledProfile {
    pub id: String,
    pub json_path: PathBuf,
}

pub struct FabricClient {
    http: reqwest::Client,
    meta_url: String,
}

impl Default for FabricClient {
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

impl FabricClient {
    pub fn new() -> Self {
        Self::with_meta_url(DEFAULT_META_URL)
    }

    /// `meta_url` : base sans `/v2` (ex. `https://meta.fabricmc.net`).
    pub fn with_meta_url(url: impl Into<String>) -> Self {
        Self { http: reqwest::Client::new(), meta_url: url.into().trim_end_matches('/').to_string() }
    }

    async fn get_bytes(&self, url: &str) -> Result<Vec<u8>> {
        crate::mojang::versions::get_bytes(&self.http, url).await
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
            #[serde(default)]
            stable: bool,
        }
        let url = format!("{}/v2/versions/loader/{}", self.meta_url, encode_segment(minecraft));
        let bytes = self.get_bytes(&url).await.map_err(|e| match e {
            Error::NotFound(_) => Error::NotFound(format!("Minecraft {minecraft} pour Fabric")),
            other => other,
        })?;
        let entries: Vec<Entry> =
            serde_json::from_slice(&bytes).map_err(|e| Error::Network(format!("liste Fabric invalide : {e}")))?;
        Ok(entries.into_iter().map(|e| LoaderVersion { version: e.loader.version, stable: e.loader.stable }).collect())
    }

    /// JSON de profil de version (`fabric-loader-<loader>-<minecraft>`).
    pub async fn fetch_profile(&self, minecraft: &str, loader_version: &str) -> Result<Value> {
        let url = format!(
            "{}/v2/versions/loader/{}/{}/profile/json",
            self.meta_url,
            encode_segment(minecraft),
            encode_segment(loader_version)
        );
        let bytes = self.get_bytes(&url).await.map_err(|e| match e {
            Error::NotFound(_) => Error::NotFound(format!("Fabric {loader_version} pour Minecraft {minecraft}")),
            other => other,
        })?;
        let v: Value =
            serde_json::from_slice(&bytes).map_err(|e| Error::Network(format!("profil Fabric invalide : {e}")))?;
        if v.get("id").and_then(Value::as_str).is_none() {
            return Err(Error::Network("profil Fabric sans `id`".into()));
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
        super::write_atomic(&json_path, &body).await?;
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
        super::write_atomic(&dest, &bytes).await
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

/// Liste des versions du loader (client par défaut).
pub async fn list_loader_versions(minecraft: &str) -> Result<Vec<LoaderVersion>> {
    FabricClient::new().list_loader_versions(minecraft).await
}

/// Profil de version (client par défaut).
pub async fn fetch_profile(minecraft: &str, loader_version: &str) -> Result<Value> {
    FabricClient::new().fetch_profile(minecraft, loader_version).await
}

/// Installation (client par défaut).
pub async fn install(profile: &Value, libs_dir: &Path, versions_dir: &Path) -> Result<InstalledProfile> {
    FabricClient::new().install(profile, libs_dir, versions_dir).await
}

fn arg_list(args: Option<&Value>, key: &str) -> Vec<Value> {
    args.and_then(|a| a.get(key)).and_then(Value::as_array).cloned().unwrap_or_default()
}

/// Fusionne le profil loader avec le JSON vanilla : `mainClass` du loader, libraries du loader puis
/// vanilla (dédoublonnées par `name`), arguments jvm/game vanilla puis loader.
pub fn merge_with_vanilla(vanilla: &VersionJson, profile: &Value) -> MergedVersion {
    let id = profile.get("id").and_then(Value::as_str).unwrap_or(&vanilla.id).to_string();
    let inherits_from = profile.get("inheritsFrom").and_then(Value::as_str).unwrap_or(&vanilla.id).to_string();
    let main_class = profile
        .get("mainClass")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| vanilla.main_class.clone());

    let loader_libs = profile.get("libraries").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let mut libraries: Vec<Value> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for lib in loader_libs.iter().chain(vanilla.libraries.iter()) {
        match lib.get("name").and_then(Value::as_str) {
            Some(n) if !seen.insert(n.to_string()) => {}
            _ => libraries.push(lib.clone()),
        }
    }

    let p_args = profile.get("arguments");
    let mut jvm_args = arg_list(vanilla.arguments.as_ref(), "jvm");
    jvm_args.extend(arg_list(p_args, "jvm"));
    let mut game_args = arg_list(vanilla.arguments.as_ref(), "game");
    game_args.extend(arg_list(p_args, "game"));

    MergedVersion {
        id,
        inherits_from,
        main_class,
        asset_index: vanilla.asset_index.clone(),
        downloads: vanilla.downloads.clone(),
        libraries,
        jvm_args,
        game_args,
        minecraft_arguments: vanilla.minecraft_arguments.clone(),
        java_version: vanilla.java_version.clone(),
    }
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

    fn profile(server: &MockServer) -> Value {
        json!({
            "id": "fabric-loader-0.16.5-1.21.1",
            "inheritsFrom": "1.21.1",
            "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
            "arguments": {"game": [], "jvm": ["-DFabricMcEmu= net.minecraft.client.main.Main "]},
            "libraries": [
                {"name": "net.fabricmc:fabric-loader:0.16.5", "url": format!("{}/maven/", server.uri()), "sha1": sha1_hex(A)},
                {"name": "org.ow2.asm:asm:9.7", "url": format!("{}/maven", server.uri())},
                {"name": "net.fabricmc:intermediary:1.21.1", "url": format!("{}/maven/", server.uri()), "sha1": sha1_hex(C)}
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
            .and(path("/v2/versions/loader/1.21.1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"loader": {"version": "0.17.0-beta.1", "stable": false}, "intermediary": {}},
                {"loader": {"version": "0.16.5", "stable": true}, "intermediary": {}},
                {"loader": {"version": "0.16.4", "stable": true}}
            ])))
            .mount(&s)
            .await;
        let c = FabricClient::with_meta_url(s.uri());
        let v = c.list_loader_versions("1.21.1").await.unwrap();
        assert_eq!(v.len(), 3);
        assert!(!v[0].stable);
        assert_eq!(latest_stable(&v).unwrap().version, "0.16.5");
        assert!(latest_stable(&[]).is_none());
    }

    #[tokio::test]
    async fn fetches_profile() {
        let s = MockServer::start().await;
        let p = profile(&s);
        Mock::given(method("GET"))
            .and(path("/v2/versions/loader/1.21.1/0.16.5/profile/json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(p.clone()))
            .mount(&s)
            .await;
        let got = FabricClient::with_meta_url(s.uri()).fetch_profile("1.21.1", "0.16.5").await.unwrap();
        assert_eq!(got["id"], "fabric-loader-0.16.5-1.21.1");
        assert_eq!(got["inheritsFrom"], "1.21.1");
        assert_eq!(got["libraries"].as_array().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn unknown_version_is_not_found() {
        let s = MockServer::start().await;
        let c = FabricClient::with_meta_url(s.uri());
        assert!(matches!(c.fetch_profile("1.21.1", "9.9.9").await, Err(Error::NotFound(_))));
        assert!(matches!(c.list_loader_versions("0.0").await, Err(Error::NotFound(_))));
    }

    #[tokio::test]
    async fn server_error_is_network() {
        let s = MockServer::start().await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(500)).mount(&s).await;
        let c = FabricClient::with_meta_url(s.uri());
        assert!(matches!(c.list_loader_versions("1.21.1").await, Err(Error::Network(_))));
    }

    #[tokio::test]
    async fn installs_three_libs_skipping_existing() {
        let s = MockServer::start().await;
        // fabric-loader déjà présent et valide : aucune requête attendue.
        serve(&s, "/maven/net/fabricmc/fabric-loader/0.16.5/fabric-loader-0.16.5.jar", A, 0).await;
        serve(&s, "/maven/org/ow2/asm/asm/9.7/asm-9.7.jar", B, 1).await;
        serve(&s, "/maven/net/fabricmc/intermediary/1.21.1/intermediary-1.21.1.jar", C, 1).await;

        let tmp = tempfile::tempdir().unwrap();
        let (libs, vers) = (tmp.path().join("libraries"), tmp.path().join("versions"));
        let existing = libs.join("net/fabricmc/fabric-loader/0.16.5/fabric-loader-0.16.5.jar");
        std::fs::create_dir_all(existing.parent().unwrap()).unwrap();
        std::fs::write(&existing, A).unwrap();

        let c = FabricClient::with_meta_url(s.uri());
        let inst = c.install(&profile(&s), &libs, &vers).await.unwrap();
        assert_eq!(inst.id, "fabric-loader-0.16.5-1.21.1");
        assert_eq!(
            inst.json_path,
            vers.join("fabric-loader-0.16.5-1.21.1").join("fabric-loader-0.16.5-1.21.1.json")
        );
        let written: Value = serde_json::from_slice(&std::fs::read(&inst.json_path).unwrap()).unwrap();
        assert_eq!(written["mainClass"], "net.fabricmc.loader.impl.launch.knot.KnotClient");
        assert_eq!(std::fs::read(libs.join("org/ow2/asm/asm/9.7/asm-9.7.jar")).unwrap(), B);
        assert_eq!(std::fs::read(libs.join("net/fabricmc/intermediary/1.21.1/intermediary-1.21.1.jar")).unwrap(), C);
        assert!(!inst.json_path.with_extension("json.tmp").exists());
        // wiremock vérifie les `expect` au drop du serveur.
    }

    #[tokio::test]
    async fn corrupted_existing_lib_is_redownloaded_and_bad_sha1_rejected() {
        let s = MockServer::start().await;
        serve(&s, "/maven/net/fabricmc/fabric-loader/0.16.5/fabric-loader-0.16.5.jar", A, 1).await;
        let p = json!({
            "id": "fabric-loader-0.16.5-1.21.1",
            "libraries": [{"name": "net.fabricmc:fabric-loader:0.16.5", "url": format!("{}/maven/", s.uri()), "sha1": sha1_hex(A)}]
        });
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("l/net/fabricmc/fabric-loader/0.16.5/fabric-loader-0.16.5.jar");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, b"corrompu").unwrap();
        let c = FabricClient::with_meta_url(s.uri());
        c.install(&p, &tmp.path().join("l"), &tmp.path().join("v")).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), A);

        let bad = json!({
            "id": "x",
            "libraries": [{"name": "a:b:1", "url": format!("{}/maven/", s.uri()), "sha1": "0".repeat(40)}]
        });
        serve(&s, "/maven/a/b/1/b-1.jar", B, 1).await;
        let err = c.install(&bad, &tmp.path().join("l"), &tmp.path().join("v")).await.unwrap_err();
        assert!(matches!(err, Error::Integrity { .. }), "{err:?}");
        assert!(!tmp.path().join("v/x/x.json").exists());
    }

    #[tokio::test]
    async fn missing_lib_404_is_not_found_and_unreachable_is_network() {
        let s = MockServer::start().await;
        let p = json!({"id": "x", "libraries": [{"name": "a:b:1", "url": format!("{}/maven/", s.uri())}]});
        let tmp = tempfile::tempdir().unwrap();
        let c = FabricClient::with_meta_url(s.uri());
        let err = c.install(&p, &tmp.path().join("l"), &tmp.path().join("v")).await.unwrap_err();
        assert!(matches!(err, Error::NotFound(_)), "{err:?}");

        let dead = json!({"id": "x", "libraries": [{"name": "a:b:1", "url": "http://127.0.0.1:1/"}]});
        let err = c.install(&dead, &tmp.path().join("l"), &tmp.path().join("v")).await.unwrap_err();
        assert!(matches!(err, Error::Network(_)), "{err:?}");
    }

    #[tokio::test]
    async fn rejects_dangerous_profile_id() {
        let tmp = tempfile::tempdir().unwrap();
        let c = FabricClient::new();
        let p = json!({"id": "../evil", "libraries": []});
        assert!(matches!(c.install(&p, tmp.path(), tmp.path()).await, Err(Error::Invalid(_))));
    }

    #[test]
    fn merges_with_vanilla() {
        let s_uri = "http://x";
        let p = json!({
            "id": "fabric-loader-0.16.5-1.21.1",
            "inheritsFrom": "1.21.1",
            "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
            "arguments": {"game": ["--fabric"], "jvm": ["-DFabricMcEmu=x"]},
            "libraries": [
                {"name": "net.fabricmc:fabric-loader:0.16.5", "url": s_uri},
                {"name": "org.ow2.asm:asm:9.7", "url": s_uri}
            ]
        });
        let m = merge_with_vanilla(&vanilla(), &p);
        assert_eq!(m.id, "fabric-loader-0.16.5-1.21.1");
        assert_eq!(m.inherits_from, "1.21.1");
        assert_eq!(m.main_class, "net.fabricmc.loader.impl.launch.knot.KnotClient");
        let names: Vec<&str> = m.libraries.iter().map(|l| l["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["net.fabricmc:fabric-loader:0.16.5", "org.ow2.asm:asm:9.7", "com.google:guava:31"]);
        assert_eq!(m.jvm_args, vec![json!("-Xss1M"), json!("-DFabricMcEmu=x")]);
        assert_eq!(m.game_args, vec![json!("--demo"), json!("--fabric")]);
        assert_eq!(m.asset_index.id, "17");
        assert_eq!(m.downloads.client.sha1, "bb");
        assert_eq!(m.java_version.unwrap().major_version, 21);
    }

    #[test]
    fn merge_without_arguments_in_profile() {
        let p = json!({"id": "f", "mainClass": "K", "libraries": []});
        let m = merge_with_vanilla(&vanilla(), &p);
        assert_eq!(m.main_class, "K");
        assert_eq!(m.libraries.len(), 2);
        assert_eq!(m.jvm_args, vec![json!("-Xss1M")]);
        assert_eq!(m.inherits_from, "1.21.1");
    }
}
