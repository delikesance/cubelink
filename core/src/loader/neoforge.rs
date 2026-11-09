//! Tâche L1 : installation du loader NeoForge.
//!
//! Flux : `list_versions` -> `download_installer` -> `parse_installer` -> `install_libraries` ->
//! `run_processors` (Java injectable) -> `write_version_json` -> `merge_with_vanilla`.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use crate::loader::fabric::{InstalledProfile, LoaderVersion, MergedVersion};
use crate::mojang::libraries::maven_path;
use crate::mojang::versions::{sha1_hex, VersionJson};
use crate::types::{Error, Result};

pub const DEFAULT_MAVEN_URL: &str = "https://maven.neoforged.net";

/// Taille maximale d'une entrée lue ou extraite de l'installeur.
const MAX_ENTRY_BYTES: u64 = 256 * 1024 * 1024;
/// Taille maximale des JSON de l'installeur.
const MAX_JSON_BYTES: u64 = 16 * 1024 * 1024;
const MAX_ENTRIES: usize = 100_000;

// ---------------------------------------------------------------------------------------------
// Versions et téléchargement
// ---------------------------------------------------------------------------------------------

/// Préfixe NeoForge d'une version Minecraft : `1.21.1` -> `21.1.`, `1.21` -> `21.0.`.
pub fn version_prefix(minecraft: &str) -> Result<String> {
    let parts: Vec<&str> = minecraft.split('.').collect();
    let ok = (2..=3).contains(&parts.len())
        && parts[0] == "1"
        && parts[1..].iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    if !ok {
        return Err(Error::Invalid(format!("version Minecraft non prise en charge par NeoForge « {minecraft} »")));
    }
    let minor = parts.get(2).copied().unwrap_or("0");
    Ok(format!("{}.{}.", parts[1], minor))
}

fn sort_key(v: &str) -> Vec<u64> {
    v.split('-').next().unwrap_or("").split('.').map(|p| p.parse::<u64>().unwrap_or(0)).collect()
}

/// Filtre (par préfixe) et trie, de la plus récente à la plus ancienne. Stable = sans suffixe (`-beta`…).
pub fn filter_versions(all: &[String], minecraft: &str) -> Result<Vec<LoaderVersion>> {
    let prefix = version_prefix(minecraft)?;
    let mut out: Vec<LoaderVersion> = all
        .iter()
        .filter(|v| v.starts_with(&prefix))
        .map(|v| LoaderVersion { version: v.clone(), stable: !v.contains('-') })
        .collect();
    out.sort_by(|a, b| {
        sort_key(&b.version).cmp(&sort_key(&a.version)).then(b.stable.cmp(&a.stable))
    });
    Ok(out)
}

fn check_version_chars(version: &str) -> Result<()> {
    if version.is_empty() || !version.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+' | b'_')) {
        return Err(Error::Invalid(format!("version NeoForge invalide « {version} »")));
    }
    Ok(())
}

pub struct NeoForgeClient {
    http: reqwest::Client,
    maven_url: String,
}

impl Default for NeoForgeClient {
    fn default() -> Self {
        Self::new()
    }
}

impl NeoForgeClient {
    pub fn new() -> Self {
        Self::with_maven_url(DEFAULT_MAVEN_URL)
    }

    /// `url` : base du Maven (ex. `https://maven.neoforged.net`).
    pub fn with_maven_url(url: impl Into<String>) -> Self {
        Self { http: reqwest::Client::new(), maven_url: url.into().trim_end_matches('/').to_string() }
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
        Ok(resp.bytes().await.map_err(|e| Error::Network(e.to_string()))?.to_vec())
    }

    /// Versions NeoForge pour `minecraft`, de la plus récente à la plus ancienne (bêtas incluses, `stable` = false).
    pub async fn list_versions(&self, minecraft: &str) -> Result<Vec<LoaderVersion>> {
        #[derive(Deserialize)]
        struct Listing {
            versions: Vec<String>,
        }
        let url = format!("{}/api/maven/versions/releases/net/neoforged/neoforge", self.maven_url);
        let bytes = self.get_bytes(&url).await?;
        let listing: Listing =
            serde_json::from_slice(&bytes).map_err(|e| Error::Network(format!("liste NeoForge invalide : {e}")))?;
        filter_versions(&listing.versions, minecraft)
    }

