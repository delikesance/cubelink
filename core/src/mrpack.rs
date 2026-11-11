//! Tâche I4 : import d'un modpack Modrinth `.mrpack` (zip).
//!
//! Flux : lecture de `modrinth.index.json` -> validation complète (chemins, hôtes, zip) AVANT toute
//! création -> création de l'instance -> téléchargement via le magasin (`fetch_into_store`, URLs de
//! repli dans l'ordre, concurrence bornée) -> placement dans l'instance -> extraction de
//! `overrides/` puis `client-overrides/`. Tout échec après la création supprime l'instance
//! (rollback) et relâche les références prises dans le magasin.
//!
//! Limite d'API : `install_to_instance` n'écrit que sous `mods/` ; pour les autres chemins du pack
//! (config, resourcepacks...) on appelle directement `link_into` puis `add_ref`.

use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use futures_util::stream::{self, StreamExt};
use serde::Deserialize;
use zip::ZipArchive;

use crate::instance::InstanceStore;
use crate::providers::curseforge::CfDownload;
use crate::providers::curseforge::{CfResolver, CDN_HOSTS, MAX_FILE_BYTES};
use crate::providers::modrinth_api::{FileHashes, VersionFile};
use crate::providers::modrinth_dl::{fetch_into_store, fetch_into_store_unverified, DlOptions, StoredObject};
use crate::store::index::StoreIndex;
use crate::store::link::link_into;
use crate::types::{Error, Instance, Loader, LoaderSpec, Result};

