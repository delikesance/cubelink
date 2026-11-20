//! Préparation d'une instance : version vanilla, libraries, natives, assets, loader et Java.
//! Idempotent : tout ce qui est déjà présent et valide est sauté.

use std::path::{Path, PathBuf};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use core_lib::java::{default_search_dirs, find_installed, required_java_major, select_for};
use core_lib::java_install;
use core_lib::launch_args::LaunchVersion;
use core_lib::loader::{fabric, forge, neoforge, quilt};
use core_lib::mojang::assets::{download_assets_with, fetch_asset_index, DownloadOptions};
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
    core_lib::http::builder()
        .user_agent(concat!("Cubelink/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(core_lib::http::net_error)
}

async fn get_bytes(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    if !url.starts_with("https://") && !crate::test_hooks::http_allowed(url) {
        return Err(Error::Invalid("téléchargement refusé : seul https est accepté".into()));
    }
    fetch_bytes(client, url).await
}

async fn fetch_bytes(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let resp = client.get(url).send().await.map_err(core_lib::http::net_error)?;
    if !resp.status().is_success() {
        return Err(Error::Network(format!("réponse HTTP {}", resp.status().as_u16())));
    }
    let b = resp.bytes().await.map_err(core_lib::http::net_error)?;
    Ok(b.to_vec())
}

/// Intervalle de lecture du drapeau d'annulation pendant une requête réseau.
const CANCEL_POLL: std::time::Duration = std::time::Duration::from_millis(100);

fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::SeqCst) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