    /// Version stable la plus récente pour `minecraft`.
    pub async fn latest_stable(&self, minecraft: &str) -> Result<String> {
        self.list_versions(minecraft)
            .await?
            .into_iter()
            .find(|v| v.stable)
            .map(|v| v.version)
            .ok_or_else(|| Error::NotFound(format!("NeoForge stable pour Minecraft {minecraft}")))
    }

    /// Télécharge l'installeur dans `dest` (atomique) après vérification du `.sha1` publié par Maven.
    pub async fn download_installer(&self, version: &str, dest: &Path) -> Result<()> {
        check_version_chars(version)?;
        let url = format!(
            "{}/releases/net/neoforged/neoforge/{version}/neoforge-{version}-installer.jar",
            self.maven_url
        );
        let bytes = self.get_bytes(&url).await?;
        let sha_text = self.get_bytes(&format!("{url}.sha1")).await?;
        let expected = String::from_utf8_lossy(&sha_text)
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_lowercase();
        let actual = sha1_hex(&bytes);
        if expected.is_empty() || actual != expected {
            return Err(Error::Integrity { expected, actual });
        }
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        write_atomic(dest, &bytes).await
    }

    /// Télécharge (ou extrait de l'installeur) les libraries de l'installeur manquantes ou invalides.
    pub async fn install_libraries(&self, profile: &InstallerProfile, libs_dir: &Path) -> Result<()> {
        for lib in &profile.libraries {
            let name = lib.get("name").and_then(Value::as_str).unwrap_or("");
            let art = lib.pointer("/downloads/artifact");
            let rel = match art.and_then(|a| a.get("path")).and_then(Value::as_str) {
                Some(p) if !p.is_empty() => p.to_string(),
                _ => maven_path(name)?,
            };
            check_rel(&rel)?;
            let sha1 = art.and_then(|a| a.get("sha1")).and_then(Value::as_str).filter(|s| !s.is_empty());
            let dest = libs_dir.join(&rel);
            if file_matches(&dest, sha1) {
                continue;
            }
            let url = art.and_then(|a| a.get("url")).and_then(Value::as_str).unwrap_or("");
            let bytes = if url.is_empty() {
                profile.read_installer_entry(&format!("maven/{rel}"))?
            } else {
                self.get_bytes(url).await?
            };
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
            write_atomic(&dest, &bytes).await?;
        }
        Ok(())
    }
}

fn check_rel(rel: &str) -> Result<()> {
    if rel.is_empty() || rel.starts_with('/') || rel.contains('\\') || rel.split('/').any(|s| s == "..") {
        return Err(Error::Invalid(format!("chemin invalide « {rel} »")));
    }
    Ok(())
}

