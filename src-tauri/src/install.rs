//! Préparation d'une instance : version vanilla, libraries, natives, assets, loader et Java.
//! Idempotent : tout ce qui est déjà présent et valide est sauté.

use std::path::{Path, PathBuf};
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;

use core_lib::java::{check_configured, default_search_dirs, find_installed, required_java_major, select_for, ConfiguredJava};
use core_lib::java_install;
use futures_util::{stream, StreamExt};
use core_lib::launch_args::LaunchVersion;
use core_lib::loader::{fabric, forge, neoforge, quilt};
use core_lib::mojang::assets::{download_assets_with, fetch_asset_index, AssetProgress, DownloadOptions};
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
    /// `${game_assets}` : `assets/virtual/<id>` pour un index legacy, sinon `assets_dir`.
    pub game_assets_dir: PathBuf,
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
    // Le nom complet + `.part` : `with_extension` ferait collision entre `x.jar` et `x.pom`, écrits en parallèle.
    let mut name = dest.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".part");
    let tmp = dest.with_file_name(name);
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

/// Un chemin de Java choisi à la main, avec son origine : elle décide de ce qui se passe s'il ne convient pas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JavaSetting {
    /// Réglage propre à l'instance : imposé, un Java qui ne convient pas fait échouer le lancement (message explicite).
    Instance(String),
    /// Réglage global (Paramètres) : simple candidat, ignoré (et journalisé) s'il ne convient pas à cette version.
    Global(String),
}

impl JavaSetting {
    /// Le réglage propre à l'instance s'il est renseigné, sinon le global.
    pub fn for_instance(instance: Option<&str>, global: Option<String>) -> Option<JavaSetting> {
        let own = instance.map(str::trim).filter(|j| !j.is_empty());
        match (own, global.filter(|g| !g.trim().is_empty())) {
            (Some(i), _) => Some(JavaSetting::Instance(i.to_string())),
            (None, Some(g)) => Some(JavaSetting::Global(g)),
            (None, None) => None,
        }
    }

    /// Réglage global seul (création, import, rejoindre un ami : l'instance n'a pas encore de Java propre).
    pub fn global(global: Option<String>) -> Option<JavaSetting> {
        Self::for_instance(None, global)
    }
}

/// Évalue le Java réglé à la main. `Ok(Some)` : à utiliser. `Ok(None)` : réglage global qui ne convient pas, à ignorer.
/// `Err` : réglage d'instance qui ne convient pas. Un Java dont la version ne se lit pas (wrapper, sortie inhabituelle)
/// est gardé tel quel, comme avant : on ne peut pas affirmer qu'il est faux.
async fn resolve_setting(setting: &JavaSetting, required: u32, mc: &str) -> Result<Option<PathBuf>> {
    let (own, raw) = match setting {
        JavaSetting::Instance(p) => (true, p),
        JavaSetting::Global(p) => (false, p),
    };
    let p = PathBuf::from(raw);
    if !p.is_file() {
        if own {
            return Err(Error::NotFound("l'exécutable Java configuré est introuvable".into()));
        }
        eprintln!("[java] Java des Paramètres introuvable, ignoré : Cubelink choisit le Java de Minecraft {mc} (Java {required})");
        return Ok(None);
    }
    let probe_path = p.clone();
    let verdict = tokio::task::spawn_blocking(move || check_configured(&probe_path, required))
        .await
        .map_err(|_| Error::Other("vérification de Java interrompue".into()))?;
    match (verdict, own) {
        (ConfiguredJava::Fits | ConfiguredJava::Unprobeable, _) => Ok(Some(p)),
        // Java d'instance plus récent que le minimum d'un vieux Minecraft (11 ou 17 pour un 1.16.5 : Java 8) : choix explicite
        // du joueur, qui fonctionne en vanilla et avec Fabric. On le garde (comme avant) au lieu de faire échouer le lancement.
        (ConfiguredJava::Mismatch { found }, true) if required < 16 && found > required => {
            eprintln!("[java] Java {found} réglé pour l'instance, Minecraft {mc} demande Java {required} : gardé, choix explicite");
            Ok(Some(p))
        }
        (ConfiguredJava::Mismatch { found }, true) => Err(Error::Invalid(format!(
            "Java {found} configuré pour cette instance, Minecraft {mc} demande Java {required}. Change le chemin ou vide-le pour laisser Cubelink choisir."
        ))),
        (ConfiguredJava::Mismatch { found }, false) => {
            eprintln!("[java] Java {found} des Paramètres ignoré : Minecraft {mc} demande Java {required}, Cubelink choisit le bon");
            Ok(None)
        }
    }
}

