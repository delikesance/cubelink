//! Tâche L4 : installation du loader Forge (deux ères).
//!
//! * Moderne (Minecraft >= 1.13) : `install_profile.json` + `version.json` + processeurs, comme NeoForge
//!   (les types et l'exécution des processeurs de `neoforge` sont réutilisés).
//! * Ancienne (<= 1.12.2) : `install_profile.json` avec `versionInfo` et jar universel à copier ; sans processeurs.
//!
//! Flux : `list_versions` / `recommended` -> `download_installer` -> `parse_installer` -> `install`
//! (libraries, version.json, processeurs via `ProcessRunner`) -> `merge_with_vanilla`.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use crate::loader::fabric::{InstalledProfile, LoaderVersion, MergedVersion};
pub use crate::loader::neoforge::{
    InstallerProfile, ProcessContext, ProcessReport, ProcessRunner, ProcessSpec,
};
use crate::loader::neoforge::{self, NeoForgeClient};
use crate::mojang::libraries::maven_path;
use crate::mojang::versions::{sha1_hex, VersionJson};
use crate::types::{Error, Result};

pub const DEFAULT_MAVEN_URL: &str = "https://maven.minecraftforge.net";
pub const DEFAULT_FILES_URL: &str = "https://files.minecraftforge.net";

const MAX_ENTRY_BYTES: u64 = 256 * 1024 * 1024;
const MAX_JSON_BYTES: u64 = 16 * 1024 * 1024;
const MAX_ENTRIES: usize = 100_000;
const MAX_INSTALLER_BYTES: usize = 512 * 1024 * 1024;
const MAX_LIST_BYTES: usize = 32 * 1024 * 1024;

/// Ère de l'installeur Forge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Era {
    /// <= 1.12.2 : `versionInfo` + jar universel, pas de processeurs.
    Legacy,
    /// >= 1.13 : `version.json` + processeurs.
    Modern,
}

/// Ère attendue pour une version Minecraft (une version illisible est considérée comme moderne).
pub fn era_for(minecraft: &str) -> Era {
    let minor = minecraft.split('.').nth(1).and_then(|m| m.parse::<u32>().ok());
    match minor {
        Some(m) if m <= 12 => Era::Legacy,
        _ => Era::Modern,
    }
}

fn check_token(what: &str, s: &str) -> Result<()> {
    let ok = !s.is_empty()
        && s != ".."
        && !s.contains("..")
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+' | b'_'));
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(format!("{what} invalide « {s} »")))
    }
}

fn check_rel(rel: &str) -> Result<()> {
    if rel.is_empty() || rel.starts_with('/') || rel.contains('\\') || rel.split('/').any(|s| s == "..") {
        return Err(Error::Invalid(format!("chemin invalide « {rel} »")));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Versions
// ---------------------------------------------------------------------------------------------

/// Promotions Forge (`promotions_slim.json`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Promotions {
    promos: std::collections::BTreeMap<String, String>,
}

impl Promotions {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        #[derive(Deserialize)]
        struct Raw {
            promos: std::collections::BTreeMap<String, String>,
        }
        let raw: Raw = serde_json::from_slice(bytes)
            .map_err(|e| Error::Network(format!("promotions Forge invalides : {e}")))?;
        Ok(Self { promos: raw.promos })
    }

    pub fn recommended(&self, minecraft: &str) -> Option<&str> {
        self.promos.get(&format!("{minecraft}-recommended")).map(String::as_str)
    }

    pub fn latest(&self, minecraft: &str) -> Option<&str> {
        self.promos.get(&format!("{minecraft}-latest")).map(String::as_str)
    }
}

/// Extrait les `<version>` du `maven-metadata.xml` (balayage simple, sans dépendance XML).
pub fn parse_maven_metadata(xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(s) = rest.find("<version>") {
        let after = &rest[s + "<version>".len()..];
        let Some(e) = after.find("</version>") else { break };
        out.push(after[..e].trim().to_string());
        rest = &after[e + "</version>".len()..];
    }
    out
}

