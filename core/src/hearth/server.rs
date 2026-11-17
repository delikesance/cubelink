//! Hearth : serveur dédié d'un groupe (voir `docs/hearth.md`, § 6 et § 7).
//!
//! * [`ServerInstaller`] installe le serveur par loader (vanilla, Fabric, Quilt, Forge, NeoForge) dans un dossier ;
//! * [`sync_server_mods`] copie les mods du modpack sans ceux qui ne servent qu'au client ;
//! * [`DedicatedServer`] lance le serveur en arrière-plan, le pilote par son entrée standard (`save-off`,
//!   `save-all flush`, `save-on`, `stop`) et en attend les confirmations dans sa sortie.
//!
//! Le texte lu dans la sortie du serveur est traité comme une donnée non fiable : les lignes sont bornées, les
//! confirmations se reconnaissent sur le message (pas sur un message de chat), et rien ici ne l'exécute.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::io::Read;
use std::net::TcpListener;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::FutureExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use crate::java::{default_search_dirs, find_installed, required_java_major, select_for};
use crate::java_install;
use crate::loader::fabric::FabricClient;
use crate::loader::forge::ForgeClient;
use crate::loader::neoforge::NeoForgeClient;
use crate::loader::quilt::QuiltClient;
use crate::mojang::versions::{sha1_hex, DownloadInfo, JavaVersion, VersionsClient, DEFAULT_MANIFEST_URL};
use crate::types::{Error, Loader, LoaderSpec, Result};

// ---------------------------------------------------------------------------------------------------------------
// Lecture de la sortie du serveur (fonctions pures)
// ---------------------------------------------------------------------------------------------------------------

/// Message d'une ligne de journal, sans l'horodatage, le fil et le niveau.
///
/// Formats rencontrés : `[12:00:00] [Server thread/INFO]: Done (…)` (vanilla, Forge, NeoForge) et
/// `[12:00:00] [Server thread/INFO] (Minecraft) Done (…)` (Fabric, Quilt). Une ligne sans ce préfixe est rendue telle
/// quelle. Un message de chat (`<Alice> Done (`) garde son `<Alice>` : il ne ressemble donc à aucune confirmation.
pub fn log_message(line: &str) -> &str {
    let line = line.trim_end_matches(['\r', '\n']);
    let mut rest = line;
    if rest.starts_with('[') {
        // Deux groupes entre crochets : l'heure, puis `fil/NIVEAU`.
        let mut after = rest;
        let mut groups = 0;
        while groups < 2 && after.starts_with('[') {
            match after.find(']') {
                Some(i) => {
                    after = after[i + 1..].trim_start_matches(' ');
                    groups += 1;
                }
                None => return line,
            }
        }
        if groups < 2 {
            return line;
        }
        rest = after;
        // Nom du logger : `(Minecraft) ` ou `[Minecraft]` selon le loader, puis éventuellement `:`.
        if rest.starts_with('(') {
            if let Some(i) = rest.find(')') {
                rest = rest[i + 1..].trim_start_matches(' ');
            }
        }
        rest = rest.strip_prefix(':').unwrap_or(rest).trim_start_matches(' ');
    }
    rest
}

/// Le serveur est prêt : `Done (12.3s)! For help, type "help"`.
pub fn is_ready_line(line: &str) -> bool {
    log_message(line).starts_with("Done (")
}

/// Fin de `save-all flush` : `Saved the game` (1.13+) ou `Saved the world` (plus ancien).
pub fn is_saved_line(line: &str) -> bool {
    let m = log_message(line);
    m.starts_with("Saved the game") || m.starts_with("Saved the world")
}

/// Fin de `save-off` : `Automatic saving is now disabled`, ou `Saving is already turned off` (déjà coupée).
pub fn is_save_off_line(line: &str) -> bool {
    let m = log_message(line);
    m.starts_with("Automatic saving is now disabled") || m.starts_with("Saving is already turned off")
}

/// Fin de `save-on` : `Automatic saving is now enabled`, ou `Saving is already turned on`.
pub fn is_save_on_line(line: &str) -> bool {
    let m = log_message(line);
    m.starts_with("Automatic saving is now enabled") || m.starts_with("Saving is already turned on")
}

/// Cherche dans la sortie d'un démarrage raté le mod probablement en cause (heuristique, pour l'affichage).
///
/// Reconnaît : `provided by 'id'` (Fabric, échec d'un point d'entrée), `Mod 'Nom' (id)` (Fabric, dépendances),
/// `Mod ID: 'id'` (Forge, NeoForge), `Mixin apply for mod id failed` et `Failed to load mod id`.
pub fn diagnose_failure(lines: &[String]) -> Option<String> {
    for raw in lines {
        let m = log_message(raw);
        let found = between(m, "provided by '", "'")
            .map(|id| format!("{id} (point d'entrée en échec)"))
            .or_else(|| {
                let name = between(m, "Mod '", "'")?;
                let tail = &m[m.find("Mod '")? + 5 + name.len() + 1..];
                let id = between(tail.trim_start(), "(", ")").filter(|i| !i.contains(' '));
                Some(match id {
                    Some(id) => format!("{name} ({id})"),
                    None => name.to_string(),
                })
            })
            .or_else(|| between(m, "Mod ID: '", "'").map(str::to_string))
            .or_else(|| word_after(m, "Mixin apply for mod "))
            .or_else(|| word_after(m, "Failed to load mod "));
        if let Some(f) = found.filter(|f| !f.is_empty() && f.len() <= 120) {
            return Some(f);
        }
    }
    None
}

fn between<'a>(s: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let i = s.find(start)? + start.len();
    let j = s[i..].find(end)?;
    Some(&s[i..i + j])
}

fn word_after(s: &str, prefix: &str) -> Option<String> {
    let i = s.find(prefix)? + prefix.len();
    let w: String = s[i..].chars().take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.')).collect();
    (!w.is_empty()).then_some(w)
}

// ---------------------------------------------------------------------------------------------------------------
// Fichiers de configuration du serveur
// ---------------------------------------------------------------------------------------------------------------

/// Contenu de `eula.txt` (à n'écrire qu'après l'acceptation de l'EULA Minecraft par l'utilisateur).
pub fn eula_text() -> &'static str {
    "#En écrivant TRUE ci-dessous, vous acceptez l'EULA de Minecraft (https://aka.ms/MinecraftEULA).\neula=true\n"
}

/// Vrai si `<dir>/eula.txt` contient `eula=true`.
pub fn eula_accepted(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join("eula.txt"))
        .map(|t| t.lines().any(|l| l.trim().eq_ignore_ascii_case("eula=true")))
        .unwrap_or(false)
}

/// Réglages imposés à `server.properties` : en ligne (comptes Mojang vérifiés), local seulement (les amis passent par
/// le tunnel), port choisi, monde `world`.
pub fn render_server_properties(existing: &str, port: u16) -> String {
    let forced: [(&str, String); 4] = [
        ("online-mode", "true".into()),
        ("server-ip", "127.0.0.1".into()),
        ("server-port", port.to_string()),
        ("level-name", "world".into()),
    ];
    let mut done = [false; 4];
    let mut out = String::new();
    for line in existing.lines() {
        let key = line.trim_start().split(['=', ':']).next().unwrap_or("").trim();
        let idx = forced.iter().position(|(k, _)| !line.trim_start().starts_with(['#', '!']) && *k == key);
        match idx {
            Some(i) if !done[i] => {
                done[i] = true;
                out.push_str(&format!("{}={}\n", forced[i].0, forced[i].1));
            }
            // Doublon d'une clé déjà réécrite : supprimé.
            Some(_) => {}
            None => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    for (i, (k, v)) in forced.iter().enumerate() {
        if !done[i] {
            out.push_str(&format!("{k}={v}\n"));
        }
    }
    out
}

/// Un port TCP libre sur 127.0.0.1 (fenêtre de course minime entre ce choix et l'ouverture par le serveur).
pub fn free_port() -> Result<u16> {
    let l = TcpListener::bind(("127.0.0.1", 0))?;
    Ok(l.local_addr()?.port())
}

/// Mémoire du serveur selon la RAM disponible (Mo) : le quart, entre 2 Go et 8 Go, jamais plus que la moitié.
pub fn suggest_ram_mb(available_mb: u64) -> u32 {
    let quarter = (available_mb / 4).clamp(2048, 8192);
    quarter.min((available_mb / 2).max(1024)) as u32
}

/// Avertissement à afficher quand la machine a moins de 12 Go de RAM au total.
pub fn ram_warning(total_mb: u64) -> Option<String> {
    (total_mb < 12 * 1024).then(|| {
        format!(
            "Cet ordinateur a {} Go de mémoire : héberger un serveur moddé en jouant peut être lent. Un ami mieux équipé gagnerait à lancer la partie.",
            total_mb / 1024
        )
    })
}

// ---------------------------------------------------------------------------------------------------------------
// Mods : retirer ceux qui ne servent qu'au client
// ---------------------------------------------------------------------------------------------------------------

/// Prise en charge d'un côté, comme les métadonnées Modrinth (`client_side`, `server_side`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Required,
    Optional,
    Unsupported,
}

impl Side {
    /// `required`, `optional` ou `unsupported` ; toute autre valeur (ou `unknown`) donne `None`.
    pub fn parse(s: &str) -> Option<Side> {
        match s.trim().to_ascii_lowercase().as_str() {
            "required" => Some(Side::Required),
            "optional" => Some(Side::Optional),
            "unsupported" => Some(Side::Unsupported),
            _ => None,
        }
    }
}

/// Identifiants de mods connus pour ne servir qu'au client (rendu, minimap, interface). Dernier recours, quand ni
/// Modrinth ni le jar ne le déclarent.
pub const KNOWN_CLIENT_ONLY: &[&str] = &[
    "sodium", "sodiumextra", "sodium-extra", "reeses_sodium_options", "reeses-sodium-options", "iris", "irisshaders",
    "embeddium", "embeddiumplus", "rubidium", "oculus", "modmenu", "dynamic_fps", "dynamicfps", "entityculling",
    "immediatelyfast", "moreculling", "continuity", "lambdynamiclights", "zoomify", "fabricskyboxes", "betterf3",
    "xaerominimap", "xaeroworldmap", "journeymap-client", "replaymod", "tweakeroo", "litematica", "minihud",
    "itemscroller", "malilib", "notenoughanimations", "skinlayers3d", "3dskinlayers", "badoptimizations",
    "presencefootsteps", "freecam",
];

/// Ce qu'on sait d'un jar du dossier `mods`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModSideInfo {
    pub file_name: String,
    /// Identifiants des mods contenus dans le jar.
    pub mod_ids: Vec<String>,
    /// `server_side` de Modrinth, si le mod en vient et que la valeur est connue.
    pub server_side: Option<Side>,
    /// Le jar se déclare lui-même client seulement (`environment` de Fabric/Quilt, dépendance `side = "CLIENT"`).
    pub declares_client_only: bool,
}