const INDEX_NAME: &str = "modrinth.index.json";
const MAX_INDEX_BYTES: u64 = 8 * 1024 * 1024;
const MAX_ENTRIES: usize = 100_000;
/// Dossiers autorisés pour les fichiers listés dans l'index.
const FILE_DIRS: &[&str] = &["mods", "config", "resourcepacks", "shaderpacks", "datapacks", "defaultconfigs", "kubejs"];
/// Extensions exécutables refusées (fichiers du pack et overrides).
const DANGEROUS_EXT: &[&str] = &["exe", "bat", "cmd", "dll", "ps1", "com", "scr", "msi", "vbs", "lnk", "sh"];
const DEFAULT_HOSTS: &[&str] = &["cdn.modrinth.com", "github.com", "raw.githubusercontent.com", "gitlab.com"];

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
pub struct MrpackEnv {
    pub client: Option<String>,
    pub server: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MrpackFile {
    pub path: String,
    pub sha1: String,
    pub sha512: String,
    pub downloads: Vec<String>,
    pub file_size: u64,
    pub env: Option<MrpackEnv>,
    /// Sans hash de référence (CurseForge sans clé) : un échec de téléchargement n'est pas fatal.
    pub unverified: bool,
    /// Fichier CurseForge : un échec de téléchargement n'est pas fatal (le mod est listé comme manquant).
    pub tolerant: bool,
}

impl MrpackFile {
    fn client_unsupported(&self) -> bool {
        self.env.as_ref().and_then(|e| e.client.as_deref()) == Some("unsupported")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MrpackIndex {
    pub name: String,
    pub version_id: String,
    pub minecraft: String,
    pub loader: LoaderSpec,
    pub files: Vec<MrpackFile>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawIndex {
    format_version: u32,
    #[serde(default)]
    game: Option<String>,
    #[serde(default)]
    version_id: String,
    name: String,
    #[serde(default)]
    files: Vec<RawFile>,
    dependencies: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawFile {
    path: String,
    #[serde(default)]
    hashes: RawHashes,
    #[serde(default)]
    downloads: Vec<String>,
    #[serde(default)]
    file_size: u64,
    #[serde(default)]
    env: Option<MrpackEnv>,
}

#[derive(Deserialize, Default)]
struct RawHashes {
    #[serde(default)]
    sha1: String,
    #[serde(default)]
    sha512: String,
}

/// Callback de progression : (fichiers terminés, fichiers à installer).
pub type FilesProgress = Arc<dyn Fn(u32, u32) + Send + Sync>;

#[derive(Clone)]
pub struct ImportOptions {
    /// Hôtes de téléchargement autorisés (comparaison exacte, sans port).
    pub allowed_hosts: Vec<String>,
    /// Autorise `http://` (tests uniquement). Par défaut seul `https://` est accepté.
    pub allow_http: bool,
    pub concurrency: usize,
    pub ram_mb: u32,
    /// Taille décompressée totale maximale des overrides.
    pub max_total_bytes: u64,
    /// Ratio décompressé/compressé maximal d'une entrée (vérifié au-delà de 1 Mio).
    pub max_ratio: u64,
    pub dl: DlOptions,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            allowed_hosts: DEFAULT_HOSTS.iter().map(|s| s.to_string()).collect(),
            allow_http: false,
            concurrency: 4,
            ram_mb: 4096,
            max_total_bytes: 2 * 1024 * 1024 * 1024,
            max_ratio: 200,
            dl: DlOptions::default(),
        }
    }
}

fn invalid(msg: impl Into<String>) -> Error {
    Error::Invalid(msg.into())
}

// ---------- Validation des chemins ----------

fn has_dangerous_ext(name: &str) -> bool {
    let last = name.rsplit('/').next().unwrap_or(name);
    match last.rsplit_once('.') {
        Some((_, ext)) => DANGEROUS_EXT.iter().any(|d| ext.eq_ignore_ascii_case(d)),
        None => false,
    }
}

/// Valide un chemin relatif du pack (séparateur `/` uniquement). Retourne le premier segment.
fn validate_rel(path: &str) -> Result<&str> {
    let bad = |why: &str| invalid(format!("chemin du pack refusé ({why}) : {path:?}"));
    if path.is_empty() {
        return Err(bad("vide"));
    }
    if path.starts_with('/') {
        return Err(bad("absolu"));
    }
    if path.contains('\\') {
        return Err(bad("antislash"));
    }
    if path.contains(':') {
        return Err(bad("lettre de lecteur ou flux"));
    }
    if path.chars().any(|c| c.is_control()) {
        return Err(bad("caractère de contrôle"));
    }
    for seg in path.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." {
            return Err(bad("segment vide, '.' ou '..'"));
        }
        if seg.ends_with('.') || seg.ends_with(' ') {
            return Err(bad("segment finissant par '.' ou espace"));
        }
    }
    if has_dangerous_ext(path) {
        return Err(bad("extension exécutable"));
    }
    Ok(path.split('/').next().unwrap_or(path))
}

fn validate_file_path(path: &str) -> Result<()> {
    let first = validate_rel(path)?;
    if !path.contains('/') || !FILE_DIRS.contains(&first) {
        return Err(invalid(format!("chemin du pack hors des dossiers autorisés ({}) : {path:?}", FILE_DIRS.join(", "))));
    }
    Ok(())
}

fn check_url(url: &str, opts: &ImportOptions) -> Result<()> {
    let u = url::Url::parse(url).map_err(|e| invalid(format!("URL invalide {url:?} : {e}")))?;
    match u.scheme() {
        "https" => {}
        "http" if opts.allow_http => {}
        s => return Err(invalid(format!("schéma refusé ({s}) : {url}"))),
    }
    let host = u.host_str().unwrap_or("").to_ascii_lowercase();
    if !opts.allowed_hosts.iter().any(|h| h.eq_ignore_ascii_case(&host)) {
        return Err(invalid(format!("hôte de téléchargement non autorisé ({host}) : {url}")));
    }
    Ok(())
}

// ---------- Index ----------

fn read_index_json(archive: &mut ZipArchive<File>) -> Result<Vec<u8>> {
    let entry = archive.by_name(INDEX_NAME).map_err(|_| invalid(format!("{INDEX_NAME} absent du pack")))?;
    if entry.size() > MAX_INDEX_BYTES {
        return Err(invalid(format!("{INDEX_NAME} trop gros")));
    }
    let mut buf = Vec::new();
    entry.take(MAX_INDEX_BYTES + 1).read_to_end(&mut buf).map_err(|e| invalid(format!("{INDEX_NAME} illisible : {e}")))?;
    if buf.len() as u64 > MAX_INDEX_BYTES {
        return Err(invalid(format!("{INDEX_NAME} trop gros")));
    }
    Ok(buf)
}

fn open_zip(zip_path: &Path) -> Result<ZipArchive<File>> {
    let f = File::open(zip_path)?;
    let a = ZipArchive::new(f).map_err(|e| invalid(format!("archive de modpack invalide : {e}")))?;
    if a.len() > MAX_ENTRIES {
        return Err(invalid("trop d'entrées dans l'archive"));
    }
    Ok(a)
}

fn loader_from(deps: &BTreeMap<String, String>) -> Result<LoaderSpec> {
    let mut found = Vec::new();
    for (key, loader) in
        [("fabric-loader", Loader::Fabric), ("quilt-loader", Loader::Quilt), ("neoforge", Loader::NeoForge), ("forge", Loader::Forge)]
    {
        if let Some(v) = deps.get(key) {
            found.push(LoaderSpec { loader, version: v.trim().to_string() });
        }
    }
    match found.len() {
        0 => Ok(LoaderSpec { loader: Loader::Vanilla, version: String::new() }),
        1 => {
            let l = found.remove(0);
            if l.version.is_empty() {
                return Err(invalid("version du loader vide"));
            }
            Ok(l)
        }
        _ => Err(invalid("plusieurs loaders déclarés dans dependencies")),
    }
}

fn parse_index_bytes(bytes: &[u8]) -> Result<MrpackIndex> {
    let raw: RawIndex = serde_json::from_slice(bytes).map_err(|e| invalid(format!("{INDEX_NAME} invalide : {e}")))?;
    if raw.format_version != 1 {
        return Err(invalid(format!("formatVersion non supporté : {}", raw.format_version)));
    }
    if raw.game.as_deref().is_some_and(|g| g != "minecraft") {
        return Err(invalid("jeu non supporté (minecraft attendu)"));
    }
    let minecraft = raw.dependencies.get("minecraft").map(|s| s.trim().to_string()).unwrap_or_default();
    if minecraft.is_empty() {
        return Err(invalid("dépendance minecraft absente"));
    }
    let loader = loader_from(&raw.dependencies)?;
    let mut files = Vec::with_capacity(raw.files.len());
    let mut seen = HashSet::new();
    for f in raw.files {
        validate_file_path(&f.path)?;
        if !seen.insert(f.path.to_ascii_lowercase()) {
            return Err(invalid(format!("chemin dupliqué dans le pack : {:?}", f.path)));
        }
        if f.downloads.is_empty() {
            return Err(invalid(format!("{} : aucune URL de téléchargement", f.path)));
        }
        if f.hashes.sha1.is_empty() && f.hashes.sha512.is_empty() {
            return Err(invalid(format!("{} : aucun hash", f.path)));
        }
        files.push(MrpackFile {
            path: f.path,
            sha1: f.hashes.sha1,
            sha512: f.hashes.sha512,
            downloads: f.downloads,
            file_size: f.file_size,
            env: f.env,
            unverified: false,
            tolerant: false,
        });
    }
    Ok(MrpackIndex { name: raw.name, version_id: raw.version_id, minecraft, loader, files })
}

/// Lit et valide `modrinth.index.json`.
pub fn parse_index(zip_path: &Path) -> Result<MrpackIndex> {
    let mut archive = open_zip(zip_path)?;
    let bytes = read_index_json(&mut archive)?;
    parse_index_bytes(&bytes)
}

// ---------- Overrides ----------

/// Préfixes des dossiers du zip copiés tels quels dans l'instance.
struct Layout {
    common: String,
    client: Option<String>,
}

impl Layout {
    fn mrpack() -> Self {
        Self { common: "overrides/".into(), client: Some("client-overrides/".into()) }
    }
}

struct OverrideEntry {
    index: usize,
    rel: String,
}

/// Valide toutes les entrées d'overrides (zip-slip, extensions, bombe) sans rien écrire.
/// Retourne les entrées dans l'ordre d'application (`overrides/` puis `client-overrides/`).
fn plan_overrides(archive: &mut ZipArchive<File>, opts: &ImportOptions, layout: &Layout) -> Result<Vec<OverrideEntry>> {
    let mut common = Vec::new();
    let mut client = Vec::new();
    let mut total: u64 = 0;
    for i in 0..archive.len() {
        let e = archive.by_index_raw(i).map_err(|e| invalid(format!("entrée {i} illisible : {e}")))?;
        let name = e.name().to_string();
        let (prefix, is_client) = if let Some(r) = layout.client.as_deref().and_then(|c| name.strip_prefix(c)) {
            (r, true)
        } else if let Some(r) = name.strip_prefix(layout.common.as_str()) {
            (r, false)
        } else {
            continue;
        };
        if prefix.is_empty() {
            continue; // dossier racine
        }
        if let Some(mode) = e.unix_mode() {
            if mode & 0o170000 == 0o120000 {
                return Err(invalid(format!("lien symbolique refusé dans l'archive : {name:?}")));
            }
        }
        let is_dir = name.ends_with('/');
        let rel = prefix.trim_end_matches('/');
        if is_dir {
            if !rel.is_empty() {
                validate_rel(rel)?;
            }
            continue;
        }
        validate_rel(rel)?;
        let size = e.size();
        let comp = e.compressed_size();
        if size >= 1024 * 1024 && comp > 0 && size / comp > opts.max_ratio {
            return Err(invalid(format!("ratio de compression suspect (zip bomb) : {name:?}")));
        }
        total = total.saturating_add(size);
        if total > opts.max_total_bytes {
            return Err(invalid("taille décompressée totale trop grande (zip bomb)"));
        }
        let entry = OverrideEntry { index: i, rel: rel.to_string() };
        if is_client {
            client.push(entry);
        } else {
            common.push(entry);
        }
    }
    common.extend(client);
    Ok(common)
}

fn extract_overrides(
    archive: &mut ZipArchive<File>,
    plan: &[OverrideEntry],
    game_dir: &Path,
    opts: &ImportOptions,
    cancel: &AtomicBool,
) -> Result<()> {
    let mut budget = opts.max_total_bytes;
    for item in plan {
        if cancel.load(Ordering::SeqCst) {
            return Err(Error::Cancelled);
        }
        let entry = archive.by_index(item.index).map_err(|e| invalid(format!("{} : {e}", item.rel)))?;
        let declared = entry.size();
        let mut dest = game_dir.to_path_buf();
        for seg in item.rel.split('/') {
            dest.push(seg);
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Un fichier déjà installé peut être un hardlink vers le magasin : ne jamais écrire à travers.
        if std::fs::symlink_metadata(&dest).is_ok() {
            std::fs::remove_file(&dest)?;
        }
        let mut out = File::create(&dest)?;
        let limit = declared.min(budget);
        let mut reader = entry.take(limit + 1);
        let written = std::io::copy(&mut reader, &mut out)?;
        out.flush()?;
        if written > declared || written > budget {
            drop(out);
            let _ = std::fs::remove_file(&dest);
            return Err(invalid(format!("{} : taille réelle supérieure à celle déclarée (zip bomb)", item.rel)));
        }
        budget -= written;
    }
    Ok(())
}

// ---------- Import ----------

fn sanitize_name(name: &str) -> String {
    let cleaned: String = name.chars().map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '-' } else { c }).collect();
    let cleaned: String = cleaned.trim().chars().take(64).collect();
    let cleaned = cleaned.trim().trim_end_matches('.').trim().to_string();
    if cleaned.is_empty() {
        "Modpack".to_string()
    } else {
        cleaned
    }
}

