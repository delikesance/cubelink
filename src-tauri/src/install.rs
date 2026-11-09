//! Préparation d'une instance : version vanilla, libraries, natives, assets, loader et Java.
//! Idempotent : tout ce qui est déjà présent et valide est sauté.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use core_lib::java::{default_search_dirs, find_installed, required_java_major, select_for};
use core_lib::java_install;
use core_lib::launch_args::LaunchVersion;
use core_lib::loader::{fabric, forge, neoforge, quilt};
use core_lib::mojang::assets::{download_assets, fetch_asset_index};
use core_lib::loader::fabric::FabricClient;
use core_lib::mojang::libraries::{self, Arch, Library, Os};
use core_lib::mojang::natives::extract_natives;
use core_lib::mojang::versions::{sha1_hex, VersionJson, VersionsClient};
use core_lib::types::{Error, Instance, Loader, LoaderSpec, Result};

use crate::state::Paths;

pub type Progress = Arc<dyn Fn(String) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct Prepared {
    pub java: PathBuf,
    pub version: LaunchVersion,
    pub libs_dir: PathBuf,
    pub assets_dir: PathBuf,
    pub natives_dir: PathBuf,
    pub client_jar: PathBuf,
}

/// Chemin relatif sûr (pas d'absolu, `..`, `:` ni `\`).
pub fn safe_rel(p: &str) -> Result<()> {
    let bad = p.is_empty()
        || p.starts_with('/')
        || p.contains('\\')
        || p.contains(':')
        || p.split('/').any(|s| s.is_empty() || s == "..");
    if bad {
        Err(Error::Invalid("chemin de fichier refusé".into()))
    } else {
        Ok(())
    }
}

/// Caractères autorisés pour une version (Minecraft ou loader).
pub fn safe_version(v: &str) -> bool {
    !v.is_empty() && v.len() <= 64 && v.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'))
}

pub fn versions_client() -> VersionsClient {
    match crate::test_hooks::hooks().manifest_url() {
        Some(u) => VersionsClient::with_manifest_url(u),
        None => VersionsClient::new(),
    }
}

fn fabric_client() -> FabricClient {
    match &crate::test_hooks::hooks().fabric_meta {
        Some(u) => FabricClient::with_meta_url(u.clone()),
        None => FabricClient::new(),
    }
}

fn quilt_client() -> quilt::QuiltClient {
    match &crate::test_hooks::hooks().quilt_meta {
        Some(u) => quilt::QuiltClient::with_meta_url(u.clone()),
        None => quilt::QuiltClient::new(),
    }
}

fn neoforge_client() -> neoforge::NeoForgeClient {
    match &crate::test_hooks::hooks().neoforge_maven {
        Some(u) => neoforge::NeoForgeClient::with_maven_url(u.clone()),
        None => neoforge::NeoForgeClient::new(),
    }
}

/// Forge : le Maven et les « promotions » ; si seul le Maven est surchargé, il sert aux deux.
fn forge_client() -> forge::ForgeClient {
    let h = crate::test_hooks::hooks();
    match (&h.forge_maven, &h.forge_files) {
        (None, None) => forge::ForgeClient::new(),
        (m, f) => {
            let maven = m.clone().unwrap_or_else(|| forge::DEFAULT_MAVEN_URL.to_string());
            let files = f.clone().or_else(|| m.clone()).unwrap_or_else(|| forge::DEFAULT_FILES_URL.to_string());
            forge::ForgeClient::with_urls(maven, files)
        }
    }
}