fn file_matches(path: &Path, sha1: Option<&str>) -> bool {
    let Ok(data) = std::fs::read(path) else { return false };
    if data.is_empty() {
        return false;
    }
    sha1.is_none_or(|e| sha1_hex(&data).eq_ignore_ascii_case(e))
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

/// Versions NeoForge (client par défaut).
pub async fn list_versions(minecraft: &str) -> Result<Vec<LoaderVersion>> {
    NeoForgeClient::new().list_versions(minecraft).await
}

/// Version stable la plus récente (client par défaut).
pub async fn latest_stable(minecraft: &str) -> Result<String> {
    NeoForgeClient::new().latest_stable(minecraft).await
}

/// Téléchargement de l'installeur (client par défaut).
pub async fn download_installer(version: &str, dest: &Path) -> Result<()> {
    NeoForgeClient::new().download_installer(version, dest).await
}

// ---------------------------------------------------------------------------------------------
// Installeur
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct DataEntry {
    #[serde(default)]
    pub client: String,
    #[serde(default)]
    pub server: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Processor {
    /// Absent : tous les côtés.
    #[serde(default)]
    pub sides: Option<Vec<String>>,
    /// Coordonnées Maven du jar principal.
    pub jar: String,
    #[serde(default)]
    pub classpath: Vec<String>,
    #[serde(default)]
    pub args: Vec<String>,
    /// Fichier (templaté) -> sha1 attendu (entre apostrophes).
    #[serde(default)]
    pub outputs: BTreeMap<String, String>,
}

impl Processor {
    pub fn runs_on_client(&self) -> bool {
        self.sides.as_ref().is_none_or(|s| s.iter().any(|x| x == "client"))
    }
}

#[derive(Debug, Clone)]
pub struct InstallerProfile {
    pub installer_path: PathBuf,
    /// Identifiant du profil (`neoforge-21.1.77`).
    pub id: String,
    pub minecraft: String,
    pub data: BTreeMap<String, DataEntry>,
    pub processors: Vec<Processor>,
    /// Libraries de l'installeur (JSON brut).
    pub libraries: Vec<Value>,
    /// `version.json` du profil.
    pub version_json: Value,
}

impl InstallerProfile {
    fn open(&self) -> Result<zip::ZipArchive<std::fs::File>> {
        open_checked(&self.installer_path)
    }

    /// Lit une entrée de l'installeur (taille bornée, nom sûr).
    pub fn read_installer_entry(&self, name: &str) -> Result<Vec<u8>> {
        read_entry(&mut self.open()?, name, MAX_ENTRY_BYTES)
    }
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
            return Err(Error::Invalid(format!("installeur : entrée dangereuse « {} »", entry.name())));
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

/// Lit `install_profile.json` et `version.json` du jar installeur (zip sûr, tailles bornées).
pub fn parse_installer(jar: &Path) -> Result<InstallerProfile> {
    #[derive(Deserialize)]
    struct Raw {
        #[serde(default)]
        version: String,
        #[serde(default)]
        minecraft: String,
        #[serde(default)]
        json: Option<String>,
        #[serde(default)]
        data: BTreeMap<String, DataEntry>,
        #[serde(default)]
        processors: Vec<Processor>,
        #[serde(default)]
        libraries: Vec<Value>,
    }
    let mut ar = open_checked(jar)?;
    let raw: Raw = serde_json::from_slice(&read_entry(&mut ar, "install_profile.json", MAX_JSON_BYTES)?)
        .map_err(|e| Error::Invalid(format!("install_profile.json invalide : {e}")))?;
    let json_name = raw.json.as_deref().unwrap_or("/version.json").to_string();
    let version_json: Value = serde_json::from_slice(&read_entry(&mut ar, &json_name, MAX_JSON_BYTES)?)
        .map_err(|e| Error::Invalid(format!("version.json invalide : {e}")))?;
    let id = version_json.get("id").and_then(Value::as_str).unwrap_or(&raw.version).to_string();
    if id.is_empty() {
        return Err(Error::Invalid("installeur sans identifiant de version".into()));
    }
    Ok(InstallerProfile {
        installer_path: jar.to_path_buf(),
        id,
        minecraft: raw.minecraft,
        data: raw.data,
        processors: raw.processors,
        libraries: raw.libraries,
        version_json,
    })
}

/// Écrit `<versions_dir>/<id>/<id>.json` (atomique).
pub async fn write_version_json(profile: &InstallerProfile, versions_dir: &Path) -> Result<InstalledProfile> {
    let id = &profile.id;
    if id.contains(['/', '\\']) || id.contains("..") {
        return Err(Error::Invalid(format!("identifiant de profil invalide « {id} »")));
    }
    let dir = versions_dir.join(id);
    tokio::fs::create_dir_all(&dir).await?;
    let json_path = dir.join(format!("{id}.json"));
    let body = serde_json::to_vec_pretty(&profile.version_json).map_err(|e| Error::Other(e.to_string()))?;
    write_atomic(&json_path, &body).await?;
    Ok(InstalledProfile { id: id.clone(), json_path })
}

/// Fusion du `version.json` NeoForge avec le JSON vanilla (même résultat que Fabric/Quilt).
pub fn merge_with_vanilla(vanilla: &VersionJson, profile: &InstallerProfile) -> MergedVersion {
    crate::loader::fabric::merge_with_vanilla(vanilla, &profile.version_json)
}

// ---------------------------------------------------------------------------------------------
// Processeurs
// ---------------------------------------------------------------------------------------------

/// Contexte d'exécution des processeurs.
#[derive(Debug, Clone)]
pub struct ProcessContext {
    pub libs_dir: PathBuf,
    /// Jar client vanilla (`{MINECRAFT_JAR}`).
    pub minecraft_jar: PathBuf,
    /// Racine du launcher (`{ROOT}`).
    pub root: PathBuf,
    /// Dossier de travail : les fichiers `/data/...` de l'installeur y sont extraits.
    pub work_dir: PathBuf,
}

/// Commande Java à exécuter : `java -cp <jar>:<classpath> <Main-Class du jar> <args>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessSpec {
    pub jar: PathBuf,
    pub classpath: Vec<PathBuf>,
    pub args: Vec<String>,
}

/// Exécution d'un processeur, injectable (faux runner dans les tests).
pub trait ProcessRunner {
    fn run(&self, spec: &ProcessSpec) -> Result<()>;
}

/// Vrai lanceur : `java -cp … <mainClass> args`, la classe principale venant du manifeste du jar.
pub struct JavaRunner {
    pub java: PathBuf,
}

fn manifest_main_class(jar: &Path) -> Result<String> {
    let file = std::fs::File::open(jar).map_err(|e| Error::NotFound(format!("{} : {e}", jar.display())))?;
    let mut ar = zip::ZipArchive::new(file).map_err(|e| Error::Invalid(format!("{} illisible : {e}", jar.display())))?;
    let mut text = String::new();
    ar.by_name("META-INF/MANIFEST.MF")
        .map_err(|_| Error::Invalid(format!("{} sans manifeste", jar.display())))?
        .take(1024 * 1024)
        .read_to_string(&mut text)?;
    text.lines()
        .find_map(|l| l.strip_prefix("Main-Class:"))
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .ok_or_else(|| Error::Invalid(format!("{} sans Main-Class", jar.display())))
}

impl ProcessRunner for JavaRunner {
    fn run(&self, spec: &ProcessSpec) -> Result<()> {
        let main = manifest_main_class(&spec.jar)?;
        let sep = if cfg!(windows) { ";" } else { ":" };
        let cp = std::iter::once(&spec.jar)
            .chain(spec.classpath.iter())
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(sep);
        let out = std::process::Command::new(&self.java)
            .arg("-cp")
            .arg(cp)
            .arg(&main)
            .args(&spec.args)
            .output()
            .map_err(|e| Error::Other(format!("lancement de Java impossible ({}) : {e}", self.java.display())))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            let tail: String = err.lines().rev().take(10).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
            return Err(Error::Other(format!("processeur {main} en échec ({}) : {tail}", out.status)));
        }
        Ok(())
    }
}

/// Bilan de `run_processors`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessReport {
    pub ran: usize,
    pub skipped: usize,
}