async fn resolve_java(paths: &Paths, required: u32, mc: &str, setting: Option<&JavaSetting>, cancel: &Arc<AtomicBool>, progress: &Progress) -> Result<PathBuf> {
    // Mode test : chemin de java imposé (faux java), pris tel quel, jamais sondé.
    if let Some(p) = crate::test_hooks::hooks().java_path.as_ref() {
        return if p.is_file() { Ok(p.clone()) } else { Err(Error::NotFound("l'exécutable Java configuré est introuvable".into())) };
    }
    if let Some(s) = setting {
        if let Some(p) = resolve_setting(s, required, mc).await? {
            return Ok(p);
        }
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
    let report = pct_reporter(progress.clone(), "Téléchargement de Java…");
    let done = if crate::test_hooks::hooks().skip_java_probe {
        let probe = Arc::new(crate::test_hooks::FakeProbe(pkg.major));
        java_install::install_with(&pkg, &paths.java, report, cancel, probe).await?
    } else {
        java_install::install(&pkg, &paths.java, report, cancel).await?
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

/// Nombre de bibliothèques téléchargées en même temps (comme les assets).
const LIB_CONCURRENCY: usize = 8;

/// Pourcentage entier (0 à 100) ; `None` si le total est inconnu (0).
fn pct(done: u64, total: u64) -> Option<u8> {
    if total == 0 {
        return None;
    }
    Some((u128::from(done) * 100 / u128::from(total)).min(100) as u8)
}

/// Rappel `(fait, total)` qui annonce « `label` xx % » à `progress`, uniquement quand le pourcentage entier change
/// (les rappels viennent de tâches concurrentes et arrivent par centaines : pas d'inondation de `launch://state`).
fn pct_reporter(progress: Progress, label: &'static str) -> impl Fn(u64, u64) + Send + Sync + 'static {
    let last = AtomicU8::new(u8::MAX);
    move |done, total| {
        if let Some(p) = pct(done, total) {
            if last.swap(p, Ordering::Relaxed) != p {
                progress(format!("{label} {p} %"));
            }
        }
    }
}

/// Télécharge les bibliothèques manquantes, `LIB_CONCURRENCY` à la fois. `on_progress(faits, total)` compte les seules
/// bibliothèques à télécharger (rien si tout est déjà sur le disque). Le premier échec arrête tout : les requêtes en vol
/// sont abandonnées avec le flux, sans risque grâce à l'écriture atomique.
async fn missing_libs(client: &reqwest::Client, libs: &[Library], libs_dir: &Path, on_progress: impl Fn(usize, usize) + Send) -> Result<()> {
    // Passe synchrone : tous les chemins sont validés avant la moindre requête.
    let mut seen = std::collections::HashSet::new();
    let mut todo = Vec::new();
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
        // Deux entrées vers le même fichier : une seule requête.
        if seen.insert(lib.path.clone()) {
            todo.push((lib.url.clone(), dest, lib.sha1.clone(), lib.size));
        }
    }
    let total = todo.len();
    let mut results = stream::iter(todo)
        .map(|(url, dest, sha1, size)| {
            let client = client.clone();
            async move { download_verified(&client, &url, &dest, sha1.as_deref(), size).await }
        })
        .buffer_unordered(LIB_CONCURRENCY);
    let mut done = 0;
    while let Some(r) = results.next().await {
        r?;
        done += 1;
        on_progress(done, total);
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
pub async fn ensure_game(paths: &Paths, inst: &Instance, java_setting: Option<JavaSetting>, progress: Progress) -> Result<Prepared> {
    ensure_game_with(paths, inst, java_setting, progress, Arc::new(AtomicBool::new(false))).await
}

/// Comme [`ensure_game`], mais `cancel` levé => `Error::Cancelled` : tout de suite pour les requêtes réseau (le futur est
/// abandonné), à l'étape suivante pour l'installation des processeurs du loader et l'extraction des natives (blocs bloquants,
/// courts ou idempotents, qu'on ne coupe jamais au milieu).
pub async fn ensure_game_with(
    paths: &Paths,
    inst: &Instance,
    java_setting: Option<JavaSetting>,
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
    let java = resolve_java(paths, required, &inst.minecraft, java_setting.as_ref(), &cancel, &progress).await?;

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
    let lib_progress = progress.clone();
    let on_lib = move |done: usize, total: usize| lib_progress(format!("Téléchargement des bibliothèques… {done}/{total}"));
    cancellable(&cancel, missing_libs(&client, &libs, &paths.libraries, on_lib)).await?;

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
    let report = pct_reporter(progress.clone(), "Téléchargement des ressources…");
    let on_assets = move |p: AssetProgress| report(p.bytes_done, p.bytes_total);
    cancellable(&cancel, download_assets_with(&index, &paths.assets, resources.as_deref(), on_assets, &opts)).await?;


    // Vieux clients (index legacy / pre-1.6) : les sons sont lus par nom, pas par hash.
    let game_dir = core_lib::instance::InstanceStore::new(paths.instances.clone()).game_dir(&inst.id)?;
    let (idx, id, assets, cancel_flag) = (index, vanilla.asset_index.id.clone(), paths.assets.clone(), cancel.clone());
    let virtual_dir = tokio::task::spawn_blocking(move || {
        core_lib::mojang::assets::materialize_legacy_assets(&idx, &id, &assets, &game_dir, &cancel_flag)
    })
    .await
    .map_err(|_| Error::Other("préparation des ressources interrompue".into()))??;

    Ok(Prepared {
        java,
        version,
        libs_dir: paths.libraries.clone(),
        assets_dir: paths.assets.clone(),
        game_assets_dir: virtual_dir.unwrap_or_else(|| paths.assets.clone()),
        natives_dir,
        client_jar,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet() -> Progress {
        Arc::new(|_| {})
    }

    /// Serveur HTTP minimal : sert `GET /<nom>` (corps = `contenu-<nom>`) après `delay`, compte les requêtes et la
    /// concurrence maximale observée. Renvoie (base_url, requêtes, concurrence max, tâche).
    async fn serveur_libs(
        delay: std::time::Duration,
    ) -> (String, Arc<std::sync::atomic::AtomicUsize>, Arc<std::sync::atomic::AtomicUsize>, tokio::task::JoinHandle<()>) {
        use std::sync::atomic::AtomicUsize;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        let (reqs, max) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let (r2, m2) = (reqs.clone(), max.clone());
        let live = Arc::new(AtomicUsize::new(0));
        let h = tokio::spawn(async move {
            while let Ok((mut sock, _)) = l.accept().await {
                let (reqs, max, live) = (r2.clone(), m2.clone(), live.clone());
                tokio::spawn(async move {
                    let mut buf = [0u8; 2048];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let head = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = head.split_whitespace().nth(1).unwrap_or("/").trim_start_matches('/').to_string();
                    reqs.fetch_add(1, Ordering::SeqCst);
                    let now = live.fetch_add(1, Ordering::SeqCst) + 1;
                    max.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(delay).await;
                    live.fetch_sub(1, Ordering::SeqCst);
                    let body = format!("contenu-{path}");
                    let resp = format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
                    let _ = sock.write_all(resp.as_bytes()).await;
                });
            }
        });
        (base, reqs, max, h)
    }

    fn lib(base: &str, name: &str) -> Library {
        let body = format!("contenu-{name}");
        Library {
            name: name.into(),
            path: format!("org/x/{name}"),
            url: format!("{base}/{name}"),
            sha1: Some(sha1_hex(body.as_bytes())),
            size: Some(body.len() as u64),
            is_native: false,
            extract_excludes: vec![],
        }
    }

    #[test]
    fn pct_borne_et_total_nul() {
        assert_eq!(pct(0, 0), None);
        assert_eq!(pct(5, 0), None);
        assert_eq!(pct(0, 10), Some(0));
        assert_eq!(pct(5, 10), Some(50));
        assert_eq!(pct(10, 10), Some(100));
        assert_eq!(pct(99, 10), Some(100));
        assert_eq!(pct(u64::MAX, u64::MAX), Some(100));
    }

    #[test]
    fn pct_reporter_emet_au_changement_de_pourcent_seulement() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let s2 = seen.clone();
        let r = pct_reporter(Arc::new(move |m| s2.lock().unwrap().push(m)), "Java");
        r(1, 0); // total inconnu : rien
        for i in 0..=1000 {
            r(i, 1000);
        }
        r(2000, 1000); // au-delà : reste à 100, pas de doublon
        let got = seen.lock().unwrap().clone();
        assert_eq!(got.len(), 101, "{got:?}");
        assert_eq!(got.first().unwrap(), "Java 0 %");
        assert_eq!(got.last().unwrap(), "Java 100 %");
    }

    /// Les bibliothèques se téléchargent en parallèle (borné à `LIB_CONCURRENCY`), la progression est strictement
    /// croissante jusqu'au total, et tout est sur le disque avec le bon contenu.
    #[tokio::test]
    async fn missing_libs_en_parallele_avec_progression() {
        let (base, reqs, max, srv) = serveur_libs(std::time::Duration::from_millis(60)).await;
        let dir = tempfile::tempdir().unwrap();
        let libs: Vec<Library> = (0..20).map(|i| lib(&base, &format!("l{i}.jar"))).collect();
        let client = http().unwrap();
        let seen = Arc::new(std::sync::Mutex::new(Vec::<(usize, usize)>::new()));
        let s2 = seen.clone();
        let t = std::time::Instant::now();
        missing_libs(&client, &libs, dir.path(), move |d, t| s2.lock().unwrap().push((d, t))).await.unwrap();
        // 20 requêtes de 60 ms : en série ce serait >= 1,2 s.
        assert!(t.elapsed() < std::time::Duration::from_millis(900), "{:?}", t.elapsed());
        let m = max.load(Ordering::SeqCst);
        assert!(m > 1 && m <= LIB_CONCURRENCY, "concurrence max {m}");
        assert_eq!(reqs.load(Ordering::SeqCst), 20);
        let got = seen.lock().unwrap().clone();
        assert_eq!(got, (1..=20).map(|d| (d, 20)).collect::<Vec<_>>());
        for l in &libs {
            assert_eq!(std::fs::read_to_string(dir.path().join(&l.path)).unwrap(), format!("contenu-{}", l.name));
        }
        srv.abort();
    }

    /// Déjà valides : zéro requête, aucun rappel. Même chemin deux fois : une seule requête. `x.jar` et `x.pom` en
    /// parallèle ne partagent pas de fichier `.part`.
    #[tokio::test]
    async fn missing_libs_saute_le_present_et_dedoublonne() {
        let (base, reqs, _max, srv) = serveur_libs(std::time::Duration::from_millis(20)).await;
        let dir = tempfile::tempdir().unwrap();
        let client = http().unwrap();
        let present = lib(&base, "deja.jar");
        let p = dir.path().join(&present.path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "contenu-deja.jar").unwrap();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c2 = calls.clone();
        missing_libs(&client, &[present], dir.path(), move |_, _| {
            c2.fetch_add(1, Ordering::SeqCst);
        })
        .await
        .unwrap();
        assert_eq!((reqs.load(Ordering::SeqCst), calls.load(Ordering::SeqCst)), (0, 0));

        let a = lib(&base, "dup.jar");
        let (jar, pom) = (lib(&base, "x.jar"), lib(&base, "x.pom"));
        let c = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c3 = c.clone();
        missing_libs(&client, &[a.clone(), a, jar, pom], dir.path(), move |_, _| {
            c3.fetch_add(1, Ordering::SeqCst);
        })
        .await
        .unwrap();
        assert_eq!(reqs.load(Ordering::SeqCst), 3);
        assert_eq!(c.load(Ordering::SeqCst), 3);
        assert!(dir.path().join("org/x/x.jar").is_file() && dir.path().join("org/x/x.pom").is_file());
        srv.abort();
    }

    /// Un sha1 faux : `Integrity`, pas de fichier final pour cette lib ; un chemin piégé échoue avant toute requête.
    #[tokio::test]
    async fn missing_libs_refuse_sha1_faux_et_chemin_piege() {
        let (base, reqs, _max, srv) = serveur_libs(std::time::Duration::from_millis(5)).await;
        let dir = tempfile::tempdir().unwrap();
        let client = http().unwrap();
        let mut bad = lib(&base, "mauvais.jar");
        bad.sha1 = Some("0".repeat(40));
        bad.size = None;
        let err = missing_libs(&client, &[bad.clone()], dir.path(), |_, _| {}).await.unwrap_err();
        assert!(matches!(err, Error::Integrity { .. }), "{err:?}");
        assert!(!dir.path().join(&bad.path).exists());

        let before = reqs.load(Ordering::SeqCst);
        let mut evil = lib(&base, "evil.jar");
        evil.path = "../evil.jar".into();
        let err = missing_libs(&client, &[lib(&base, "ok.jar"), evil], dir.path(), |_, _| {}).await.unwrap_err();
        assert!(matches!(err, Error::Invalid(_)), "{err:?}");
        assert_eq!(reqs.load(Ordering::SeqCst), before, "aucune requête avant la validation des chemins");
        srv.abort();
    }

    /// Le drapeau levé pendant des requêtes qui ne répondent jamais interrompt le lot tout de suite.
    #[tokio::test]
    async fn missing_libs_annulation_rapide() {
        use std::time::{Duration, Instant};
        let (base, _reqs, _max, srv) = serveur_libs(Duration::from_secs(60)).await;
        let dir = tempfile::tempdir().unwrap();
        let libs: Vec<Library> = (0..10).map(|i| lib(&base, &format!("l{i}.jar"))).collect();
        let client = core_lib::http::builder_with(Duration::from_secs(30), Duration::from_secs(30)).build().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            c2.store(true, Ordering::SeqCst);
        });
        let t = Instant::now();
        let err = cancellable(&cancel, missing_libs(&client, &libs, dir.path(), |_, _| {})).await.unwrap_err();
        assert!(matches!(err, Error::Cancelled), "{err:?}");
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
        srv.abort();
    }

    /// Les assets relaient leur progression en octets vers `progress` (« xx % », croissant, fini à 100 %), avec le vrai
    /// téléchargeur de `core` contre un serveur loopback.
    #[tokio::test]
    async fn assets_relaient_la_progression_en_pourcentage() {
        use core_lib::mojang::assets::{AssetIndex, AssetObject};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut objects = std::collections::BTreeMap::new();
        let mut bodies = std::collections::HashMap::new();
        for i in 0..8 {
            let body = format!("asset numero {i}").repeat(i + 1);
            let hash = sha1_hex(body.as_bytes());
            objects.insert(format!("f{i}"), AssetObject { hash: hash.clone(), size: body.len() as u64 });
            bodies.insert(format!("/{}/{hash}", &hash[..2]), body);
        }
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        let bodies = Arc::new(bodies);
        let srv = tokio::spawn(async move {
            while let Ok((mut sock, _)) = l.accept().await {
                let bodies = bodies.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 2048];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let head = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let body = bodies.get(&path).cloned().unwrap_or_default();
                    let resp = format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
                    let _ = sock.write_all(resp.as_bytes()).await;
                });
            }
        });
        let index = AssetIndex { objects, map_to_resources: false, virtual_: false };
        let dir = tempfile::tempdir().unwrap();
        let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let s2 = seen.clone();
        let report = pct_reporter(Arc::new(move |m| s2.lock().unwrap().push(m)), "Téléchargement des ressources…");
        let on_assets = move |p: AssetProgress| report(p.bytes_done, p.bytes_total);
        let opts = DownloadOptions::default();
        download_assets_with(&index, dir.path(), Some(&base), on_assets, &opts).await.unwrap();
        let got = seen.lock().unwrap().clone();
        assert!(got.len() >= 2 && got.last().unwrap() == "Téléchargement des ressources… 100 %", "{got:?}");
        let nums: Vec<u8> = got.iter().map(|m| m.rsplit(' ').nth(1).unwrap().parse().unwrap()).collect();
        assert!(nums.windows(2).all(|w| w[0] < w[1]), "{nums:?}");
        srv.abort();
    }

    /// Le `.part` garde le nom complet : `x.jar` et `x.pom` n'écrivent pas dans le même fichier temporaire.
    #[tokio::test]
    async fn write_atomic_nom_temporaire_unique_par_fichier() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("x.jar"), dir.path().join("x.pom"));
        let (ra, rb) = tokio::join!(write_atomic(&a, b"aaa"), write_atomic(&b, b"bbb"));
        ra.unwrap();
        rb.unwrap();
        assert_eq!((std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap()), (b"aaa".to_vec(), b"bbb".to_vec()));
        assert!(std::fs::read_dir(dir.path()).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().ends_with(".part")));
    }

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

    // --- Java réglé à la main (#71) : un faux `java` (script shell) exécuté pour de vrai ---------------------------

    /// Écrit un exécutable qui imprime `output` sur stderr, comme `java -version`.
    #[cfg(unix)]
    fn fake_java(dir: &Path, name: &str, output: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\ncat >&2 <<'EOF'\n{output}\nEOF\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[cfg(unix)]
    fn says(major: &str) -> String {
        format!("openjdk version \"{major}\" 2024-01-16\nOpenJDK Runtime Environment Temurin")
    }

    #[test]
    fn reglage_d_instance_prioritaire_sur_le_global() {
        let g = || Some("/g/java".to_string());
        assert_eq!(JavaSetting::for_instance(Some("/i/java"), g()), Some(JavaSetting::Instance("/i/java".into())));
        assert_eq!(JavaSetting::for_instance(Some("  "), g()), Some(JavaSetting::Global("/g/java".into())));
        assert_eq!(JavaSetting::for_instance(None, g()), Some(JavaSetting::Global("/g/java".into())));
        assert_eq!(JavaSetting::for_instance(None, Some(" ".into())), None);
        assert_eq!(JavaSetting::global(None), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn java_global_qui_ne_convient_pas_est_ignore_celui_de_l_instance_echoue() {
        let dir = tempfile::tempdir().unwrap();
        let j17 = fake_java(dir.path(), "j17", &says("17.0.9")).display().to_string();
        // Global 17 pour Minecraft 1.21.1 (Java 21) : ignoré, on retombe sur la sélection automatique.
        assert_eq!(resolve_setting(&JavaSetting::Global(j17.clone()), 21, "1.21.1").await.unwrap(), None);
        // Instance : échec explicite, avec les deux versions et la version de Minecraft.
        let err = resolve_setting(&JavaSetting::Instance(j17.clone()), 21, "1.21.1").await.unwrap_err();
        let msg = err.to_string();
        assert!(matches!(err, Error::Invalid(_)), "{err:?}");
        assert!(msg.contains("Java 17") && msg.contains("Java 21") && msg.contains("1.21.1"), "{msg}");
        // Un Java qui convient est gardé, dans les deux cas ; un Java 17 convient à un Minecraft 1.18 (Java 17).
        assert_eq!(resolve_setting(&JavaSetting::Global(j17.clone()), 17, "1.18.2").await.unwrap(), Some(PathBuf::from(&j17)));
        assert_eq!(resolve_setting(&JavaSetting::Instance(j17.clone()), 17, "1.18.2").await.unwrap(), Some(PathBuf::from(&j17)));
        // Java 21 global pour un 1.12.2 (Java 8) : refusé.
        let j21 = fake_java(dir.path(), "j21", &says("21.0.2")).display().to_string();
        assert_eq!(resolve_setting(&JavaSetting::Global(j21.clone()), 8, "1.12.2").await.unwrap(), None);
        // Java d'instance plus récent pour un vieux Minecraft : choix explicite, gardé ; plus ancien que requis : échec.
        assert_eq!(resolve_setting(&JavaSetting::Instance(j17.clone()), 8, "1.16.5").await.unwrap(), Some(PathBuf::from(&j17)));
        let j8 = fake_java(dir.path(), "j8", &says("1.8.0_392")).display().to_string();
        assert!(matches!(resolve_setting(&JavaSetting::Instance(j8), 17, "1.18.2").await, Err(Error::Invalid(_))));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn java_illisible_ou_introuvable() {
        let dir = tempfile::tempdir().unwrap();
        // Sortie inhabituelle : on ne peut pas juger, le réglage est gardé tel quel (comportement d'avant).
        let odd = fake_java(dir.path(), "odd", "version inconnue").display().to_string();
        assert_eq!(resolve_setting(&JavaSetting::Global(odd.clone()), 21, "1.21.1").await.unwrap(), Some(PathBuf::from(&odd)));
        // Introuvable : le global est ignoré, celui de l'instance fait échouer.
        let gone = dir.path().join("absent").display().to_string();
        assert_eq!(resolve_setting(&JavaSetting::Global(gone.clone()), 21, "1.21.1").await.unwrap(), None);
        assert!(matches!(resolve_setting(&JavaSetting::Instance(gone), 21, "1.21.1").await, Err(Error::NotFound(_))));
    }

    /// De bout en bout dans `resolve_java` : un Java 17 global pour un Minecraft qui veut Java 21 n'est pas utilisé,
    /// c'est le Java 21 présent dans le dossier de runtimes de Cubelink qui est retenu (aucun téléchargement).
    #[cfg(unix)]
    #[tokio::test]
    async fn resolve_java_prefere_le_java_installe_au_global_inadapte() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let jre21 = paths.java.join("temurin-21");
        std::fs::create_dir_all(jre21.join("bin")).unwrap();
        let exe = fake_java(&jre21.join("bin"), "java", &says("21.0.2"));
        std::fs::write(jre21.join("release"), "JAVA_VERSION=\"21.0.2\"\nIMPLEMENTOR=\"Eclipse Adoptium\"\n").unwrap();
        let j17 = fake_java(dir.path(), "j17", &says("17.0.9")).display().to_string();
        let cancel = Arc::new(AtomicBool::new(false));

        let got = resolve_java(&paths, 21, "1.21.1", Some(&JavaSetting::Global(j17.clone())), &cancel, &quiet()).await.unwrap();
        assert_eq!(got, exe);
        // Le même Java 17, propre à l'instance : le lancement échoue au lieu de planter dans la JVM.
        let err = resolve_java(&paths, 21, "1.21.1", Some(&JavaSetting::Instance(j17.clone())), &cancel, &quiet()).await.unwrap_err();
        assert!(err.to_string().contains("Java 17"), "{err}");
        // Et pour une version de Minecraft qui accepte Java 17, le global est bien utilisé.
        let got = resolve_java(&paths, 17, "1.18.2", Some(&JavaSetting::Global(j17.clone())), &cancel, &quiet()).await.unwrap();
        assert_eq!(got, PathBuf::from(j17));
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