fn http() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(concat!("Cubelink/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| Error::Network(e.without_url().to_string()))
}

async fn get_bytes(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    if !url.starts_with("https://") && !crate::test_hooks::http_allowed(url) {
        return Err(Error::Invalid("téléchargement refusé : seul https est accepté".into()));
    }
    let resp = client.get(url).send().await.map_err(|e| Error::Network(e.without_url().to_string()))?;
    if !resp.status().is_success() {
        return Err(Error::Network(format!("réponse HTTP {}", resp.status().as_u16())));
    }
    let b = resp.bytes().await.map_err(|e| Error::Network(e.without_url().to_string()))?;
    Ok(b.to_vec())
}

fn valid_on_disk(dest: &Path, size: Option<u64>) -> bool {
    std::fs::metadata(dest).map(|m| m.is_file() && m.len() > 0 && size.is_none_or(|s| s == m.len())).unwrap_or(false)
}

async fn write_atomic(dest: &Path, data: &[u8]) -> Result<()> {
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = dest.with_extension("part");
    tokio::fs::write(&tmp, data).await?;
    tokio::fs::rename(&tmp, dest).await?;
    Ok(())
}

/// Télécharge `url` vers `dest` si absent ou de taille inattendue ; vérifie sha1 et taille.
async fn download_verified(client: &reqwest::Client, url: &str, dest: &Path, sha1: Option<&str>, size: Option<u64>) -> Result<()> {
    if valid_on_disk(dest, size) {
        return Ok(());
    }
    let data = get_bytes(client, url).await?;
    if let Some(s) = size {
        if s != data.len() as u64 {
            return Err(Error::Integrity { expected: format!("{s} octets"), actual: format!("{} octets", data.len()) });
        }
    }
    if let Some(expected) = sha1 {
        let actual = sha1_hex(&data);
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(Error::Integrity { expected: expected.to_lowercase(), actual });
        }
    }
    write_atomic(dest, &data).await
}

/// JSON de version vanilla, mis en cache sous `versions/<id>/<id>.json`.
async fn vanilla_json(paths: &Paths, client: &reqwest::Client, mc: &str) -> Result<VersionJson> {
    let file = paths.versions.join(mc).join(format!("{mc}.json"));
    if let Ok(bytes) = tokio::fs::read(&file).await {
        if let Ok(v) = serde_json::from_slice::<VersionJson>(&bytes) {
            return Ok(v);
        }
    }
    let manifest = versions_client().fetch_manifest().await?;
    let entry = manifest.find(mc).map_err(|_| Error::NotFound(format!("version Minecraft {mc} inconnue")))?;
    let bytes = get_bytes(client, &entry.url).await?;
    let actual = sha1_hex(&bytes);
    if !actual.eq_ignore_ascii_case(&entry.sha1) {
        return Err(Error::Integrity { expected: entry.sha1.to_lowercase(), actual });
    }
    let parsed: VersionJson = serde_json::from_slice(&bytes).map_err(|_| Error::Network("JSON de version invalide".into()))?;
    write_atomic(&file, &bytes).await?;
    Ok(parsed)
}