fn sort_key(v: &str) -> Vec<u64> {
    v.split('-').next().unwrap_or("").split('.').map(|p| p.parse::<u64>().unwrap_or(0)).collect()
}

/// Filtre `<mc>-<version>` et trie, de la plus récente à la plus ancienne. `version` = partie après `<mc>-`
/// (ex. `47.2.0` ou `10.13.4.1614-1.7.10`). `stable` = aucune lettre dans la version.
pub fn filter_versions(all: &[String], minecraft: &str) -> Result<Vec<LoaderVersion>> {
    check_token("version Minecraft", minecraft)?;
    let prefix = format!("{minecraft}-");
    let mut out: Vec<LoaderVersion> = all
        .iter()
        .filter_map(|v| v.strip_prefix(&prefix))
        .filter(|v| !v.is_empty())
        .map(|v| LoaderVersion { version: v.to_string(), stable: !v.bytes().any(|b| b.is_ascii_alphabetic()) })
        .collect();
    out.sort_by(|a, b| sort_key(&b.version).cmp(&sort_key(&a.version)).then_with(|| b.version.cmp(&a.version)));
    out.dedup_by(|a, b| a.version == b.version);
    Ok(out)
}

pub struct ForgeClient {
    http: reqwest::Client,
    maven_url: String,
    files_url: String,
}

impl Default for ForgeClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ForgeClient {
    pub fn new() -> Self {
        Self::with_urls(DEFAULT_MAVEN_URL, DEFAULT_FILES_URL)
    }

    /// `maven_url` : base du Maven Forge ; `files_url` : base de `files.minecraftforge.net` (promotions).
    pub fn with_urls(maven_url: impl Into<String>, files_url: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            maven_url: maven_url.into().trim_end_matches('/').to_string(),
            files_url: files_url.into().trim_end_matches('/').to_string(),
        }
    }

    async fn get_bytes(&self, url: &str, limit: usize) -> Result<Vec<u8>> {
        let resp = self.http.get(url).send().await.map_err(|e| Error::Network(e.to_string()))?;
        let status = resp.status();
        if status.as_u16() == 404 {
            return Err(Error::NotFound(url.to_string()));
        }
        if !status.is_success() {
            return Err(Error::Network(format!("HTTP {} pour {url}", status.as_u16())));
        }
        if resp.content_length().is_some_and(|l| l > limit as u64) {
            return Err(Error::Invalid(format!("réponse trop volumineuse pour {url}")));
        }
        let bytes = resp.bytes().await.map_err(|e| Error::Network(e.to_string()))?;
        if bytes.len() > limit {
            return Err(Error::Invalid(format!("réponse trop volumineuse pour {url}")));
        }
        Ok(bytes.to_vec())
    }

    /// Versions Forge pour `minecraft`, de la plus récente à la plus ancienne.
    pub async fn list_versions(&self, minecraft: &str) -> Result<Vec<LoaderVersion>> {
        check_token("version Minecraft", minecraft)?;
        let url = format!("{}/net/minecraftforge/forge/maven-metadata.xml", self.maven_url);
        let bytes = self.get_bytes(&url, MAX_LIST_BYTES).await?;
        filter_versions(&parse_maven_metadata(&String::from_utf8_lossy(&bytes)), minecraft)
    }

    async fn promotions(&self) -> Result<Promotions> {
        let url = format!("{}/net/minecraftforge/forge/promotions_slim.json", self.files_url);
        Promotions::parse(&self.get_bytes(&url, MAX_JSON_BYTES as usize).await?)
    }

    /// Version recommandée pour `minecraft` (`NotFound` s'il n'y en a pas).
    pub async fn recommended(&self, minecraft: &str) -> Result<String> {
        check_token("version Minecraft", minecraft)?;
        self.promotions()
            .await?
            .recommended(minecraft)
            .map(str::to_string)
            .ok_or_else(|| Error::NotFound(format!("Forge recommandé pour Minecraft {minecraft}")))
    }

    /// Dernière version (`latest`) pour `minecraft`.
    pub async fn latest(&self, minecraft: &str) -> Result<String> {
        check_token("version Minecraft", minecraft)?;
        self.promotions()
            .await?
            .latest(minecraft)
            .map(str::to_string)
            .ok_or_else(|| Error::NotFound(format!("Forge latest pour Minecraft {minecraft}")))
    }

    /// Télécharge l'installeur de `<minecraft>-<version>` dans `dest` (atomique) après vérification du `.sha1`.
    /// Les anciennes versions sont aussi publiées sous `<mc>-<version>-<mc>` : essayé en second.
    pub async fn download_installer(&self, minecraft: &str, version: &str, dest: &Path) -> Result<()> {
        check_token("version Minecraft", minecraft)?;
        check_token("version Forge", version)?;
        let ids = [format!("{minecraft}-{version}"), format!("{minecraft}-{version}-{minecraft}")];
        let mut found = None;
        for id in &ids {
            let url = format!("{}/net/minecraftforge/forge/{id}/forge-{id}-installer.jar", self.maven_url);
            match self.get_bytes(&url, MAX_INSTALLER_BYTES).await {
                Ok(b) => {
                    found = Some((url, b));
                    break;
                }
                Err(Error::NotFound(_)) => continue,
                Err(e) => return Err(e),
            }
        }
        let (url, bytes) =
            found.ok_or_else(|| Error::NotFound(format!("installeur Forge {minecraft}-{version}")))?;
        let sha_text = self.get_bytes(&format!("{url}.sha1"), 4096).await?;
        let expected =
            String::from_utf8_lossy(&sha_text).split_whitespace().next().unwrap_or("").to_lowercase();
        let actual = sha1_hex(&bytes);
        if expected.is_empty() || actual != expected {
            return Err(Error::Integrity { expected, actual });
        }
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        write_atomic(dest, &bytes)
    }
}