fn with_file_ctx(path: &str, e: Error) -> Error {
    match e {
        Error::Integrity { expected, actual } => Error::Integrity { expected: format!("{path} : {expected}"), actual },
        Error::Network(m) => Error::Network(format!("{path} : {m}")),
        Error::Invalid(m) => Error::Invalid(format!("{path} : {m}")),
        Error::NotFound(m) => Error::NotFound(format!("{path} : {m}")),
        Error::Io(e) => Error::Other(format!("{path} : {e}")),
        Error::Other(m) => Error::Other(format!("{path} : {m}")),
        other => other,
    }
}

async fn fetch_file(file: &MrpackFile, store_root: &Path, index: &StoreIndex, dl: &DlOptions) -> Result<StoredObject> {
    if file.unverified {
        let Some(url) = file.downloads.first() else { return Err(with_file_ctx(&file.path, Error::Network("aucune URL".into()))) };
        return fetch_into_store_unverified(url, MAX_FILE_BYTES, store_root, index, dl).await.map_err(|e| with_file_ctx(&file.path, e));
    }
    let filename = file.path.rsplit('/').next().unwrap_or(&file.path).to_string();
    let mut last = Error::Network("aucune URL".into());
    for url in &file.downloads {
        let vf = VersionFile {
            url: url.clone(),
            filename: filename.clone(),
            primary: true,
            size: file.file_size,
            hashes: FileHashes { sha1: file.sha1.clone(), sha512: file.sha512.clone() },
        };
        match fetch_into_store(&vf, store_root, index, dl).await {
            Ok(o) => return Ok(o),
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(e) => last = e,
        }
    }
    Err(with_file_ctx(&file.path, last))
}

/// Importe un `.mrpack` : crée l'instance, télécharge et installe les fichiers, extrait les overrides.
#[allow(clippy::too_many_arguments)]
pub async fn import(
    zip_path: &Path,
    instances: &InstanceStore,
    store_root: &Path,
    index: &StoreIndex,
    opts: &ImportOptions,
    progress: Option<FilesProgress>,
    cancel: Arc<AtomicBool>,
) -> Result<Instance> {
    let mut archive = open_zip(zip_path)?;
    let bytes = read_index_json(&mut archive)?;
    let pack = parse_index_bytes(&bytes)?;
    for f in &pack.files {
        for u in &f.downloads {
            check_url(u, opts)?;
        }
    }
    import_pack(pack, archive, &Layout::mrpack(), instances, store_root, index, opts, progress, cancel).await.map(|(i, _)| i)
}