async fn resolve_java(paths: &Paths, required: u32, setting: Option<&str>) -> Result<PathBuf> {
    // Mode test : chemin de java imposé (faux java).
    let forced = crate::test_hooks::hooks().java_path.as_ref().map(|p| p.display().to_string());
    if let Some(p) = forced.as_deref().or(setting) {
        let p = PathBuf::from(p);
        return if p.is_file() { Ok(p) } else { Err(Error::NotFound("l'exécutable Java configuré est introuvable".into())) };
    }
    let dirs = default_search_dirs(&paths.java);
    let installs = tokio::task::spawn_blocking(move || find_installed(&dirs))
        .await
        .map_err(|_| Error::Other("recherche de Java interrompue".into()))?;
    if let Some(i) = select_for(required, &installs) {
        return Ok(i.path.clone());
    }
    let (os, arch) = (java_install::host_os(), java_install::host_arch());
    let pkg = match &crate::test_hooks::hooks().adoptium {
        Some(u) => java_install::resolve_package_from(u, required, os, arch).await?,
        None => java_install::resolve_package(required, os, arch).await?,
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let done = if crate::test_hooks::hooks().skip_java_probe {
        let probe = Arc::new(crate::test_hooks::FakeProbe(pkg.major));
        java_install::install_with(&pkg, &paths.java, |_, _| {}, cancel, probe).await?
    } else {
        java_install::install(&pkg, &paths.java, |_, _| {}, cancel).await?
    };
    Ok(done.java_exe)
}

pub fn loader_key(mc: &str, spec: &LoaderSpec) -> String {
    let name = match spec.loader {
        Loader::Vanilla => "vanilla",
        Loader::Fabric => "fabric",
        Loader::Quilt => "quilt",
        Loader::NeoForge => "neoforge",
        Loader::Forge => "forge",
    };
    format!("{name}-{mc}-{}", spec.version)
}

/// Choisit la dernière version stable du loader si `version` est vide.
pub async fn resolve_loader_version(mc: &str, spec: &LoaderSpec) -> Result<String> {
    if !spec.version.is_empty() || spec.loader == Loader::Vanilla {
        return Ok(spec.version.clone());
    }
    match spec.loader {
        Loader::Vanilla => Ok(String::new()),
        Loader::Fabric => fabric::latest_stable(&fabric_client().list_loader_versions(mc).await?)
            .map(|v| v.version.clone())
            .ok_or_else(|| Error::NotFound(format!("aucune version de Fabric pour Minecraft {mc}"))),
        Loader::Quilt => quilt::latest_stable(&quilt_client().list_loader_versions(mc).await?)
            .map(|v| v.version.clone())
            .ok_or_else(|| Error::NotFound(format!("aucune version de Quilt pour Minecraft {mc}"))),
        Loader::NeoForge => neoforge_client().latest_stable(mc).await,
        Loader::Forge => forge_client().recommended(mc).await,
    }
}

async fn missing_libs(client: &reqwest::Client, libs: &[Library], libs_dir: &Path) -> Result<()> {
    for lib in libs {
        safe_rel(&lib.path)?;
        let dest = libs_dir.join(&lib.path);
        if valid_on_disk(&dest, lib.size) {
            continue;
        }
        if lib.url.is_empty() {
            // Produite localement par les processeurs du loader.
            continue;
        }
        download_verified(client, &lib.url, &dest, lib.sha1.as_deref(), lib.size).await?;
    }
    Ok(())
}

/// Installe le loader et renvoie la version de lancement fusionnée.
async fn install_loader(
    paths: &Paths,
    client_jar: &Path,
    mc: &str,
    spec: &LoaderSpec,
    vanilla: &VersionJson,
    java: &Path,
) -> Result<LaunchVersion> {
    let dir = paths.loaders.join(loader_key(mc, spec));
    tokio::fs::create_dir_all(&dir).await?;
    let versions_dir = dir.join("versions");
    match spec.loader {
        Loader::Vanilla => Ok(LaunchVersion::Vanilla(vanilla.clone())),
        Loader::Fabric | Loader::Quilt => {
            let profile_file = dir.join("profile.json");
            let profile: serde_json::Value = match tokio::fs::read(&profile_file).await.ok().and_then(|b| serde_json::from_slice(&b).ok()) {
                Some(p) => p,
                None => {
                    let p = if spec.loader == Loader::Fabric {
                        fabric_client().fetch_profile(mc, &spec.version).await?
                    } else {
                        quilt_client().fetch_profile(mc, &spec.version).await?
                    };
                    write_atomic(&profile_file, p.to_string().as_bytes()).await?;
                    p
                }
            };
            if spec.loader == Loader::Fabric {
                fabric_client().install(&profile, &paths.libraries, &versions_dir).await?;
                Ok(LaunchVersion::Merged(fabric::merge_with_vanilla(vanilla, &profile)))
            } else {
                quilt_client().install(&profile, &paths.libraries, &versions_dir).await?;
                Ok(LaunchVersion::Merged(quilt::merge_with_vanilla(vanilla, &profile)))
            }
        }
        Loader::NeoForge | Loader::Forge => {
            let installer = dir.join("installer.jar");
            let is_neo = spec.loader == Loader::NeoForge;
            if !valid_on_disk(&installer, None) {
                if is_neo {
                    neoforge_client().download_installer(&spec.version, &installer).await?;
                } else {
                    forge_client().download_installer(mc, &spec.version, &installer).await?;
                }
            }
            let ctx = neoforge::ProcessContext {
                libs_dir: paths.libraries.clone(),
                minecraft_jar: client_jar.to_path_buf(),
                root: paths.root.clone(),
                work_dir: dir.join("work"),
            };
            let marker = dir.join(".installed");
            let merged = if is_neo {
                let profile = neoforge::parse_installer(&installer)?;
                if !marker.exists() {
                    neoforge_client().install_libraries(&profile, &paths.libraries).await?;
                    neoforge::write_version_json(&profile, &versions_dir).await?;
                    run_processors_blocking(installer.clone(), ctx, java.to_path_buf(), true).await?;
                    tokio::fs::write(&marker, b"ok").await?;
                }
                neoforge::merge_with_vanilla(vanilla, &profile)
            } else {
                let inst = forge::parse_installer(&installer)?;
                if !marker.exists() {
                    match inst.era {
                        forge::Era::Legacy => forge::extract_universal(&inst, &paths.libraries)?,
                        forge::Era::Modern => {
                            neoforge_client().install_libraries(&inst.profile, &paths.libraries).await?
                        }
                    }
                    neoforge::write_version_json(&inst.profile, &versions_dir).await?;
                    run_processors_blocking(installer.clone(), ctx, java.to_path_buf(), false).await?;
                    tokio::fs::write(&marker, b"ok").await?;
                }
                forge::merge_with_vanilla(vanilla, &inst)
            };
            Ok(LaunchVersion::Merged(merged))
        }
    }
}

/// Les processeurs lancent des processus Java synchrones : hors du runtime async.
async fn run_processors_blocking(installer: PathBuf, ctx: neoforge::ProcessContext, java: PathBuf, is_neo: bool) -> Result<()> {
    tokio::task::spawn_blocking(move || -> Result<()> {
        let runner = neoforge::JavaRunner { java };
        if is_neo {
            let p = neoforge::parse_installer(&installer)?;
            neoforge::run_processors(&p, &ctx, &runner)?;
        } else {
            let p = forge::parse_installer(&installer)?;
            if p.era == forge::Era::Modern {
                neoforge::run_processors(&p.profile, &ctx, &runner)?;
            }
        }
        Ok(())
    })
    .await
    .map_err(|_| Error::Other("installation du loader interrompue".into()))?
}

/// Prépare tout ce qu'il faut pour lancer `inst`.
pub async fn ensure_game(paths: &Paths, inst: &Instance, java_setting: Option<String>, progress: Progress) -> Result<Prepared> {
    if !safe_version(&inst.minecraft) || (inst.loader.loader != Loader::Vanilla && !safe_version(&inst.loader.version)) {
        return Err(Error::Invalid("version de Minecraft ou de loader invalide".into()));
    }
    let client = http()?;
    progress("Récupération de la version Minecraft…".into());
    let vanilla = vanilla_json(paths, &client, &inst.minecraft).await?;

    progress("Recherche de Java…".into());
    let required = required_java_major(&inst.minecraft, vanilla.java_version.as_ref().map(|j| j.major_version));
    let java = resolve_java(paths, required, java_setting.as_deref()).await?;

    progress("Téléchargement du jeu…".into());
    let client_jar = paths.versions.join(&inst.minecraft).join(format!("{}.jar", inst.minecraft));
    let dl = &vanilla.downloads.client;
    download_verified(&client, &dl.url, &client_jar, Some(&dl.sha1), Some(dl.size)).await?;

    let spec = LoaderSpec { loader: inst.loader.loader, version: inst.loader.version.clone() };
    if spec.loader != Loader::Vanilla {
        progress("Installation du loader…".into());
    }
    let version = install_loader(paths, &client_jar, &inst.minecraft, &spec, &vanilla, &java).await?;

    progress("Téléchargement des bibliothèques…".into());
    let raw_libs = match &version {
        LaunchVersion::Vanilla(v) => v.libraries.clone(),
        LaunchVersion::Merged(m) => m.libraries.clone(),
    };
    let libs = libraries::resolve(&raw_libs, Os::current(), Arch::current())?;
    missing_libs(&client, &libs, &paths.libraries).await?;

    let natives_dir = paths.natives.join(&inst.id);
    let (l, ld, nd) = (libs.clone(), paths.libraries.clone(), natives_dir.clone());
    tokio::task::spawn_blocking(move || extract_natives(&l, &ld, &nd))
        .await
        .map_err(|_| Error::Other("extraction des natives interrompue".into()))??;

    progress("Téléchargement des ressources…".into());
    let index = fetch_asset_index(&vanilla.asset_index, &paths.assets).await?;
    let resources = crate::test_hooks::hooks().resources_url();
    download_assets(&index, &paths.assets, resources.as_deref(), |_| {}, 8).await?;

    Ok(Prepared {
        java,
        version,
        libs_dir: paths.libraries.clone(),
        assets_dir: paths.assets.clone(),
        natives_dir,
        client_jar,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chemins_et_versions() {
        assert!(safe_rel("org/lwjgl/a.jar").is_ok());
        for bad in ["", "../x", "/x", "a\\b", "C:/x", "a//b"] {
            assert!(safe_rel(bad).is_err(), "{bad}");
        }
        assert!(safe_version("1.21.1") && safe_version("21.1.77-beta"));
        assert!(!safe_version("../1") && !safe_version("") && !safe_version("a b"));
    }
}