fn write_atomic(dest: &Path, data: &[u8]) -> Result<()> {
    let mut tmp = dest.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, data)?;
    if let Err(e) = std::fs::rename(&tmp, dest) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

/// Versions Forge (client par défaut).
pub async fn list_versions(minecraft: &str) -> Result<Vec<LoaderVersion>> {
    ForgeClient::new().list_versions(minecraft).await
}

/// Version recommandée (client par défaut).
pub async fn recommended(minecraft: &str) -> Result<String> {
    ForgeClient::new().recommended(minecraft).await
}

/// Téléchargement de l'installeur (client par défaut).
pub async fn download_installer(minecraft: &str, version: &str, dest: &Path) -> Result<()> {
    ForgeClient::new().download_installer(minecraft, version, dest).await
}

// ---------------------------------------------------------------------------------------------
// Installeur
// ---------------------------------------------------------------------------------------------

/// Jar universel d'un installeur ancienne ère.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Universal {
    /// Coordonnées Maven de destination (`install.path`).
    pub coords: String,
    /// Entrée du zip (`install.filePath`).
    pub entry: String,
}

#[derive(Debug, Clone)]
pub struct ForgeInstaller {
    pub era: Era,
    /// Profil commun aux deux ères (ancienne : pas de processeurs, `version_json` = `versionInfo`).
    pub profile: InstallerProfile,
    /// Présent pour l'ancienne ère.
    pub universal: Option<Universal>,
}

fn open_checked(path: &Path) -> Result<zip::ZipArchive<std::fs::File>> {
    let file = std::fs::File::open(path)?;
    let mut ar = zip::ZipArchive::new(file).map_err(|e| Error::Invalid(format!("installeur illisible : {e}")))?;
    if ar.len() > MAX_ENTRIES {
        return Err(Error::Invalid("installeur : trop d'entrées".into()));
    }
    for i in 0..ar.len() {
        let entry = ar.by_index(i).map_err(|e| Error::Invalid(format!("installeur corrompu : {e}")))?;
        if entry.enclosed_name().is_none() {
            return Err(Error::Invalid(format!("installeur : entrée dangereuse « {} »", String::from_utf8_lossy(entry.name_raw()))));
        }
    }
    Ok(ar)
}