struct Vars<'a> {
    ctx: &'a ProcessContext,
    map: BTreeMap<String, String>,
}

impl Vars<'_> {
    fn lib_path(&self, coords: &str) -> Result<String> {
        let rel = maven_path(coords)?;
        check_rel(&rel)?;
        Ok(self.ctx.libs_dir.join(rel).to_string_lossy().into_owned())
    }

    /// Valeur d'une entrée `data` : `[maven]`, `'littéral'`, `/fichier de l'installeur` ou texte brut.
    fn data_value(&self, raw: &str) -> Result<String> {
        if let Some(inner) = raw.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            self.lib_path(inner)
        } else if let Some(inner) = raw.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')) {
            Ok(inner.to_string())
        } else if let Some(rest) = raw.strip_prefix('/') {
            check_rel(rest)?;
            Ok(self.ctx.work_dir.join("data").join(rest).to_string_lossy().into_owned())
        } else {
            Ok(raw.to_string())
        }
    }

    fn replace(&self, s: &str) -> Result<String> {
        let mut out = String::new();
        let mut rest = s;
        while let Some(start) = rest.find('{') {
            out.push_str(&rest[..start]);
            let after = &rest[start + 1..];
            let end = after.find('}').ok_or_else(|| Error::Invalid(format!("accolade non fermée dans « {s} »")))?;
            let key = &after[..end];
            let val = self.map.get(key).ok_or_else(|| Error::Invalid(format!("variable inconnue {{{key}}} dans « {s} »")))?;
            out.push_str(val);
            rest = &after[end + 1..];
        }
        out.push_str(rest);
        Ok(out)
    }

    /// Argument : `[maven]` -> chemin de library ; sinon substitution des `{VAR}` puis retrait des apostrophes.
    fn arg(&self, raw: &str) -> Result<String> {
        if let Some(inner) = raw.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            return self.lib_path(inner);
        }
        let s = self.replace(raw)?;
        Ok(match s.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')) {
            Some(inner) => inner.to_string(),
            None => s,
        })
    }
}

