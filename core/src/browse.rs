//! « Parcourir » : recherche et installation depuis CurseForge et Modrinth.
//!
//! - `search` / `versions` interrogent l'API de la source choisie et renvoient des types communs.
//! - `install_content` installe un mod / resourcepack / shader dans une instance (dépendances obligatoires
//!   comprises) via le magasin adressé par contenu ; tout est téléchargé et vérifié AVANT de toucher à l'instance.
//! - `fetch_modpack` télécharge le fichier d'un modpack (le flux d'import existant crée ensuite l'instance).
//!
//! Sécurité : URLs `https` vers des hôtes autorisés (CDN Modrinth / CurseForge, GitHub), noms de fichiers
//! stricts, extension imposée par le type, hashes vérifiés quand la source en fournit.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::mods::{self, Provenance};
use crate::providers::curseforge::MAX_FILE_BYTES;
use crate::providers::curseforge_api::{self as cfapi, CfApi, CfFile};
use crate::providers::modrinth_api::{FileHashes, ModrinthClient, SearchFilters, Version, VersionFile};
use crate::providers::modrinth_dl::{fetch_into_store, fetch_into_store_unverified, DlOptions};
use crate::store::index::StoreIndex;
use crate::store::link::link_into;
use crate::types::{Error, Instance, Loader, Result};

pub const MODRINTH_HOSTS: &[&str] = &["cdn.modrinth.com", "github.com", "raw.githubusercontent.com", "objects.githubusercontent.com"];
pub const CURSEFORGE_HOSTS: &[&str] = &["edge.forgecdn.net", "mediafilez.forgecdn.net"];
/// Nombre maximal de fichiers (dépendances comprises) installés en une fois.
const MAX_FILES: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BrowseSource {
    Curseforge,
    Modrinth,
}