/// Décision pour un mod : `Some(raison)` s'il faut l'écarter du serveur.
///
/// Ordre : la donnée explicite de Modrinth l'emporte (`unsupported` écarte, `required`/`optional` garde) ; sinon la
/// déclaration du jar ; sinon la liste [`KNOWN_CLIENT_ONLY`].
pub fn client_only_reason(info: &ModSideInfo) -> Option<String> {
    match info.server_side {
        Some(Side::Unsupported) => return Some("Modrinth : ne fonctionne pas côté serveur".into()),
        Some(Side::Required | Side::Optional) => return None,
        None => {}
    }
    if info.declares_client_only {
        return Some("le mod se déclare client seulement".into());
    }
    info.mod_ids
        .iter()
        .find(|id| KNOWN_CLIENT_ONLY.iter().any(|k| k.eq_ignore_ascii_case(id)))
        .map(|id| format!("mod client connu : {id}"))
}

/// Mod écarté, avec la raison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedMod {
    pub file_name: String,
    pub reason: String,
}

/// Résultat du tri.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModFilter {
    pub kept: Vec<String>,
    pub skipped: Vec<SkippedMod>,
}

/// Sépare les mods à garder de ceux à écarter. Fonction pure.
pub fn filter_server_mods(mods: &[ModSideInfo]) -> ModFilter {
    let mut out = ModFilter::default();
    for m in mods {
        match client_only_reason(m) {
            Some(reason) => out.skipped.push(SkippedMod { file_name: m.file_name.clone(), reason }),
            None => out.kept.push(m.file_name.clone()),
        }
    }
    out
}

const MAX_META_BYTES: u64 = 1 << 20;

/// Lit les métadonnées d'un jar : identifiants de mods, et déclaration « client seulement » (`environment` de
/// `fabric.mod.json` ou `quilt.mod.json`, dépendance `minecraft` avec `side = "CLIENT"` de Forge/NeoForge).
pub fn jar_side_info(path: &Path) -> ModSideInfo {
    let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut info = ModSideInfo { file_name, ..ModSideInfo::default() };
    if let Some(meta) = crate::mod_meta::jar_meta(path) {
        info.mod_ids = meta.mods.iter().map(|m| m.id.clone()).collect();
        let forge_client = !meta.mods.is_empty()
            && meta
                .mods
                .iter()
                .all(|m| m.deps.iter().any(|d| d.id == "minecraft" && d.side.eq_ignore_ascii_case("CLIENT")));
        info.declares_client_only = forge_client;
    }
    if !info.declares_client_only {
        info.declares_client_only = jar_environment_is_client(path);
    }
    info
}

fn jar_environment_is_client(path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(path) else { return false };
    let Ok(mut zip) = zip::ZipArchive::new(std::io::BufReader::new(file)) else { return false };
    for (name, pointer) in [("fabric.mod.json", "/environment"), ("quilt.mod.json", "/minecraft/environment")] {
        let Ok(entry) = zip.by_name(name) else { continue };
        let mut buf = Vec::new();
        if entry.take(MAX_META_BYTES).read_to_end(&mut buf).is_err() {
            continue;
        }
        let Ok(v) = serde_json::from_slice::<Value>(&buf) else { continue };
        if v.pointer(pointer).and_then(Value::as_str) == Some("client") {
            return true;
        }
    }
    false
}

/// Recopie dans `server_mods` les jars de `pack_mods` qui servent côté serveur, et retire de `server_mods` les jars
/// qui n'y ont plus leur place. `modrinth_server_side` : `server_side` connu par nom de fichier.
pub fn sync_server_mods(
    pack_mods: &Path,
    server_mods: &Path,
    modrinth_server_side: &HashMap<String, Side>,
) -> Result<ModFilter> {
    std::fs::create_dir_all(server_mods)?;
    let mut infos = Vec::new();
    match std::fs::read_dir(pack_mods) {
        Ok(rd) => {
            for e in rd {
                let e = e?;
                let p = e.path();
                if !e.file_type()?.is_file() || p.extension().and_then(|x| x.to_str()).is_none_or(|x| !x.eq_ignore_ascii_case("jar")) {
                    continue;
                }
                let mut info = jar_side_info(&p);
                info.server_side = modrinth_server_side.get(&info.file_name).copied();
                infos.push(info);
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    infos.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    let filter = filter_server_mods(&infos);
    for name in &filter.kept {
        let dst = server_mods.join(name);
        let src = pack_mods.join(name);
        let same = match (std::fs::metadata(&src), std::fs::metadata(&dst)) {
            (Ok(a), Ok(b)) => a.len() == b.len() && a.modified().ok() == b.modified().ok(),
            _ => false,
        };
        if !same {
            std::fs::copy(&src, &dst)?;
            // La date de la source est reprise : une recopie identique n'est pas refaite au passage suivant.
            if let Ok(t) = std::fs::metadata(&src).and_then(|m| m.modified()) {
                if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&dst) {
                    let _ = f.set_modified(t);
                }
            }
        }
    }
    for e in std::fs::read_dir(server_mods)? {
        let e = e?;
        let name = e.file_name().to_string_lossy().into_owned();
        if e.file_type()?.is_file() && name.to_ascii_lowercase().ends_with(".jar") && !filter.kept.contains(&name) {
            std::fs::remove_file(e.path())?;
        }
    }
    Ok(filter)
}

// ---------------------------------------------------------------------------------------------------------------
// Java
// ---------------------------------------------------------------------------------------------------------------

/// Trouve un Java de version majeure `major` (ou compatible), l'installe au besoin dans `java_root`. `configured`
/// (réglage de l'utilisateur) est utilisé tel quel s'il existe.
pub async fn resolve_java(java_root: &Path, major: u32, configured: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = configured {
        return if p.is_file() { Ok(p.to_path_buf()) } else { Err(Error::NotFound("l'exécutable Java configuré est introuvable".into())) };
    }
    let dirs = default_search_dirs(java_root);
    let installs = tokio::task::spawn_blocking(move || find_installed(&dirs))
        .await
        .map_err(|_| Error::Other("recherche de Java interrompue".into()))?;
    if let Some(i) = select_for(major, &installs) {
        return Ok(i.path.clone());
    }
    let pkg = java_install::resolve_package(major, java_install::host_os(), java_install::host_arch()).await?;
    let cancel = Arc::new(AtomicBool::new(false));
    Ok(java_install::install(&pkg, java_root, |_, _| {}, cancel).await?.java_exe)
}

// ---------------------------------------------------------------------------------------------------------------
// Installation
// ---------------------------------------------------------------------------------------------------------------

const RECORD_FILE: &str = ".cubelink-server.json";
const VERSION_JSON_MAX: usize = 4 << 20;
const LIST_MAX: usize = 4 << 20;
const SERVER_JAR_MAX: u64 = 256 << 20;
const LAUNCHER_JAR_MAX: u64 = 128 << 20;
const INSTALLER_JAR_MAX: u64 = 128 << 20;
const INSTALLER_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const QUILT_MAVEN_URL: &str = "https://maven.quiltmc.org/repository/release";

/// Adresses des services utilisés à l'installation (remplaçables dans les tests).
#[derive(Debug, Clone)]
pub struct ServerSources {
    pub manifest_url: String,
    pub fabric_meta_url: String,
    pub quilt_meta_url: String,
    pub quilt_maven_url: String,
}

impl Default for ServerSources {
    fn default() -> Self {
        Self {
            manifest_url: DEFAULT_MANIFEST_URL.into(),
            fabric_meta_url: crate::loader::fabric::DEFAULT_META_URL.into(),
            quilt_meta_url: crate::loader::quilt::DEFAULT_META_URL.into(),
            quilt_maven_url: QUILT_MAVEN_URL.into(),
        }
    }
}

/// Ce qu'il faut installer et où.
#[derive(Clone)]
pub struct InstallRequest {
    /// Dossier du serveur (`hearth/<groupe>/server`).
    pub dir: PathBuf,
    pub minecraft: String,
    /// Version du loader vide : la plus récente stable (recommandée pour Forge).
    pub loader: LoaderSpec,
    /// Dossier des Java du launcher (recherche et installation).
    pub java_root: PathBuf,
    /// Java imposé par l'utilisateur, au lieu de la recherche.
    pub java_override: Option<PathBuf>,
    /// Dossier où garder les jars serveur Mojang téléchargés (nommés par leur SHA-1) ; `None` : pas de cache.
    pub jar_cache: Option<PathBuf>,
    /// Étapes en cours, en français, pour l'affichage.
    pub progress: Option<LineSink>,
}

/// Serveur installé : de quoi le lancer. Enregistré dans `<dir>/.cubelink-server.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledServer {
    pub dir: PathBuf,
    /// `<loader>-<minecraft>-<version>` : une autre clé réinstalle.
    pub key: String,
    pub minecraft: String,
    /// Version exacte du loader (résolue).
    pub loader_version: String,
    /// Version majeure de Java requise.
    pub java_major: u32,
    /// Arguments Java après les options de mémoire, relatifs à `dir` (ex. `-jar server.jar nogui`).
    pub args: Vec<String>,
    /// Fichiers (relatifs à `dir`) qui doivent exister pour que l'installation soit valide.
    pub required_files: Vec<String>,
}

fn loader_name(l: Loader) -> &'static str {
    match l {
        Loader::Vanilla => "vanilla",
        Loader::Fabric => "fabric",
        Loader::Quilt => "quilt",
        Loader::NeoForge => "neoforge",
        Loader::Forge => "forge",
    }
}

/// Jeton sûr pour un chemin d'URL ou un argument : lettres, chiffres, `. - _ +`.
fn check_token(what: &str, s: &str) -> Result<()> {
    if s.is_empty() || s.len() > 64 || !s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'+')) {
        return Err(Error::Invalid(format!("{what} invalide « {s} »")));
    }
    Ok(())
}