/// Crée l'instance décrite par `pack`, installe ses fichiers puis extrait les overrides de `layout`.
#[allow(clippy::too_many_arguments)]
async fn import_pack(
    pack: MrpackIndex,
    mut archive: ZipArchive<File>,
    layout: &Layout,
    instances: &InstanceStore,
    store_root: &Path,
    index: &StoreIndex,
    opts: &ImportOptions,
    progress: Option<FilesProgress>,
    cancel: Arc<AtomicBool>,
) -> Result<(Instance, Vec<String>)> {
    let plan = plan_overrides(&mut archive, opts, layout)?;
    if cancel.load(Ordering::SeqCst) {
        return Err(Error::Cancelled);
    }

    let name = sanitize_name(&pack.name);
    let instance = match instances.create(&name, &pack.minecraft, pack.loader.clone(), opts.ram_mb) {
        Err(Error::Invalid(_)) => instances.create("Modpack", &pack.minecraft, pack.loader.clone(), opts.ram_mb)?,
        other => other?,
    };
    let mut installed: Vec<String> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    let res = run_import(&instance, &pack, &mut archive, &plan, instances, store_root, index, opts, &progress, &cancel, &mut installed, &mut missing).await;
    match res {
        Ok(()) => instances.get(&instance.id).map(|i| (i, missing)),
        Err(e) => {
            for sha in &installed {
                let _ = index.release_ref(sha);
            }
            let _ = instances.delete(&instance.id);
            Err(e)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_import(
    instance: &Instance,
    pack: &MrpackIndex,
    archive: &mut ZipArchive<File>,
    plan: &[OverrideEntry],
    instances: &InstanceStore,
    store_root: &Path,
    index: &StoreIndex,
    opts: &ImportOptions,
    progress: &Option<FilesProgress>,
    cancel: &Arc<AtomicBool>,
    installed: &mut Vec<String>,
    missing: &mut Vec<String>,
) -> Result<()> {
    let game_dir = instances.game_dir(&instance.id)?;
    let dl = DlOptions { cancel: cancel.clone(), progress: None, ..opts.dl.clone() };
    // Un fichier sans hash déjà fourni par les overrides au même chemin n'est pas retéléchargé.
    let in_overrides: HashSet<String> = plan.iter().map(|o| o.rel.to_ascii_lowercase()).collect();
    let files: Vec<&MrpackFile> =
        pack.files.iter().filter(|f| !f.client_unsupported() && !(f.tolerant && in_overrides.contains(&f.path.to_ascii_lowercase()))).collect();
    let total = files.len() as u32;
    let mut done = 0u32;
    let mut results = stream::iter(files.iter().copied())
        .map(|f| {
            let dl = &dl;
            async move { (f, fetch_file(f, store_root, index, dl).await) }
        })
        .buffer_unordered(opts.concurrency.max(1));
    while let Some((f, res)) = results.next().await {
        let obj = match res {
            Ok(o) => o,
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(e) if f.tolerant => {
                tracing::warn!("mod CurseForge non récupéré ({}) : {e}", f.path);
                missing.push(f.path.rsplit('/').next().unwrap_or(&f.path).to_string());
                done += 1;
                if let Some(p) = progress {
                    p(done, total);
                }
                continue;
            }
            Err(e) => return Err(e),
        };
        let src = store_root.join(&obj.rel_path);
        link_into(&game_dir, &f.path, &src).map_err(|e| with_file_ctx(&f.path, e))?;
        index.add_ref(&obj.sha256)?;
        installed.push(obj.sha256);
        done += 1;
        if let Some(p) = progress {
            p(done, total);
        }
    }
    drop(results);
    if cancel.load(Ordering::SeqCst) {
        return Err(Error::Cancelled);
    }
    extract_overrides(archive, plan, &game_dir, opts, cancel)
}

// ---------- CurseForge et zip générique ----------

/// Format d'une archive de modpack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackKind {
    Modrinth,
    CurseForge,
    Plain,
}

/// Cible choisie par l'utilisateur pour un zip sans métadonnées (dossier d'instance brut).
#[derive(Debug, Clone)]
pub struct PlainSpec {
    pub name: String,
    pub minecraft: String,
    pub loader: LoaderSpec,
}

#[derive(Deserialize)]
struct CfManifest {
    minecraft: CfMinecraft,
    #[serde(default)]
    name: String,
    #[serde(default)]
    files: Vec<CfEntry>,
    #[serde(default)]
    overrides: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfMinecraft {
    version: String,
    #[serde(default)]
    mod_loaders: Vec<CfLoader>,
}

#[derive(Deserialize)]
struct CfLoader {
    id: String,
    #[serde(default)]
    primary: bool,
}

#[derive(Deserialize)]
struct CfEntry {
    #[serde(rename = "projectID")]
    project_id: u64,
    #[serde(rename = "fileID")]
    file_id: u64,
}

fn read_cf_manifest(archive: &mut ZipArchive<File>) -> Option<CfManifest> {
    let entry = archive.by_name("manifest.json").ok()?;
    if entry.size() > MAX_INDEX_BYTES {
        return None;
    }
    let mut buf = Vec::new();
    entry.take(MAX_INDEX_BYTES).read_to_end(&mut buf).ok()?;
    let m: CfManifest = serde_json::from_slice(&buf).ok()?;
    (!m.minecraft.version.trim().is_empty()).then_some(m)
}

/// Détecte le format de l'archive.
pub fn detect(zip_path: &Path) -> Result<PackKind> {
    let mut archive = open_zip(zip_path)?;
    if archive.by_name(INDEX_NAME).is_ok() {
        return Ok(PackKind::Modrinth);
    }
    if read_cf_manifest(&mut archive).is_some() {
        return Ok(PackKind::CurseForge);
    }
    Ok(PackKind::Plain)
}

fn cf_loader(m: &CfMinecraft) -> Result<LoaderSpec> {
    let Some(l) = m.mod_loaders.iter().find(|l| l.primary).or(m.mod_loaders.first()) else {
        return Ok(LoaderSpec { loader: Loader::Vanilla, version: String::new() });
    };
    let (name, version) = l.id.split_once('-').ok_or_else(|| invalid(format!("loader CurseForge invalide : {:?}", l.id)))?;
    let loader = match name.to_ascii_lowercase().as_str() {
        "forge" => Loader::Forge,
        "neoforge" => Loader::NeoForge,
        "fabric" => Loader::Fabric,
        "quilt" => Loader::Quilt,
        other => return Err(invalid(format!("loader CurseForge non supporté : {other}"))),
    };
    if version.is_empty() {
        return Err(invalid("version du loader vide"));
    }
    Ok(LoaderSpec { loader, version: version.to_string() })
}

/// Résultat d'un import CurseForge : l'instance et les mods qu'on n'a pas pu récupérer.
#[derive(Debug, Clone)]
pub struct CfImportResult {
    pub instance: Instance,
    /// Descriptions lisibles des mods manquants (nom de fichier, ou `projet X / fichier Y`).
    pub missing: Vec<String>,
}

/// Importe un zip CurseForge SANS clé API. Les `.jar` déjà dans les overrides ne sont pas
/// retéléchargés ; les autres sont résolus via l'endpoint web (redirection vers le CDN) puis
/// téléchargés sans hash de référence. Un mod introuvable n'échoue pas l'import : il est listé
/// dans `missing`.
#[allow(clippy::too_many_arguments)]
pub async fn import_curseforge(
    zip_path: &Path,
    cf: &CfResolver,
    instances: &InstanceStore,
    store_root: &Path,
    index: &StoreIndex,
    opts: &ImportOptions,
    progress: Option<FilesProgress>,
    cancel: Arc<AtomicBool>,
) -> Result<CfImportResult> {
    let mut archive = open_zip(zip_path)?;
    let m = read_cf_manifest(&mut archive).ok_or_else(|| invalid("manifest.json CurseForge invalide"))?;
    let loader = cf_loader(&m.minecraft)?;
    let overrides = if m.overrides.is_empty() { "overrides".to_string() } else { m.overrides.clone() };
    validate_rel(&overrides)?;

    let mut entries: Vec<(u64, u64)> = Vec::new();
    let mut seen_ids = HashSet::new();
    for f in &m.files {
        if seen_ids.insert((f.project_id, f.file_id)) {
            entries.push((f.project_id, f.file_id));
        }
    }

    let mut opts = opts.clone();
    opts.allowed_hosts.extend(CDN_HOSTS.iter().map(|h| h.to_string()));
    // Beaucoup de petits fichiers : la latence domine, on parallélise davantage (résolution et téléchargement).
    opts.concurrency = opts.concurrency.max(16);
    let mut missing: Vec<String> = Vec::new();
    let mut files = Vec::with_capacity(entries.len());
    let mut seen_paths = HashSet::new();
    // Avec une clé de développeur : une seule requête résout tous les mods (URL + SHA-1). Sinon, ou pour
    // les mods sans URL de distribution, on retombe sur la résolution par redirection (sans clé).
    let batch = if cf.has_api_key() {
        let ids: Vec<u64> = entries.iter().map(|(_, f)| *f).collect();
        match cf.resolve_batch(&ids).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("résolution groupée CurseForge indisponible, repli sans clé : {e}");
                Default::default()
            }
        }
    } else {
        Default::default()
    };
    let batch_ref = &batch;
    let opts_ref = &opts;
    let cancel_ref = &cancel;
    let mut resolved = stream::iter(entries.iter().copied())
        .map(|(p, f)| async move {
            if cancel_ref.load(Ordering::SeqCst) {
                return ((p, f), Err(Error::Cancelled));
            }
            let r = async {
                let (d, sha1, size) = match batch_ref.get(&f) {
                    Some(b) => (CfDownload { url: b.url.clone(), file_name: b.file_name.clone() }, b.sha1.clone(), b.size),
                    None => (cf.resolve(p, f).await?, String::new(), 0),
                };
                check_url(&d.url, opts_ref)?;
                let dir = if d.file_name.to_ascii_lowercase().ends_with(".zip") { "resourcepacks" } else { "mods" };
                let path = format!("{dir}/{}", d.file_name);
                validate_file_path(&path)?;
                Ok::<_, Error>((path, d.url, sha1, size))
            }
            .await;
            ((p, f), r)
        })
        .buffer_unordered(opts.concurrency.max(1));
    while let Some(((p, f), r)) = resolved.next().await {
        match r {
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(e) => {
                tracing::warn!("mod CurseForge {p}/{f} non résolu : {e}");
                missing.push(format!("projet {p} / fichier {f}"));
            }
            Ok((path, url, sha1, size)) => {
                if seen_paths.insert(path.to_ascii_lowercase()) {
                    let unverified = sha1.is_empty();
                    files.push(MrpackFile { path, sha1, sha512: String::new(), downloads: vec![url], file_size: size, env: None, unverified, tolerant: true });
                }
            }
        }
    }
    drop(resolved);
    if cancel.load(Ordering::SeqCst) {
        return Err(Error::Cancelled);
    }
    let pack = MrpackIndex { name: m.name, version_id: String::new(), minecraft: m.minecraft.version.trim().to_string(), loader, files };
    let layout = Layout { common: format!("{overrides}/"), client: None };
    let (instance, dl_missing) = import_pack(pack, archive, &layout, instances, store_root, index, &opts, progress, cancel).await?;
    missing.extend(dl_missing);
    Ok(CfImportResult { instance, missing })
}

/// Dossier racine du contenu d'un zip générique, avec `/` final (vide si le contenu est à la racine).
/// Retire un unique dossier englobant (sauf `mods/`, `config/`...) puis `.minecraft/` ou `minecraft/`.
fn plain_root(archive: &mut ZipArchive<File>) -> String {
    let mut names = Vec::new();
    for i in 0..archive.len() {
        if let Ok(e) = archive.by_index_raw(i) {
            let n = e.name().to_string();
            if !n.is_empty() && !n.starts_with("__MACOSX/") {
                names.push(n);
            }
        }
    }
    let mut first: Option<&str> = None;
    let mut single = true;
    for n in &names {
        match n.split_once('/') {
            None => {
                single = false;
                break;
            }
            Some((seg, _)) => match first {
                None => first = Some(seg),
                Some(f) if f == seg => {}
                Some(_) => {
                    single = false;
                    break;
                }
            },
        }
    }
    let mut root = String::new();
    if let (true, Some(f)) = (single, first) {
        if !FILE_DIRS.contains(&f) {
            root = format!("{f}/");
        }
    }
    for sub in [".minecraft/", "minecraft/"] {
        let candidate = format!("{root}{sub}");
        if names.iter().any(|n| n.starts_with(&candidate)) {
            root = candidate;
            break;
        }
    }
    root
}

/// Importe un zip sans métadonnées : son contenu est extrait tel quel dans l'instance.
pub async fn import_plain(
    zip_path: &Path,
    spec: &PlainSpec,
    instances: &InstanceStore,
    store_root: &Path,
    index: &StoreIndex,
    opts: &ImportOptions,
    cancel: Arc<AtomicBool>,
) -> Result<Instance> {
    let mut archive = open_zip(zip_path)?;
    let root = plain_root(&mut archive);
    let pack = MrpackIndex {
        name: spec.name.clone(),
        version_id: String::new(),
        minecraft: spec.minecraft.clone(),
        loader: spec.loader.clone(),
        files: Vec::new(),
    };
    let layout = Layout { common: root, client: None };
    import_pack(pack, archive, &layout, instances, store_root, index, opts, None, cancel).await.map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha512};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    use zip::write::SimpleFileOptions;

    const A: &[u8] = b"contenu du mod A";
    const B: &[u8] = b"contenu du mod B";

    fn sha512(b: &[u8]) -> String {
        hex::encode(Sha512::digest(b))
    }

    fn build_zip(dir: &Path, entries: &[(&str, &[u8])]) -> std::path::PathBuf {
        let p = dir.join("pack.mrpack");
        let mut w = zip::ZipWriter::new(File::create(&p).unwrap());
        for (n, c) in entries {
            w.start_file(*n, SimpleFileOptions::default()).unwrap();
            w.write_all(c).unwrap();
        }
        w.finish().unwrap();
        p
    }

    fn file_json(path: &str, url: &str, content: &[u8]) -> serde_json::Value {
        serde_json::json!({
            "path": path, "hashes": {"sha512": sha512(content)}, "downloads": [url], "fileSize": content.len()
        })
    }

    fn index_json(deps: serde_json::Value, files: Vec<serde_json::Value>) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "formatVersion": 1, "game": "minecraft", "versionId": "1.0.0", "name": "Mon Pack", "files": files, "dependencies": deps
        }))
        .unwrap()
    }

    struct Env {
        tmp: tempfile::TempDir,
        instances: InstanceStore,
        store: std::path::PathBuf,
        idx: StoreIndex,
        server: MockServer,
    }

    async fn env() -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().join("store");
        std::fs::create_dir_all(&store).unwrap();
        let idx = StoreIndex::open(store.join("index.db")).unwrap();
        let server = MockServer::start().await;
        for (p, c) in [("/a.jar", A), ("/b.jar", B)] {
            Mock::given(method("GET")).and(path(p)).respond_with(ResponseTemplate::new(200).set_body_bytes(c.to_vec())).mount(&server).await;
        }
        let instances = InstanceStore::new(tmp.path().join("instances"));
        Env { tmp, instances, store, idx, server }
    }

    fn opts() -> ImportOptions {
        ImportOptions {
            allowed_hosts: vec!["127.0.0.1".into()],
            allow_http: true,
            dl: DlOptions { retry_delay: std::time::Duration::from_millis(5), attempts: 1, ..Default::default() },
            ..Default::default()
        }
    }

    async fn run(e: &Env, pack: &Path, o: &ImportOptions) -> Result<Instance> {
        import(pack, &e.instances, &e.store, &e.idx, o, None, Arc::new(AtomicBool::new(false))).await
    }

    fn std_files(e: &Env) -> Vec<serde_json::Value> {
        vec![
            file_json("mods/a.jar", &format!("{}/a.jar", e.server.uri()), A),
            file_json("mods/b.jar", &format!("{}/b.jar", e.server.uri()), B),
        ]
    }

    #[tokio::test]
    async fn imports_fabric_pack_with_override() {
        let e = env().await;
        let idxj = index_json(serde_json::json!({"minecraft": "1.21.1", "fabric-loader": "0.16.5"}), std_files(&e));
        let pack = build_zip(e.tmp.path(), &[(INDEX_NAME, &idxj), ("overrides/config/x.toml", b"a=1"), ("client-overrides/options.txt", b"fov:70")]);
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let s2 = seen.clone();
        let prog: FilesProgress = Arc::new(move |d, t| s2.lock().unwrap().push((d, t)));
        let inst = import(&pack, &e.instances, &e.store, &e.idx, &opts(), Some(prog), Arc::new(AtomicBool::new(false))).await.unwrap();
        assert_eq!(inst.name, "Mon Pack");
        assert_eq!(inst.minecraft, "1.21.1");
        assert_eq!(inst.loader, LoaderSpec { loader: Loader::Fabric, version: "0.16.5".into() });
        assert_eq!(inst.mod_count, 2);
        let g = e.instances.game_dir(&inst.id).unwrap();
        assert_eq!(std::fs::read(g.join("mods/a.jar")).unwrap(), A);
        assert_eq!(std::fs::read(g.join("mods/b.jar")).unwrap(), B);
        assert_eq!(std::fs::read(g.join("config/x.toml")).unwrap(), b"a=1");
        assert_eq!(std::fs::read(g.join("options.txt")).unwrap(), b"fov:70");
        assert_eq!(seen.lock().unwrap().last(), Some(&(2, 2)));
    }

    #[tokio::test]
    async fn neoforge_loader_and_parse_index() {
        let e = env().await;
        let idxj = index_json(serde_json::json!({"minecraft": "1.21.1", "neoforge": "21.1.77"}), std_files(&e));
        let pack = build_zip(e.tmp.path(), &[(INDEX_NAME, &idxj)]);
        let parsed = parse_index(&pack).unwrap();
        assert_eq!(parsed.files.len(), 2);
        assert_eq!(parsed.loader, LoaderSpec { loader: Loader::NeoForge, version: "21.1.77".into() });
        assert_eq!(parsed.version_id, "1.0.0");
        let inst = run(&e, &pack, &opts()).await.unwrap();
        assert_eq!(inst.loader.loader, Loader::NeoForge);
    }

    #[tokio::test]
    async fn bad_hash_gives_integrity_and_rollback() {
        let e = env().await;
        let mut files = std_files(&e);
        files[1] = file_json("mods/b.jar", &format!("{}/b.jar", e.server.uri()), b"autre contenu");
        let idxj = index_json(serde_json::json!({"minecraft": "1.21.1", "fabric-loader": "0.16.5"}), files);
        let pack = build_zip(e.tmp.path(), &[(INDEX_NAME, &idxj)]);
        let err = run(&e, &pack, &opts()).await.unwrap_err();
        match err {
            Error::Integrity { expected, .. } => assert!(expected.contains("mods/b.jar"), "{expected}"),
            other => panic!("{other:?}"),
        }
        assert!(e.instances.list().is_empty());
        assert!(e.instances.list_with_issues().1.is_empty());
    }

    #[tokio::test]
    async fn fallback_url_is_used() {
        let e = env().await;
        let mut f = file_json("mods/a.jar", &format!("{}/missing.jar", e.server.uri()), A);
        f["downloads"].as_array_mut().unwrap().push(serde_json::json!(format!("{}/a.jar", e.server.uri())));
        let idxj = index_json(serde_json::json!({"minecraft": "1.21.1"}), vec![f]);
        let pack = build_zip(e.tmp.path(), &[(INDEX_NAME, &idxj)]);
        let inst = run(&e, &pack, &opts()).await.unwrap();
        assert_eq!(inst.loader.loader, Loader::Vanilla);
        assert_eq!(inst.mod_count, 1);
    }

    #[tokio::test]
    async fn host_not_allowed_rejected() {
        let e = env().await;
        let idxj = index_json(serde_json::json!({"minecraft": "1.21.1"}), std_files(&e));
        let pack = build_zip(e.tmp.path(), &[(INDEX_NAME, &idxj)]);
        let err = run(&e, &pack, &ImportOptions::default()).await.unwrap_err();
        assert!(matches!(err, Error::Invalid(ref m) if m.contains("hôte") || m.contains("schéma")), "{err:?}");
        assert!(e.instances.list().is_empty());
        let f = file_json("mods/a.jar", "https://evil.example.com/a.jar", A);
        let idxj = index_json(serde_json::json!({"minecraft": "1.21.1"}), vec![f]);
        let pack = build_zip(e.tmp.path(), &[(INDEX_NAME, &idxj)]);
        assert!(matches!(run(&e, &pack, &opts()).await, Err(Error::Invalid(_))));
    }

    #[tokio::test]
    async fn bad_pack_paths_rejected() {
        let e = env().await;
        let url = format!("{}/a.jar", e.server.uri());
        for bad in ["../evil.jar", "mods/../../evil.jar", "/etc/x.jar", "mods\\a.jar", "C:/x.jar", "mods/x.exe", "saves/x.dat", "a.jar", "mods//a.jar"] {
            let idxj = index_json(serde_json::json!({"minecraft": "1.21.1"}), vec![file_json(bad, &url, A)]);
            let pack = build_zip(e.tmp.path(), &[(INDEX_NAME, &idxj)]);
            assert!(matches!(run(&e, &pack, &opts()).await, Err(Error::Invalid(_))), "{bad}");
            assert!(parse_index(&pack).is_err(), "{bad}");
        }
        assert!(e.instances.list().is_empty());
    }

    #[tokio::test]
    async fn dangerous_and_slip_overrides_rejected() {
        let e = env().await;
        let idxj = index_json(serde_json::json!({"minecraft": "1.21.1"}), vec![]);
        for bad in ["overrides/run.exe", "overrides/scripts/x.BAT", "overrides/a.dll", "overrides/p.ps1", "overrides/c.cmd", "overrides/../evil.txt", "overrides/a/../../evil.txt", "overrides/a\\..\\evil.txt", "overrides//abs.txt", "overrides/C:evil.txt"] {
            let pack = build_zip(e.tmp.path(), &[(INDEX_NAME, &idxj), (bad, b"x")]);
            assert!(matches!(run(&e, &pack, &opts()).await, Err(Error::Invalid(_))), "{bad}");
        }
        assert!(e.instances.list().is_empty());
        assert!(!e.tmp.path().join("evil.txt").exists());
    }

    #[tokio::test]
    async fn client_unsupported_is_skipped() {
        let e = env().await;
        let mut files = std_files(&e);
        files[1]["env"] = serde_json::json!({"client": "unsupported", "server": "required"});
        let idxj = index_json(serde_json::json!({"minecraft": "1.21.1"}), files);
        let pack = build_zip(e.tmp.path(), &[(INDEX_NAME, &idxj)]);
        let inst = run(&e, &pack, &opts()).await.unwrap();
        let g = e.instances.game_dir(&inst.id).unwrap();
        assert!(g.join("mods/a.jar").exists());
        assert!(!g.join("mods/b.jar").exists());
    }

    #[tokio::test]
    async fn zip_bomb_rejected() {
        let e = env().await;
        let idxj = index_json(serde_json::json!({"minecraft": "1.21.1"}), vec![]);
        let zeros = vec![0u8; 4 * 1024 * 1024];
        let pack = build_zip(e.tmp.path(), &[(INDEX_NAME, &idxj), ("overrides/big.bin", &zeros)]);
        assert!(matches!(run(&e, &pack, &opts()).await, Err(Error::Invalid(ref m)) if m.contains("zip bomb")));
        let small = ImportOptions { max_total_bytes: 10, max_ratio: u64::MAX, ..opts() };
        assert!(matches!(run(&e, &pack, &small).await, Err(Error::Invalid(ref m)) if m.contains("zip bomb")));
        assert!(e.instances.list().is_empty());
    }

    #[tokio::test]
    async fn cancellation_rolls_back() {
        let e = env().await;
        let idxj = index_json(serde_json::json!({"minecraft": "1.21.1"}), std_files(&e));
        let pack = build_zip(e.tmp.path(), &[(INDEX_NAME, &idxj)]);
        let c = Arc::new(AtomicBool::new(true));
        assert!(matches!(import(&pack, &e.instances, &e.store, &e.idx, &opts(), None, c).await, Err(Error::Cancelled)));
        assert!(e.instances.list().is_empty());
        // Annulation en cours d'import (après le premier fichier, concurrence 1).
        let c = Arc::new(AtomicBool::new(false));
        let c2 = c.clone();
        let prog: FilesProgress = Arc::new(move |_, _| c2.store(true, Ordering::SeqCst));
        let o = ImportOptions { concurrency: 1, ..opts() };
        let r = import(&pack, &e.instances, &e.store, &e.idx, &o, Some(prog), c).await;
        assert!(matches!(r, Err(Error::Cancelled)), "{r:?}");
        assert!(e.instances.list().is_empty());
        assert!(e.instances.list_with_issues().1.is_empty());
    }

    #[tokio::test]
    async fn missing_or_invalid_index() {
        let e = env().await;
        let pack = build_zip(e.tmp.path(), &[("overrides/a.txt", b"x")]);
        assert!(matches!(parse_index(&pack), Err(Error::Invalid(ref m)) if m.contains("absent")));
        assert!(matches!(run(&e, &pack, &opts()).await, Err(Error::Invalid(_))));
        let pack = build_zip(e.tmp.path(), &[(INDEX_NAME, b"{ pas du json")]);
        assert!(matches!(parse_index(&pack), Err(Error::Invalid(_))));
        let v2 = br#"{"formatVersion":2,"name":"x","files":[],"dependencies":{"minecraft":"1.21.1"}}"#;
        let pack = build_zip(e.tmp.path(), &[(INDEX_NAME, v2)]);
        assert!(matches!(parse_index(&pack), Err(Error::Invalid(_))));
        let nomc = br#"{"formatVersion":1,"name":"x","files":[],"dependencies":{}}"#;
        let pack = build_zip(e.tmp.path(), &[(INDEX_NAME, nomc)]);
        assert!(matches!(parse_index(&pack), Err(Error::Invalid(_))));
        let notzip = e.tmp.path().join("nz.mrpack");
        std::fs::write(&notzip, b"pas un zip").unwrap();
        assert!(matches!(parse_index(&notzip), Err(Error::Invalid(_))));
        assert!(e.instances.list().is_empty());
    }

    fn plain_spec() -> PlainSpec {
        PlainSpec { name: "Zip Pack".into(), minecraft: "1.20.1".into(), loader: LoaderSpec { loader: Loader::Forge, version: "47.2.0".into() } }
    }

    async fn run_plain(e: &Env, pack: &Path) -> Result<Instance> {
        import_plain(pack, &plain_spec(), &e.instances, &e.store, &e.idx, &opts(), Arc::new(AtomicBool::new(false))).await
    }

    #[tokio::test]
    async fn plain_zip_strips_wrapping_dir_and_dot_minecraft() {
        let e = env().await;
        let pack = build_zip(
            e.tmp.path(),
            &[("MyPack/.minecraft/mods/a.jar", A), ("MyPack/.minecraft/config/x.toml", b"a=1"), ("MyPack/readme.txt", b"x"), ("__MACOSX/junk", b"j")],
        );
        assert_eq!(detect(&pack).unwrap(), PackKind::Plain);
        let inst = run_plain(&e, &pack).await.unwrap();
        assert_eq!(inst.minecraft, "1.20.1");
        assert_eq!(inst.loader.loader, Loader::Forge);
        let g = e.instances.game_dir(&inst.id).unwrap();
        assert_eq!(std::fs::read(g.join("mods/a.jar")).unwrap(), A);
        assert_eq!(std::fs::read(g.join("config/x.toml")).unwrap(), b"a=1");
        assert!(!g.join("readme.txt").exists());
    }

    #[tokio::test]
    async fn plain_zip_at_root_and_dangerous_rejected() {
        let e = env().await;
        let pack = build_zip(e.tmp.path(), &[("mods/a.jar", A), ("options.txt", b"fov:70")]);
        let inst = run_plain(&e, &pack).await.unwrap();
        let g = e.instances.game_dir(&inst.id).unwrap();
        assert_eq!(std::fs::read(g.join("mods/a.jar")).unwrap(), A);
        assert_eq!(std::fs::read(g.join("options.txt")).unwrap(), b"fov:70");
        let bad = build_zip(e.tmp.path(), &[("mods/a.jar", A), ("evil.exe", b"MZ")]);
        assert!(run_plain(&e, &bad).await.is_err());
        assert_eq!(e.instances.list().len(), 1, "pas d'instance en trop après échec");
    }

    fn cf_manifest(files: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "minecraft": {"version": "1.20.1", "modLoaders": [{"id": "forge-47.2.0", "primary": true}]},
            "manifestType": "minecraftModpack", "name": "CF Pack", "overrides": "overrides", "files": files
        }))
        .unwrap()
    }

    async fn cf_redirect(e: &Env, project: u64, file: u64, cdn_path: &str) {
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/mods/{project}/files/{file}/download")))
            .respond_with(ResponseTemplate::new(307).insert_header("Location", format!("{}{cdn_path}", e.server.uri()).as_str()))
            .mount(&e.server)
            .await;
    }

    async fn run_cf(e: &Env, pack: &Path) -> Result<CfImportResult> {
        let cf = CfResolver::with_base_url(e.server.uri());
        import_curseforge(pack, &cf, &e.instances, &e.store, &e.idx, &opts(), None, Arc::new(AtomicBool::new(false))).await
    }

    #[tokio::test]
    async fn curseforge_pack_end_to_end_without_key() {
        let e = env().await;
        Mock::given(method("GET")).and(path("/files/0/77/abc%20d.jar")).respond_with(ResponseTemplate::new(200).set_body_bytes(b"abc".to_vec())).mount(&e.server).await;
        cf_redirect(&e, 1, 77, "/files/0/77/abc%20d.jar?api-key=zz").await;
        let manifest = cf_manifest(serde_json::json!([{"projectID": 1, "fileID": 77, "required": true}]));
        let pack = build_zip(e.tmp.path(), &[("manifest.json", &manifest), ("overrides/config/y.toml", b"b=2")]);
        assert_eq!(detect(&pack).unwrap(), PackKind::CurseForge);
        let r = run_cf(&e, &pack).await.unwrap();
        assert!(r.missing.is_empty(), "{:?}", r.missing);
        assert_eq!(r.instance.name, "CF Pack");
        assert_eq!(r.instance.loader, LoaderSpec { loader: Loader::Forge, version: "47.2.0".into() });
        let g = e.instances.game_dir(&r.instance.id).unwrap();
        assert_eq!(std::fs::read(g.join("mods/abc d.jar")).unwrap(), b"abc");
        assert_eq!(std::fs::read(g.join("config/y.toml")).unwrap(), b"b=2");
    }

    #[tokio::test]
    async fn curseforge_missing_mod_does_not_fail_import() {
        let e = env().await;
        // 1 : OK ; 2 : 404 ; 3 : redirection vers un hôte non autorisé ; 4 : CDN qui répond 404.
        Mock::given(method("GET")).and(path("/files/0/1/ok.jar")).respond_with(ResponseTemplate::new(200).set_body_bytes(b"ok".to_vec())).mount(&e.server).await;
        Mock::given(method("GET")).and(path("/files/0/4/gone.jar")).respond_with(ResponseTemplate::new(404)).mount(&e.server).await;
        cf_redirect(&e, 1, 1, "/files/0/1/ok.jar").await;
        Mock::given(method("GET")).and(path("/api/v1/mods/2/files/2/download")).respond_with(ResponseTemplate::new(404)).mount(&e.server).await;
        Mock::given(method("GET"))
            .and(path("/api/v1/mods/3/files/3/download"))
            .respond_with(ResponseTemplate::new(307).insert_header("Location", "https://evil.example.com/files/0/3/x.jar"))
            .mount(&e.server)
            .await;
        cf_redirect(&e, 4, 4, "/files/0/4/gone.jar").await;
        let manifest = cf_manifest(serde_json::json!([
            {"projectID": 1, "fileID": 1}, {"projectID": 2, "fileID": 2}, {"projectID": 3, "fileID": 3}, {"projectID": 4, "fileID": 4}
        ]));
        let pack = build_zip(e.tmp.path(), &[("manifest.json", &manifest)]);
        let r = run_cf(&e, &pack).await.unwrap();
        let g = e.instances.game_dir(&r.instance.id).unwrap();
        assert_eq!(std::fs::read(g.join("mods/ok.jar")).unwrap(), b"ok");
        let mut miss = r.missing.clone();
        miss.sort();
        assert_eq!(miss, vec!["gone.jar", "projet 2 / fichier 2", "projet 3 / fichier 3"]);
        assert_eq!(e.instances.list().len(), 1);
    }

    #[tokio::test]
    async fn curseforge_skips_mod_already_in_overrides() {
        let e = env().await;
        // Le CDN ne doit jamais être appelé : le jar est dans les overrides.
        Mock::given(method("GET")).and(path("/files/0/9/in.jar")).respond_with(ResponseTemplate::new(200)).expect(0).mount(&e.server).await;
        cf_redirect(&e, 9, 9, "/files/0/9/in.jar").await;
        let manifest = cf_manifest(serde_json::json!([{"projectID": 9, "fileID": 9}]));
        let pack = build_zip(e.tmp.path(), &[("manifest.json", &manifest), ("overrides/mods/in.jar", b"embarque")]);
        let r = run_cf(&e, &pack).await.unwrap();
        assert!(r.missing.is_empty());
        let g = e.instances.game_dir(&r.instance.id).unwrap();
        assert_eq!(std::fs::read(g.join("mods/in.jar")).unwrap(), b"embarque");
    }

    #[tokio::test]
    async fn curseforge_cancel_rolls_back() {
        let e = env().await;
        Mock::given(method("GET")).and(path("/files/0/1/ok.jar")).respond_with(ResponseTemplate::new(200).set_body_bytes(b"ok".to_vec())).mount(&e.server).await;
        cf_redirect(&e, 1, 1, "/files/0/1/ok.jar").await;
        let manifest = cf_manifest(serde_json::json!([{"projectID": 1, "fileID": 1}]));
        let pack = build_zip(e.tmp.path(), &[("manifest.json", &manifest)]);
        let cf = CfResolver::with_base_url(e.server.uri());
        let r = import_curseforge(&pack, &cf, &e.instances, &e.store, &e.idx, &opts(), None, Arc::new(AtomicBool::new(true))).await;
        assert!(matches!(r, Err(Error::Cancelled)));
        assert!(e.instances.list().is_empty());
    }

    #[test]
    fn detects_modrinth() {
        let tmp = tempfile::tempdir().unwrap();
        let idxj = index_json(serde_json::json!({"minecraft": "1.21.1"}), vec![]);
        let pack = build_zip(tmp.path(), &[(INDEX_NAME, &idxj)]);
        assert_eq!(detect(&pack).unwrap(), PackKind::Modrinth);
    }
}