fn build_vars<'a>(profile: &InstallerProfile, ctx: &'a ProcessContext) -> Result<Vars<'a>> {
    let mut vars = Vars { ctx, map: BTreeMap::new() };
    for (k, v) in &profile.data {
        let val = vars.data_value(&v.client)?;
        vars.map.insert(k.clone(), val);
    }
    let s = |p: &Path| p.to_string_lossy().into_owned();
    vars.map.insert("SIDE".into(), "client".into());
    vars.map.insert("MINECRAFT_JAR".into(), s(&ctx.minecraft_jar));
    vars.map.insert("ROOT".into(), s(&ctx.root));
    vars.map.insert("INSTALLER".into(), s(&profile.installer_path));
    vars.map.insert("LIBRARY_DIR".into(), s(&ctx.libs_dir));
    Ok(vars)
}

/// Extrait de l'installeur les fichiers `/data/...` référencés par les données côté client.
fn extract_data_files(profile: &InstallerProfile, ctx: &ProcessContext) -> Result<()> {
    let mut ar: Option<zip::ZipArchive<std::fs::File>> = None;
    for entry in profile.data.values() {
        let Some(rest) = entry.client.strip_prefix('/') else { continue };
        check_rel(rest)?;
        if ar.is_none() {
            ar = Some(profile.open()?);
        }
        let bytes = read_entry(ar.as_mut().expect("ouvert"), rest, MAX_ENTRY_BYTES)?;
        let dest = ctx.work_dir.join("data").join(rest);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(dest, bytes)?;
    }
    Ok(())
}

/// Sorties attendues (chemin, sha1) du processeur.
fn expected_outputs(p: &Processor, vars: &Vars) -> Result<Vec<(PathBuf, String)>> {
    p.outputs
        .iter()
        .map(|(k, v)| Ok((PathBuf::from(vars.arg(k)?), vars.arg(v)?.to_lowercase())))
        .collect()
}

fn output_matches(path: &Path, sha1: &str) -> bool {
    std::fs::read(path).is_ok_and(|d| sha1_hex(&d).eq_ignore_ascii_case(sha1))
}