/// Un message d'avancement affichable, ou une ligne de journal.
pub type LineSink = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Deserialize)]
struct ServerVersionJson {
    downloads: ServerDownloads,
    #[serde(default, rename = "javaVersion")]
    java_version: Option<JavaVersion>,
}

#[derive(Deserialize)]
struct ServerDownloads {
    #[serde(default)]
    server: Option<DownloadInfo>,
}

/// Installe des serveurs dédiés.
pub struct ServerInstaller {
    http: reqwest::Client,
    sources: ServerSources,
}

impl Default for ServerInstaller {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerInstaller {
    pub fn new() -> Self {
        Self::with_sources(ServerSources::default())
    }

    pub fn with_sources(sources: ServerSources) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(600))
            .user_agent("Cubelink")
            .build()
            .unwrap_or_default();
        Self { http, sources }
    }

    /// Installe (ou réutilise) le serveur de `req`. Idempotent : une installation valide pour la même clé est
    /// rendue telle quelle, le monde, les mods et la configuration ne sont jamais touchés.
    pub async fn install(&self, req: &InstallRequest) -> Result<InstalledServer> {
        check_token("version Minecraft", &req.minecraft)?;
        if req.loader.loader != Loader::Vanilla {
            check_token("version du loader", &req.loader.version).or_else(|e| if req.loader.version.is_empty() { Ok(()) } else { Err(e) })?;
        }
        let say = |m: &str| {
            if let Some(p) = &req.progress {
                p(m);
            }
        };
        tokio::fs::create_dir_all(&req.dir).await?;
        let loader_version = self.resolve_loader_version(&req.minecraft, &req.loader).await?;
        let key = format!("{}-{}-{}", loader_name(req.loader.loader), req.minecraft, loader_version);
        if let Some(rec) = read_record(&req.dir).await {
            if rec.key == key && rec.required_files.iter().all(|f| req.dir.join(f).is_file()) {
                return Ok(rec);
            }
        }

        say(&format!("Lecture des versions Minecraft {}…", req.minecraft));
        let (server_dl, declared_java) = self.vanilla_meta(&req.minecraft).await?;
        let java_major = required_java_major(&req.minecraft, declared_java);
        remove_install_artifacts(&req.dir).await?;

        let (args, required_files) = match req.loader.loader {
            Loader::Vanilla => {
                say("Téléchargement du serveur Minecraft…");
                self.place_vanilla_jar(&server_dl, req, "server.jar").await?;
                (vec!["-jar".into(), "server.jar".into(), "nogui".into()], vec!["server.jar".to_string()])
            }
            Loader::Fabric => self.install_fabric(req, &loader_version, &server_dl, &say).await?,
            Loader::Quilt => {
                let java = resolve_java(&req.java_root, java_major, req.java_override.as_deref()).await?;
                self.install_quilt(req, &loader_version, &java, &say).await?
            }
            Loader::Forge | Loader::NeoForge => {
                let java = resolve_java(&req.java_root, java_major, req.java_override.as_deref()).await?;
                self.install_forge_like(req, &loader_version, &java, &say).await?
            }
        };
        let rec = InstalledServer {
            dir: req.dir.clone(),
            key,
            minecraft: req.minecraft.clone(),
            loader_version,
            java_major,
            args,
            required_files,
        };
        let body = serde_json::to_vec_pretty(&rec).map_err(|e| Error::Other(e.to_string()))?;
        write_atomic(&req.dir.join(RECORD_FILE), &body).await?;
        Ok(rec)
    }

    async fn resolve_loader_version(&self, mc: &str, spec: &LoaderSpec) -> Result<String> {
        if !spec.version.is_empty() || spec.loader == Loader::Vanilla {
            return Ok(spec.version.clone());
        }
        let none = |l: &str| Error::NotFound(format!("aucune version stable de {l} pour Minecraft {mc}"));
        match spec.loader {
            Loader::Vanilla => Ok(String::new()),
            Loader::Fabric => {
                let c = FabricClient::with_meta_url(&self.sources.fabric_meta_url);
                crate::loader::fabric::latest_stable(&c.list_loader_versions(mc).await?).map(|v| v.version.clone()).ok_or_else(|| none("Fabric"))
            }
            Loader::Quilt => {
                let c = QuiltClient::with_meta_url(&self.sources.quilt_meta_url);
                crate::loader::quilt::latest_stable(&c.list_loader_versions(mc).await?).map(|v| v.version.clone()).ok_or_else(|| none("Quilt"))
            }
            Loader::Forge => {
                let c = ForgeClient::new();
                match c.recommended(mc).await {
                    Ok(v) => Ok(v),
                    Err(Error::NotFound(_)) => c.latest(mc).await,
                    Err(e) => Err(e),
                }
            }
            Loader::NeoForge => NeoForgeClient::new().latest_stable(mc).await,
        }
    }

    /// Jar serveur Mojang et version de Java requise, d'après le manifeste officiel (SHA-1 du JSON vérifié).
    async fn vanilla_meta(&self, mc: &str) -> Result<(DownloadInfo, Option<u32>)> {
        let manifest = VersionsClient::with_manifest_url(&self.sources.manifest_url).fetch_manifest().await?;
        let entry = manifest.find(mc)?;
        require_https(&entry.url, &self.sources.manifest_url)?;
        let bytes = get_limited(&self.http, &entry.url, VERSION_JSON_MAX as u64).await?;
        let actual = sha1_hex(&bytes);
        if !actual.eq_ignore_ascii_case(&entry.sha1) {
            return Err(Error::Integrity { expected: entry.sha1.to_lowercase(), actual });
        }
        let v: ServerVersionJson = serde_json::from_slice(&bytes).map_err(|e| Error::Network(format!("JSON de version {mc} invalide : {e}")))?;
        let dl = v.downloads.server.ok_or_else(|| Error::NotFound(format!("serveur Minecraft {mc} (pas de jar serveur publié)")))?;
        Ok((dl, v.java_version.map(|j| j.major_version)))
    }

    /// Place le jar serveur vanilla sous `<dir>/<name>` (cache par SHA-1, taille et SHA-1 vérifiés).
    async fn place_vanilla_jar(&self, dl: &DownloadInfo, req: &InstallRequest, name: &str) -> Result<()> {
        let sha1 = dl.sha1.to_ascii_lowercase();
        if sha1.len() != 40 || !sha1.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::Invalid("SHA-1 du serveur Mojang mal formé".into()));
        }
        let dest = req.dir.join(name);
        let cached = req.jar_cache.as_ref().map(|c| c.join(format!("server-{sha1}.jar")));
        if let Some(c) = &cached {
            if file_sha1_matches(c, &sha1).await {
                tokio::fs::copy(c, &dest).await?;
                return Ok(());
            }
        }
        require_https(&dl.url, &self.sources.manifest_url)?;
        let bytes = get_limited(&self.http, &dl.url, SERVER_JAR_MAX).await?;
        let actual = sha1_hex(&bytes);
        if actual != sha1 || (dl.size != 0 && bytes.len() as u64 != dl.size) {
            return Err(Error::Integrity { expected: sha1, actual });
        }
        write_atomic(&dest, &bytes).await?;
        if let Some(c) = &cached {
            if let Some(parent) = c.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            // Le cache est une commodité : son échec n'arrête pas l'installation.
            let _ = write_atomic(c, &bytes).await;
        }
        Ok(())
    }

    async fn install_fabric(
        &self,
        req: &InstallRequest,
        loader_version: &str,
        server_dl: &DownloadInfo,
        say: &dyn Fn(&str),
    ) -> Result<(Vec<String>, Vec<String>)> {
        #[derive(Deserialize)]
        struct Inst {
            version: String,
            #[serde(default)]
            stable: bool,
        }
        check_token("version du loader", loader_version)?;
        say("Recherche de l'installeur Fabric…");
        let url = format!("{}/v2/versions/installer", self.sources.fabric_meta_url.trim_end_matches('/'));
        let list: Vec<Inst> = serde_json::from_slice(&get_limited(&self.http, &url, LIST_MAX as u64).await?)
            .map_err(|e| Error::Network(format!("liste des installeurs Fabric invalide : {e}")))?;
        let installer = list.iter().find(|i| i.stable).or(list.first()).ok_or_else(|| Error::NotFound("installeur Fabric".into()))?;
        check_token("version de l'installeur Fabric", &installer.version)?;
        say("Téléchargement du lanceur de serveur Fabric…");
        let url = format!(
            "{}/v2/versions/loader/{}/{}/{}/server/jar",
            self.sources.fabric_meta_url.trim_end_matches('/'),
            req.minecraft,
            loader_version,
            installer.version
        );
        let jar = get_limited(&self.http, &url, LAUNCHER_JAR_MAX).await.map_err(|e| match e {
            Error::NotFound(_) => Error::NotFound(format!("Fabric {loader_version} pour Minecraft {}", req.minecraft)),
            other => other,
        })?;
        if !jar.starts_with(b"PK") {
            return Err(Error::Invalid("le lanceur Fabric reçu n'est pas un jar".into()));
        }
        write_atomic(&req.dir.join("fabric-server-launch.jar"), &jar).await?;
        // Le jar vanilla (vérifié, mis en cache) est donné au lanceur : il n'a rien à télécharger lui-même.
        say("Téléchargement du serveur Minecraft…");
        self.place_vanilla_jar(server_dl, req, "server.jar").await?;
        tokio::fs::write(req.dir.join("fabric-server-launcher.properties"), "serverJar=server.jar\n").await?;
        Ok((
            vec!["-jar".into(), "fabric-server-launch.jar".into(), "nogui".into()],
            vec!["fabric-server-launch.jar".into(), "server.jar".into()],
        ))
    }

    async fn install_quilt(
        &self,
        req: &InstallRequest,
        loader_version: &str,
        java: &Path,
        say: &dyn Fn(&str),
    ) -> Result<(Vec<String>, Vec<String>)> {
        #[derive(Deserialize)]
        struct Inst {
            version: String,
        }
        check_token("version du loader", loader_version)?;
        say("Recherche de l'installeur Quilt…");
        let url = format!("{}/v3/versions/installer", self.sources.quilt_meta_url.trim_end_matches('/'));
        let list: Vec<Inst> = serde_json::from_slice(&get_limited(&self.http, &url, LIST_MAX as u64).await?)
            .map_err(|e| Error::Network(format!("liste des installeurs Quilt invalide : {e}")))?;
        let installer = list
            .iter()
            .find(|i| !i.version.contains('-'))
            .or(list.first())
            .ok_or_else(|| Error::NotFound("installeur Quilt".into()))?;
        check_token("version de l'installeur Quilt", &installer.version)?;
        // L'adresse est reconstruite (pas reprise de la réponse de l'API) : elle ne peut viser que ce Maven.
        let v = &installer.version;
        let base = self.sources.quilt_maven_url.trim_end_matches('/');
        let jar_url = format!("{base}/org/quiltmc/quilt-installer/{v}/quilt-installer-{v}.jar");
        say("Téléchargement de l'installeur Quilt…");
        let installer_jar = self.download_installer_jar(&jar_url, req).await?;
        say("Installation du serveur Quilt…");
        let dir_arg = format!("--install-dir={}", req.dir.display());
        let args = ["-jar".to_string(), installer_jar.display().to_string(), "install".into(), "server".into(), req.minecraft.clone(), loader_version.into(), "--download-server".into(), dir_arg];
        let res = run_installer(java, &args, &req.dir).await;
        let _ = tokio::fs::remove_file(&installer_jar).await;
        res?;
        let launch = "quilt-server-launch.jar";
        if !req.dir.join(launch).is_file() {
            return Err(Error::Other("l'installeur Quilt n'a pas créé quilt-server-launch.jar".into()));
        }
        Ok((vec!["-jar".into(), launch.into(), "nogui".into()], vec![launch.into()]))
    }

    /// Télécharge un installeur Maven avec son `.sha1` dans `<dir>/.installateur/`, et rend son chemin.
    async fn download_installer_jar(&self, url: &str, req: &InstallRequest) -> Result<PathBuf> {
        let bytes = get_limited(&self.http, url, INSTALLER_JAR_MAX).await?;
        let sha = get_limited(&self.http, &format!("{url}.sha1"), 4096).await?;
        let expected = String::from_utf8_lossy(&sha).split_whitespace().next().unwrap_or("").to_lowercase();
        let actual = sha1_hex(&bytes);
        if expected.is_empty() || expected != actual {
            return Err(Error::Integrity { expected, actual });
        }
        let dir = req.dir.join(".installateur");
        tokio::fs::create_dir_all(&dir).await?;
        let path = dir.join("installer.jar");
        write_atomic(&path, &bytes).await?;
        Ok(path)
    }

    async fn install_forge_like(
        &self,
        req: &InstallRequest,
        loader_version: &str,
        java: &Path,
        say: &dyn Fn(&str),
    ) -> Result<(Vec<String>, Vec<String>)> {
        check_token("version du loader", loader_version)?;
        let dir = req.dir.join(".installateur");
        tokio::fs::create_dir_all(&dir).await?;
        let installer = dir.join("installer.jar");
        say("Téléchargement de l'installeur…");
        match req.loader.loader {
            Loader::Forge => ForgeClient::new().download_installer(&req.minecraft, loader_version, &installer).await?,
            _ => NeoForgeClient::new().download_installer(loader_version, &installer).await?,
        }
        say("Installation du serveur (quelques minutes)…");
        let args = ["-jar".to_string(), installer.display().to_string(), "--installServer".into()];
        let res = run_installer(java, &args, &req.dir).await;
        let _ = tokio::fs::remove_dir_all(&dir).await;
        let _ = tokio::fs::remove_file(req.dir.join("installer.jar.log")).await;
        res?;
        let dir = req.dir.clone();
        let (loader, mc, ver) = (req.loader.loader, req.minecraft.clone(), loader_version.to_string());
        let found = tokio::task::spawn_blocking(move || detect_forge_launch(&dir, loader, &mc, &ver))
            .await
            .map_err(|e| Error::Other(e.to_string()))?;
        found.ok_or_else(|| Error::Other("l'installeur n'a produit aucun lanceur de serveur reconnu".into()))
    }
}