fn read_entry(ar: &mut zip::ZipArchive<std::fs::File>, name: &str, limit: u64) -> Result<Vec<u8>> {
    let name = name.trim_start_matches('/');
    check_rel(name)?;
    let entry = ar.by_name(name).map_err(|_| Error::NotFound(format!("{name} dans l'installeur")))?;
    if entry.size() > limit {
        return Err(Error::Invalid(format!("installeur : « {name} » trop volumineux")));
    }
    let mut buf = Vec::new();
    entry.take(limit + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > limit {
        return Err(Error::Invalid(format!("installeur : « {name} » trop volumineux")));
    }
    Ok(buf)
}

/// Lit le jar installeur (zip sûr, tailles bornées) et détecte l'ère d'après son contenu.
pub fn parse_installer(jar: &Path) -> Result<ForgeInstaller> {
    let mut ar = open_checked(jar)?;
    let raw: Value = serde_json::from_slice(&read_entry(&mut ar, "install_profile.json", MAX_JSON_BYTES)?)
        .map_err(|e| Error::Invalid(format!("install_profile.json invalide : {e}")))?;
    drop(ar);
    let Some(info) = raw.get("versionInfo") else {
        let profile = neoforge::parse_installer(jar)?;
        return Ok(ForgeInstaller { era: Era::Modern, profile, universal: None });
    };

    let id = info.get("id").and_then(Value::as_str).unwrap_or("");
    check_token("identifiant de version", id)?;
    let install = raw.get("install").ok_or_else(|| Error::Invalid("install_profile.json sans « install »".into()))?;
    let field = |k: &str| install.get(k).and_then(Value::as_str).filter(|s| !s.is_empty());
    let coords = field("path").ok_or_else(|| Error::Invalid("install.path manquant".into()))?;
    let entry = field("filePath").ok_or_else(|| Error::Invalid("install.filePath manquant".into()))?;
    check_rel(entry)?;
    maven_path(coords)?;
    let minecraft = field("minecraft")
        .or_else(|| info.get("inheritsFrom").and_then(Value::as_str))
        .unwrap_or("")
        .to_string();
    let libraries = info.get("libraries").and_then(Value::as_array).cloned().unwrap_or_default();
    let profile = InstallerProfile {
        installer_path: jar.to_path_buf(),
        id: id.to_string(),
        minecraft,
        data: Default::default(),
        processors: Vec::new(),
        libraries,
        version_json: info.clone(),
    };
    Ok(ForgeInstaller {
        era: Era::Legacy,
        profile,
        universal: Some(Universal { coords: coords.to_string(), entry: entry.to_string() }),
    })
}

/// Copie le jar universel de l'installeur vers `libs_dir` (ancienne ère ; no-op sinon).
pub fn extract_universal(inst: &ForgeInstaller, libs_dir: &Path) -> Result<()> {
    let Some(u) = &inst.universal else { return Ok(()) };
    let rel = maven_path(&u.coords)?;
    check_rel(&rel)?;
    let bytes = read_entry(&mut open_checked(&inst.profile.installer_path)?, &u.entry, MAX_ENTRY_BYTES)?;
    if bytes.is_empty() {
        return Err(Error::Invalid(format!("jar universel vide « {} »", u.entry)));
    }
    let dest = libs_dir.join(rel);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_atomic(&dest, &bytes)
}

/// Fusion du profil Forge avec le JSON vanilla (`minecraftArguments` du profil prioritaire pour l'ancienne ère).
pub fn merge_with_vanilla(vanilla: &VersionJson, inst: &ForgeInstaller) -> MergedVersion {
    let mut merged = neoforge::merge_with_vanilla(vanilla, &inst.profile);
    if let Some(args) = inst.profile.version_json.get("minecraftArguments").and_then(Value::as_str) {
        merged.minecraft_arguments = Some(args.to_string());
    }
    merged
}

/// Installation complète : libraries (ou jar universel), `version.json` atomique, puis processeurs.
pub async fn install(
    inst: &ForgeInstaller,
    ctx: &ProcessContext,
    versions_dir: &Path,
    runner: &dyn ProcessRunner,
) -> Result<InstalledProfile> {
    match inst.era {
        Era::Legacy => extract_universal(inst, &ctx.libs_dir)?,
        Era::Modern => NeoForgeClient::new().install_libraries(&inst.profile, &ctx.libs_dir).await?,
    }
    let installed = neoforge::write_version_json(&inst.profile, versions_dir).await?;
    neoforge::run_processors(&inst.profile, ctx, runner)?;
    Ok(installed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::RefCell;
    use std::io::Write;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn make_zip(path: &Path, files: &[(&str, &[u8])]) {
        let mut w = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        for (n, d) in files {
            w.start_file(*n, opts).unwrap();
            w.write_all(d).unwrap();
        }
        w.finish().unwrap();
    }

    #[test]
    fn era_by_version() {
        assert_eq!(era_for("1.7.10"), Era::Legacy);
        assert_eq!(era_for("1.12.2"), Era::Legacy);
        assert_eq!(era_for("1.13"), Era::Modern);
        assert_eq!(era_for("1.20.1"), Era::Modern);
        assert_eq!(era_for("1.21.1"), Era::Modern);
        assert_eq!(era_for("n'importe quoi"), Era::Modern);
    }

    const XML: &str = "<metadata><groupId>net.minecraftforge</groupId><artifactId>forge</artifactId><versioning>\
        <latest>1.21.1-52.0.1</latest><versions>\
        <version>1.7.10-10.13.4.1614-1.7.10</version><version>1.7.10-10.13.4.1614</version>\
        <version>1.20.1-47.2.0</version><version>1.20.1-47.10.0</version><version>1.20.1-47.3.0</version>\
        <version>1.20.1-47.4.0-beta</version><version>1.21.1-52.0.1</version></versions></versioning></metadata>";

    #[test]
    fn parses_and_sorts_metadata() {
        let all = parse_maven_metadata(XML);
        assert_eq!(all.len(), 7);
        let v = filter_versions(&all, "1.20.1").unwrap();
        let names: Vec<&str> = v.iter().map(|x| x.version.as_str()).collect();
        assert_eq!(names, ["47.10.0", "47.4.0-beta", "47.3.0", "47.2.0"]);
        assert!(!v[1].stable && v[0].stable);
        assert_eq!(filter_versions(&all, "1.7.10").unwrap().len(), 2);
        assert!(filter_versions(&all, "../x").is_err());
    }

    #[test]
    fn parses_promotions() {
        let p = Promotions::parse(
            br#"{"homepage":"x","promos":{"1.20.1-latest":"47.3.0","1.20.1-recommended":"47.2.0","1.7.10-latest":"10.13.4.1614"}}"#,
        )
        .unwrap();
        assert_eq!(p.recommended("1.20.1"), Some("47.2.0"));
        assert_eq!(p.latest("1.20.1"), Some("47.3.0"));
        assert_eq!(p.recommended("1.7.10"), None);
        assert!(Promotions::parse(b"nope").is_err());
    }

    #[tokio::test]
    async fn client_lists_and_recommends() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/net/minecraftforge/forge/maven-metadata.xml"))
            .respond_with(ResponseTemplate::new(200).set_body_string(XML))
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path("/net/minecraftforge/forge/promotions_slim.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"promos": {"1.20.1-recommended": "47.2.0", "1.20.1-latest": "47.3.0"}}),
            ))
            .mount(&s)
            .await;
        let c = ForgeClient::with_urls(format!("{}/", s.uri()), s.uri());
        assert_eq!(c.list_versions("1.20.1").await.unwrap().len(), 4);
        assert_eq!(c.recommended("1.20.1").await.unwrap(), "47.2.0");
        assert_eq!(c.latest("1.20.1").await.unwrap(), "47.3.0");
        assert!(matches!(c.recommended("1.19.2").await, Err(Error::NotFound(_))));
    }

    #[tokio::test]
    async fn downloads_installer_with_sha1_and_legacy_fallback() {
        let body = b"fake-installer".to_vec();
        let s = MockServer::start().await;
        // Ancienne ère : seul `<mc>-<v>-<mc>` existe.
        let base = "/net/minecraftforge/forge/1.7.10-10.13.4.1614-1.7.10/forge-1.7.10-10.13.4.1614-1.7.10-installer.jar";
        Mock::given(method("GET")).and(path(base)).respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone())).mount(&s).await;
        Mock::given(method("GET"))
            .and(path(format!("{base}.sha1")))
            .respond_with(ResponseTemplate::new(200).set_body_string(sha1_hex(&body)))
            .mount(&s)
            .await;
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("dl/i.jar");
        let c = ForgeClient::with_urls(s.uri(), s.uri());
        c.download_installer("1.7.10", "10.13.4.1614", &dest).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);

        let s2 = MockServer::start().await;
        let base2 = "/net/minecraftforge/forge/1.20.1-47.2.0/forge-1.20.1-47.2.0-installer.jar";
        Mock::given(method("GET")).and(path(base2)).respond_with(ResponseTemplate::new(200).set_body_bytes(body)).mount(&s2).await;
        Mock::given(method("GET"))
            .and(path(format!("{base2}.sha1")))
            .respond_with(ResponseTemplate::new(200).set_body_string("0".repeat(40)))
            .mount(&s2)
            .await;
        let bad = tmp.path().join("bad.jar");
        let err = ForgeClient::with_urls(s2.uri(), s2.uri()).download_installer("1.20.1", "47.2.0", &bad).await.unwrap_err();
        assert!(matches!(err, Error::Integrity { .. }), "{err:?}");
        assert!(!bad.exists());
        assert!(matches!(c.download_installer("1.20.1", "../x", &bad).await, Err(Error::Invalid(_))));
        assert!(matches!(c.download_installer("1.20.1", "47.9.9", &bad).await, Err(Error::NotFound(_))));
    }

    const OUT: &[u8] = b"patched-forge";

    fn modern_jar(dir: &Path) -> PathBuf {
        let jar = dir.join("modern.jar");
        let profile = json!({
            "version": "1.20.1-forge-47.2.0", "json": "/version.json", "minecraft": "1.20.1",
            "data": {
                "BINPATCH": {"client": "/data/client.lzma", "server": "/data/server.lzma"},
                "PATCHED": {"client": "[net.minecraftforge:forge:1.20.1-47.2.0:client]", "server": "[x:y:1]"}
            },
            "processors": [
                {"sides": ["server"], "jar": "a:server:1", "args": []},
                {"jar": "net.minecraftforge:binarypatcher:1.1.1", "classpath": ["org.ow2.asm:asm:9.7"],
                 "args": ["--clean", "{MINECRAFT_JAR}", "--output", "{PATCHED}", "--apply", "{BINPATCH}"],
                 "outputs": {"{PATCHED}": format!("'{}'", sha1_hex(OUT))}}
            ],
            "libraries": [{"name": "org.ow2.asm:asm:9.7"}]
        });
        let version = json!({"id": "1.20.1-forge-47.2.0", "inheritsFrom": "1.20.1",
            "mainClass": "cpw.mods.bootstraplauncher.BootstrapLauncher",
            "arguments": {"game": ["--fml.forgeVersion", "47.2.0"], "jvm": ["-DX=1"]},
            "libraries": [{"name": "net.minecraftforge:forge:1.20.1-47.2.0"}]});
        make_zip(&jar, &[
            ("install_profile.json", profile.to_string().as_bytes()),
            ("version.json", version.to_string().as_bytes()),
            ("data/client.lzma", b"binpatch"),
        ]);
        jar
    }

    fn legacy_jar(dir: &Path) -> PathBuf {
        let jar = dir.join("legacy.jar");
        let profile = json!({
            "install": {"profileName": "Forge", "target": "1.7.10-Forge10.13.4.1614-1.7.10",
                "path": "net.minecraftforge:forge:1.7.10-10.13.4.1614-1.7.10",
                "filePath": "forge-1.7.10-10.13.4.1614-1.7.10-universal.jar", "minecraft": "1.7.10"},
            "versionInfo": {"id": "1.7.10-Forge10.13.4.1614-1.7.10", "mainClass": "net.minecraft.launchwrapper.Launch",
                "minecraftArguments": "--username ${auth_player_name} --tweakClass cpw.mods.fml.common.launcher.FMLTweaker",
                "libraries": [{"name": "net.minecraftforge:forge:1.7.10-10.13.4.1614-1.7.10"},
                              {"name": "net.minecraft:launchwrapper:1.12"}]}
        });
        make_zip(&jar, &[
            ("install_profile.json", profile.to_string().as_bytes()),
            ("forge-1.7.10-10.13.4.1614-1.7.10-universal.jar", b"universal-bytes"),
        ]);
        jar
    }

    fn ctx(dir: &Path) -> ProcessContext {
        ProcessContext {
            libs_dir: dir.join("libraries"),
            minecraft_jar: dir.join("mc/1.20.1.jar"),
            root: dir.to_path_buf(),
            work_dir: dir.join("work"),
        }
    }

    struct Fake {
        calls: RefCell<Vec<ProcessSpec>>,
        write: Option<(PathBuf, Vec<u8>)>,
        fail: bool,
    }

    impl ProcessRunner for Fake {
        fn run(&self, spec: &ProcessSpec) -> Result<()> {
            self.calls.borrow_mut().push(spec.clone());
            if self.fail {
                return Err(Error::Other("exit code 1".into()));
            }
            if let Some((p, d)) = &self.write {
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(p, d).unwrap();
            }
            Ok(())
        }
    }

    fn patched(c: &ProcessContext) -> PathBuf {
        c.libs_dir.join("net/minecraftforge/forge/1.20.1-47.2.0/forge-1.20.1-47.2.0-client.jar")
    }

    #[test]
    fn parses_modern_installer() {
        let tmp = tempfile::tempdir().unwrap();
        let i = parse_installer(&modern_jar(tmp.path())).unwrap();
        assert_eq!(i.era, Era::Modern);
        assert!(i.universal.is_none());
        assert_eq!(i.profile.id, "1.20.1-forge-47.2.0");
        assert_eq!(i.profile.minecraft, "1.20.1");
        assert_eq!(i.profile.processors.len(), 2);
    }

    #[test]
    fn parses_legacy_installer() {
        let tmp = tempfile::tempdir().unwrap();
        let i = parse_installer(&legacy_jar(tmp.path())).unwrap();
        assert_eq!(i.era, Era::Legacy);
        assert_eq!(i.profile.id, "1.7.10-Forge10.13.4.1614-1.7.10");
        assert_eq!(i.profile.minecraft, "1.7.10");
        assert!(i.profile.processors.is_empty());
        assert_eq!(i.profile.libraries.len(), 2);
        assert_eq!(i.universal.as_ref().unwrap().entry, "forge-1.7.10-10.13.4.1614-1.7.10-universal.jar");
    }

    #[tokio::test]
    async fn installs_legacy_without_processors() {
        let tmp = tempfile::tempdir().unwrap();
        let i = parse_installer(&legacy_jar(tmp.path())).unwrap();
        let c = ctx(tmp.path());
        let fake = Fake { calls: RefCell::new(vec![]), write: None, fail: true };
        let vers = tmp.path().join("versions");
        let done = install(&i, &c, &vers, &fake).await.unwrap();
        assert!(fake.calls.borrow().is_empty());
        assert_eq!(done.json_path, vers.join("1.7.10-Forge10.13.4.1614-1.7.10/1.7.10-Forge10.13.4.1614-1.7.10.json"));
        let uni = c.libs_dir.join("net/minecraftforge/forge/1.7.10-10.13.4.1614-1.7.10/forge-1.7.10-10.13.4.1614-1.7.10.jar");
        assert_eq!(std::fs::read(uni).unwrap(), b"universal-bytes");

        let vanilla: VersionJson = serde_json::from_value(json!({
            "id": "1.7.10", "mainClass": "net.minecraft.client.main.Main", "minecraftArguments": "--vanilla",
            "assetIndex": {"id":"1.7.10","url":"u","sha1":"aa","size":1},
            "downloads": {"client": {"url":"u","sha1":"bb","size":2}},
            "libraries": [{"name":"com.google:guava:31"}]
        }))
        .unwrap();
        let m = merge_with_vanilla(&vanilla, &i);
        assert_eq!(m.main_class, "net.minecraft.launchwrapper.Launch");
        assert!(m.minecraft_arguments.unwrap().contains("FMLTweaker"));
        assert_eq!(m.libraries.len(), 3);
    }

    #[tokio::test]
    async fn installs_modern_with_processors() {
        let tmp = tempfile::tempdir().unwrap();
        let i = parse_installer(&modern_jar(tmp.path())).unwrap();
        let c = ctx(tmp.path());
        // La library de l'installeur n'a ni url ni sha1 : on la pré-installe (sinon extraction depuis maven/).
        let asm = c.libs_dir.join("org/ow2/asm/asm/9.7/asm-9.7.jar");
        std::fs::create_dir_all(asm.parent().unwrap()).unwrap();
        std::fs::write(&asm, b"asm").unwrap();
        let fake = Fake { calls: RefCell::new(vec![]), write: Some((patched(&c), OUT.to_vec())), fail: false };
        let vers = tmp.path().join("versions");
        let done = install(&i, &c, &vers, &fake).await.unwrap();
        assert!(done.json_path.exists());
        let calls = fake.calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].args[3], patched(&c).to_string_lossy());
    }

    #[tokio::test]
    async fn runner_failure_is_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let i = parse_installer(&modern_jar(tmp.path())).unwrap();
        let c = ctx(tmp.path());
        let asm = c.libs_dir.join("org/ow2/asm/asm/9.7/asm-9.7.jar");
        std::fs::create_dir_all(asm.parent().unwrap()).unwrap();
        std::fs::write(&asm, b"asm").unwrap();
        let fake = Fake { calls: RefCell::new(vec![]), write: None, fail: true };
        let err = install(&i, &c, &tmp.path().join("v"), &fake).await.unwrap_err();
        assert!(err.to_string().contains("exit code 1"), "{err}");
        // Sortie au mauvais hash : Integrity.
        let bad = Fake { calls: RefCell::new(vec![]), write: Some((patched(&c), b"autre".to_vec())), fail: false };
        let err = install(&i, &c, &tmp.path().join("v"), &bad).await.unwrap_err();
        assert!(matches!(err, Error::Integrity { .. }), "{err:?}");
    }

    #[test]
    fn malicious_zips_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let evil = tmp.path().join("evil.jar");
        make_zip(&evil, &[("install_profile.json", b"{}"), ("../evil.txt", b"x")]);
        assert!(matches!(parse_installer(&evil), Err(Error::Invalid(_))));
        let garbage = tmp.path().join("g.jar");
        std::fs::write(&garbage, b"pas un zip").unwrap();
        assert!(matches!(parse_installer(&garbage), Err(Error::Invalid(_))));
        // id ancienne ère avec `..`, et filePath hors de l'archive.
        let bad_id = tmp.path().join("id.jar");
        let p = json!({"install": {"path": "a:b:1", "filePath": "x.jar"}, "versionInfo": {"id": "../x"}});
        make_zip(&bad_id, &[("install_profile.json", p.to_string().as_bytes())]);
        assert!(matches!(parse_installer(&bad_id), Err(Error::Invalid(_))));
        let bad_path = tmp.path().join("fp.jar");
        let p = json!({"install": {"path": "a:b:1", "filePath": "../x.jar"}, "versionInfo": {"id": "ok"}});
        make_zip(&bad_path, &[("install_profile.json", p.to_string().as_bytes())]);
        assert!(matches!(parse_installer(&bad_path), Err(Error::Invalid(_))));
    }

    #[test]
    fn oversized_json_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let jar = tmp.path().join("big.jar");
        let big = vec![b' '; (MAX_JSON_BYTES + 1) as usize];
        make_zip(&jar, &[("install_profile.json", &big)]);
        assert!(matches!(parse_installer(&jar), Err(Error::Invalid(_))));
    }
}