/// Exécute les processeurs côté client. Un processeur dont toutes les sorties sont déjà valides est sauté ;
/// après exécution, chaque sortie est vérifiée par sha1 (`Error::Integrity` sinon).
pub fn run_processors(profile: &InstallerProfile, ctx: &ProcessContext, runner: &dyn ProcessRunner) -> Result<ProcessReport> {
    let vars = build_vars(profile, ctx)?;
    let mut report = ProcessReport { ran: 0, skipped: 0 };
    let mut extracted = false;
    for p in profile.processors.iter().filter(|p| p.runs_on_client()) {
        let outputs = expected_outputs(p, &vars)?;
        if !outputs.is_empty() && outputs.iter().all(|(path, sha)| output_matches(path, sha)) {
            report.skipped += 1;
            continue;
        }
        if !extracted {
            extract_data_files(profile, ctx)?;
            extracted = true;
        }
        let jar = PathBuf::from(vars.lib_path(&p.jar)?);
        let classpath = p.classpath.iter().map(|c| vars.lib_path(c).map(PathBuf::from)).collect::<Result<Vec<_>>>()?;
        let args = p.args.iter().map(|a| vars.arg(a)).collect::<Result<Vec<_>>>()?;
        let spec = ProcessSpec { jar, classpath, args };
        runner.run(&spec).map_err(|e| match e {
            Error::Other(m) => Error::Other(format!("processeur NeoForge {} : {m}", p.jar)),
            other => other,
        })?;
        for (path, sha) in &outputs {
            let data = std::fs::read(path)
                .map_err(|e| Error::Other(format!("sortie manquante {} : {e}", path.display())))?;
            let actual = sha1_hex(&data);
            if !actual.eq_ignore_ascii_case(sha) {
                return Err(Error::Integrity { expected: sha.clone(), actual });
            }
        }
        report.ran += 1;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::RefCell;
    use std::io::Write;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn names(v: &[LoaderVersion]) -> Vec<&str> {
        v.iter().map(|x| x.version.as_str()).collect()
    }

    fn all() -> Vec<String> {
        ["21.0.5", "21.1.1", "21.1.77", "21.1.100", "21.1.200-beta", "21.10.5", "20.4.2", "21.1.9"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn prefix_rules() {
        assert_eq!(version_prefix("1.21.1").unwrap(), "21.1.");
        assert_eq!(version_prefix("1.20.4").unwrap(), "20.4.");
        assert_eq!(version_prefix("1.21").unwrap(), "21.0.");
        assert!(matches!(version_prefix("abc"), Err(Error::Invalid(_))));
        assert!(matches!(version_prefix("2.1.1"), Err(Error::Invalid(_))));
    }

    #[test]
    fn filters_and_sorts_versions() {
        let v = filter_versions(&all(), "1.21.1").unwrap();
        assert_eq!(names(&v), ["21.1.200-beta", "21.1.100", "21.1.77", "21.1.9", "21.1.1"]);
        assert!(!v[0].stable && v[1].stable);
        assert_eq!(names(&filter_versions(&all(), "1.21").unwrap()), ["21.0.5"]);
    }

    #[tokio::test]
    async fn lists_versions_and_latest_stable() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/maven/versions/releases/net/neoforged/neoforge"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"isSnapshot": false, "versions": all()})))
            .mount(&s)
            .await;
        let c = NeoForgeClient::with_maven_url(format!("{}/", s.uri()));
        assert_eq!(c.latest_stable("1.21.1").await.unwrap(), "21.1.100");
        assert_eq!(c.list_versions("1.21.1").await.unwrap().len(), 5);
        assert!(matches!(c.latest_stable("1.19.2").await, Err(Error::NotFound(_))));
    }

    #[tokio::test]
    async fn downloads_installer_with_sha1_check() {
        let s = MockServer::start().await;
        let body = b"fake-installer".to_vec();
        let base = "/releases/net/neoforged/neoforge/21.1.77/neoforge-21.1.77-installer.jar";
        Mock::given(method("GET")).and(path(base)).respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone())).mount(&s).await;
        Mock::given(method("GET"))
            .and(path(format!("{base}.sha1")))
            .respond_with(ResponseTemplate::new(200).set_body_string(sha1_hex(&body)))
            .mount(&s)
            .await;
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("dl/installer.jar");
        let c = NeoForgeClient::with_maven_url(s.uri());
        c.download_installer("21.1.77", &dest).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);

        // Hash publié différent : rejet, rien d'écrit.
        let s2 = MockServer::start().await;
        Mock::given(method("GET")).and(path(base)).respond_with(ResponseTemplate::new(200).set_body_bytes(body)).mount(&s2).await;
        Mock::given(method("GET")).and(path(format!("{base}.sha1"))).respond_with(ResponseTemplate::new(200).set_body_string("0".repeat(40))).mount(&s2).await;
        let dest2 = tmp.path().join("bad.jar");
        let err = NeoForgeClient::with_maven_url(s2.uri()).download_installer("21.1.77", &dest2).await.unwrap_err();
        assert!(matches!(err, Error::Integrity { .. }), "{err:?}");
        assert!(!dest2.exists());
        assert!(matches!(c.download_installer("../x", &dest2).await, Err(Error::Invalid(_))));
    }

    fn make_zip(path: &Path, files: &[(&str, &[u8])]) {
        let mut w = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        for (n, d) in files {
            w.start_file(*n, opts).unwrap();
            w.write_all(d).unwrap();
        }
        w.finish().unwrap();
    }

    const OUT_BYTES: &[u8] = b"patched-client";

    fn profile_json() -> Value {
        json!({
            "version": "neoforge-21.1.77",
            "json": "/version.json",
            "minecraft": "1.21.1",
            "data": {
                "BINPATCH": {"client": "/data/client.lzma", "server": "/data/server.lzma"},
                "MAPPINGS": {"client": "[net.minecraft:client:1.21.1:mappings@txt]", "server": "[x:y:1]"},
                "PATCHED": {"client": "[net.neoforged:neoforge:21.1.77:client]", "server": "[x:y:2]"},
                "NAME": {"client": "'literal'", "server": "'s'"}
            },
            "processors": [
                {"sides": ["server"], "jar": "a:server-only:1", "args": []},
                {
                    "sides": ["client"],
                    "jar": "net.neoforged.installertools:installertools:2.1.2",
                    "classpath": ["org.ow2.asm:asm:9.7"],
                    "args": ["--task", "PATCH", "--side", "{SIDE}", "--patch", "{BINPATCH}", "--maps", "{MAPPINGS}", "--name", "{NAME}",
                             "--out", "{PATCHED}", "--lib", "[org.ow2.asm:asm:9.7]", "--mc", "{MINECRAFT_JAR}"],
                    "outputs": {"{PATCHED}": format!("'{}'", sha1_hex(OUT_BYTES))}
                }
            ],
            "libraries": [{"name": "org.ow2.asm:asm:9.7"}]
        })
    }

    fn installer(dir: &Path) -> InstallerProfile {
        let jar = dir.join("installer.jar");
        let version = json!({"id": "neoforge-21.1.77", "inheritsFrom": "1.21.1", "mainClass": "cpw.mods.bootstraplauncher.BootstrapLauncher",
            "arguments": {"game": ["--fml.neoForgeVersion", "21.1.77"], "jvm": ["-DX=1"]},
            "libraries": [{"name": "net.neoforged:neoforge:21.1.77"}]});
        make_zip(
            &jar,
            &[
                ("install_profile.json", profile_json().to_string().as_bytes()),
                ("version.json", version.to_string().as_bytes()),
                ("data/client.lzma", b"binpatch"),
            ],
        );
        parse_installer(&jar).unwrap()
    }

    fn ctx(dir: &Path) -> ProcessContext {
        ProcessContext {
            libs_dir: dir.join("libraries"),
            minecraft_jar: dir.join("mc/1.21.1.jar"),
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

    fn patched_path(c: &ProcessContext) -> PathBuf {
        c.libs_dir.join("net/neoforged/neoforge/21.1.77/neoforge-21.1.77-client.jar")
    }

    #[test]
    fn parses_installer_in_memory() {
        let tmp = tempfile::tempdir().unwrap();
        let p = installer(tmp.path());
        assert_eq!(p.id, "neoforge-21.1.77");
        assert_eq!(p.minecraft, "1.21.1");
        assert_eq!(p.processors.len(), 2);
        assert!(!p.processors[0].runs_on_client());
        assert!(p.processors[1].runs_on_client());
        assert_eq!(p.data["BINPATCH"].client, "/data/client.lzma");
        assert_eq!(p.read_installer_entry("data/client.lzma").unwrap(), b"binpatch");
    }

    #[test]
    fn substitutes_variables_and_runs_client_processor() {
        let tmp = tempfile::tempdir().unwrap();
        let p = installer(tmp.path());
        let c = ctx(tmp.path());
        let fake = Fake { calls: RefCell::new(vec![]), write: Some((patched_path(&c), OUT_BYTES.to_vec())), fail: false };
        let r = run_processors(&p, &c, &fake).unwrap();
        assert_eq!(r, ProcessReport { ran: 1, skipped: 0 });
        let calls = fake.calls.borrow();
        assert_eq!(calls.len(), 1, "le processeur serveur n'est pas lancé");
        let a = &calls[0].args;
        let s = |x: PathBuf| x.to_string_lossy().into_owned();
        assert_eq!(a[3], "client");
        assert_eq!(a[5], s(c.work_dir.join("data").join("data/client.lzma")));
        assert_eq!(a[7], s(c.libs_dir.join("net/minecraft/client/1.21.1/client-1.21.1-mappings.txt")));
        assert_eq!(a[9], "literal");
        assert_eq!(a[11], s(patched_path(&c)));
        assert_eq!(a[13], s(c.libs_dir.join("org/ow2/asm/asm/9.7/asm-9.7.jar")));
        assert_eq!(a[15], s(c.minecraft_jar.clone()));
        assert_eq!(calls[0].classpath, vec![c.libs_dir.join("org/ow2/asm/asm/9.7/asm-9.7.jar")]);
        assert_eq!(std::fs::read(c.work_dir.join("data/data/client.lzma")).unwrap(), b"binpatch");
    }

    #[test]
    fn skips_processor_when_outputs_valid() {
        let tmp = tempfile::tempdir().unwrap();
        let p = installer(tmp.path());
        let c = ctx(tmp.path());
        std::fs::create_dir_all(patched_path(&c).parent().unwrap()).unwrap();
        std::fs::write(patched_path(&c), OUT_BYTES).unwrap();
        let fake = Fake { calls: RefCell::new(vec![]), write: None, fail: true };
        let r = run_processors(&p, &c, &fake).unwrap();
        assert_eq!(r, ProcessReport { ran: 0, skipped: 1 });
        assert!(fake.calls.borrow().is_empty());
    }

    #[test]
    fn invalid_output_is_integrity_error() {
        let tmp = tempfile::tempdir().unwrap();
        let p = installer(tmp.path());
        let c = ctx(tmp.path());
        let fake = Fake { calls: RefCell::new(vec![]), write: Some((patched_path(&c), b"autre".to_vec())), fail: false };
        let err = run_processors(&p, &c, &fake).unwrap_err();
        assert!(matches!(err, Error::Integrity { .. }), "{err:?}");
    }

    #[test]
    fn missing_output_and_runner_failure_are_clear_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let p = installer(tmp.path());
        let c = ctx(tmp.path());
        let none = Fake { calls: RefCell::new(vec![]), write: None, fail: false };
        let err = run_processors(&p, &c, &none).unwrap_err();
        assert!(err.to_string().contains("sortie manquante"), "{err}");
        let bad = Fake { calls: RefCell::new(vec![]), write: None, fail: true };
        let err = run_processors(&p, &c, &bad).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("installertools") && msg.contains("exit code 1"), "{msg}");
    }

    #[test]
    fn unknown_variable_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let mut p = installer(tmp.path());
        p.processors[1].args.push("{NOPE}".into());
        let c = ctx(tmp.path());
        let fake = Fake { calls: RefCell::new(vec![]), write: None, fail: false };
        assert!(matches!(run_processors(&p, &c, &fake), Err(Error::Invalid(_))));
    }

    #[test]
    fn malicious_zip_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let jar = tmp.path().join("evil.jar");
        make_zip(&jar, &[("install_profile.json", b"{}"), ("version.json", b"{}"), ("../evil.txt", b"x")]);
        assert!(matches!(parse_installer(&jar), Err(Error::Invalid(_))));
        let garbage = tmp.path().join("garbage.jar");
        std::fs::write(&garbage, b"pas un zip").unwrap();
        assert!(matches!(parse_installer(&garbage), Err(Error::Invalid(_))));
    }

    #[test]
    fn oversized_entry_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let jar = tmp.path().join("big.jar");
        let big = vec![b' '; (MAX_JSON_BYTES + 1) as usize];
        make_zip(&jar, &[("install_profile.json", &big), ("version.json", b"{}")]);
        assert!(matches!(parse_installer(&jar), Err(Error::Invalid(_))));
    }

    #[tokio::test]
    async fn writes_version_json_and_merges() {
        let tmp = tempfile::tempdir().unwrap();
        let p = installer(tmp.path());
        let vers = tmp.path().join("versions");
        let inst = write_version_json(&p, &vers).await.unwrap();
        assert_eq!(inst.json_path, vers.join("neoforge-21.1.77/neoforge-21.1.77.json"));
        let back: Value = serde_json::from_slice(&std::fs::read(&inst.json_path).unwrap()).unwrap();
        assert_eq!(back["mainClass"], "cpw.mods.bootstraplauncher.BootstrapLauncher");
        assert!(!inst.json_path.with_extension("json.tmp").exists());

        let vanilla: VersionJson = serde_json::from_value(json!({
            "id": "1.21.1", "mainClass": "net.minecraft.client.main.Main",
            "assetIndex": {"id":"17","url":"u","sha1":"aa","size":1},
            "downloads": {"client": {"url":"u","sha1":"bb","size":2}},
            "libraries": [{"name":"com.google:guava:31"}],
            "arguments": {"game": ["--demo"], "jvm": ["-Xss1M"]}
        }))
        .unwrap();
        let m = merge_with_vanilla(&vanilla, &p);
        assert_eq!(m.id, "neoforge-21.1.77");
        assert_eq!(m.inherits_from, "1.21.1");
        assert_eq!(m.libraries.len(), 2);
        assert_eq!(m.jvm_args, vec![json!("-Xss1M"), json!("-DX=1")]);
    }

    #[tokio::test]
    async fn installs_library_embedded_in_installer() {
        let tmp = tempfile::tempdir().unwrap();
        let jar = tmp.path().join("i.jar");
        let lib = b"embedded-jar";
        let prof = json!({"version": "v", "libraries": [{"name": "a:b:1", "downloads": {"artifact": {
            "path": "a/b/1/b-1.jar", "url": "", "sha1": sha1_hex(lib)}}}]});
        make_zip(&jar, &[("install_profile.json", prof.to_string().as_bytes()), ("version.json", br#"{"id":"v"}"#), ("maven/a/b/1/b-1.jar", lib)]);
        let p = parse_installer(&jar).unwrap();
        let libs = tmp.path().join("libs");
        NeoForgeClient::new().install_libraries(&p, &libs).await.unwrap();
        assert_eq!(std::fs::read(libs.join("a/b/1/b-1.jar")).unwrap(), lib);
    }
}