/// Trouve comment lancer un serveur Forge ou NeoForge installé : fichier d'arguments (1.17+) ou jar (plus ancien).
/// Rend `(arguments Java, fichiers requis)`.
pub fn detect_forge_launch(dir: &Path, loader: Loader, mc: &str, version: &str) -> Option<(Vec<String>, Vec<String>)> {
    let platform = if cfg!(windows) { "win_args.txt" } else { "unix_args.txt" };
    let rel = match loader {
        Loader::Forge => format!("libraries/net/minecraftforge/forge/{mc}-{version}/{platform}"),
        Loader::NeoForge => format!("libraries/net/neoforged/neoforge/{version}/{platform}"),
        _ => return None,
    };
    if dir.join(&rel).is_file() {
        return Some((vec![format!("@{rel}"), "nogui".into()], vec![rel]));
    }
    // Forge ancien (1.12 à 1.16) : un jar à la racine, lancé avec `-jar`.
    let mut jars: Vec<String> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| {
            let l = n.to_ascii_lowercase();
            l.starts_with("forge-") && l.ends_with(".jar") && !l.contains("installer") && !l.contains("shim")
        })
        .collect();
    jars.sort();
    // `forge-<mc>-<ver>.jar` avant `-universal`.
    let jar = jars.iter().find(|n| !n.to_ascii_lowercase().contains("universal")).or(jars.first())?.clone();
    Some((vec!["-jar".into(), jar.clone(), "nogui".into()], vec![jar]))
}

async fn read_record(dir: &Path) -> Option<InstalledServer> {
    let bytes = tokio::fs::read(dir.join(RECORD_FILE)).await.ok()?;
    let mut rec: InstalledServer = serde_json::from_slice(&bytes).ok()?;
    // Le dossier de l'enregistrement peut avoir été déplacé : celui de l'appelant fait foi.
    rec.dir = dir.to_path_buf();
    Some(rec)
}

/// Retire ce qui vient d'une installation précédente (jars à la racine, `libraries`, `versions`, `.fabric`).
/// Le monde, les mods, la configuration, `eula.txt` et `server.properties` restent.
async fn remove_install_artifacts(dir: &Path) -> Result<()> {
    let mut rd = tokio::fs::read_dir(dir).await?;
    while let Some(e) = rd.next_entry().await? {
        let name = e.file_name().to_string_lossy().into_owned();
        let ty = e.file_type().await?;
        let is_jar = ty.is_file() && name.to_ascii_lowercase().ends_with(".jar");
        let is_dir = ty.is_dir() && matches!(name.as_str(), "libraries" | "versions" | ".fabric" | ".installateur");
        let is_record = ty.is_file() && (name == RECORD_FILE || name == "fabric-server-launcher.properties");
        if is_jar || is_record {
            tokio::fs::remove_file(e.path()).await?;
        } else if is_dir {
            tokio::fs::remove_dir_all(e.path()).await?;
        }
    }
    Ok(())
}

/// Exécute un installeur Java (sans fenêtre de console) ; en cas d'échec, l'erreur porte la fin de sa sortie.
async fn run_installer(java: &Path, args: &[String], cwd: &Path) -> Result<()> {
    let mut cmd = Command::new(java);
    cmd.args(args).current_dir(cwd).stdin(Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let out = tokio::time::timeout(INSTALLER_TIMEOUT, cmd.output())
        .await
        .map_err(|_| Error::Other("l'installeur du serveur met trop de temps (20 min)".into()))?
        .map_err(|e| Error::Other(format!("impossible de lancer Java : {e}")))?;
    if out.status.success() {
        return Ok(());
    }
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    let tail: Vec<&str> = text.lines().rev().take(12).collect();
    let tail: Vec<&str> = tail.into_iter().rev().collect();
    Err(Error::Other(format!("l'installeur du serveur a échoué ({}) :\n{}", out.status, tail.join("\n"))))
}

fn require_https(url: &str, reference: &str) -> Result<()> {
    // Le serveur de test local est en `http` : on accepte le schéma de l'adresse de référence.
    let ok = url.starts_with("https://") || (reference.starts_with("http://") && url.starts_with("http://"));
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(format!("adresse de téléchargement refusée : {url}")))
    }
}