impl BrowseSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Curseforge => "curseforge",
            Self::Modrinth => "modrinth",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "curseforge" => Ok(Self::Curseforge),
            "modrinth" => Ok(Self::Modrinth),
            _ => Err(Error::Invalid(format!("source inconnue : {s:?}"))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BrowseKind {
    Modpack,
    Mod,
    Resourcepack,
    Shader,
}

impl BrowseKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Modpack => "modpack",
            Self::Mod => "mod",
            Self::Resourcepack => "resourcepack",
            Self::Shader => "shader",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "modpack" => Ok(Self::Modpack),
            "mod" => Ok(Self::Mod),
            "resourcepack" => Ok(Self::Resourcepack),
            "shader" => Ok(Self::Shader),
            _ => Err(Error::Invalid(format!("type de contenu inconnu : {s:?}"))),
        }
    }

    /// Dossier de jeu (hors modpack) et extension imposée.
    fn dir_and_ext(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::Mod => Some(("mods", ".jar")),
            Self::Resourcepack => Some(("resourcepacks", ".zip")),
            Self::Shader => Some(("shaderpacks", ".zip")),
            Self::Modpack => None,
        }
    }

    /// Le loader n'a de sens que pour les mods et les modpacks.
    fn uses_loader(self) -> bool {
        matches!(self, Self::Mod | Self::Modpack)
    }

    fn cf_class(self) -> u32 {
        match self {
            Self::Modpack => cfapi::CLASS_MODPACK,
            Self::Mod => cfapi::CLASS_MOD,
            Self::Resourcepack => cfapi::CLASS_RESOURCEPACK,
            Self::Shader => cfapi::CLASS_SHADER,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowseQuery {
    pub source: BrowseSource,
    pub kind: BrowseKind,
    pub query: String,
    #[serde(default)]
    pub minecraft: Option<String>,
    #[serde(default)]
    pub loader: Option<String>,
    #[serde(default)]
    pub sort: Option<String>,
    pub offset: u32,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowseHit {
    pub source: BrowseSource,
    pub id: String,
    pub kind: BrowseKind,
    pub name: String,
    pub summary: String,
    pub author: String,
    pub icon_url: Option<String>,
    pub downloads: u64,
    pub page_url: String,
    pub downloadable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowsePage {
    pub hits: Vec<BrowseHit>,
    pub total: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowseVersion {
    pub id: String,
    pub name: String,
    pub minecraft: Vec<String>,
    pub loaders: Vec<String>,
    pub published: String,
    pub downloadable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowseInstallInput {
    pub source: BrowseSource,
    pub id: String,
    pub kind: BrowseKind,
    #[serde(default)]
    pub version_id: Option<String>,
    #[serde(default)]
    pub instance_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InstallReport {
    pub installed: Vec<String>,
    pub missing: Vec<String>,
}

/// Clients des deux sources. `allow_loopback` (tests) autorise `http(s)://127.0.0.1|localhost` pour les téléchargements.
pub struct Providers {
    pub modrinth: ModrinthClient,
    pub cf: CfApi,
    pub allow_loopback: bool,
}

/// Progression : (fichiers terminés, fichiers à installer, libellé).
pub type ProgressCb<'a> = &'a (dyn Fn(u32, u32, &str) + Send + Sync);

fn invalid(m: impl Into<String>) -> Error {
    Error::Invalid(m.into())
}

// ---------- Validation ----------

fn parse_loader(s: Option<&str>) -> Result<Option<Loader>> {
    match s.map(str::trim) {
        None | Some("") | Some("vanilla") => Ok(None),
        Some("fabric") => Ok(Some(Loader::Fabric)),
        Some("quilt") => Ok(Some(Loader::Quilt)),
        Some("forge") => Ok(Some(Loader::Forge)),
        Some("neoforge") => Ok(Some(Loader::NeoForge)),
        Some(o) => Err(invalid(format!("loader inconnu : {o:?}"))),
    }
}

pub(crate) fn safe_token(s: &str, what: &str) -> Result<()> {
    if s.is_empty() || s.len() > 64 || !s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+')) {
        return Err(invalid(format!("{what} invalide")));
    }
    Ok(())
}

fn clean_minecraft(m: Option<&str>) -> Result<Option<String>> {
    match m.map(str::trim).filter(|m| !m.is_empty()) {
        Some(v) => safe_token(v, "version de Minecraft").map(|_| Some(v.to_string())),
        None => Ok(None),
    }
}

fn clean_sort(s: Option<&str>) -> Result<Option<&'static str>> {
    match s {
        None | Some("") => Ok(None),
        Some("relevance") => Ok(Some("relevance")),
        Some("downloads") => Ok(Some("downloads")),
        Some("updated") => Ok(Some("updated")),
        Some(o) => Err(invalid(format!("tri inconnu : {o:?}"))),
    }
}

/// Loaders acceptés pour un loader d'instance (Quilt exécute aussi les mods Fabric). Vide : tout.
pub(crate) fn accepted(l: Option<Loader>) -> Vec<&'static str> {
    match l {
        None | Some(Loader::Vanilla) => vec![],
        Some(Loader::Quilt) => vec!["quilt", "fabric"],
        Some(Loader::Fabric) => vec!["fabric"],
        Some(Loader::Forge) => vec!["forge"],
        Some(Loader::NeoForge) => vec!["neoforge"],
    }
}

fn loader_ok(loaders: &[String], acc: &[&str]) -> bool {
    acc.is_empty() || loaders.is_empty() || loaders.iter().any(|l| acc.contains(&l.to_ascii_lowercase().as_str()))
}

fn mc_ok(versions: &[String], mc: Option<&str>) -> bool {
    mc.is_none_or(|m| versions.is_empty() || versions.iter().any(|v| v == m))
}

/// Filtre loader pour les requêtes serveur : seulement s'il n'y a qu'un loader accepté.
fn server_loader(acc: &[&str], l: Option<Loader>) -> Option<Loader> {
    if acc.len() == 1 {
        l
    } else {
        None
    }
}

pub(crate) fn check_url(url: &str, source: BrowseSource, allow_loopback: bool) -> Result<()> {
    let u = url::Url::parse(url).map_err(|_| invalid("URL de téléchargement invalide"))?;
    let host = u.host_str().unwrap_or("").to_ascii_lowercase();
    if u.username() != "" || u.password().is_some() {
        return Err(invalid("URL de téléchargement refusée"));
    }
    let loop_ok = allow_loopback && matches!(u.scheme(), "http" | "https") && matches!(host.as_str(), "127.0.0.1" | "localhost" | "[::1]");
    if loop_ok {
        return Ok(());
    }
    let hosts = match source {
        BrowseSource::Modrinth => MODRINTH_HOSTS,
        BrowseSource::Curseforge => CURSEFORGE_HOSTS,
    };
    if u.scheme() != "https" || !hosts.contains(&host.as_str()) {
        return Err(invalid(format!("hôte de téléchargement non autorisé ({host})")));
    }
    Ok(())
}

// ---------- Recherche ----------

pub async fn search(p: &Providers, q: &BrowseQuery) -> Result<BrowsePage> {
    let limit = q.limit.clamp(1, 50);
    let loader = if q.kind.uses_loader() { parse_loader(q.loader.as_deref())? } else { None };
    let mc = clean_minecraft(q.minecraft.as_deref())?;
    let sort = clean_sort(q.sort.as_deref())?;
    let query = q.query.trim();
    if query.chars().count() > 200 {
        return Err(invalid("recherche trop longue"));
    }
    match q.source {
        BrowseSource::Modrinth => {
            let f = SearchFilters {
                minecraft_version: mc,
                loader,
                project_type: Some(q.kind.as_str().to_string()),
                limit: Some(limit),
                offset: Some(q.offset),
                sort: sort.map(str::to_string),
            };
            let r = p.modrinth.search(query, &f).await?;
            let hits = r
                .hits
                .into_iter()
                .map(|h| BrowseHit {
                    source: BrowseSource::Modrinth,
                    page_url: format!("https://modrinth.com/{}/{}", q.kind.as_str(), if h.slug.is_empty() { &h.project_id } else { &h.slug }),
                    id: h.project_id,
                    kind: q.kind,
                    name: h.title,
                    summary: h.description,
                    author: h.author,
                    icon_url: h.icon_url.filter(|u| !u.is_empty()),
                    downloads: h.downloads,
                    downloadable: true,
                })
                .collect();
            Ok(BrowsePage { hits, total: r.total_hits.min(u32::MAX as u64) as u32 })
        }
        BrowseSource::Curseforge => {
            let r = p
                .cf
                .search(q.kind.cf_class(), query, mc.as_deref(), loader.and_then(cfapi::loader_type), cfapi::sort_field(sort), q.offset, limit)
                .await?;
            let total = r.pagination.map_or(r.data.len() as u64, |p| p.total_count).min(u32::MAX as u64) as u32;
            let hits = r
                .data
                .into_iter()
                .map(|m| BrowseHit {
                    source: BrowseSource::Curseforge,
                    page_url: m
                        .links
                        .and_then(|l| l.website_url)
                        .filter(|u| u.starts_with("https://"))
                        .unwrap_or_else(|| "https://www.curseforge.com/minecraft".to_string()),
                    id: m.id.to_string(),
                    kind: q.kind,
                    name: m.name,
                    summary: m.summary,
                    author: m.authors.into_iter().next().map(|a| a.name).unwrap_or_default(),
                    icon_url: m.logo.and_then(|l| l.thumbnail_url.or(l.url)).filter(|u| u.starts_with("https://") || u.starts_with("http://")),
                    downloads: m.download_count.max(0.0) as u64,
                    downloadable: m.allow_mod_distribution != Some(false),
                })
                .collect();
            Ok(BrowsePage { hits, total })
        }
    }
}

// ---------- Versions ----------

fn parse_id(id: &str) -> Result<u64> {
    id.trim().parse::<u64>().map_err(|_| invalid("identifiant CurseForge invalide"))
}

pub(crate) async fn modrinth_versions(p: &Providers, id: &str, kind: BrowseKind, mc: Option<&str>, loader: Option<Loader>) -> Result<Vec<Version>> {
    safe_token(id, "identifiant Modrinth")?;
    let loader = if kind.uses_loader() { loader } else { None };
    let acc = accepted(loader);
    let mut v = p.modrinth.project_versions(id, mc, server_loader(&acc, loader)).await?;
    v.retain(|x| mc_ok(&x.game_versions, mc) && loader_ok(&x.loaders, &acc));
    v.sort_by(|a, b| b.date_published.cmp(&a.date_published));
    Ok(v)
}

pub(crate) async fn cf_versions(p: &Providers, id: &str, kind: BrowseKind, mc: Option<&str>, loader: Option<Loader>) -> Result<Vec<CfFile>> {
    let loader = if kind.uses_loader() { loader } else { None };
    let acc = accepted(loader);
    let mut v = p.cf.mod_files(parse_id(id)?, mc, server_loader(&acc, loader).and_then(cfapi::loader_type)).await?;
    v.retain(|x| mc_ok(&x.minecraft_versions(), mc) && loader_ok(&x.loaders(), &acc));
    Ok(v)
}

pub async fn versions(p: &Providers, source: &str, id: &str, kind: &str, minecraft: Option<&str>, loader: Option<&str>) -> Result<Vec<BrowseVersion>> {
    let (source, kind) = (BrowseSource::parse(source)?, BrowseKind::parse(kind)?);
    let loader = parse_loader(loader)?;
    let mc = clean_minecraft(minecraft)?;
    Ok(match source {
        BrowseSource::Modrinth => modrinth_versions(p, id, kind, mc.as_deref(), loader)
            .await?
            .into_iter()
            .map(|v| BrowseVersion {
                downloadable: !v.files.is_empty(),
                name: if v.name.trim().is_empty() { v.version_number.clone() } else { v.name.clone() },
                id: v.id,
                minecraft: v.game_versions,
                loaders: v.loaders,
                published: v.date_published,
            })
            .collect(),
        BrowseSource::Curseforge => cf_versions(p, id, kind, mc.as_deref(), loader)
            .await?
            .into_iter()
            .map(|f| BrowseVersion {
                downloadable: f.url().is_some(),
                name: if f.display_name.trim().is_empty() { f.file_name.clone() } else { f.display_name.clone() },
                minecraft: f.minecraft_versions(),
                loaders: f.loaders(),
                published: f.file_date.clone(),
                id: f.id.to_string(),
            })
            .collect(),
    })
}

// ---------- Plan d'installation ----------

#[derive(Debug, Clone)]
pub(crate) struct Planned {
    pub(crate) source: BrowseSource,
    pub(crate) project_id: String,
    pub(crate) name: String,
    pub(crate) url: String,
    pub(crate) size: u64,
    pub(crate) sha1: String,
    pub(crate) sha512: String,
}

fn check_name(name: &str, ext: &str) -> Result<()> {
    mods::validate_file_name(name)?;
    if !name.to_ascii_lowercase().ends_with(ext) {
        return Err(invalid(format!("type de fichier inattendu ({name}) : {ext} attendu")));
    }
    Ok(())
}

pub(crate) fn plan_modrinth(p: &Providers, v: &Version, ext: &str) -> Result<Planned> {
    let f = v
        .files
        .iter()
        .find(|f| f.primary && f.filename.to_ascii_lowercase().ends_with(ext))
        .or_else(|| v.files.iter().find(|f| f.filename.to_ascii_lowercase().ends_with(ext)))
        .ok_or_else(|| Error::NotFound(format!("aucun fichier {ext} dans cette version")))?;
    check_name(&f.filename, ext)?;
    check_url(&f.url, BrowseSource::Modrinth, p.allow_loopback)?;
    Ok(Planned {
        source: BrowseSource::Modrinth,
        project_id: v.project_id.clone(),
        name: f.filename.clone(),
        url: f.url.clone(),
        size: f.size,
        sha1: f.hashes.sha1.to_ascii_lowercase(),
        sha512: f.hashes.sha512.to_ascii_lowercase(),
    })
}

const NOT_DOWNLOADABLE: &str = "ce fichier ne peut pas être téléchargé par Cubelink : l'auteur a désactivé la distribution par des applications tierces. Ouvre-le sur CurseForge.";

pub(crate) fn plan_cf(p: &Providers, f: &CfFile, mod_id: u64, ext: &str) -> Result<Planned> {
    let url = f.url().ok_or_else(|| invalid(NOT_DOWNLOADABLE))?;
    check_name(&f.file_name, ext)?;
    check_url(url, BrowseSource::Curseforge, p.allow_loopback)?;
    Ok(Planned {
        source: BrowseSource::Curseforge,
        project_id: if f.mod_id != 0 { f.mod_id } else { mod_id }.to_string(),
        name: f.file_name.clone(),
        url: url.to_string(),
        size: f.file_length,
        sha1: f.sha1(),
        sha512: String::new(),
    })
}

/// Version choisie (ou la plus récente compatible) puis fichier à télécharger.
async fn plan_main(p: &Providers, input: &BrowseInstallInput, mc: Option<&str>, loader: Option<Loader>, ext: &str) -> Result<(Planned, Vec<DepRef>)> {
    let (mc, loader) = if input.kind == BrowseKind::Modpack { (None, None) } else { (mc, loader) };
    match input.source {
        BrowseSource::Modrinth => {
            let v = match input.version_id.as_deref() {
                Some(vid) => {
                    safe_token(vid, "identifiant de version")?;
                    p.modrinth.version(vid).await?
                }
                None => modrinth_versions(p, &input.id, input.kind, mc, loader)
                    .await?
                    .into_iter()
                    .next()
                    .ok_or_else(|| no_compatible(mc, loader))?,
            };
            let deps = v
                .dependencies
                .iter()
                .filter(|d| d.dependency_type == "required")
                .map(|d| DepRef::Modrinth { project_id: d.project_id.clone(), version_id: d.version_id.clone() })
                .collect();
            Ok((plan_modrinth(p, &v, ext)?, deps))
        }
        BrowseSource::Curseforge => {
            let mod_id = parse_id(&input.id)?;
            let f = match input.version_id.as_deref() {
                Some(vid) => p.cf.file(mod_id, parse_id(vid)?).await?,
                None => cf_versions(p, &input.id, input.kind, mc, loader).await?.into_iter().next().ok_or_else(|| no_compatible(mc, loader))?,
            };
            let deps = f.dependencies.iter().filter(|d| d.relation_type == cfapi::REL_REQUIRED).map(|d| DepRef::Curseforge(d.mod_id)).collect();
            Ok((plan_cf(p, &f, mod_id, ext)?, deps))
        }
    }
}

pub(crate) fn no_compatible(mc: Option<&str>, loader: Option<Loader>) -> Error {
    let l = accepted(loader).join("/");
    Error::NotFound(format!("aucune version compatible avec Minecraft {} ({})", mc.unwrap_or("?"), if l.is_empty() { "tout loader" } else { &l }))
}

#[derive(Debug, Clone)]
enum DepRef {
    Modrinth { project_id: Option<String>, version_id: Option<String> },
    Curseforge(u64),
}

/// Dépendances obligatoires, de proche en proche (profondeur et nombre bornés). Les échecs vont dans `missing`.
async fn plan_deps(p: &Providers, first: Vec<DepRef>, mc: Option<&str>, loader: Option<Loader>, game_dir: &Path, seen: &mut HashSet<String>, missing: &mut Vec<String>) -> Vec<Planned> {
    let mut out = Vec::new();
    let mut queue: Vec<(DepRef, u32)> = first.into_iter().map(|d| (d, 0)).collect();
    while let Some((dep, depth)) = queue.pop() {
        if out.len() + seen.len() > MAX_FILES * 2 {
            break;
        }
        let (key, label) = match &dep {
            DepRef::Modrinth { project_id: Some(pid), .. } => (format!("modrinth:{pid}"), format!("dépendance Modrinth {pid}")),
            DepRef::Modrinth { version_id: Some(vid), .. } => (format!("modrinth-v:{vid}"), format!("dépendance Modrinth (version {vid})")),
            DepRef::Modrinth { .. } => continue,
            DepRef::Curseforge(id) => (format!("curseforge:{id}"), format!("dépendance CurseForge {id}")),
        };
        if !seen.insert(key) {
            continue;
        }
        let already = match &dep {
            DepRef::Modrinth { project_id: Some(pid), .. } => installed_project(game_dir, "modrinth", pid),
            DepRef::Curseforge(id) => installed_project(game_dir, "curseforge", &id.to_string()),
            _ => false,
        };
        if already {
            continue;
        }
        let r: Result<(Planned, Vec<DepRef>)> = async {
            match &dep {
                DepRef::Modrinth { project_id, version_id } => {
                    let v = match (version_id, project_id) {
                        (Some(vid), _) => p.modrinth.version(vid).await?,
                        (None, Some(pid)) => modrinth_versions(p, pid, BrowseKind::Mod, mc, loader).await?.into_iter().next().ok_or_else(|| no_compatible(mc, loader))?,
                        _ => return Err(invalid("dépendance sans identifiant")),
                    };
                    let next = v
                        .dependencies
                        .iter()
                        .filter(|d| d.dependency_type == "required")
                        .map(|d| DepRef::Modrinth { project_id: d.project_id.clone(), version_id: d.version_id.clone() })
                        .collect();
                    Ok((plan_modrinth(p, &v, ".jar")?, next))
                }
                DepRef::Curseforge(id) => {
                    let f = cf_versions(p, &id.to_string(), BrowseKind::Mod, mc, loader).await?.into_iter().next().ok_or_else(|| no_compatible(mc, loader))?;
                    let next = f.dependencies.iter().filter(|d| d.relation_type == cfapi::REL_REQUIRED).map(|d| DepRef::Curseforge(d.mod_id)).collect();
                    Ok((plan_cf(p, &f, *id, ".jar")?, next))
                }
            }
        }
        .await;
        match r {
            Ok((pl, next)) => {
                if depth < 4 {
                    queue.extend(next.into_iter().map(|d| (d, depth + 1)));
                }
                out.push(pl);
            }
            Err(e) => {
                tracing::warn!("{label} non résolue : {e}");
                missing.push(label);
            }
        }
    }
    out
}

pub(crate) fn installed_project(game_dir: &Path, source: &str, id: &str) -> bool {
    let dir = game_dir.join("mods");
    mods::files_of_project(game_dir, source, id).iter().any(|f| dir.join(f).is_file() || dir.join(format!("{f}.disabled")).is_file())
}

// ---------- Téléchargement ----------

pub(crate) async fn fetch(pl: &Planned, store_root: &Path, index: &StoreIndex, dl: &DlOptions) -> Result<crate::providers::modrinth_dl::StoredObject> {
    if pl.sha1.is_empty() && pl.sha512.is_empty() {
        return fetch_into_store_unverified(&pl.url, MAX_FILE_BYTES, store_root, index, dl).await;
    }
    let vf = VersionFile {
        url: pl.url.clone(),
        filename: pl.name.clone(),
        primary: true,
        size: pl.size,
        hashes: FileHashes { sha1: pl.sha1.clone(), sha512: pl.sha512.clone() },
    };
    fetch_into_store(&vf, store_root, index, dl).await
}

/// Installe un mod / resourcepack / shader (et les dépendances obligatoires des mods) dans l'instance.
#[allow(clippy::too_many_arguments)]
pub async fn install_content(
    p: &Providers,
    input: &BrowseInstallInput,
    inst: &Instance,
    game_dir: &Path,
    store_root: &Path,
    index: &StoreIndex,
    dl: &DlOptions,
    progress: ProgressCb<'_>,
) -> Result<InstallReport> {
    let (dir, ext) = input.kind.dir_and_ext().ok_or_else(|| invalid("un modpack s'installe dans une nouvelle instance"))?;
    let loader = Some(inst.loader.loader).filter(|l| *l != Loader::Vanilla);
    let mc = Some(inst.minecraft.as_str());
    let mut report = InstallReport::default();
    let (main, deps) = plan_main(p, input, mc, loader, ext).await?;
    if input.kind == BrowseKind::Mod {
        if let Some(pr) = mods::read_provenance(game_dir).get(&main.name) {
            if pr.source == "custom" {
                return Err(invalid(format!("un mod ajouté à la main porte déjà le nom {} : renomme-le ou supprime-le d'abord", main.name)));
            }
        }
    }
    let mut files = vec![(main, false)];
    if input.kind == BrowseKind::Mod {
        let mut seen: HashSet<String> = HashSet::new();
        seen.insert(format!("{}:{}", input.source.as_str(), files[0].0.project_id));
        seen.insert(format!("{}:{}", input.source.as_str(), input.id));
        for d in plan_deps(p, deps, mc, loader, game_dir, &mut seen, &mut report.missing).await {
            if !files.iter().any(|(f, _)| f.name.eq_ignore_ascii_case(&d.name)) {
                files.push((d, true));
            }
        }
    }
    files.truncate(MAX_FILES);
    let total = files.len() as u32;
    progress(0, total, &files[0].0.name);
    // 1) tout télécharger (et vérifier) avant de modifier l'instance.
    let mut stored = Vec::with_capacity(files.len());
    for (i, (pl, _)) in files.iter().enumerate() {
        check_url(&pl.url, pl.source, p.allow_loopback)?;
        let o = fetch(pl, store_root, index, dl).await?;
        stored.push(o);
        progress(i as u32 + 1, total, &pl.name);
    }
    // 2) placer.
    let mods_dir = game_dir.join(dir);
    for ((pl, is_dep), obj) in files.iter().zip(&stored) {
        let exists = mods_dir.join(&pl.name).exists() || mods_dir.join(format!("{}.disabled", pl.name)).exists();
        if *is_dep && exists {
            continue; // dépendance déjà présente (ex. issue d'un modpack) : on n'y touche pas
        }
        if input.kind == BrowseKind::Mod {
            mods::remove_old_versions(game_dir, pl.source.as_str(), &pl.project_id, std::slice::from_ref(&pl.name))?;
            let _ = std::fs::remove_file(mods_dir.join(format!("{}.disabled", pl.name)));
        }
        link_into(game_dir, &format!("{dir}/{}", pl.name), &store_root.join(&obj.rel_path))?;
        index.add_ref(&obj.sha256)?;
        if input.kind == BrowseKind::Mod {
            mods::record_provenance(game_dir, &[(pl.name.clone(), Provenance { source: pl.source.as_str().into(), id: pl.project_id.clone() })])?;
        }
        report.installed.push(pl.name.clone());
    }
    Ok(report)
}

/// Télécharge le fichier d'un modpack (`.mrpack` Modrinth, zip CurseForge) et le copie sous `tmp_dir`
/// avec son nom d'origine. L'appelant lance ensuite l'import. Renvoie le chemin du fichier copié.
pub async fn fetch_modpack(
    p: &Providers,
    input: &BrowseInstallInput,
    store_root: &Path,
    index: &StoreIndex,
    tmp_dir: &Path,
    dl: &DlOptions,
    progress: ProgressCb<'_>,
) -> Result<PathBuf> {
    if input.kind != BrowseKind::Modpack {
        return Err(invalid("ce n'est pas un modpack"));
    }
    let ext = match input.source {
        BrowseSource::Modrinth => ".mrpack",
        BrowseSource::Curseforge => ".zip",
    };
    let (pl, _) = plan_main(p, input, None, None, ext).await?;
    progress(0, 1, &pl.name);
    let obj = fetch(&pl, store_root, index, dl).await?;
    std::fs::create_dir_all(tmp_dir)?;
    let dest = tmp_dir.join(&pl.name);
    std::fs::copy(store_root.join(&obj.rel_path), &dest)?;
    progress(1, 1, &pl.name);
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::LoaderSpec;
    use serde_json::json;
    use sha2::{Digest, Sha512};
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn providers(mr: &MockServer, cf: &MockServer) -> Providers {
        Providers { modrinth: ModrinthClient::with_base_url(format!("{}/v2", mr.uri()), "test"), cf: CfApi::with_base(cf.uri(), "k"), allow_loopback: true }
    }

    fn sha512(b: &[u8]) -> String {
        hex::encode(Sha512::digest(b))
    }

    fn inst(loader: Loader) -> Instance {
        Instance {
            id: "i1".into(),
            name: "t".into(),
            minecraft: "1.20.1".into(),
            loader: LoaderSpec { loader, version: "1".into() },
            ram_mb: 2048,
            mod_count: 0,
            java_path: None,
            jvm_args: String::new(),
            art: None,
        }
    }

    struct Env {
        _d: tempfile::TempDir,
        store: PathBuf,
        index: StoreIndex,
        game: PathBuf,
    }

    fn env() -> Env {
        let d = tempfile::tempdir().unwrap();
        let store = d.path().join("store");
        std::fs::create_dir_all(&store).unwrap();
        let index = StoreIndex::open(store.join("index.sqlite")).unwrap();
        let game = d.path().join("inst").join("minecraft");
        std::fs::create_dir_all(&game).unwrap();
        Env { _d: d, store, index, game }
    }

    fn dlo() -> DlOptions {
        DlOptions { attempts: 1, ..Default::default() }
    }

    fn input(source: BrowseSource, id: &str, kind: BrowseKind) -> BrowseInstallInput {
        BrowseInstallInput { source, id: id.into(), kind, version_id: None, instance_id: Some("i1".into()) }
    }

    async fn serve(s: &MockServer, p: &str, body: &[u8]) {
        Mock::given(method("GET")).and(path(p.to_string())).respond_with(ResponseTemplate::new(200).set_body_bytes(body.to_vec())).mount(s).await;
    }

    fn mr_version(s: &MockServer, id: &str, project: &str, file: &str, body: &[u8], deps: serde_json::Value, date: &str) -> serde_json::Value {
        json!({"id": id, "project_id": project, "name": format!("v {id}"), "version_number": "1", "date_published": date,
               "game_versions": ["1.20.1"], "loaders": ["fabric"],
               "files": [{"url": format!("{}/cdn/{file}", s.uri()), "filename": file, "primary": true, "size": body.len(),
                          "hashes": {"sha1": "", "sha512": sha512(body)}}],
               "dependencies": deps})
    }

    #[tokio::test]
    async fn search_modrinth_maps_hits_and_clamps_limit() {
        let (mr, cf) = (MockServer::start().await, MockServer::start().await);
        Mock::given(method("GET"))
            .and(path("/v2/search"))
            .and(query_param("limit", "50"))
            .and(query_param("index", "downloads"))
            .and(query_param("facets", r#"[["versions:1.20.1"],["categories:fabric"],["project_type:mod"]]"#))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"hits": [
                {"project_id": "P1", "slug": "sodium", "title": "Sodium", "description": "fast", "downloads": 7, "icon_url": "https://cdn.modrinth.com/i.png", "author": "jelly"}
            ], "total_hits": 321})))
            .expect(1)
            .mount(&mr)
            .await;
        let q = BrowseQuery {
            source: BrowseSource::Modrinth,
            kind: BrowseKind::Mod,
            query: " ".into(),
            minecraft: Some("1.20.1".into()),
            loader: Some("fabric".into()),
            sort: Some("downloads".into()),
            offset: 0,
            limit: 500,
        };
        let r = search(&providers(&mr, &cf), &q).await.unwrap();
        assert_eq!(r.total, 321);
        let h = &r.hits[0];
        assert_eq!((h.id.as_str(), h.name.as_str(), h.author.as_str(), h.downloads, h.downloadable), ("P1", "Sodium", "jelly", 7, true));
        assert_eq!(h.page_url, "https://modrinth.com/mod/sodium");
        assert_eq!(h.icon_url.as_deref(), Some("https://cdn.modrinth.com/i.png"));
        let v = serde_json::to_value(h).unwrap();
        assert_eq!((v["source"].as_str(), v["kind"].as_str()), (Some("modrinth"), Some("mod")));
    }

    #[tokio::test]
    async fn search_curseforge_flags_non_distributable_and_ignores_loader_for_shaders() {
        let (mr, cf) = (MockServer::start().await, MockServer::start().await);
        Mock::given(method("GET"))
            .and(path("/v1/mods/search"))
            .and(query_param("classId", "6552"))
            .and(query_param("index", "10"))
            .and(query_param("pageSize", "5"))
            .and(query_param("sortField", "3"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": [
                {"id": 11, "name": "Shade", "summary": "s", "links": {"websiteUrl": "https://www.curseforge.com/minecraft/shaders/shade"},
                 "authors": [{"name": "a"}], "logo": {"thumbnailUrl": "https://media.forgecdn.net/x.png"}, "downloadCount": 1500.7, "allowModDistribution": false},
                {"id": 12, "name": "Open", "downloadCount": 1.0}
            ], "pagination": {"totalCount": 2}})))
            .expect(1)
            .mount(&cf)
            .await;
        let q = BrowseQuery {
            source: BrowseSource::Curseforge,
            kind: BrowseKind::Shader,
            query: "".into(),
            minecraft: None,
            loader: Some("fabric".into()),
            sort: Some("updated".into()),
            offset: 10,
            limit: 5,
        };
        let r = search(&providers(&mr, &cf), &q).await.unwrap();
        assert_eq!(r.total, 2);
        assert!(!r.hits[0].downloadable && r.hits[1].downloadable);
        assert_eq!((r.hits[0].downloads, r.hits[0].id.as_str()), (1500, "11"));
        assert_eq!(r.hits[1].page_url, "https://www.curseforge.com/minecraft");
        // Entrées invalides.
        let bad = BrowseQuery { loader: Some("zzz".into()), kind: BrowseKind::Mod, ..q.clone() };
        assert!(search(&providers(&mr, &cf), &bad).await.is_err());
        let bad = BrowseQuery { minecraft: Some("1.20/../x".into()), ..q };
        assert!(search(&providers(&mr, &cf), &bad).await.is_err());
    }

    #[tokio::test]
    async fn versions_modrinth_sorted_filtered_and_cf_downloadable() {
        let (mr, cf) = (MockServer::start().await, MockServer::start().await);
        let a = mr_version(&mr, "old", "P", "a-1.jar", b"1", json!([]), "2023-01-01T00:00:00Z");
        let mut b = mr_version(&mr, "new", "P", "a-2.jar", b"2", json!([]), "2024-01-01T00:00:00Z");
        b["loaders"] = json!(["fabric", "quilt"]);
        let mut c = mr_version(&mr, "forge", "P", "a-f.jar", b"3", json!([]), "2025-01-01T00:00:00Z");
        c["loaders"] = json!(["forge"]);
        Mock::given(method("GET"))
            .and(path("/v2/project/P/version"))
            .and(query_param("loaders", r#"["fabric"]"#))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([a, c, b])))
            .mount(&mr)
            .await;
        let p = providers(&mr, &cf);
        let v = versions(&p, "modrinth", "P", "mod", Some("1.20.1"), Some("fabric")).await.unwrap();
        assert_eq!(v.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), vec!["new", "old"]);
        assert_eq!(v[0].name, "v new");
        assert!(v[0].downloadable);
        assert!(versions(&p, "nope", "P", "mod", None, None).await.is_err());
        assert!(versions(&p, "modrinth", "P", "weird", None, None).await.is_err());

        Mock::given(method("GET"))
            .and(path("/v1/mods/9/files"))
            .and(query_param("modLoaderType", "4"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": [
                {"id": 1, "displayName": "A", "fileName": "a.jar", "fileDate": "2024-01-01T00:00:00Z", "downloadUrl": null, "gameVersions": ["1.20.1", "Fabric"]},
                {"id": 2, "displayName": "B", "fileName": "b.jar", "fileDate": "2024-02-01T00:00:00Z", "downloadUrl": "https://edge.forgecdn.net/b.jar", "gameVersions": ["1.20.1", "Fabric"]},
                {"id": 3, "displayName": "C", "fileName": "c.jar", "fileDate": "2024-03-01T00:00:00Z", "downloadUrl": "https://edge.forgecdn.net/c.jar", "gameVersions": ["1.19.2", "Fabric"]}
            ]})))
            .mount(&cf)
            .await;
        let v = versions(&p, "curseforge", "9", "mod", Some("1.20.1"), Some("fabric")).await.unwrap();
        assert_eq!(v.iter().map(|x| (x.id.as_str(), x.downloadable)).collect::<Vec<_>>(), vec![("2", true), ("1", false)]);
        assert_eq!(v[0].minecraft, vec!["1.20.1"]);
        assert_eq!(v[0].loaders, vec!["fabric"]);
    }

    #[tokio::test]
    async fn install_modrinth_mod_with_required_dependency() {
        let (mr, cf) = (MockServer::start().await, MockServer::start().await);
        let (main_body, dep_body) = (b"MAIN JAR".to_vec(), b"DEP JAR".to_vec());
        let deps = json!([
            {"project_id": "DEP", "version_id": null, "dependency_type": "required"},
            {"project_id": "OPT", "version_id": null, "dependency_type": "optional"}
        ]);
        let v = mr_version(&mr, "m1", "MAIN", "main-1.0.jar", &main_body, deps, "2024-01-01T00:00:00Z");
        Mock::given(method("GET")).and(path("/v2/project/MAIN/version")).respond_with(ResponseTemplate::new(200).set_body_json(json!([v]))).mount(&mr).await;
        let dv = mr_version(&mr, "d1", "DEP", "dep-2.jar", &dep_body, json!([]), "2024-01-01T00:00:00Z");
        Mock::given(method("GET")).and(path("/v2/project/DEP/version")).respond_with(ResponseTemplate::new(200).set_body_json(json!([dv]))).mount(&mr).await;
        serve(&mr, "/cdn/main-1.0.jar", &main_body).await;
        serve(&mr, "/cdn/dep-2.jar", &dep_body).await;
        let e = env();
        let prog = std::sync::Mutex::new(Vec::new());
        let cb = |d: u32, t: u32, l: &str| prog.lock().unwrap().push((d, t, l.to_string()));
        let r = install_content(&providers(&mr, &cf), &input(BrowseSource::Modrinth, "MAIN", BrowseKind::Mod), &inst(Loader::Fabric), &e.game, &e.store, &e.index, &dlo(), &cb)
            .await
            .unwrap();
        assert_eq!(r.installed.len(), 2);
        assert!(r.installed.contains(&"main-1.0.jar".to_string()) && r.installed.contains(&"dep-2.jar".to_string()));
        assert!(r.missing.is_empty());
        assert_eq!(std::fs::read(e.game.join("mods/main-1.0.jar")).unwrap(), main_body);
        assert_eq!(std::fs::read(e.game.join("mods/dep-2.jar")).unwrap(), dep_body);
        assert!(!e.game.join("mods/opt.jar").exists());
        let prov = mods::read_provenance(&e.game);
        assert_eq!(prov["main-1.0.jar"], Provenance { source: "modrinth".into(), id: "MAIN".into() });
        assert_eq!(prov["dep-2.jar"].id, "DEP");
        {
        let pr = prog.lock().unwrap();
        assert_eq!(pr.first().map(|x| (x.0, x.1)), Some((0, 2)));
        assert_eq!(pr.last().map(|x| (x.0, x.1)), Some((2, 2)));
        // Réinstaller une version plus récente retire l'ancienne.
        }

        let v2 = mr_version(&mr, "m2", "MAIN", "main-2.0.jar", b"NEW", json!([]), "2025-01-01T00:00:00Z");
        mr.reset().await;
        Mock::given(method("GET")).and(path("/v2/project/MAIN/version")).respond_with(ResponseTemplate::new(200).set_body_json(json!([v2]))).mount(&mr).await;
        serve(&mr, "/cdn/main-2.0.jar", b"NEW").await;
        install_content(&providers(&mr, &cf), &input(BrowseSource::Modrinth, "MAIN", BrowseKind::Mod), &inst(Loader::Fabric), &e.game, &e.store, &e.index, &dlo(), &|_, _, _| {})
            .await
            .unwrap();
        assert!(!e.game.join("mods/main-1.0.jar").exists() && e.game.join("mods/main-2.0.jar").is_file());
        assert!(e.game.join("mods/dep-2.jar").is_file());
    }

    #[tokio::test]
    async fn hash_mismatch_installs_nothing() {
        let (mr, cf) = (MockServer::start().await, MockServer::start().await);
        let v = mr_version(&mr, "m1", "MAIN", "x.jar", b"expected", json!([]), "2024-01-01T00:00:00Z");
        Mock::given(method("GET")).and(path("/v2/project/MAIN/version")).respond_with(ResponseTemplate::new(200).set_body_json(json!([v]))).mount(&mr).await;
        serve(&mr, "/cdn/x.jar", b"tampered").await;
        let e = env();
        let r = install_content(&providers(&mr, &cf), &input(BrowseSource::Modrinth, "MAIN", BrowseKind::Mod), &inst(Loader::Fabric), &e.game, &e.store, &e.index, &dlo(), &|_, _, _| {}).await;
        assert!(r.is_err());
        assert!(!e.game.join("mods").exists() || std::fs::read_dir(e.game.join("mods")).unwrap().count() == 0);
    }

    #[tokio::test]
    async fn install_curseforge_mod_with_dep_and_non_downloadable() {
        let (mr, cf) = (MockServer::start().await, MockServer::start().await);
        let (body, dep_body) = (b"CF MOD".to_vec(), b"CF DEP".to_vec());
        let files = json!({"data": [
            {"id": 100, "modId": 1, "displayName": "M", "fileName": "cfmod.jar", "fileDate": "2024-01-01T00:00:00Z",
             "downloadUrl": format!("{}/dl/cfmod.jar", cf.uri()), "fileLength": body.len(), "hashes": [],
             "gameVersions": ["1.20.1", "Fabric"],
             "dependencies": [{"modId": 2, "relationType": 3}, {"modId": 3, "relationType": 3}, {"modId": 4, "relationType": 2}]}
        ]});
        let dep_files = json!({"data": [
            {"id": 200, "modId": 2, "fileName": "cfdep.jar", "fileDate": "2024-01-01T00:00:00Z", "downloadUrl": format!("{}/dl/cfdep.jar", cf.uri()),
             "gameVersions": ["1.20.1", "Fabric"]}
        ]});
        let closed = json!({"data": [
            {"id": 300, "modId": 3, "fileName": "closed.jar", "fileDate": "2024-01-01T00:00:00Z", "downloadUrl": null, "gameVersions": ["1.20.1", "Fabric"]}
        ]});
        for (m, b) in [(1, files), (2, dep_files), (3, closed)] {
            Mock::given(method("GET")).and(path(format!("/v1/mods/{m}/files"))).respond_with(ResponseTemplate::new(200).set_body_json(b)).mount(&cf).await;
        }
        serve(&cf, "/dl/cfmod.jar", &body).await;
        serve(&cf, "/dl/cfdep.jar", &dep_body).await;
        let e = env();
        let r = install_content(&providers(&mr, &cf), &input(BrowseSource::Curseforge, "1", BrowseKind::Mod), &inst(Loader::Fabric), &e.game, &e.store, &e.index, &dlo(), &|_, _, _| {})
            .await
            .unwrap();
        assert_eq!(r.installed.len(), 2);
        assert_eq!(r.missing.len(), 1, "{:?}", r.missing);
        assert!(r.missing[0].contains('3'));
        assert!(e.game.join("mods/cfmod.jar").is_file() && e.game.join("mods/cfdep.jar").is_file());
        assert_eq!(mods::read_provenance(&e.game)["cfdep.jar"], Provenance { source: "curseforge".into(), id: "2".into() });

        // Fichier principal non téléchargeable : refus clair, rien n'est posé.
        Mock::given(method("GET")).and(path("/v1/mods/9/files")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": [
            {"id": 900, "modId": 9, "fileName": "x.jar", "fileDate": "2024-01-01T00:00:00Z", "downloadUrl": null, "gameVersions": ["1.20.1", "Fabric"]}
        ]}))).mount(&cf).await;
        let e2 = env();
        let err = install_content(&providers(&mr, &cf), &input(BrowseSource::Curseforge, "9", BrowseKind::Mod), &inst(Loader::Fabric), &e2.game, &e2.store, &e2.index, &dlo(), &|_, _, _| {})
            .await
            .unwrap_err();
        assert!(err.to_string().contains("CurseForge"), "{err}");
        assert!(!e2.game.join("mods").exists());
    }

    #[tokio::test]
    async fn install_resourcepack_and_no_compatible_version() {
        let (mr, cf) = (MockServer::start().await, MockServer::start().await);
        let body = b"PK zip".to_vec();
        let mut v = mr_version(&mr, "r1", "RP", "pack.zip", &body, json!([{"project_id": "X", "dependency_type": "required"}]), "2024-01-01T00:00:00Z");
        v["loaders"] = json!(["minecraft"]);
        Mock::given(method("GET")).and(path("/v2/project/RP/version")).respond_with(ResponseTemplate::new(200).set_body_json(json!([v]))).mount(&mr).await;
        serve(&mr, "/cdn/pack.zip", &body).await;
        let e = env();
        let r = install_content(&providers(&mr, &cf), &input(BrowseSource::Modrinth, "RP", BrowseKind::Resourcepack), &inst(Loader::Forge), &e.game, &e.store, &e.index, &dlo(), &|_, _, _| {})
            .await
            .unwrap();
        assert_eq!(r.installed, vec!["pack.zip"]);
        assert!(e.game.join("resourcepacks/pack.zip").is_file());
        assert!(mods::read_provenance(&e.game).is_empty());

        Mock::given(method("GET")).and(path("/v2/project/NONE/version")).respond_with(ResponseTemplate::new(200).set_body_json(json!([]))).mount(&mr).await;
        let err = install_content(&providers(&mr, &cf), &input(BrowseSource::Modrinth, "NONE", BrowseKind::Mod), &inst(Loader::Fabric), &e.game, &e.store, &e.index, &dlo(), &|_, _, _| {})
            .await
            .unwrap_err();
        assert!(matches!(err, Error::NotFound(_)), "{err}");
        assert!(install_content(&providers(&mr, &cf), &input(BrowseSource::Modrinth, "RP", BrowseKind::Modpack), &inst(Loader::Fabric), &e.game, &e.store, &e.index, &dlo(), &|_, _, _| {}).await.is_err());
    }

    #[tokio::test]
    async fn refuses_bad_hosts_names_and_custom_conflict() {
        let (mr, cf) = (MockServer::start().await, MockServer::start().await);
        let strict = Providers { allow_loopback: false, ..providers(&mr, &cf) };
        let mut v = mr_version(&mr, "m1", "MAIN", "x.jar", b"x", json!([]), "2024-01-01T00:00:00Z");
        Mock::given(method("GET")).and(path("/v2/project/MAIN/version")).respond_with(ResponseTemplate::new(200).set_body_json(json!([v.clone()]))).mount(&mr).await;
        let e = env();
        let err = install_content(&strict, &input(BrowseSource::Modrinth, "MAIN", BrowseKind::Mod), &inst(Loader::Fabric), &e.game, &e.store, &e.index, &dlo(), &|_, _, _| {}).await.unwrap_err();
        assert!(err.to_string().contains("hôte"), "{err}");

        for bad in ["../evil.jar", "a/b.jar", "evil.exe", "x.txt"] {
            v["files"][0]["filename"] = json!(bad);
            mr.reset().await;
            Mock::given(method("GET")).and(path("/v2/project/MAIN/version")).respond_with(ResponseTemplate::new(200).set_body_json(json!([v.clone()]))).mount(&mr).await;
            assert!(install_content(&providers(&mr, &cf), &input(BrowseSource::Modrinth, "MAIN", BrowseKind::Mod), &inst(Loader::Fabric), &e.game, &e.store, &e.index, &dlo(), &|_, _, _| {}).await.is_err(), "{bad}");
        }

        // Un mod « custom » de même nom n'est pas écrasé.
        v["files"][0]["filename"] = json!("x.jar");
        mr.reset().await;
        Mock::given(method("GET")).and(path("/v2/project/MAIN/version")).respond_with(ResponseTemplate::new(200).set_body_json(json!([v]))).mount(&mr).await;
        serve(&mr, "/cdn/x.jar", b"x").await;
        std::fs::create_dir_all(e.game.join("mods")).unwrap();
        std::fs::write(e.game.join("mods/x.jar"), b"mine").unwrap();
        mods::record_provenance(&e.game, &[("x.jar".into(), Provenance { source: "custom".into(), id: String::new() })]).unwrap();
        assert!(install_content(&providers(&mr, &cf), &input(BrowseSource::Modrinth, "MAIN", BrowseKind::Mod), &inst(Loader::Fabric), &e.game, &e.store, &e.index, &dlo(), &|_, _, _| {}).await.is_err());
        assert_eq!(std::fs::read(e.game.join("mods/x.jar")).unwrap(), b"mine");
    }

    #[tokio::test]
    async fn fetch_modpack_downloads_for_both_sources() {
        let (mr, cf) = (MockServer::start().await, MockServer::start().await);
        let body = b"PK mrpack".to_vec();
        let v = mr_version(&mr, "pk1", "PACK", "My Pack-1.mrpack", &body, json!([]), "2024-01-01T00:00:00Z");
        Mock::given(method("GET")).and(path("/v2/version/pk1")).respond_with(ResponseTemplate::new(200).set_body_json(v)).mount(&mr).await;
        Mock::given(method("GET")).and(path("/cdn/My%20Pack-1.mrpack")).respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone())).mount(&mr).await;
        let e = env();
        let tmp = e.store.join("tmp-browse");
        let mut i = input(BrowseSource::Modrinth, "PACK", BrowseKind::Modpack);
        i.version_id = Some("pk1".into());
        i.instance_id = None;
        let f = fetch_modpack(&providers(&mr, &cf), &i, &e.store, &e.index, &tmp, &dlo(), &|_, _, _| {}).await.unwrap();
        assert_eq!(f.file_name().unwrap(), "My Pack-1.mrpack");
        assert_eq!(std::fs::read(&f).unwrap(), body);

        let zip_body = b"PK cf".to_vec();
        Mock::given(method("GET"))
            .and(path("/v1/mods/50/files/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {"id": 7, "modId": 50, "fileName": "pack.zip", "fileDate": "2024-01-01T00:00:00Z",
                "downloadUrl": format!("{}/dl/pack.zip", cf.uri()), "gameVersions": []}})))
            .mount(&cf)
            .await;
        serve(&cf, "/dl/pack.zip", &zip_body).await;
        let ci = BrowseInstallInput { source: BrowseSource::Curseforge, id: "50".into(), kind: BrowseKind::Modpack, version_id: Some("7".into()), instance_id: None };
        let f = fetch_modpack(&providers(&mr, &cf), &ci, &e.store, &e.index, &tmp, &dlo(), &|_, _, _| {}).await.unwrap();
        assert_eq!(std::fs::read(f).unwrap(), zip_body);
    }

    #[test]
    fn url_policy() {
        assert!(check_url("https://cdn.modrinth.com/data/a/b.jar", BrowseSource::Modrinth, false).is_ok());
        assert!(check_url("https://edge.forgecdn.net/files/1/2/a.jar", BrowseSource::Curseforge, false).is_ok());
        assert!(check_url("https://mediafilez.forgecdn.net/files/1/2/a.jar", BrowseSource::Curseforge, false).is_ok());
        assert!(check_url("https://edge.forgecdn.net/a.jar", BrowseSource::Modrinth, false).is_err());
        assert!(check_url("http://cdn.modrinth.com/a.jar", BrowseSource::Modrinth, false).is_err());
        assert!(check_url("https://cdn.modrinth.com.evil.com/a.jar", BrowseSource::Modrinth, false).is_err());
        assert!(check_url("https://user@cdn.modrinth.com/a.jar", BrowseSource::Modrinth, false).is_err());
        assert!(check_url("http://127.0.0.1:1/a.jar", BrowseSource::Modrinth, false).is_err());
        assert!(check_url("http://127.0.0.1:1/a.jar", BrowseSource::Modrinth, true).is_ok());
        assert!(check_url("http://127.0.0.1.evil.com/a.jar", BrowseSource::Modrinth, true).is_err());
    }

    /// Test MANUEL contre les vraies API : `cargo test -p cubelink-core live_ -- --ignored --nocapture`
    /// (CurseForge nécessite `CUBELINK_CF_API_KEY` dans l'environnement).
    #[tokio::test]
    #[ignore = "réseau réel"]
    async fn live_modrinth_and_curseforge() {
        let ua = "Cubelink-live-test";
        let p = Providers { modrinth: ModrinthClient::new(ua), cf: CfApi::new(), allow_loopback: false };
        let mut instance = inst(Loader::Fabric);
        instance.minecraft = "1.21.1".into();
        for (source, query) in [(BrowseSource::Modrinth, "sodium"), (BrowseSource::Curseforge, "appleskin")] {
            let q = BrowseQuery { source, kind: BrowseKind::Mod, query: query.into(), minecraft: Some("1.21.1".into()), loader: Some("fabric".into()), sort: Some("downloads".into()), offset: 0, limit: 5 };
            let page = search(&p, &q).await.expect("recherche");
            println!("[{source:?}] {} résultats, premiers : {:?}", page.total, page.hits.iter().map(|h| (&h.name, h.downloadable)).collect::<Vec<_>>());
            let hit = page.hits.iter().find(|h| h.downloadable).expect("un résultat téléchargeable");
            let vs = versions(&p, hit.source.as_str(), &hit.id, "mod", Some("1.21.1"), Some("fabric")).await.expect("versions");
            println!("[{source:?}] {} : {} versions, plus récente : {:?}", hit.name, vs.len(), vs.first().map(|v| (&v.name, &v.published)));
            let e = env();
            let input = BrowseInstallInput { source, id: hit.id.clone(), kind: BrowseKind::Mod, version_id: None, instance_id: Some("i1".into()) };
            let r = install_content(&p, &input, &instance, &e.game, &e.store, &e.index, &DlOptions::default(), &|_, _, _| {}).await.expect("installation");
            println!("[{source:?}] installés : {:?} ; manquants : {:?}", r.installed, r.missing);
            for f in &r.installed {
                let len = std::fs::metadata(e.game.join("mods").join(f)).expect("fichier posé").len();
                assert!(len > 1000, "{f} trop petit ({len} o)");
            }
        }
        // modpacks : recherche + versions seulement (pas d'installation, fichiers volumineux)
        for source in [BrowseSource::Modrinth, BrowseSource::Curseforge] {
            let q = BrowseQuery { source, kind: BrowseKind::Modpack, query: "fabulously optimized".into(), minecraft: None, loader: None, sort: None, offset: 0, limit: 3 };
            let page = search(&p, &q).await.expect("recherche modpack");
            println!("[{source:?}] modpacks : {:?}", page.hits.iter().map(|h| &h.name).collect::<Vec<_>>());
            assert!(!page.hits.is_empty());
        }
    }
}