/// Attend `fut`, mais rend `Error::Cancelled` dès que `cancel` est levé (le futur est abandonné : la connexion se ferme).
/// Réservé aux étapes réseau dont l'abandon est sans danger (écritures atomiques) : jamais autour d'un `spawn_blocking`.
pub(crate) async fn cancellable<T>(cancel: &AtomicBool, fut: impl Future<Output = Result<T>>) -> Result<T> {
    check_cancel(cancel)?;
    tokio::pin!(fut);
    loop {
        tokio::select! {
            r = &mut fut => return r,
            _ = tokio::time::sleep(CANCEL_POLL) => check_cancel(cancel)?,
        }
    }
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

async fn resolve_java(paths: &Paths, required: u32, setting: Option<&str>, cancel: &Arc<AtomicBool>) -> Result<PathBuf> {
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
    let pkg = cancellable(cancel, async {
        match &crate::test_hooks::hooks().adoptium {
            Some(u) => java_install::resolve_package_from(u, required, os, arch).await,
            None => java_install::resolve_package(required, os, arch).await,
        }
    })
    .await?;
    // `install` lit le drapeau pendant le téléchargement et avant l'extraction.
    let cancel = cancel.clone();
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
    cancel: &AtomicBool,
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
                        cancellable(cancel, fabric_client().fetch_profile(mc, &spec.version)).await?
                    } else {
                        cancellable(cancel, quilt_client().fetch_profile(mc, &spec.version)).await?
                    };
                    write_atomic(&profile_file, p.to_string().as_bytes()).await?;
                    p
                }
            };
            if spec.loader == Loader::Fabric {
                cancellable(cancel, fabric_client().install(&profile, &paths.libraries, &versions_dir)).await?;
                Ok(LaunchVersion::Merged(fabric::merge_with_vanilla(vanilla, &profile)))
            } else {
                cancellable(cancel, quilt_client().install(&profile, &paths.libraries, &versions_dir)).await?;
                Ok(LaunchVersion::Merged(quilt::merge_with_vanilla(vanilla, &profile)))
            }
        }
        Loader::NeoForge | Loader::Forge => {
            let installer = dir.join("installer.jar");
            let is_neo = spec.loader == Loader::NeoForge;
            if !valid_on_disk(&installer, None) {
                if is_neo {
                    cancellable(cancel, neoforge_client().download_installer(&spec.version, &installer)).await?;
                } else {
                    cancellable(cancel, forge_client().download_installer(mc, &spec.version, &installer)).await?;
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
                    cancellable(cancel, neoforge_client().install_libraries(&profile, &paths.libraries)).await?;
                    neoforge::write_version_json(&profile, &versions_dir).await?;
                    // Les processeurs (Java synchrone) ne s'interrompent pas : on ne les démarre pas si l'annulation est déjà demandée.
                    check_cancel(cancel)?;
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
                            cancellable(cancel, neoforge_client().install_libraries(&inst.profile, &paths.libraries)).await?
                        }
                    }
                    neoforge::write_version_json(&inst.profile, &versions_dir).await?;
                    check_cancel(cancel)?;
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

/// Prépare tout ce qu'il faut pour lancer `inst` (sans annulation possible : création, import, rejoindre un ami).
pub async fn ensure_game(paths: &Paths, inst: &Instance, java_setting: Option<String>, progress: Progress) -> Result<Prepared> {
    ensure_game_with(paths, inst, java_setting, progress, Arc::new(AtomicBool::new(false))).await
}

/// Comme [`ensure_game`], mais `cancel` levé => `Error::Cancelled` : tout de suite pour les requêtes réseau (le futur est
/// abandonné), à l'étape suivante pour l'installation des processeurs du loader et l'extraction des natives (blocs bloquants,
/// courts ou idempotents, qu'on ne coupe jamais au milieu).
pub async fn ensure_game_with(
    paths: &Paths,
    inst: &Instance,
    java_setting: Option<String>,
    progress: Progress,
    cancel: Arc<AtomicBool>,
) -> Result<Prepared> {
    check_cancel(&cancel)?;
    if !safe_version(&inst.minecraft) || (inst.loader.loader != Loader::Vanilla && !safe_version(&inst.loader.version)) {
        return Err(Error::Invalid("version de Minecraft ou de loader invalide".into()));
    }
    let client = http()?;
    progress("Récupération de la version Minecraft…".into());
    let vanilla = cancellable(&cancel, vanilla_json(paths, &client, &inst.minecraft)).await?;

    progress("Recherche de Java…".into());
    let required = required_java_major(&inst.minecraft, vanilla.java_version.as_ref().map(|j| j.major_version));
    let java = resolve_java(paths, required, java_setting.as_deref(), &cancel).await?;

    progress("Téléchargement du jeu…".into());
    let client_jar = paths.versions.join(&inst.minecraft).join(format!("{}.jar", inst.minecraft));
    let dl = &vanilla.downloads.client;
    cancellable(&cancel, download_verified(&client, &dl.url, &client_jar, Some(&dl.sha1), Some(dl.size))).await?;

    let spec = LoaderSpec { loader: inst.loader.loader, version: inst.loader.version.clone() };
    if spec.loader != Loader::Vanilla {
        progress("Installation du loader…".into());
    }
    let version = install_loader(paths, &client_jar, &inst.minecraft, &spec, &vanilla, &java, &cancel).await?;

    progress("Téléchargement des bibliothèques…".into());
    let raw_libs = match &version {
        LaunchVersion::Vanilla(v) => v.libraries.clone(),
        LaunchVersion::Merged(m) => m.libraries.clone(),
    };
    let libs = libraries::resolve(&raw_libs, Os::current(), Arch::current())?;
    cancellable(&cancel, missing_libs(&client, &libs, &paths.libraries)).await?;

    check_cancel(&cancel)?;
    let natives_dir = paths.natives.join(&inst.id);
    let (l, ld, nd) = (libs.clone(), paths.libraries.clone(), natives_dir.clone());
    tokio::task::spawn_blocking(move || extract_natives(&l, &ld, &nd))
        .await
        .map_err(|_| Error::Other("extraction des natives interrompue".into()))??;

    progress("Téléchargement des ressources…".into());
    let index = cancellable(&cancel, fetch_asset_index(&vanilla.asset_index, &paths.assets)).await?;
    let resources = crate::test_hooks::hooks().resources_url();
    // `download_assets_with` lit le drapeau entre les objets ; les requêtes en vol sont abandonnées avec lui.
    let opts = DownloadOptions { cancel: cancel.clone(), ..DownloadOptions::default() };
    cancellable(&cancel, download_assets_with(&index, &paths.assets, resources.as_deref(), |_| {}, &opts)).await?;

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

    /// Une connexion qui accepte puis ne répond plus doit échouer en `Network` (le lancement ne reste pas figé).
    /// Délai court injecté ; le client de production utilise `core_lib::http::builder()` (30 s).
    #[tokio::test]
    async fn telechargement_bloque_echoue_en_network() {
        use std::time::{Duration, Instant};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/lib.jar", l.local_addr().unwrap());
        let srv = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((s, _)) = l.accept().await {
                held.push(s);
            }
        });
        let client = core_lib::http::builder_with(Duration::from_secs(5), Duration::from_millis(300)).build().unwrap();
        let t = Instant::now();
        let err = fetch_bytes(&client, &url).await.unwrap_err();
        assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
        assert!(matches!(&err, Error::Network(m) if m.contains("délai dépassé")), "{err:?}");
        srv.abort();
    }

    /// Une requête qui ne répond jamais s'interrompt dès que le drapeau est levé (et pas au bout du délai réseau).
    #[tokio::test]
    async fn annulation_interrompt_une_requete_bloquee() {
        use std::time::{Duration, Instant};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/lib.jar", l.local_addr().unwrap());
        let srv = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((s, _)) = l.accept().await {
                held.push(s);
            }
        });
        let client = core_lib::http::builder_with(Duration::from_secs(30), Duration::from_secs(30)).build().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(250)).await;
            c2.store(true, Ordering::SeqCst);
        });
        let t = Instant::now();
        let err = cancellable(&cancel, fetch_bytes(&client, &url)).await.unwrap_err();
        assert!(matches!(err, Error::Cancelled), "{err:?}");
        assert!(t.elapsed() >= Duration::from_millis(200) && t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
        srv.abort();
    }

    #[tokio::test]
    async fn drapeau_deja_leve_rien_n_est_lance() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let cancel = Arc::new(AtomicBool::new(true));
        // `cancellable` refuse de démarrer
        let r: Result<()> = cancellable(&cancel, async { panic!("ne doit pas être lancé") }).await;
        assert!(matches!(r, Err(Error::Cancelled)));
        // `ensure_game_with` rend `Cancelled` avant tout accès réseau ou disque
        let store = core_lib::instance::InstanceStore::new(paths.instances.clone());
        let inst = store.create("T", "1.21.1", LoaderSpec { loader: Loader::Vanilla, version: String::new() }, 2048).unwrap();
        let progress: Progress = Arc::new(|m| panic!("aucune étape ne doit démarrer : {m}"));
        let err = ensure_game_with(&paths, &inst, None, progress, cancel).await.unwrap_err();
        assert!(matches!(err, Error::Cancelled), "{err:?}");
        assert!(!paths.versions.join("1.21.1").exists());
    }

    #[test]
    fn chemins_et_versions() {
        assert!(safe_rel("org/lwjgl/a.jar").is_ok());
        for bad in ["", "../x", "/x", "a\\b", "C:/x", "a//b"] {
            assert!(safe_rel(bad).is_err(), "{bad}");
        }
        assert!(safe_version("1.21.1") && safe_version("21.1.77-beta"));
        assert!(!safe_version("../1") && !safe_version("") && !safe_version("a b"));
    }

    /// Mesure (manuelle) : durée de `ensure_game` sur une instance déjà installée du vrai dossier de données.
    ///   CUBELINK_TIMING_DIR=<dossier de données> CUBELINK_TIMING_ID=<instance> cargo test -p cubelink --lib mesure_ensure_game -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn mesure_ensure_game() {
        let (Ok(dir), Ok(id)) = (std::env::var("CUBELINK_TIMING_DIR"), std::env::var("CUBELINK_TIMING_ID")) else { return };
        let paths = Paths::new(std::path::Path::new(&dir));
        let inst = core_lib::instance::InstanceStore::new(paths.instances.clone()).get(&id).unwrap();
        for run in 1..=2 {
            let t = std::time::Instant::now();
            let last = std::sync::Arc::new(std::sync::Mutex::new(t));
            let l2 = last.clone();
            let progress: Progress = std::sync::Arc::new(move |m: String| {
                let mut g = l2.lock().unwrap();
                eprintln!("  [{:>6} ms] {m}", g.elapsed().as_millis());
                *g = std::time::Instant::now();
            });
            ensure_game(&paths, &inst, None, progress).await.unwrap();
            eprintln!("run {run} : total {} ms (dernier palier : {} ms)", t.elapsed().as_millis(), last.lock().unwrap().elapsed().as_millis());
        }
    }
}