/// GET en mémoire borné à `limit` octets (en-tête `Content-Length` puis flux).
async fn get_limited(http: &reqwest::Client, url: &str, limit: u64) -> Result<Vec<u8>> {
    let mut resp = http.get(url).send().await.map_err(|e| Error::Network(e.to_string()))?;
    let status = resp.status();
    if status.as_u16() == 404 {
        return Err(Error::NotFound(url.to_string()));
    }
    if !status.is_success() {
        return Err(Error::Network(format!("HTTP {} pour {url}", status.as_u16())));
    }
    if resp.content_length().is_some_and(|l| l > limit) {
        return Err(Error::Invalid(format!("réponse trop volumineuse pour {url}")));
    }
    let mut out = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| Error::Network(e.to_string()))? {
        if (out.len() + chunk.len()) as u64 > limit {
            return Err(Error::Invalid(format!("réponse trop volumineuse pour {url}")));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
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

async fn file_sha1_matches(path: &Path, sha1: &str) -> bool {
    match tokio::fs::read(path).await {
        Ok(b) => sha1_hex(&b) == sha1,
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Serveur en marche
// ---------------------------------------------------------------------------------------------------------------

/// Nombre de lignes de sortie gardées en mémoire.
const KEEP_LINES: usize = 2000;
/// Longueur maximale d'une ligne gardée (octets).
const MAX_LINE: usize = 4096;
const SAVE_OFF_TIMEOUT: Duration = Duration::from_secs(30);
const SAVE_FLUSH_TIMEOUT: Duration = Duration::from_secs(180);
const SAVE_ON_TIMEOUT: Duration = Duration::from_secs(30);

/// Réglages du lancement.
#[derive(Clone)]
pub struct ServerConfig {
    pub dir: PathBuf,
    /// Exécutable Java (voir [`resolve_java`]).
    pub java: PathBuf,
    pub ram_mb: u32,
    /// Port d'écoute local ; 0 : un port libre est choisi.
    pub port: u16,
    /// L'utilisateur a accepté l'EULA de Minecraft : `eula.txt` est alors écrit. Sinon il doit déjà exister.
    pub accept_eula: bool,
    pub extra_jvm_args: Vec<String>,
    /// Délai maximal avant `Done (`.
    pub ready_timeout: Duration,
    /// Reçoit chaque ligne de la sortie du serveur (journal).
    pub on_line: Option<LineSink>,
}

impl ServerConfig {
    pub fn new(dir: impl Into<PathBuf>, java: impl Into<PathBuf>, ram_mb: u32) -> Self {
        Self {
            dir: dir.into(),
            java: java.into(),
            ram_mb,
            port: 0,
            accept_eula: false,
            extra_jvm_args: Vec::new(),
            ready_timeout: Duration::from_secs(300),
            on_line: None,
        }
    }
}

/// Comment le serveur s'est arrêté.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopOutcome {
    /// Il a fallu le tuer : `stop` n'a pas suffi dans le délai.
    pub forced: bool,
    pub exit_code: Option<i32>,
}

struct Lines {
    next_seq: u64,
    buf: VecDeque<(u64, String)>,
}

struct Shared {
    lines: Mutex<Lines>,
    seq_tx: watch::Sender<u64>,
    exit_tx: watch::Sender<Option<i32>>,
    sink: Option<LineSink>,
}

impl Shared {
    fn push(&self, line: String) {
        if let Some(s) = &self.sink {
            s(&line);
        }
        let mut g = self.lines.lock().unwrap_or_else(|e| e.into_inner());
        let seq = g.next_seq;
        g.next_seq += 1;
        if g.buf.len() >= KEEP_LINES {
            g.buf.pop_front();
        }
        g.buf.push_back((seq, line));
        let next = g.next_seq;
        drop(g);
        self.seq_tx.send_replace(next);
    }

    fn next_seq(&self) -> u64 {
        self.lines.lock().unwrap_or_else(|e| e.into_inner()).next_seq
    }

    fn find_from(&self, from: u64, pred: &dyn Fn(&str) -> bool) -> (Option<String>, u64) {
        let g = self.lines.lock().unwrap_or_else(|e| e.into_inner());
        let hit = g.buf.iter().filter(|(s, _)| *s >= from).find(|(_, l)| pred(l)).map(|(_, l)| l.clone());
        (hit, g.next_seq)
    }

    fn tail(&self, n: usize) -> Vec<String> {
        let g = self.lines.lock().unwrap_or_else(|e| e.into_inner());
        let skip = g.buf.len().saturating_sub(n);
        g.buf.iter().skip(skip).map(|(_, l)| l.clone()).collect()
    }
}

/// Serveur dédié en marche. L'abandonner (`drop`) le tue ; pour le fermer proprement : [`DedicatedServer::stop`].
pub struct DedicatedServer {
    pid: u32,
    port: u16,
    dir: PathBuf,
    shared: Arc<Shared>,
    stdin: tokio::sync::Mutex<Option<ChildStdin>>,
    kill_tx: mpsc::Sender<()>,
}

impl DedicatedServer {
    /// Écrit la configuration, lance le serveur et attend qu'il soit prêt (`Done (`).
    ///
    /// Si le serveur s'arrête avant, ou dépasse `ready_timeout`, l'erreur porte les dernières lignes de sa sortie et,
    /// si on le devine, le mod en cause.
    pub async fn start(cfg: &ServerConfig, installed: &InstalledServer) -> Result<DedicatedServer> {
        let server = Self::spawn(cfg, installed).await?;
        if let Err(e) = server.wait_ready(cfg.ready_timeout).await {
            server.kill().await;
            return Err(e);
        }
        Ok(server)
    }

    /// Comme [`DedicatedServer::start`], sans attendre que le serveur soit prêt.
    pub async fn spawn(cfg: &ServerConfig, installed: &InstalledServer) -> Result<DedicatedServer> {
        if !(512..=1 << 20).contains(&cfg.ram_mb) {
            return Err(Error::Invalid(format!("mémoire du serveur invalide : {} Mo", cfg.ram_mb)));
        }
        if !cfg.java.is_file() {
            return Err(Error::NotFound("Java du serveur".into()));
        }
        if installed.args.is_empty() {
            return Err(Error::Invalid("serveur non installé".into()));
        }
        let dir = cfg.dir.clone();
        tokio::fs::create_dir_all(&dir).await?;
        if cfg.accept_eula {
            tokio::fs::write(dir.join("eula.txt"), eula_text()).await?;
        } else if !eula_accepted(&dir) {
            return Err(Error::Invalid("l'EULA de Minecraft n'a pas été acceptée".into()));
        }
        let port = if cfg.port == 0 { free_port()? } else { cfg.port };
        let props_path = dir.join("server.properties");
        let existing = tokio::fs::read_to_string(&props_path).await.unwrap_or_default();
        tokio::fs::write(&props_path, render_server_properties(&existing, port)).await?;

        let mut args: Vec<String> = vec![format!("-Xmx{}M", cfg.ram_mb), format!("-Xms{}M", cfg.ram_mb.min(1024))];
        args.extend(cfg.extra_jvm_args.iter().cloned());
        args.extend(installed.args.iter().cloned());
        let mut cmd = Command::new(&cfg.java);
        cmd.args(&args)
            .current_dir(&dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let mut child = cmd.spawn().map_err(|e| Error::Other(format!("impossible de lancer le serveur : {e}")))?;
        let pid = child.id().unwrap_or(0);
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let (seq_tx, _) = watch::channel(0u64);
        let (exit_tx, _) = watch::channel(None);
        let shared = Arc::new(Shared {
            lines: Mutex::new(Lines { next_seq: 0, buf: VecDeque::new() }),
            seq_tx,
            exit_tx,
            sink: cfg.on_line.clone(),
        });
        let mut readers = Vec::new();
        if let Some(o) = stdout {
            readers.push(tokio::spawn(read_lines(o, shared.clone())));
        }
        if let Some(e) = stderr {
            readers.push(tokio::spawn(read_lines(e, shared.clone())));
        }
        let (kill_tx, mut kill_rx) = mpsc::channel::<()>(1);
        let sh = shared.clone();
        tokio::spawn(async move {
            let status = tokio::select! {
                s = child.wait() => s,
                // Demande d'arrêt forcé, ou `DedicatedServer` abandonné.
                _ = kill_rx.recv() => {
                    let _ = child.start_kill();
                    child.wait().await
                }
            };
            // Les dernières lignes sont lues avant d'annoncer la fin (les tubes peuvent rester ouverts : délai).
            for r in readers {
                let _ = tokio::time::timeout(Duration::from_secs(2), r).await;
            }
            sh.exit_tx.send_replace(Some(status.ok().and_then(|s| s.code()).unwrap_or(-1)));
        });
        Ok(DedicatedServer { pid, port, dir, shared, stdin: tokio::sync::Mutex::new(stdin), kill_tx })
    }

    /// PID du processus Java.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Port d'écoute local (127.0.0.1).
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Dossier du serveur.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Dossier du monde (`level-name=world`).
    pub fn world_dir(&self) -> PathBuf {
        self.dir.join("world")
    }

    pub fn is_running(&self) -> bool {
        self.shared.exit_tx.borrow().is_none()
    }

    /// Code de sortie, une fois le serveur arrêté.
    pub fn exit_code(&self) -> Option<i32> {
        *self.shared.exit_tx.borrow()
    }

    /// Les `n` dernières lignes de la sortie.
    pub fn recent_lines(&self, n: usize) -> Vec<String> {
        self.shared.tail(n)
    }

    /// Attend la fin du processus et rend son code de sortie.
    pub async fn wait_exit(&self) -> i32 {
        let mut rx = self.shared.exit_tx.subscribe();
        let code = match rx.wait_for(|c| c.is_some()).await {
            Ok(c) => c.unwrap_or(-1),
            Err(_) => -1,
        };
        code
    }

    /// Attend `Done (` dans la sortie.
    pub async fn wait_ready(&self, timeout: Duration) -> Result<()> {
        match self.wait_line(0, &is_ready_line, timeout).await {
            Ok(_) => Ok(()),
            Err(WaitError::Exited(code)) => Err(Error::Other(self.failure_text(&format!("le serveur s'est arrêté au démarrage (code {code})")))),
            Err(WaitError::Timeout) => {
                Err(Error::Other(self.failure_text(&format!("le serveur n'est pas prêt après {} s", timeout.as_secs()))))
            }
        }
    }

    fn failure_text(&self, head: &str) -> String {
        let lines = self.shared.tail(KEEP_LINES);
        let mut s = head.to_string();
        if let Some(m) = diagnose_failure(&lines) {
            s.push_str(&format!(" ; mod probablement en cause : {m}"));
        }
        let tail: Vec<String> = lines.iter().rev().take(10).rev().cloned().collect();
        if !tail.is_empty() {
            s.push_str(&format!("\n{}", tail.join("\n")));
        }
        s
    }

    async fn wait_line(&self, from: u64, pred: &dyn Fn(&str) -> bool, timeout: Duration) -> std::result::Result<String, WaitError> {
        let mut seq_rx = self.shared.seq_tx.subscribe();
        let mut exit_rx = self.shared.exit_tx.subscribe();
        let deadline = Instant::now() + timeout;
        let mut scanned = from;
        loop {
            let (hit, next) = self.shared.find_from(scanned, pred);
            if let Some(l) = hit {
                return Ok(l);
            }
            scanned = next;
            if let Some(code) = *exit_rx.borrow() {
                // La fin est publiée après la lecture des dernières lignes : cette passe était la dernière.
                return Err(WaitError::Exited(code));
            }
            tokio::select! {
                _ = seq_rx.changed() => {}
                _ = exit_rx.changed() => {}
                _ = tokio::time::sleep_until(deadline) => return Err(WaitError::Timeout),
            }
        }
    }

    async fn send(&self, command: &str) -> Result<()> {
        let mut g = self.stdin.lock().await;
        let stdin = g.as_mut().ok_or_else(|| Error::Other("le serveur n'écoute plus ses commandes".into()))?;
        let line = format!("{command}\n");
        let res = tokio::time::timeout(Duration::from_secs(10), async {
            stdin.write_all(line.as_bytes()).await?;
            stdin.flush().await
        })
        .await;
        match res {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => {
                *g = None;
                Err(Error::Other(format!("commande « {command} » non envoyée : {e}")))
            }
            Err(_) => Err(Error::Other(format!("commande « {command} » non envoyée : délai dépassé"))),
        }
    }

    /// Envoie `command` et attend la ligne de confirmation `ok` (reconnue sur le message du journal).
    async fn command_expect(&self, command: &str, ok: &dyn Fn(&str) -> bool, timeout: Duration) -> Result<()> {
        if !self.is_running() {
            return Err(Error::Other(format!("commande « {command} » impossible : le serveur est arrêté")));
        }
        // Le repère est pris AVANT l'envoi : la confirmation ne peut pas être manquée.
        let from = self.shared.next_seq();
        self.send(command).await?;
        match self.wait_line(from, ok, timeout).await {
            Ok(_) => Ok(()),
            Err(WaitError::Exited(code)) => Err(Error::Other(format!("le serveur s'est arrêté pendant « {command} » (code {code})"))),
            Err(WaitError::Timeout) => Err(Error::Other(format!("« {command} » : pas de confirmation du serveur après {} s", timeout.as_secs()))),
        }
    }

    /// `save-off` : coupe la sauvegarde automatique.
    pub async fn save_off(&self) -> Result<()> {
        self.command_expect("save-off", &is_save_off_line, SAVE_OFF_TIMEOUT).await
    }

    /// `save-all flush` : écrit tous les chunks sur le disque.
    pub async fn save_all_flush(&self) -> Result<()> {
        self.command_expect("save-all flush", &is_saved_line, SAVE_FLUSH_TIMEOUT).await
    }

    /// `save-on` : rétablit la sauvegarde automatique.
    pub async fn save_on(&self) -> Result<()> {
        self.command_expect("save-on", &is_save_on_line, SAVE_ON_TIMEOUT).await
    }

    /// Point de reprise : `save-off`, `save-all flush`, `copy`, `save-on`. La sauvegarde automatique est rétablie même
    /// si `copy` échoue (ou panique, la panique est alors relancée). L'erreur de `copy` prime sur celle de `save-on`.
    pub async fn checkpoint<F, Fut, T>(&self, copy: F) -> Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let first = async {
            self.save_off().await?;
            self.save_all_flush().await
        }
        .await;
        let outcome = match first {
            Ok(()) => AssertUnwindSafe(async { copy().await }).catch_unwind().await,
            Err(e) => Ok(Err(e)),
        };
        // Toujours tenté, même après un échec de `save-off` (la commande a pu partir sans que la réponse arrive).
        let on = self.save_on().await;
        match outcome {
            Err(panic) => std::panic::resume_unwind(panic),
            Ok(Err(e)) => Err(e),
            Ok(Ok(v)) => on.map(|()| v),
        }
    }

    /// Arrêt propre : `stop`, puis, si le serveur n'est pas parti après `grace`, arrêt forcé.
    pub async fn stop(&self, grace: Duration) -> StopOutcome {
        if let Some(code) = self.exit_code() {
            return StopOutcome { forced: false, exit_code: Some(code) };
        }
        let _ = self.send("stop").await;
        match tokio::time::timeout(grace, self.wait_exit()).await {
            Ok(code) => StopOutcome { forced: false, exit_code: Some(code) },
            Err(_) => {
                self.kill().await;
                StopOutcome { forced: true, exit_code: self.exit_code() }
            }
        }
    }

    /// Arrêt forcé immédiat (le monde peut perdre les dernières minutes).
    pub async fn kill(&self) {
        let _ = self.kill_tx.send(()).await;
        let _ = tokio::time::timeout(Duration::from_secs(10), self.wait_exit()).await;
    }
}

#[derive(Debug)]
enum WaitError {
    Exited(i32),
    Timeout,
}

/// Lit un tube ligne à ligne (UTF-8 avec remplacement, lignes tronquées à `MAX_LINE`).
async fn read_lines<R: tokio::io::AsyncRead + Unpin>(r: R, shared: Arc<Shared>) {
    let mut reader = BufReader::new(r);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        // Une ligne sans fin est coupée à 1 Mio de lecture : la mémoire reste bornée.
        let n = match (&mut reader).take(1 << 20).read_until(b'\n', &mut buf).await {
            Ok(n) => n,
            Err(_) => break,
        };
        if n == 0 {
            break;
        }
        buf.truncate(MAX_LINE);
        let text = String::from_utf8_lossy(&buf);
        let text = text.trim_end_matches(['\r', '\n']);
        if !text.is_empty() {
            shared.push(text.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn log_message_formats() {
        assert_eq!(log_message("[12:00:00] [Server thread/INFO]: Done (3.2s)! For help"), "Done (3.2s)! For help");
        assert_eq!(log_message("[12:00:00] [Server thread/INFO] (Minecraft) Done (3.2s)!"), "Done (3.2s)!");
        assert_eq!(log_message("[12:00:00] [Server thread/INFO] [minecraft/DedicatedServer]: Done (1s)!"), "[minecraft/DedicatedServer]: Done (1s)!");
        assert_eq!(log_message("Done (1s)!\r\n"), "Done (1s)!");
        assert_eq!(log_message("[12:00:00]"), "[12:00:00]");
        assert_eq!(log_message("[12:00:00] [Server thread/INFO]: <Alice> Done ("), "<Alice> Done (");
        assert_eq!(log_message(""), "");
    }

    #[test]
    fn ready_and_save_lines() {
        assert!(is_ready_line("[19:43:19] [Server thread/INFO]: Done (4.5s)! For help, type \"help\""));
        assert!(is_ready_line("[19:43:19] [Server thread/INFO] (Minecraft) Done (4.5s)! For help, type \"help\""));
        assert!(!is_ready_line("[19:43:19] [Server thread/INFO]: <Bob> Done (fake)"));
        assert!(!is_ready_line("[19:43:19] [Server thread/INFO]: Preparing spawn area: 80%"));
        assert!(is_saved_line("[19:43:19] [Server thread/INFO]: Saved the game"));
        assert!(is_saved_line("[19:43:19] [Server thread/INFO] (Minecraft) Saved the game"));
        assert!(is_saved_line("[19:43:19] [Server thread/INFO]: Saved the world"));
        assert!(!is_saved_line("[19:43:19] [Server thread/INFO]: <Bob> Saved the game"));
        assert!(!is_saved_line("[19:43:19] [Server thread/INFO]: Saving the game (this may take a moment!)"));
        assert!(is_save_off_line("[19:43:19] [Server thread/INFO]: Automatic saving is now disabled"));
        assert!(is_save_off_line("[19:43:19] [Server thread/INFO]: Saving is already turned off"));
        assert!(!is_save_off_line("[19:43:19] [Server thread/INFO]: Automatic saving is now enabled"));
        assert!(is_save_on_line("[19:43:19] [Server thread/INFO]: Automatic saving is now enabled"));
        assert!(is_save_on_line("[19:43:19] [Server thread/INFO]: Saving is already turned on"));
        assert!(!is_save_on_line("[19:43:19] [Server thread/INFO]: <Bob> Automatic saving is now enabled"));
    }

    fn lines(l: &[&str]) -> Vec<String> {
        l.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn failure_diagnosis() {
        let fabric = lines(&[
            "[12:00:00] [main/INFO] (FabricLoader) Loading 40 mods",
            "[12:00:01] [main/ERROR] (FabricLoader) Could not execute entrypoint stage 'main' due to errors, provided by 'create'!",
        ]);
        assert_eq!(diagnose_failure(&fabric).as_deref(), Some("create (point d'entrée en échec)"));
        let deps = lines(&["[12:00:01] [main/ERROR]: - Mod 'Fabric API' (fabric-api) 0.100.0 requires version 0.105 of mod 'x'"]);
        assert_eq!(diagnose_failure(&deps).as_deref(), Some("Fabric API (fabric-api)"));
        let neo = lines(&["[12:00:01] [main/ERROR]: Mod ID: 'jei', Requested by: 'foo', Expected range: '[19,)', Actual version: '18'"]);
        assert_eq!(diagnose_failure(&neo).as_deref(), Some("jei"));
        let mixin = lines(&["[12:00:01] [main/ERROR]: Mixin apply for mod sodium failed sodium.mixins.json"]);
        assert_eq!(diagnose_failure(&mixin).as_deref(), Some("sodium"));
        assert_eq!(diagnose_failure(&lines(&["[12:00:01] [main/INFO]: rien d'utile"])), None);
        assert_eq!(diagnose_failure(&[]), None);
    }

    #[test]
    fn eula_and_properties() {
        let t = tempfile::tempdir().unwrap();
        assert!(!eula_accepted(t.path()));
        std::fs::write(t.path().join("eula.txt"), "#x\neula=false\n").unwrap();
        assert!(!eula_accepted(t.path()));
        std::fs::write(t.path().join("eula.txt"), eula_text()).unwrap();
        assert!(eula_accepted(t.path()));

        let out = render_server_properties("", 25599);
        assert_eq!(out, "online-mode=true\nserver-ip=127.0.0.1\nserver-port=25599\nlevel-name=world\n");
        let existing = "#Minecraft server properties\nmotd=Chez nous\nonline-mode=false\nserver-port : 25565\nserver-ip=\nonline-mode=false\n#level-name=autre\nmax-players=5\n";
        let out = render_server_properties(existing, 4242);
        assert!(out.contains("motd=Chez nous\n"));
        assert!(out.contains("max-players=5\n"));
        assert!(out.contains("#level-name=autre\n"));
        assert_eq!(out.matches("online-mode=").count(), 1);
        assert!(out.contains("online-mode=true\n"));
        assert!(out.contains("server-ip=127.0.0.1\n"));
        assert!(out.contains("server-port=4242\n"));
        assert!(!out.contains("25565"));
        assert!(out.contains("level-name=world\n"));
        // Idempotent.
        assert_eq!(render_server_properties(&out, 4242), out);
    }

    #[test]
    fn ports_and_ram() {
        let p = free_port().unwrap();
        assert!(p > 0);
        assert_eq!(suggest_ram_mb(4 * 1024), 2048);
        assert_eq!(suggest_ram_mb(16 * 1024), 4096);
        assert_eq!(suggest_ram_mb(64 * 1024), 8192);
        assert_eq!(suggest_ram_mb(2 * 1024), 1024);
        assert!(ram_warning(8 * 1024).is_some());
        assert!(ram_warning(16 * 1024).is_none());
    }

    #[test]
    fn side_parsing_and_filtering() {
        assert_eq!(Side::parse("required"), Some(Side::Required));
        assert_eq!(Side::parse(" Unsupported "), Some(Side::Unsupported));
        assert_eq!(Side::parse("unknown"), None);
        let info = |name: &str, ids: &[&str], side: Option<Side>, client: bool| ModSideInfo {
            file_name: name.into(),
            mod_ids: ids.iter().map(|s| s.to_string()).collect(),
            server_side: side,
            declares_client_only: client,
        };
        let mods = vec![
            info("a.jar", &["lithium"], None, false),
            info("b.jar", &["whatever"], Some(Side::Unsupported), false),
            info("c.jar", &["Sodium"], None, false),
            info("d.jar", &["x"], None, true),
            // La donnée Modrinth explicite l'emporte sur la liste connue et sur le jar.
            info("e.jar", &["sodium"], Some(Side::Optional), true),
            info("f.jar", &["mystere"], Some(Side::Required), false),
        ];
        let f = filter_server_mods(&mods);
        assert_eq!(f.kept, vec!["a.jar", "e.jar", "f.jar"]);
        let skipped: Vec<&str> = f.skipped.iter().map(|s| s.file_name.as_str()).collect();
        assert_eq!(skipped, vec!["b.jar", "c.jar", "d.jar"]);
        assert!(f.skipped[1].reason.contains("sodium") || f.skipped[1].reason.contains("Sodium"));
    }

    fn make_jar(path: &Path, files: &[(&str, &str)]) {
        use std::io::Write;
        let mut w = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (n, d) in files {
            w.start_file(*n, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(d.as_bytes()).unwrap();
        }
        w.finish().unwrap();
    }

    #[test]
    fn jar_environment_and_sync() {
        let t = tempfile::tempdir().unwrap();
        let pack = t.path().join("pack");
        let srv = t.path().join("srv").join("mods");
        std::fs::create_dir_all(&pack).unwrap();
        make_jar(&pack.join("both.jar"), &[("fabric.mod.json", r#"{"schemaVersion":1,"id":"both","version":"1","environment":"*"}"#)]);
        make_jar(&pack.join("client.jar"), &[("fabric.mod.json", r#"{"schemaVersion":1,"id":"cl","version":"1","environment":"client"}"#)]);
        make_jar(&pack.join("known.jar"), &[("fabric.mod.json", r#"{"schemaVersion":1,"id":"sodium","version":"1"}"#)]);
        make_jar(&pack.join("quilt.jar"), &[("quilt.mod.json", r#"{"schema_version":1,"quilt_loader":{"id":"q","version":"1"},"minecraft":{"environment":"client"}}"#)]);
        make_jar(&pack.join("modrinth.jar"), &[("fabric.mod.json", r#"{"schemaVersion":1,"id":"m","version":"1"}"#)]);
        std::fs::write(pack.join("notes.txt"), "x").unwrap();
        // Reste d'une synchro précédente : retiré.
        std::fs::create_dir_all(&srv).unwrap();
        make_jar(&srv.join("stale.jar"), &[("fabric.mod.json", r#"{"schemaVersion":1,"id":"s","version":"1"}"#)]);

        assert!(jar_side_info(&pack.join("client.jar")).declares_client_only);
        assert!(!jar_side_info(&pack.join("both.jar")).declares_client_only);
        assert_eq!(jar_side_info(&pack.join("known.jar")).mod_ids, vec!["sodium".to_string()]);

        let mut side = HashMap::new();
        side.insert("modrinth.jar".to_string(), Side::Unsupported);
        let f = sync_server_mods(&pack, &srv, &side).unwrap();
        assert_eq!(f.kept, vec!["both.jar"]);
        let mut skipped: Vec<&str> = f.skipped.iter().map(|s| s.file_name.as_str()).collect();
        skipped.sort();
        assert_eq!(skipped, vec!["client.jar", "known.jar", "modrinth.jar", "quilt.jar"]);
        let mut present: Vec<String> = std::fs::read_dir(&srv).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        present.sort();
        assert_eq!(present, vec!["both.jar"]);
        // Deuxième passage : même résultat.
        assert_eq!(sync_server_mods(&pack, &srv, &side).unwrap(), f);
        // Dossier du pack absent : serveur sans mods.
        assert_eq!(sync_server_mods(&t.path().join("absent"), &srv, &side).unwrap(), ModFilter::default());
        assert!(std::fs::read_dir(&srv).unwrap().next().is_none());
    }

    #[test]
    fn forge_launch_detection() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        assert_eq!(detect_forge_launch(d, Loader::Forge, "1.20.1", "47.2.0"), None);
        let platform = if cfg!(windows) { "win_args.txt" } else { "unix_args.txt" };
        let rel = format!("libraries/net/neoforged/neoforge/21.1.77/{platform}");
        std::fs::create_dir_all(d.join(&rel).parent().unwrap()).unwrap();
        std::fs::write(d.join(&rel), "x").unwrap();
        let (args, files) = detect_forge_launch(d, Loader::NeoForge, "1.21.1", "21.1.77").unwrap();
        assert_eq!(args, vec![format!("@{rel}"), "nogui".to_string()]);
        assert_eq!(files, vec![rel]);
        // Forge ancien : jar à la racine.
        std::fs::write(d.join("forge-1.12.2-14.23.5.2860-universal.jar"), "x").unwrap();
        std::fs::write(d.join("forge-1.12.2-14.23.5.2860-installer.jar"), "x").unwrap();
        let (args, _) = detect_forge_launch(d, Loader::Forge, "1.12.2", "14.23.5.2860").unwrap();
        assert_eq!(args, vec!["-jar", "forge-1.12.2-14.23.5.2860-universal.jar", "nogui"]);
        assert_eq!(detect_forge_launch(d, Loader::Fabric, "1.21.1", "x"), None);
    }

    #[test]
    fn tokens_are_checked() {
        assert!(check_token("v", "1.21.1").is_ok());
        assert!(check_token("v", "21.1.77-beta").is_ok());
        for bad in ["", "../x", "a b", "a/b", "a?b=c", "é"] {
            assert!(check_token("v", bad).is_err(), "{bad:?}");
        }
        assert!(check_token("v", &"1".repeat(65)).is_err());
    }

    /// Serveur HTTP simulé : manifeste, JSON de version, jar serveur, API Fabric.
    async fn mock_world(server: &MockServer, jar: &[u8]) -> ServerSources {
        let jar_sha = sha1_hex(jar);
        let version_json = serde_json::json!({
            "downloads": {"server": {"url": format!("{}/dl/server.jar", server.uri()), "sha1": jar_sha, "size": jar.len()}},
            "javaVersion": {"majorVersion": 21}
        })
        .to_string();
        let manifest = serde_json::json!({
            "latest": {"release": "1.21.1", "snapshot": "1.21.1"},
            "versions": [{"id": "1.21.1", "type": "release", "url": format!("{}/v/1.21.1.json", server.uri()),
                          "sha1": sha1_hex(version_json.as_bytes()), "releaseTime": "2024-08-08T12:00:00+00:00"}]
        });
        Mock::given(method("GET")).and(path("/manifest.json")).respond_with(ResponseTemplate::new(200).set_body_json(manifest)).mount(server).await;
        Mock::given(method("GET")).and(path("/v/1.21.1.json")).respond_with(ResponseTemplate::new(200).set_body_string(version_json)).mount(server).await;
        Mock::given(method("GET")).and(path("/dl/server.jar")).respond_with(ResponseTemplate::new(200).set_body_bytes(jar.to_vec())).mount(server).await;
        Mock::given(method("GET"))
            .and(path("/v2/versions/loader/1.21.1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"loader": {"version": "0.17.0-beta", "stable": false}},
                {"loader": {"version": "0.16.5", "stable": true}}
            ])))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v2/versions/installer"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([{"version": "1.0.1", "stable": true}])))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v2/versions/loader/1.21.1/0.16.5/1.0.1/server/jar"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"PK-fabric-launcher".to_vec()))
            .mount(server)
            .await;
        ServerSources {
            manifest_url: format!("{}/manifest.json", server.uri()),
            fabric_meta_url: server.uri(),
            quilt_meta_url: server.uri(),
            quilt_maven_url: server.uri(),
        }
    }

    fn request(dir: &Path, cache: Option<PathBuf>, loader: Loader, version: &str) -> InstallRequest {
        InstallRequest {
            dir: dir.to_path_buf(),
            minecraft: "1.21.1".into(),
            loader: LoaderSpec { loader, version: version.into() },
            java_root: dir.join("java-inexistant"),
            java_override: None,
            jar_cache: cache,
            progress: None,
        }
    }

    #[tokio::test]
    async fn vanilla_install_is_verified_cached_and_idempotent() {
        let server = MockServer::start().await;
        let jar = b"PK-fake-server-jar".to_vec();
        let sources = mock_world(&server, &jar).await;
        let installer = ServerInstaller::with_sources(sources);
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("server");
        let cache = t.path().join("cache");
        let rec = installer.install(&request(&dir, Some(cache.clone()), Loader::Vanilla, "")).await.unwrap();
        assert_eq!(rec.key, "vanilla-1.21.1-");
        assert_eq!(rec.java_major, 21);
        assert_eq!(rec.args, vec!["-jar", "server.jar", "nogui"]);
        assert_eq!(std::fs::read(dir.join("server.jar")).unwrap(), jar);
        assert_eq!(std::fs::read(cache.join(format!("server-{}.jar", sha1_hex(&jar)))).unwrap(), jar);

        // Le monde et la configuration survivent ; une installation valide est rendue telle quelle (sans réseau).
        std::fs::write(dir.join("server.properties"), "motd=x\n").unwrap();
        let before = server.received_requests().await.unwrap().len();
        let again = installer.install(&request(&dir, Some(cache.clone()), Loader::Vanilla, "")).await.unwrap();
        assert_eq!(again, rec);
        assert_eq!(server.received_requests().await.unwrap().len(), before);

        // Jar absent : réinstallation depuis le cache, sans télécharger le jar.
        std::fs::remove_file(dir.join("server.jar")).unwrap();
        installer.install(&request(&dir, Some(cache), Loader::Vanilla, "")).await.unwrap();
        assert_eq!(std::fs::read(dir.join("server.jar")).unwrap(), jar);
        let jar_downloads = server.received_requests().await.unwrap().iter().filter(|r| r.url.path() == "/dl/server.jar").count();
        assert_eq!(jar_downloads, 1);
        assert_eq!(std::fs::read_to_string(dir.join("server.properties")).unwrap(), "motd=x\n");
    }

    #[tokio::test]
    async fn vanilla_install_rejects_corrupted_jar() {
        let server = MockServer::start().await;
        let sources = mock_world(&server, b"PK-good").await;
        // Le jar servi n'est pas celui annoncé.
        server.reset().await;
        let jar_sha = sha1_hex(b"PK-good");
        let vj = serde_json::json!({"downloads": {"server": {"url": format!("{}/dl/server.jar", server.uri()), "sha1": jar_sha, "size": 7}}}).to_string();
        let manifest = serde_json::json!({"latest": {"release": "1.21.1", "snapshot": "1.21.1"}, "versions": [
            {"id": "1.21.1", "type": "release", "url": format!("{}/v/1.21.1.json", server.uri()), "sha1": sha1_hex(vj.as_bytes()), "releaseTime": "2024-08-08T12:00:00+00:00"}]});
        Mock::given(method("GET")).and(path("/manifest.json")).respond_with(ResponseTemplate::new(200).set_body_json(manifest)).mount(&server).await;
        Mock::given(method("GET")).and(path("/v/1.21.1.json")).respond_with(ResponseTemplate::new(200).set_body_string(vj)).mount(&server).await;
        Mock::given(method("GET")).and(path("/dl/server.jar")).respond_with(ResponseTemplate::new(200).set_body_bytes(b"PK-evil".to_vec())).mount(&server).await;
        let t = tempfile::tempdir().unwrap();
        let err = ServerInstaller::with_sources(sources).install(&request(&t.path().join("s"), None, Loader::Vanilla, "")).await.unwrap_err();
        assert!(matches!(err, Error::Integrity { .. }), "{err}");
        assert!(!t.path().join("s").join("server.jar").exists());
        assert!(!t.path().join("s").join(RECORD_FILE).exists());
    }

    #[tokio::test]
    async fn fabric_install_resolves_latest_stable() {
        let server = MockServer::start().await;
        let jar = b"PK-vanilla".to_vec();
        let sources = mock_world(&server, &jar).await;
        let installer = ServerInstaller::with_sources(sources);
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("server");
        let rec = installer.install(&request(&dir, None, Loader::Fabric, "")).await.unwrap();
        assert_eq!(rec.loader_version, "0.16.5");
        assert_eq!(rec.key, "fabric-1.21.1-0.16.5");
        assert_eq!(rec.args, vec!["-jar", "fabric-server-launch.jar", "nogui"]);
        assert_eq!(std::fs::read(dir.join("fabric-server-launch.jar")).unwrap(), b"PK-fabric-launcher");
        assert_eq!(std::fs::read(dir.join("server.jar")).unwrap(), jar);
        assert_eq!(std::fs::read_to_string(dir.join("fabric-server-launcher.properties")).unwrap(), "serverJar=server.jar\n");
        // Changer de loader retire les jars de l'ancien et garde le reste.
        std::fs::create_dir_all(dir.join("world")).unwrap();
        std::fs::write(dir.join("world").join("level.dat"), "x").unwrap();
        let rec2 = installer.install(&request(&dir, None, Loader::Vanilla, "")).await.unwrap();
        assert_eq!(rec2.key, "vanilla-1.21.1-");
        assert!(!dir.join("fabric-server-launch.jar").exists());
        assert!(!dir.join("fabric-server-launcher.properties").exists());
        assert!(dir.join("world").join("level.dat").is_file());
    }

    #[tokio::test]
    async fn bad_versions_are_refused_before_any_request() {
        let t = tempfile::tempdir().unwrap();
        let inst = ServerInstaller::with_sources(ServerSources { manifest_url: "http://127.0.0.1:1/m".into(), ..ServerSources::default() });
        let mut req = request(t.path(), None, Loader::Vanilla, "");
        req.minecraft = "../evil".into();
        assert!(matches!(inst.install(&req).await, Err(Error::Invalid(_))));
        let mut req = request(t.path(), None, Loader::Fabric, "a/b");
        req.minecraft = "1.21.1".into();
        assert!(matches!(inst.install(&req).await, Err(Error::Invalid(_))));
    }

    // --- Pilotage avec un faux serveur (script shell) ---------------------------------------------------------

    #[cfg(unix)]
    mod fake {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        /// Faux « java » : ignore ses arguments et joue un serveur qui lit stdin.
        fn fake_java(dir: &Path, script: &str) -> PathBuf {
            let p = dir.join("fake-java.sh");
            std::fs::write(&p, format!("#!/bin/bash\n{script}\n")).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p
        }

        const WORKING: &str = r#"
echo "[12:00:00] [main/INFO]: Starting"
echo "[12:00:01] [Server thread/INFO]: Done (1.0s)! For help, type \"help\""
while read -r line; do
  case "$line" in
    save-off) echo "[12:00:02] [Server thread/INFO]: Automatic saving is now disabled" ;;
    "save-all flush") echo "[12:00:02] [Server thread/INFO]: Saving the game (this may take a moment!)"; echo "[12:00:03] [Server thread/INFO]: Saved the game" ;;
    save-on) echo "[12:00:04] [Server thread/INFO]: Automatic saving is now enabled" ;;
    stop) echo "[12:00:05] [Server thread/INFO]: Stopping the server"; exit 0 ;;
  esac
done
"#;

        fn installed(dir: &Path) -> InstalledServer {
            InstalledServer {
                dir: dir.to_path_buf(),
                key: "vanilla-1.21.1-".into(),
                minecraft: "1.21.1".into(),
                loader_version: String::new(),
                java_major: 21,
                args: vec!["-jar".into(), "server.jar".into(), "nogui".into()],
                required_files: vec![],
            }
        }

        fn cfg(dir: &Path, java: &Path) -> ServerConfig {
            let mut c = ServerConfig::new(dir, java, 1024);
            c.accept_eula = true;
            c.ready_timeout = Duration::from_secs(20);
            c
        }

        #[tokio::test]
        async fn checkpoint_sequence_and_clean_stop() {
            let t = tempfile::tempdir().unwrap();
            let dir = t.path().join("srv");
            let java = fake_java(t.path(), WORKING);
            let server = DedicatedServer::start(&cfg(&dir, &java), &installed(&dir)).await.unwrap();
            assert!(server.is_running());
            assert!(eula_accepted(&dir));
            let props = std::fs::read_to_string(dir.join("server.properties")).unwrap();
            assert!(props.contains(&format!("server-port={}", server.port())));
            let got = server.checkpoint(|| async { Ok::<_, Error>(42) }).await.unwrap();
            assert_eq!(got, 42);
            let log = server.recent_lines(50).join("\n");
            let (a, b, c) = (log.find("disabled").unwrap(), log.find("Saved the game").unwrap(), log.find("is now enabled").unwrap());
            assert!(a < b && b < c, "{log}");
            let out = server.stop(Duration::from_secs(10)).await;
            assert_eq!(out, StopOutcome { forced: false, exit_code: Some(0) });
            assert!(!server.is_running());
            // Une commande après l'arrêt échoue proprement.
            assert!(server.save_off().await.is_err());
        }

        #[tokio::test]
        async fn checkpoint_restores_saving_when_copy_fails() {
            let t = tempfile::tempdir().unwrap();
            let dir = t.path().join("srv");
            let java = fake_java(t.path(), WORKING);
            let server = DedicatedServer::start(&cfg(&dir, &java), &installed(&dir)).await.unwrap();
            let err = server.checkpoint(|| async { Err::<(), _>(Error::Other("copie ratée".into())) }).await.unwrap_err();
            assert!(err.to_string().contains("copie ratée"));
            assert!(server.recent_lines(50).iter().any(|l| is_save_on_line(l)));
            // Même chose si la copie panique.
            let r = AssertUnwindSafe(server.checkpoint(|| async { panic!("boum"); #[allow(unreachable_code)] Ok::<(), Error>(()) })).catch_unwind().await;
            assert!(r.is_err());
            let ons = server.recent_lines(100).iter().filter(|l| is_save_on_line(l)).count();
            assert_eq!(ons, 2);
            server.stop(Duration::from_secs(10)).await;
        }

        #[tokio::test]
        async fn eula_is_required_and_stop_is_forced_after_grace() {
            let t = tempfile::tempdir().unwrap();
            let dir = t.path().join("srv");
            let java = fake_java(t.path(), WORKING);
            let mut c = cfg(&dir, &java);
            c.accept_eula = false;
            assert!(matches!(DedicatedServer::start(&c, &installed(&dir)).await, Err(Error::Invalid(_))));
            assert!(!dir.join("eula.txt").exists());

            // Serveur qui ignore `stop`.
            let stubborn = fake_java(t.path(), "echo 'Done (1s)!'\nwhile true; do read -r l; sleep 0.1; done");
            let server = DedicatedServer::start(&cfg(&dir, &stubborn), &installed(&dir)).await.unwrap();
            let out = server.stop(Duration::from_millis(500)).await;
            assert!(out.forced);
            assert!(!server.is_running());
        }

        #[tokio::test]
        async fn startup_failure_reports_output_and_culprit() {
            let t = tempfile::tempdir().unwrap();
            let dir = t.path().join("srv");
            let java = fake_java(
                t.path(),
                r#"echo "[12:00:01] [main/ERROR] (FabricLoader) Could not execute entrypoint stage 'main' due to errors, provided by 'badmod'!"; exit 1"#,
            );
            let err = DedicatedServer::start(&cfg(&dir, &java), &installed(&dir)).await.err().unwrap();
            let msg = err.to_string();
            assert!(msg.contains("badmod"), "{msg}");
            assert!(msg.contains("code 1"), "{msg}");
        }

        #[tokio::test]
        async fn ready_timeout_kills_the_server() {
            let t = tempfile::tempdir().unwrap();
            let dir = t.path().join("srv");
            let java = fake_java(t.path(), "echo 'chargement'; sleep 30");
            let mut c = cfg(&dir, &java);
            c.ready_timeout = Duration::from_millis(600);
            let err = DedicatedServer::start(&c, &installed(&dir)).await.err().unwrap();
            assert!(err.to_string().contains("pas prêt"), "{err}");
        }

        #[tokio::test]
        async fn drop_kills_the_process() {
            let t = tempfile::tempdir().unwrap();
            let dir = t.path().join("srv");
            let java = fake_java(t.path(), WORKING);
            let server = DedicatedServer::start(&cfg(&dir, &java), &installed(&dir)).await.unwrap();
            let pid = server.pid();
            drop(server);
            for _ in 0..50 {
                if !Path::new(&format!("/proc/{pid}")).exists() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            panic!("le processus {pid} survit à l'abandon du serveur");
        }
    }
}
