//! Ticket #138 : importer les instances de Prism / MultiMC, de la CurseForge App et de la Modrinth App.
//!
//! Tout est lecture seule côté source : un adaptateur par lanceur lit la version de Minecraft, le loader et le
//! nom ([`scan_known`], [`scan_folder`]), puis [`import_instance`] crée une instance Cubelink et y place `mods/`,
//! `config/`, `saves/`, `resourcepacks/`, `shaderpacks/`, `options.txt` et `servers.dat`.
//!
//! Règles de sûreté :
//! - la source n'est jamais écrite, et jamais liée à l'instance : un lien physique partagerait l'inode et le jeu
//!   réécrirait les fichiers du lanceur d'origine ;
//! - les jars (et zips de packs) sont copiés UNE fois dans le magasin par hash, puis liés (lien physique, repli copie
//!   entre volumes) vers l'instance : ils sont immuables et dédoublonnés avec le reste de Cubelink ;
//! - tout le reste (configs, mondes, options, fichiers écrits par les shaders) est copié ;
//! - liens symboliques non suivis, noms de fichiers hostiles ignorés, liste blanche de dossiers, plafonds de taille ;
//! - échec ou annulation : l'instance est supprimée et les références du magasin relâchées.
//!
//! Le schéma de `app.db` (Modrinth App) n'est pas un contrat : base lue sur une copie, colonnes vérifiées, et un
//! schéma inattendu donne une liste vide plutôt qu'une erreur.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::instance::InstanceStore;
use crate::mods::validate_file_name;
use crate::mrpack::{has_dangerous_ext, sanitize_name, FilesProgress};
use crate::store::index::{object_rel_path, StoreIndex};
use crate::store::link::{copy_into, link_into};
use crate::types::{Error, Instance, Loader, LoaderSpec, Result};

/// Taille maximale d'un fichier JSON ou INI de lanceur lu.
const MAX_META_BYTES: u64 = 8 * 1024 * 1024;
/// Taille maximale de `app.db` copiée pour être lue.
const MAX_DB_BYTES: u64 = 256 * 1024 * 1024;
/// Nombre maximal de fichiers d'une instance importée.
const MAX_FILES: usize = 200_000;
/// Plafond par défaut du volume copié (les mondes peuvent peser plusieurs Go).
pub const DEFAULT_MAX_BYTES: u64 = 16 * 1024 * 1024 * 1024;
/// Sous-dossiers du dossier de jeu repris.
const IMPORT_DIRS: &[&str] = &["mods", "config", "saves", "resourcepacks", "shaderpacks"];
/// Fichiers à la racine du dossier de jeu repris.
const IMPORT_FILES: &[&str] = &["options.txt", "servers.dat"];

/// Lanceur d'origine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum Launcher {
    Prism,
    Curseforge,
    Modrinth,
}

/// Une instance trouvée chez un autre lanceur.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct DetectedInstance {
    pub launcher: Launcher,
    pub name: String,
    /// Dossier de l'instance chez le lanceur d'origine (c'est ce chemin que l'import relit).
    pub path: String,
    pub minecraft: String,
    /// Loader et sa version ; la version est vide quand le lanceur ne la donne pas (Cubelink choisit la recommandée).
    pub loader: LoaderSpec,
    /// Nombre de mods (`.jar`, activés ou non).
    pub mod_count: u32,
    /// L'instance contient au moins un monde.
    pub has_saves: bool,
    /// Raison pour laquelle l'instance ne peut pas être importée (loader que Cubelink ne gère pas).
    pub unsupported: Option<String>,
}

impl DetectedInstance {
    /// Dossier de jeu (celui qui contient `mods/`, `saves/`…), déduit du lanceur et du dossier de l'instance.
    pub fn game_dir(&self) -> PathBuf {
        let p = Path::new(&self.path);
        match self.launcher {
            Launcher::Prism => prism_game_dir(p),
            Launcher::Curseforge | Launcher::Modrinth => p.to_path_buf(),
        }
    }
}

// ---------- Lecture bornée ----------

fn read_limited(path: &Path, max: u64) -> Option<Vec<u8>> {
    let meta = fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.len() > max {
        return None;
    }
    let mut buf = Vec::new();
    File::open(path).ok()?.take(max).read_to_end(&mut buf).ok()?;
    Some(buf)
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&read_limited(path, MAX_META_BYTES)?).ok()
}

fn str_of<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

fn real_dir(p: &Path) -> bool {
    fs::symlink_metadata(p).map(|m| m.is_dir()).unwrap_or(false)
}

/// Sous-dossiers directs (sans suivre les liens symboliques ni les dossiers cachés), triés.
fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<PathBuf> = rd
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.') && e.file_name() != "_LAUNCHER_TEMP")
        .map(|e| e.path())
        .collect();
    out.sort();
    out
}

fn is_jar(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    l.ends_with(".jar") || l.ends_with(".jar.disabled")
}

fn count_mods(game: &Path) -> u32 {
    let Ok(rd) = fs::read_dir(game.join("mods")) else { return 0 };
    rd.flatten().filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false)).filter(|e| is_jar(&e.file_name().to_string_lossy())).count() as u32
}

fn has_saves(game: &Path) -> bool {
    !subdirs(&game.join("saves")).is_empty()
}

fn finish(launcher: Launcher, name: String, dir: &Path, game: &Path, minecraft: String, loader: Result<LoaderSpec>) -> DetectedInstance {
    let (loader, unsupported) = match loader {
        Ok(l) => (l, None),
        Err(e) => (LoaderSpec { loader: Loader::Vanilla, version: String::new() }, Some(e.to_string())),
    };
    DetectedInstance {
        launcher,
        name,
        path: dir.display().to_string(),
        minecraft,
        loader,
        mod_count: count_mods(game),
        has_saves: has_saves(game),
        unsupported,
    }
}

/// Retire un préfixe `<minecraft>-` de la version d'un loader (Forge ancien : `1.12.2-14.23.5.2860`).
fn strip_mc(version: &str, minecraft: &str) -> String {
    let v = version.trim();
    v.strip_prefix(&format!("{minecraft}-")).unwrap_or(v).to_string()
}

fn loader_from_name(name: &str, version: &str, minecraft: &str) -> Result<LoaderSpec> {
    let loader = match name.trim().to_ascii_lowercase().as_str() {
        "" | "vanilla" => return Ok(LoaderSpec { loader: Loader::Vanilla, version: String::new() }),
        "forge" => Loader::Forge,
        "neoforge" => Loader::NeoForge,
        "fabric" => Loader::Fabric,
        "quilt" => Loader::Quilt,
        other => return Err(Error::Invalid(format!("le loader « {other} » n'est pas géré par Cubelink"))),
    };
    let version = strip_mc(version, minecraft);
    if !version.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+')) {
        return Err(Error::Invalid("version du loader illisible".into()));
    }
    Ok(LoaderSpec { loader, version })
}

// ---------- Prism / MultiMC ----------

fn prism_game_dir(instance: &Path) -> PathBuf {
    for sub in ["minecraft", ".minecraft"] {
        let p = instance.join(sub);
        if real_dir(&p) {
            return p;
        }
    }
    instance.join("minecraft")
}

/// Valeur d'une clé d'un fichier INI (`instance.cfg`), tolérant au BOM et aux fins de ligne Windows.
fn ini_value(text: &str, key: &str) -> Option<String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    text.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == key).then(|| v.trim().to_string()).filter(|v| !v.is_empty())
    })
}

/// Lit une instance Prism / MultiMC : `instance.cfg` + `mmc-pack.json`.
fn read_prism(dir: &Path) -> Option<DetectedInstance> {
    let cfg = String::from_utf8_lossy(&read_limited(&dir.join("instance.cfg"), MAX_META_BYTES)?).into_owned();
    let pack = read_json(&dir.join("mmc-pack.json"))?;
    let comps = pack.get("components")?.as_array()?;
    let version_of = |uid: &str| comps.iter().find(|c| str_of(c, "uid") == Some(uid)).and_then(|c| str_of(c, "version")).map(str::to_string);
    let minecraft = version_of("net.minecraft")?;
    let loader = prism_loader(&version_of, &minecraft);
    let name = ini_value(&cfg, "name").unwrap_or_else(|| dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    let game = prism_game_dir(dir);
    Some(finish(Launcher::Prism, name, dir, &game, minecraft, loader))
}

fn prism_loader(version_of: &dyn Fn(&str) -> Option<String>, minecraft: &str) -> Result<LoaderSpec> {
    for (uid, loader) in [
        ("net.neoforged", Loader::NeoForge),
        ("net.fabricmc.fabric-loader", Loader::Fabric),
        ("org.quiltmc.quilt-loader", Loader::Quilt),
        ("net.minecraftforge", Loader::Forge),
    ] {
        if let Some(v) = version_of(uid) {
            let name = match loader {
                Loader::NeoForge => "neoforge",
                Loader::Fabric => "fabric",
                Loader::Quilt => "quilt",
                _ => "forge",
            };
            return loader_from_name(name, &v, minecraft);
        }
    }
    for (uid, label) in [("com.mumfrey.liteloader", "LiteLoader"), ("net.legacyfabric.legacy-fabric-loader", "Legacy Fabric")] {
        if version_of(uid).is_some() {
            return Err(Error::Invalid(format!("le loader « {label} » n'est pas géré par Cubelink")));
        }
    }
    Ok(LoaderSpec { loader: Loader::Vanilla, version: String::new() })
}

// ---------- CurseForge App ----------

/// `baseModLoader.name` : « forge-47.2.0 », « neoforge-21.1.77 », « fabric-0.16.5-1.21.1 » (Fabric et Quilt ajoutent
/// le Minecraft à la fin).
fn cf_loader(id: &str, forge_version: Option<&str>, minecraft: &str) -> Result<LoaderSpec> {
    let (name, rest) = id.split_once('-').unwrap_or((id, ""));
    let version = match name.to_ascii_lowercase().as_str() {
        "vanilla" => return Ok(LoaderSpec { loader: Loader::Vanilla, version: String::new() }),
        "fabric" | "quilt" => rest.split('-').next().unwrap_or(""),
        "forge" => forge_version.unwrap_or(rest),
        _ => rest,
    };
    if version.is_empty() {
        return Err(Error::Invalid(format!("loader CurseForge invalide : {id:?}")));
    }
    loader_from_name(name, version, minecraft)
}

fn read_curseforge(dir: &Path) -> Option<DetectedInstance> {
    let j = read_json(&dir.join("minecraftinstance.json"))?;
    let base = j.get("baseModLoader").filter(|v| v.is_object());
    let minecraft = str_of(&j, "gameVersion").or_else(|| base.and_then(|b| str_of(b, "minecraftVersion")))?.to_string();
    let loader = match base.and_then(|b| str_of(b, "name")) {
        None => Ok(LoaderSpec { loader: Loader::Vanilla, version: String::new() }),
        Some(id) => cf_loader(id, base.and_then(|b| str_of(b, "forgeVersion")), &minecraft),
    };
    let name = str_of(&j, "name").map(str::to_string).or_else(|| dir.file_name().map(|n| n.to_string_lossy().into_owned()))?;
    Some(finish(Launcher::Curseforge, name, dir, dir, minecraft, loader))
}

// ---------- Modrinth App ----------

struct TempDb(PathBuf);
impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Lit les profils de `app.db` sur une COPIE (avec son journal WAL : l'app peut tourner), jamais sur l'original.
/// `None` si la base est absente, illisible ou d'un schéma inattendu.
fn read_modrinth_profiles(db: &Path) -> Option<Vec<[Option<String>; 5]>> {
    let meta = fs::symlink_metadata(db).ok()?;
    if !meta.is_file() || meta.len() > MAX_DB_BYTES {
        return None;
    }
    let tmp = TempDb(std::env::temp_dir().join(format!("cubelink-mrdb-{}", uuid::Uuid::new_v4())));
    fs::create_dir_all(&tmp.0).ok()?;
    let copy = tmp.0.join("app.db");
    fs::copy(db, &copy).ok()?;
    let wal = PathBuf::from(format!("{}-wal", db.display()));
    if fs::symlink_metadata(&wal).map(|m| m.is_file() && m.len() <= MAX_DB_BYTES).unwrap_or(false) {
        let _ = fs::copy(&wal, tmp.0.join("app.db-wal"));
    }
    let conn = rusqlite::Connection::open(&copy).ok()?;
    let cols: BTreeSet<String> = {
        let mut st = conn.prepare("PRAGMA table_info(profiles)").ok()?;
        let rows = st.query_map([], |r| r.get::<_, String>(1)).ok()?;
        rows.flatten().collect()
    };
    if !["path", "name", "game_version", "mod_loader"].iter().all(|c| cols.contains(*c)) {
        return None;
    }
    let version_col = if cols.contains("mod_loader_version") { "mod_loader_version" } else { "NULL" };
    let sql = format!("SELECT path, name, game_version, mod_loader, {version_col} FROM profiles");
    let mut st = conn.prepare(&sql).ok()?;
    let rows = st
        .query_map([], |r| {
            let mut out: [Option<String>; 5] = Default::default();
            for (i, slot) in out.iter_mut().enumerate() {
                *slot = r.get::<_, Option<String>>(i).ok().flatten();
            }
            Ok(out)
        })
        .ok()?;
    Some(rows.flatten().collect())
}

/// Profils de la Modrinth App : `root` contient `app.db` et `profiles/`.
fn scan_modrinth(root: &Path) -> Vec<DetectedInstance> {
    let Some(rows) = read_modrinth_profiles(&root.join("app.db")) else { return Vec::new() };
    let mut out = Vec::new();
    for [path, name, mc, loader, version] in rows {
        let (Some(path), Some(mc)) = (path, mc) else { continue };
        let mc = mc.trim().to_string();
        // Le nom du dossier vient d'une base non fiable : un simple nom, sans séparateur ni `..`.
        if mc.is_empty() || validate_file_name(&path).is_err() {
            continue;
        }
        let dir = root.join("profiles").join(&path);
        if !real_dir(&dir) {
            continue;
        }
        let loader = loader_from_name(loader.as_deref().unwrap_or(""), version.as_deref().unwrap_or(""), &mc);
        let name = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| path.clone());
        out.push(finish(Launcher::Modrinth, name, &dir, &dir, mc, loader));
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

// ---------- Détection ----------

/// Dossiers de base servant à chercher les lanceurs (injectables pour les tests).
#[derive(Debug, Clone, Default)]
pub struct Roots {
    /// `%APPDATA%` (Windows).
    pub appdata: Option<PathBuf>,
    pub home: Option<PathBuf>,
    /// `$XDG_DATA_HOME`, par défaut `~/.local/share` (Linux).
    pub data_home: Option<PathBuf>,
}

impl Roots {
    pub fn from_env() -> Self {
        let var = |k: &str| std::env::var_os(k).map(PathBuf::from).filter(|p| p.is_absolute());
        let home = var("USERPROFILE").or_else(|| var("HOME"));
        let data_home = var("XDG_DATA_HOME").or_else(|| home.as_ref().map(|h| h.join(".local/share")));
        Self { appdata: var("APPDATA"), home, data_home }
    }
}

fn scan_prism_dir(instances: &Path) -> Vec<DetectedInstance> {
    subdirs(instances).iter().filter_map(|d| read_prism(d)).collect()
}

fn scan_curseforge_dir(instances: &Path) -> Vec<DetectedInstance> {
    subdirs(instances).iter().filter_map(|d| read_curseforge(d)).collect()
}

fn dedup(list: Vec<DetectedInstance>) -> Vec<DetectedInstance> {
    let mut seen = BTreeSet::new();
    list.into_iter().filter(|d| seen.insert((d.launcher as u8, fs::canonicalize(&d.path).unwrap_or_else(|_| PathBuf::from(&d.path))))).collect()
}

/// Cherche les instances dans les dossiers habituels des trois lanceurs. Un dossier absent est ignoré sans erreur.
pub fn scan_known(roots: &Roots) -> Vec<DetectedInstance> {
    let mut prism: Vec<PathBuf> = Vec::new();
    let mut modrinth: Vec<PathBuf> = Vec::new();
    let mut cf: Vec<PathBuf> = Vec::new();
    if let Some(a) = &roots.appdata {
        prism.push(a.join("PrismLauncher"));
        modrinth.push(a.join("ModrinthApp"));
    }
    if let Some(d) = &roots.data_home {
        prism.push(d.join("PrismLauncher"));
        modrinth.push(d.join("ModrinthApp"));
    }
    if let Some(h) = &roots.home {
        prism.push(h.join(".var/app/org.prismlauncher.PrismLauncher/data/PrismLauncher"));
        modrinth.push(h.join(".var/app/com.modrinth.ModrinthApp/data/ModrinthApp"));
        cf.push(h.join("curseforge/minecraft/Instances"));
    }
    let mut out = Vec::new();
    for r in prism {
        out.extend(scan_prism_dir(&r.join("instances")));
    }
    for r in modrinth {
        out.extend(scan_modrinth(&r));
    }
    for r in cf {
        out.extend(scan_curseforge_dir(&r));
    }
    dedup(out)
}

/// Instances d'un dossier choisi à la main : une instance, un dossier d'instances (`instances/`, `Instances/`) ou la
/// racine d'un lanceur (Prism, Modrinth App, CurseForge).
pub fn scan_folder(path: &Path) -> Vec<DetectedInstance> {
    if !real_dir(path) {
        return Vec::new();
    }
    let mut out = Vec::new();
    if let Some(i) = read_prism(path).or_else(|| read_curseforge(path)) {
        out.push(i);
    } else {
        out.extend(scan_prism_dir(path));
        out.extend(scan_curseforge_dir(path));
        for sub in ["instances", "Instances", "minecraft/Instances"] {
            let d = path.join(sub);
            out.extend(scan_prism_dir(&d));
            out.extend(scan_curseforge_dir(&d));
        }
        out.extend(scan_modrinth(path));
        // Un profil Modrinth choisi seul : `<racine>/profiles/<profil>`.
        if let (Some(profiles), Some(root)) = (path.parent(), path.parent().and_then(Path::parent)) {
            if profiles.file_name().is_some_and(|n| n == "profiles") {
                out.extend(scan_modrinth(root).into_iter().filter(|d| Path::new(&d.path).file_name() == path.file_name()));
            }
        }
    }
    dedup(out)
}

/// Relit une instance depuis son dossier (la commande d'import ne fait jamais confiance à un objet venu du front).
pub fn load_instance(path: &Path, launcher: Launcher) -> Result<DetectedInstance> {
    if !real_dir(path) {
        return Err(Error::NotFound("dossier de l'instance".into()));
    }
    let found = match launcher {
        Launcher::Prism => read_prism(path),
        Launcher::Curseforge => read_curseforge(path),
        Launcher::Modrinth => path
            .parent()
            .and_then(Path::parent)
            .and_then(|root| scan_modrinth(root).into_iter().find(|d| Path::new(&d.path).file_name() == path.file_name())),
    };
    found.ok_or_else(|| Error::Invalid("ce dossier n'est pas une instance lisible de ce lanceur".into()))
}

// ---------- Import ----------

struct Planned {
    rel: String,
    src: PathBuf,
    size: u64,
    /// Fichier immuable : ingéré dans le magasin puis lié. Sinon copie.
    store: bool,
}

/// Segment de chemin acceptable : ce que `safe_join` accepterait, sans caractère de contrôle.
fn clean_segment(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains([':', '\\', '/', '\0']) && !name.chars().any(|c| c.is_control())
}

/// Fichiers jamais rejoués : verrou de monde en cours d'usage dans l'autre lanceur.
fn ignored_name(name: &str) -> bool {
    name == "session.lock" || name.contains(".cubelink-tmp-")
}

fn immutable_kind(dir: &str, name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    match dir {
        "mods" => l.ends_with(".jar") || l.ends_with(".jar.disabled"),
        "resourcepacks" | "shaderpacks" => l.ends_with(".zip"),
        _ => false,
    }
}

/// Liste ce qui sera importé. Ne suit aucun lien symbolique ; ignore (sans échouer) les noms douteux.
fn plan_files(game: &Path, max_bytes: u64, cancel: &AtomicBool) -> Result<Vec<Planned>> {
    let mut out: Vec<Planned> = Vec::new();
    let mut total = 0u64;
    let mut add = |out: &mut Vec<Planned>, rel: String, src: PathBuf, size: u64, store: bool| -> Result<()> {
        total = total.saturating_add(size);
        if total > max_bytes {
            return Err(Error::Invalid(format!("l'instance pèse plus de {} Go : import refusé", max_bytes / (1024 * 1024 * 1024))));
        }
        if out.len() >= MAX_FILES {
            return Err(Error::Invalid("l'instance contient trop de fichiers".into()));
        }
        out.push(Planned { rel, src, size, store });
        Ok(())
    };
    for file in IMPORT_FILES {
        let p = game.join(file);
        if let Ok(m) = fs::symlink_metadata(&p) {
            if m.is_file() {
                add(&mut out, (*file).to_string(), p, m.len(), false)?;
            }
        }
    }
    for dir in IMPORT_DIRS {
        let root = game.join(dir);
        if !real_dir(&root) {
            continue;
        }
        let mut stack = vec![(root, String::from(*dir))];
        while let Some((cur, rel)) = stack.pop() {
            if cancel.load(Ordering::SeqCst) {
                return Err(Error::Cancelled);
            }
            let Ok(rd) = fs::read_dir(&cur) else { continue };
            for e in rd.flatten() {
                let Some(name) = e.file_name().to_str().map(str::to_string) else { continue };
                if !clean_segment(&name) || ignored_name(&name) {
                    continue;
                }
                let Ok(ft) = e.file_type() else { continue };
                let child_rel = format!("{rel}/{name}");
                if ft.is_dir() {
                    stack.push((e.path(), child_rel));
                } else if ft.is_file() && !has_dangerous_ext(&name) {
                    let Ok(m) = e.metadata() else { continue };
                    add(&mut out, child_rel, e.path(), m.len(), immutable_kind(dir, &name))?;
                }
                // Lien symbolique ou fichier spécial : ignoré.
            }
        }
    }
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(out)
}

/// Copie `src` dans le magasin (hash calculé pendant la copie : un fichier modifié entre-temps reste cohérent),
/// puis inscrit l'objet dans l'index. Renvoie `(sha256, taille)`.
fn ingest(src: &Path, store_root: &Path, index: &StoreIndex, cancel: &AtomicBool) -> Result<(String, u64)> {
    let tmp_dir = store_root.join(".tmp");
    fs::create_dir_all(&tmp_dir)?;
    let tmp = tmp_dir.join(format!("{}.part", uuid::Uuid::new_v4()));
    let copied = (|| -> Result<(String, u64)> {
        let mut r = File::open(src)?;
        let mut w = File::create(&tmp)?;
        let mut h = Sha256::new();
        let mut buf = vec![0u8; 64 * 1024];
        let mut size = 0u64;
        loop {
            if cancel.load(Ordering::SeqCst) {
                return Err(Error::Cancelled);
            }
            let n = r.read(&mut buf)?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
            w.write_all(&buf[..n])?;
            size += n as u64;
        }
        w.flush()?;
        Ok((hex::encode(h.finalize()), size))
    })();
    let (sha, size) = match copied {
        Ok(x) => x,
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
    };
    let rel = object_rel_path(&sha)?;
    let dest = store_root.join(&rel);
    let placed = (|| -> Result<()> {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        if !dest.exists() {
            if let Err(e) = fs::rename(&tmp, &dest) {
                if !dest.exists() {
                    return Err(e.into());
                }
            }
        }
        index.insert(&sha, &rel, size)
    })();
    let _ = fs::remove_file(&tmp);
    placed?;
    Ok((sha, size))
}

/// Paramètres d'un import.
#[derive(Clone)]
pub struct ImportParams {
    /// Nom choisi par le joueur ; sinon celui du lanceur d'origine.
    pub name: Option<String>,
    pub ram_mb: u32,
    pub max_bytes: u64,
}

impl Default for ImportParams {
    fn default() -> Self {
        Self { name: None, ram_mb: crate::instance::RAM_DEFAULT, max_bytes: DEFAULT_MAX_BYTES }
    }
}

/// Importe `det` : crée l'instance puis place les fichiers. Tout échec ou annulation supprime l'instance et relâche
/// les références prises dans le magasin. La source n'est jamais modifiée.
pub fn import_instance(
    det: &DetectedInstance,
    params: &ImportParams,
    instances: &InstanceStore,
    store_root: &Path,
    index: &StoreIndex,
    progress: Option<FilesProgress>,
    cancel: Arc<AtomicBool>,
) -> Result<Instance> {
    if let Some(why) = &det.unsupported {
        return Err(Error::Invalid(why.clone()));
    }
    let game = det.game_dir();
    let plan = plan_files(&game, params.max_bytes, &cancel)?;
    if cancel.load(Ordering::SeqCst) {
        return Err(Error::Cancelled);
    }
    let name = sanitize_name(params.name.as_deref().unwrap_or(&det.name));
    let instance = match instances.create(&name, &det.minecraft, det.loader.clone(), params.ram_mb) {
        Err(Error::Invalid(_)) => instances.create("Modpack", &det.minecraft, det.loader.clone(), params.ram_mb)?,
        other => other?,
    };
    let mut installed: Vec<String> = Vec::new();
    let res = place_files(&instance, &plan, instances, store_root, index, &progress, &cancel, &mut installed);
    match res {
        Ok(()) => instances.get(&instance.id),
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
fn place_files(
    instance: &Instance,
    plan: &[Planned],
    instances: &InstanceStore,
    store_root: &Path,
    index: &StoreIndex,
    progress: &Option<FilesProgress>,
    cancel: &AtomicBool,
    installed: &mut Vec<String>,
) -> Result<()> {
    let game_dir = instances.game_dir(&instance.id)?;
    let total = plan.len() as u32;
    let step = (total / 200).max(1);
    let mut bytes = 0u64;
    for (i, f) in plan.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            return Err(Error::Cancelled);
        }
        // Le fichier a pu être remplacé par un lien depuis le listage : on ne suit rien.
        match fs::symlink_metadata(&f.src) {
            Ok(m) if m.is_file() => {}
            // Disparu pendant la copie (l'autre lanceur tourne) : on passe.
            _ => continue,
        }
        if f.store {
            let (sha, _) = ingest(&f.src, store_root, index, cancel)?;
            let obj = store_root.join(object_rel_path(&sha)?);
            link_into(&game_dir, &f.rel, &obj)?;
            index.add_ref(&sha)?;
            installed.push(sha);
        } else {
            copy_into(&game_dir, &f.rel, &f.src)?;
        }
        bytes += f.size;
        let done = i as u32 + 1;
        if let Some(p) = progress {
            if done.is_multiple_of(step) {
                p(done, total);
            }
        }
    }
    if let Some(p) = progress {
        p(total, total);
    }
    tracing::info!("import d'instance : {} fichiers, {} octets", plan.len(), bytes);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::link::same_file;

    fn w(p: &Path, content: &[u8]) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    fn mmc_pack(comps: &[(&str, &str)]) -> String {
        let c: Vec<Value> = comps.iter().map(|(u, v)| serde_json::json!({"uid": u, "version": v})).collect();
        serde_json::json!({"formatVersion": 1, "components": c}).to_string()
    }

    fn prism(instances: &Path, dir: &str, name: &str, comps: &[(&str, &str)]) -> PathBuf {
        let d = instances.join(dir);
        w(&d.join("instance.cfg"), format!("[General]\r\nInstanceType=OneSix\r\nname={name}\r\n").as_bytes());
        w(&d.join("mmc-pack.json"), mmc_pack(comps).as_bytes());
        d
    }

    fn cf(instances: &Path, dir: &str, name: &str, mc: &str, loader: Option<&str>) -> PathBuf {
        let d = instances.join(dir);
        let base = loader.map(|l| serde_json::json!({"name": l, "minecraftVersion": mc}));
        w(&d.join("minecraftinstance.json"), serde_json::json!({"name": name, "gameVersion": mc, "baseModLoader": base}).to_string().as_bytes());
        d
    }

    fn modrinth_db(root: &Path, rows: &[(&str, &str, &str, &str, Option<&str>)]) {
        fs::create_dir_all(root).unwrap();
        let c = rusqlite::Connection::open(root.join("app.db")).unwrap();
        c.execute_batch("CREATE TABLE profiles (path TEXT PRIMARY KEY, name TEXT, icon_path TEXT, game_version TEXT, mod_loader TEXT, mod_loader_version TEXT, created INTEGER)").unwrap();
        for (path, name, mc, loader, ver) in rows {
            c.execute("INSERT INTO profiles (path, name, game_version, mod_loader, mod_loader_version, created) VALUES (?1, ?2, ?3, ?4, ?5, 0)", rusqlite::params![path, name, mc, loader, ver]).unwrap();
            fs::create_dir_all(root.join("profiles").join(path)).unwrap();
        }
    }

    fn lone(l: Loader, v: &str) -> LoaderSpec {
        LoaderSpec { loader: l, version: v.into() }
    }

    #[test]
    fn prism_lit_version_loader_nom_et_dossier_de_jeu() {
        let t = tempfile::tempdir().unwrap();
        let i = t.path();
        prism(i, "a", "Fabric Pack", &[("net.minecraft", "1.21.1"), ("net.fabricmc.intermediary", "1.21.1"), ("net.fabricmc.fabric-loader", "0.16.5")]);
        prism(i, "b", "Forge Pack", &[("org.lwjgl3", "3.3.1"), ("net.minecraft", "1.20.1"), ("net.minecraftforge", "47.2.0")]);
        prism(i, "c", "Neo Pack", &[("net.minecraft", "1.21.1"), ("net.neoforged", "21.1.77")]);
        prism(i, "d", "Quilt Pack", &[("net.minecraft", "1.20.4"), ("org.quiltmc.quilt-loader", "0.26.0")]);
        prism(i, "e", "Vanilla", &[("net.minecraft", "1.8.9")]);
        prism(i, "f", "Vieux", &[("net.minecraft", "1.12.2"), ("net.minecraftforge", "1.12.2-14.23.5.2860")]);
        prism(i, "g", "Lite", &[("net.minecraft", "1.8.9"), ("com.mumfrey.liteloader", "1.8.9")]);
        w(&i.join("d/.minecraft/mods/x.jar"), b"x");
        w(&i.join("a/minecraft/mods/y.jar.disabled"), b"y");
        w(&i.join("a/minecraft/mods/z.jar"), b"z");
        fs::create_dir_all(i.join("a/minecraft/saves/Monde")).unwrap();
        let list = scan_prism_dir(i);
        let by = |n: &str| list.iter().find(|d| d.name == n).unwrap_or_else(|| panic!("{n}")).clone();
        let a = by("Fabric Pack");
        assert_eq!((a.minecraft.as_str(), a.loader.clone(), a.mod_count, a.has_saves), ("1.21.1", lone(Loader::Fabric, "0.16.5"), 2, true));
        assert_eq!(by("Forge Pack").loader, lone(Loader::Forge, "47.2.0"));
        assert_eq!(by("Neo Pack").loader, lone(Loader::NeoForge, "21.1.77"));
        assert_eq!(by("Quilt Pack").loader, lone(Loader::Quilt, "0.26.0"));
        assert_eq!(by("Vanilla").loader, lone(Loader::Vanilla, ""));
        assert_eq!(by("Vieux").loader, lone(Loader::Forge, "14.23.5.2860"));
        assert!(by("Lite").unsupported.as_deref().unwrap().contains("LiteLoader"));
        // `.minecraft` quand `minecraft` n'existe pas.
        let q = by("Quilt Pack");
        assert!(q.game_dir().ends_with(".minecraft"));
        assert_eq!(q.mod_count, 1);
        assert!(a.game_dir().ends_with("minecraft") && !a.game_dir().ends_with(".minecraft"));
    }

    #[test]
    fn prism_ignore_les_dossiers_illisibles_et_tolere_bom() {
        let t = tempfile::tempdir().unwrap();
        let i = t.path();
        let d = prism(i, "ok", "x", &[("net.minecraft", "1.21.1")]);
        w(&d.join("instance.cfg"), "\u{feff}[General]\r\nname = Avec BOM \r\n".as_bytes());
        // mmc-pack.json corrompu, sans net.minecraft, ou sans instance.cfg.
        let bad = prism(i, "bad", "Bad", &[("net.minecraft", "1.21.1")]);
        w(&bad.join("mmc-pack.json"), b"{pas du json");
        prism(i, "nomc", "NoMc", &[("net.fabricmc.fabric-loader", "0.16.5")]);
        w(&i.join("nocfg/mmc-pack.json"), mmc_pack(&[("net.minecraft", "1.21.1")]).as_bytes());
        w(&i.join(".hidden/instance.cfg"), b"name=h");
        fs::create_dir_all(i.join("_LAUNCHER_TEMP")).unwrap();
        let list = scan_prism_dir(i);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Avec BOM");
        // Sans clé `name`, le nom du dossier.
        w(&d.join("instance.cfg"), b"InstanceType=OneSix\n");
        assert_eq!(scan_prism_dir(i)[0].name, "ok");
    }

    #[test]
    fn curseforge_lit_les_formats_de_loader() {
        let t = tempfile::tempdir().unwrap();
        let i = t.path();
        cf(i, "forge", "Forge", "1.20.1", Some("forge-47.2.0"));
        cf(i, "fabric", "Fabric", "1.21.1", Some("fabric-0.16.5-1.21.1"));
        cf(i, "neo", "Neo", "1.21.1", Some("neoforge-21.1.77"));
        cf(i, "quilt", "Quilt", "1.20.1", Some("quilt-0.26.0"));
        cf(i, "van", "Vanille", "1.20.1", None);
        cf(i, "lite", "Lite", "1.12.2", Some("liteloader-1.12.2"));
        cf(i, "bad", "Bad", "1.20.1", Some("forge"));
        w(&i.join("nomc/minecraftinstance.json"), b"{\"name\":\"x\"}");
        w(&i.join("junk/minecraftinstance.json"), b"[[");
        w(&i.join("forge/mods/a.jar"), b"a");
        w(&i.join("forge/mods/readme.txt"), b"a");
        let list = scan_curseforge_dir(i);
        let by = |n: &str| list.iter().find(|d| d.name == n).unwrap_or_else(|| panic!("{n}")).clone();
        assert_eq!(by("Forge").loader, lone(Loader::Forge, "47.2.0"));
        assert_eq!(by("Forge").mod_count, 1);
        assert_eq!(by("Fabric").loader, lone(Loader::Fabric, "0.16.5"));
        assert_eq!(by("Neo").loader, lone(Loader::NeoForge, "21.1.77"));
        assert_eq!(by("Quilt").loader, lone(Loader::Quilt, "0.26.0"));
        assert_eq!(by("Vanille").loader, lone(Loader::Vanilla, ""));
        assert!(by("Lite").unsupported.is_some());
        assert!(by("Bad").unsupported.is_some());
        assert_eq!(list.len(), 7, "nomc et junk sont ignorés");
        assert_eq!(by("Forge").game_dir(), PathBuf::from(&by("Forge").path));
    }

    #[test]
    fn modrinth_lit_app_db_sans_la_modifier() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("ModrinthApp");
        modrinth_db(
            &root,
            &[
                ("Pack One", "Pack Un", "1.21.1", "fabric", Some("0.16.5")),
                ("vanilla", "Vanilla", "1.21", "vanilla", None),
                ("neo", "Neo", "1.21.1", "neoforge", Some("21.1.77")),
                ("lite", "Lite", "1.12.2", "liteloader", None),
                ("sansversion", "SV", "1.20.1", "forge", None),
                ("..", "Evil", "1.21", "vanilla", None),
            ],
        );
        w(&root.join("profiles/Pack One/mods/a.jar"), b"a");
        // Profil dont le dossier n'existe plus.
        let c = rusqlite::Connection::open(root.join("app.db")).unwrap();
        c.execute("INSERT INTO profiles (path, name, game_version, mod_loader) VALUES ('ghost','G','1.21','vanilla')", []).unwrap();
        drop(c);
        let before = fs::read(root.join("app.db")).unwrap();
        let list = scan_modrinth(&root);
        let names: Vec<&str> = list.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names.len(), 5, "{names:?}");
        let by = |n: &str| list.iter().find(|d| d.name == n).unwrap().clone();
        assert_eq!(by("Pack Un").loader, lone(Loader::Fabric, "0.16.5"));
        assert_eq!(by("Pack Un").mod_count, 1);
        assert_eq!(by("Vanilla").loader, lone(Loader::Vanilla, ""));
        assert_eq!(by("Neo").loader, lone(Loader::NeoForge, "21.1.77"));
        assert!(by("Lite").unsupported.is_some());
        assert_eq!(by("SV").loader, lone(Loader::Forge, ""));
        assert_eq!(by("Pack Un").game_dir(), root.join("profiles/Pack One"));
        // Rien n'a été écrit à côté de la base d'origine.
        assert_eq!(fs::read(root.join("app.db")).unwrap(), before);
        let mut files: Vec<String> = fs::read_dir(&root).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        files.sort();
        assert_eq!(files, ["app.db", "profiles"]);
    }

    #[test]
    fn modrinth_schema_inattendu_ou_base_illisible_donne_une_liste_vide() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        assert!(scan_modrinth(root).is_empty(), "pas de base");
        w(&root.join("app.db"), b"ceci n'est pas du sqlite");
        assert!(scan_modrinth(root).is_empty(), "base illisible");
        fs::remove_file(root.join("app.db")).unwrap();
        let c = rusqlite::Connection::open(root.join("app.db")).unwrap();
        c.execute_batch("CREATE TABLE profiles (id INTEGER, titre TEXT); CREATE TABLE autre (x TEXT)").unwrap();
        drop(c);
        assert!(scan_modrinth(root).is_empty(), "colonnes absentes");
    }

    #[test]
    fn modrinth_lit_aussi_le_journal_wal() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("m");
        modrinth_db(&root, &[("p", "Profil", "1.21.1", "vanilla", None)]);
        // L'app tourne : une ligne vient d'être écrite et n'est que dans le journal (connexion gardée ouverte).
        let c = rusqlite::Connection::open(root.join("app.db")).unwrap();
        c.pragma_update(None, "journal_mode", "WAL").unwrap();
        c.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
        c.execute("INSERT INTO profiles (path, name, game_version, mod_loader) VALUES ('q','Neuf','1.20.1','vanilla')", []).unwrap();
        fs::create_dir_all(root.join("profiles/q")).unwrap();
        let list = scan_modrinth(&root);
        assert!(list.iter().any(|d| d.name == "Neuf"), "{list:?}");
        drop(c);
    }

    #[test]
    fn scan_known_cherche_les_dossiers_de_chaque_systeme() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("home");
        // Windows
        let appdata = t.path().join("AppData");
        prism(&appdata.join("PrismLauncher/instances"), "w", "PrismWin", &[("net.minecraft", "1.21.1")]);
        modrinth_db(&appdata.join("ModrinthApp"), &[("m", "ModWin", "1.21.1", "vanilla", None)]);
        cf(&home.join("curseforge/minecraft/Instances"), "c", "CfWin", "1.20.1", Some("forge-47.2.0"));
        // Linux, natif et Flatpak
        prism(&home.join(".local/share/PrismLauncher/instances"), "l", "PrismLinux", &[("net.minecraft", "1.20.1")]);
        prism(&home.join(".var/app/org.prismlauncher.PrismLauncher/data/PrismLauncher/instances"), "f", "PrismFlat", &[("net.minecraft", "1.20.1")]);
        modrinth_db(&home.join(".var/app/com.modrinth.ModrinthApp/data/ModrinthApp"), &[("m", "ModFlat", "1.21.1", "vanilla", None)]);
        let roots = Roots { appdata: Some(appdata), home: Some(home.clone()), data_home: Some(home.join(".local/share")) };
        let mut names: Vec<String> = scan_known(&roots).into_iter().map(|d| d.name).collect();
        names.sort();
        assert_eq!(names, ["CfWin", "ModFlat", "ModWin", "PrismFlat", "PrismLinux", "PrismWin"]);
        // Rien d'installé : liste vide, jamais d'erreur.
        let empty = Roots { appdata: Some(t.path().join("rien")), home: Some(t.path().join("nope")), data_home: None };
        assert!(scan_known(&empty).is_empty());
        assert!(scan_known(&Roots::default()).is_empty());
        // Doublon : `data_home` = `~/.local/share` donne le même dossier que lui-même, une seule fois.
        let twice = Roots { appdata: None, home: Some(home.clone()), data_home: Some(home.join(".local/share")) };
        assert_eq!(scan_known(&twice).iter().filter(|d| d.name == "PrismLinux").count(), 1);
    }

    #[test]
    fn scan_folder_accepte_instance_dossier_d_instances_ou_racine() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("Prism");
        let inst = prism(&root.join("instances"), "p", "P", &[("net.minecraft", "1.21.1")]);
        assert_eq!(scan_folder(&inst).len(), 1, "une instance");
        assert_eq!(scan_folder(&root.join("instances")).len(), 1, "dossier d'instances");
        assert_eq!(scan_folder(&root).len(), 1, "racine du lanceur");
        let mr = t.path().join("Modrinth");
        modrinth_db(&mr, &[("a", "A", "1.21.1", "vanilla", None), ("b", "B", "1.21.1", "vanilla", None)]);
        assert_eq!(scan_folder(&mr).len(), 2, "racine Modrinth");
        let one = scan_folder(&mr.join("profiles/b"));
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].name, "B");
        let cfroot = t.path().join("cf");
        cf(&cfroot.join("Instances"), "x", "X", "1.20.1", None);
        assert_eq!(scan_folder(&cfroot).len(), 1);
        assert!(scan_folder(&t.path().join("inexistant")).is_empty());
        // Un dossier quelconque (le parent de toutes les racines) ne déclenche pas de recherche récursive.
        assert!(scan_folder(t.path()).is_empty());
    }

    #[test]
    fn load_instance_relit_depuis_le_disque_et_refuse_le_reste() {
        let t = tempfile::tempdir().unwrap();
        let inst = prism(&t.path().join("instances"), "p", "P", &[("net.minecraft", "1.21.1")]);
        let d = load_instance(&inst, Launcher::Prism).unwrap();
        assert_eq!(d.name, "P");
        assert!(load_instance(&inst, Launcher::Curseforge).is_err(), "mauvais lanceur");
        assert!(load_instance(&t.path().join("rien"), Launcher::Prism).is_err());
        assert!(load_instance(t.path(), Launcher::Prism).is_err(), "dossier quelconque");
        let mr = t.path().join("M");
        modrinth_db(&mr, &[("a", "A", "1.21.1", "fabric", Some("0.16.5"))]);
        assert_eq!(load_instance(&mr.join("profiles/a"), Launcher::Modrinth).unwrap().loader, lone(Loader::Fabric, "0.16.5"));
    }

    // ----- import -----

    struct Env {
        _t: tempfile::TempDir,
        src: PathBuf,
        store: PathBuf,
        index: StoreIndex,
        instances: InstanceStore,
    }

    /// Une instance Prism Fabric avec mods, configs, monde, options, pack de ressources et shader.
    fn env() -> Env {
        let t = tempfile::tempdir().unwrap();
        let store = t.path().join("store");
        fs::create_dir_all(&store).unwrap();
        let index = StoreIndex::open(t.path().join("index.db")).unwrap();
        let instances = InstanceStore::new(t.path().join("instances"));
        let src = prism(&t.path().join("prism/instances"), "pack", "Mon Pack", &[("net.minecraft", "1.21.1"), ("net.fabricmc.fabric-loader", "0.16.5")]);
        let g = src.join("minecraft");
        w(&g.join("mods/sodium.jar"), b"jar sodium");
        w(&g.join("mods/lithium.jar.disabled"), b"jar lithium");
        w(&g.join("mods/sub/dossier.jar"), b"jar dans un sous-dossier");
        w(&g.join("mods/dup.jar"), b"jar sodium");
        w(&g.join("mods/tool.exe"), b"MZ");
        w(&g.join("config/sodium.json"), b"{\"a\":1}");
        w(&g.join("saves/Monde/level.dat"), b"niveau");
        w(&g.join("saves/Monde/region/r.0.0.mca"), b"region");
        w(&g.join("saves/Monde/session.lock"), b"lock");
        w(&g.join("options.txt"), b"fov:70");
        w(&g.join("servers.dat"), b"srv");
        w(&g.join("resourcepacks/pack.zip"), b"zip pack");
        w(&g.join("shaderpacks/bsl.zip"), b"zip shader");
        w(&g.join("shaderpacks/bsl.zip.txt"), b"options=1");
        w(&g.join("logs/latest.log"), b"ne pas importer");
        w(&g.join("crash-reports/c.txt"), b"ne pas importer");
        w(&g.join(".fabric/x"), b"ne pas importer");
        w(&g.join("versions/x.jar"), b"ne pas importer");
        Env { _t: t, src, store, index, instances }
    }

    fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if e.file_type().unwrap().is_dir() {
                    stack.push(p);
                } else {
                    out.push((p.strip_prefix(dir).unwrap().display().to_string(), fs::read(&p).unwrap()));
                }
            }
        }
        out.sort();
        out
    }

    fn run(e: &Env, params: &ImportParams) -> Result<Instance> {
        let det = load_instance(&e.src, Launcher::Prism).unwrap();
        import_instance(&det, params, &e.instances, &e.store, &e.index, None, Arc::new(AtomicBool::new(false)))
    }

    #[test]
    fn import_place_les_fichiers_lie_les_jars_copie_le_reste_et_laisse_la_source_intacte() {
        let e = env();
        let before = snapshot(&e.src);
        let inst = run(&e, &ImportParams::default()).unwrap();
        assert_eq!(inst.name, "Mon Pack");
        assert_eq!(inst.minecraft, "1.21.1");
        assert_eq!(inst.loader, lone(Loader::Fabric, "0.16.5"));
        // `Instance::mod_count` compte les jars activés de `mods/` (sodium et dup), pas le désactivé ni le sous-dossier.
        assert_eq!(inst.mod_count, 2);
        let g = e.instances.game_dir(&inst.id).unwrap();
        let sg = e.src.join("minecraft");
        for rel in [
            "mods/sodium.jar",
            "mods/lithium.jar.disabled",
            "mods/sub/dossier.jar",
            "config/sodium.json",
            "saves/Monde/level.dat",
            "saves/Monde/region/r.0.0.mca",
            "options.txt",
            "servers.dat",
            "resourcepacks/pack.zip",
            "shaderpacks/bsl.zip",
            "shaderpacks/bsl.zip.txt",
        ] {
            assert_eq!(fs::read(g.join(rel)).unwrap(), fs::read(sg.join(rel)).unwrap(), "{rel}");
        }
        // Jamais importés.
        for rel in ["mods/tool.exe", "saves/Monde/session.lock", "logs", "crash-reports", ".fabric", "versions"] {
            assert!(!g.join(rel).exists(), "{rel}");
        }
        // Jars et zips de packs : liés au magasin, pas à la source.
        let sha = crate::store::hash::sha256_file(&g.join("mods/sodium.jar")).unwrap();
        let obj = e.store.join(object_rel_path(&sha).unwrap());
        assert!(same_file(&obj, &g.join("mods/sodium.jar")));
        assert!(same_file(&obj, &g.join("mods/dup.jar")), "deux jars identiques : un seul objet");
        assert!(!same_file(&sg.join("mods/sodium.jar"), &g.join("mods/sodium.jar")));
        assert!(same_file(&e.store.join(object_rel_path(&crate::store::hash::sha256_file(&g.join("resourcepacks/pack.zip")).unwrap()).unwrap()), &g.join("resourcepacks/pack.zip")));
        assert_eq!(e.index.get(&sha).unwrap().unwrap().refs, 2);
        // Ce que le jeu réécrit est copié, jamais lié (et le fichier d'options d'un shader aussi).
        for rel in ["config/sodium.json", "saves/Monde/level.dat", "options.txt", "servers.dat", "shaderpacks/bsl.zip.txt"] {
            assert!(!same_file(&sg.join(rel), &g.join(rel)), "{rel}");
        }
        fs::write(g.join("options.txt"), b"fov:110").unwrap();
        fs::write(g.join("saves/Monde/level.dat"), b"autre").unwrap();
        // Source octet pour octet identique, rien d'ajouté.
        assert_eq!(snapshot(&e.src), before);
        // Une seule instance, pas de fichier temporaire dans l'instance.
        assert_eq!(e.instances.list().len(), 1);
        assert!(!snapshot(&g).iter().any(|(p, _)| p.contains("cubelink-tmp")));
    }

    #[test]
    fn le_magasin_reste_coherent_avec_les_liens() {
        let e = env();
        let inst = run(&e, &ImportParams::default()).unwrap();
        let live = crate::store::live::live_refs(&e.store, &e.index, e.instances.root()).unwrap();
        for (sha, n) in &live.counts {
            assert_eq!(e.index.get(sha).unwrap().unwrap().refs, *n, "références de {sha}");
        }
        assert!(!live.counts.is_empty());
        // Supprimer l'instance puis balayer : les objets ne servent plus.
        e.instances.delete(&inst.id).unwrap();
        let rep = crate::store::gc::sweep(&e.store, &e.index, e.instances.root(), std::time::Duration::ZERO).unwrap();
        assert!(!rep.removed.is_empty(), "{rep:?}");
    }

    #[test]
    fn progression_croissante_et_complete() {
        let e = env();
        let seen = Arc::new(std::sync::Mutex::new(Vec::<(u32, u32)>::new()));
        let s = seen.clone();
        let det = load_instance(&e.src, Launcher::Prism).unwrap();
        let prog: FilesProgress = Arc::new(move |d, t| s.lock().unwrap().push((d, t)));
        import_instance(&det, &ImportParams::default(), &e.instances, &e.store, &e.index, Some(prog), Arc::new(AtomicBool::new(false))).unwrap();
        let v = seen.lock().unwrap().clone();
        assert!(v.windows(2).all(|p| p[0].0 <= p[1].0));
        assert_eq!(v.last().unwrap().0, v.last().unwrap().1);
        assert!(v.last().unwrap().1 >= 10);
    }

    #[test]
    fn annulation_en_cours_supprime_l_instance_et_relache_les_references() {
        let e = env();
        let det = load_instance(&e.src, Launcher::Prism).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let c2 = cancel.clone();
        // Annule après le premier fichier posé (une progression par fichier : le plan est petit).
        let prog: FilesProgress = Arc::new(move |d, _| {
            if d >= 5 {
                c2.store(true, Ordering::SeqCst);
            }
        });
        let before = snapshot(&e.src);
        let r = import_instance(&det, &ImportParams::default(), &e.instances, &e.store, &e.index, Some(prog), cancel);
        assert!(matches!(r, Err(Error::Cancelled)), "{r:?}");
        assert!(e.instances.list().is_empty());
        assert!(!e.instances.root().join("mon-pack").exists());
        assert!(!e.index.all().unwrap().is_empty(), "des jars avaient déjà été ingérés avant l'annulation");
        assert!(e.index.all().unwrap().iter().all(|o| o.refs == 0), "références relâchées");
        assert_eq!(snapshot(&e.src), before);
    }

    #[test]
    fn annule_avant_de_commencer_ne_cree_rien() {
        let e = env();
        let det = load_instance(&e.src, Launcher::Prism).unwrap();
        let r = import_instance(&det, &ImportParams::default(), &e.instances, &e.store, &e.index, None, Arc::new(AtomicBool::new(true)));
        assert!(matches!(r, Err(Error::Cancelled)));
        assert!(e.instances.list().is_empty());
    }

    #[test]
    fn trop_volumineux_ou_loader_non_gere_sont_refuses_sans_rien_creer() {
        let e = env();
        let r = run(&e, &ImportParams { max_bytes: 10, ..Default::default() });
        assert!(matches!(r, Err(Error::Invalid(_))), "{r:?}");
        assert!(e.instances.list().is_empty());
        let mut det = load_instance(&e.src, Launcher::Prism).unwrap();
        det.unsupported = Some("le loader « liteloader » n'est pas géré par Cubelink".into());
        let r = import_instance(&det, &ImportParams::default(), &e.instances, &e.store, &e.index, None, Arc::new(AtomicBool::new(false)));
        assert!(matches!(&r, Err(Error::Invalid(m)) if m.contains("liteloader")));
        assert!(e.instances.list().is_empty());
    }

    #[test]
    fn nom_choisi_nom_hostile_et_doublons() {
        let e = env();
        let a = run(&e, &ImportParams { name: Some("  Mon <Import>  ".into()), ..Default::default() }).unwrap();
        assert_eq!(a.name, "Mon -Import-");
        let b = run(&e, &ImportParams::default()).unwrap();
        let c = run(&e, &ImportParams::default()).unwrap();
        assert_eq!(b.name, c.name);
        assert_ne!(b.id, c.id, "deux imports ne se marchent pas dessus");
        let d = run(&e, &ImportParams { name: Some("...".into()), ..Default::default() }).unwrap();
        assert_eq!(d.name, "Modpack");
    }

    #[test]
    fn noms_de_fichiers_hostiles_ignores_sans_faire_echouer_l_import() {
        let e = env();
        let g = e.src.join("minecraft");
        // Sous Linux ces noms existent vraiment ; sous Windows la création échoue et le test ne vérifie que le reste.
        for bad in ["config/a:b.toml", "config/c\\d.toml", "config/e\u{7}f.toml", "mods/mal.dll"] {
            let name = bad.rsplit('/').next().unwrap();
            let _ = fs::write(g.join("config").join(name), b"x").or_else(|_| fs::write(g.join(name), b"x"));
        }
        let _ = fs::write(g.join("mods/mal.dll"), b"x");
        let inst = run(&e, &ImportParams::default()).unwrap();
        let ig = e.instances.game_dir(&inst.id).unwrap();
        let names: Vec<String> = snapshot(&ig).into_iter().map(|(p, _)| p).collect();
        assert!(names.iter().all(|p| !p.contains(':') && !p.contains("mal.dll") && !p.contains('\u{7}')), "{names:?}");
        assert!(names.iter().any(|p| p.ends_with("sodium.json")));
    }

    #[cfg(unix)]
    #[test]
    fn liens_symboliques_jamais_suivis() {
        let e = env();
        let secret = e._t.path().join("secret.txt");
        fs::write(&secret, b"mot de passe").unwrap();
        let g = e.src.join("minecraft");
        std::os::unix::fs::symlink(&secret, g.join("config/lien.txt")).unwrap();
        std::os::unix::fs::symlink(&secret, g.join("mods/lien.jar")).unwrap();
        std::os::unix::fs::symlink(e._t.path(), g.join("saves/Dehors")).unwrap();
        std::os::unix::fs::symlink(&secret, g.join("options.txt.bak")).unwrap();
        fs::remove_file(g.join("options.txt")).unwrap();
        std::os::unix::fs::symlink(&secret, g.join("options.txt")).unwrap();
        let inst = run(&e, &ImportParams::default()).unwrap();
        let ig = e.instances.game_dir(&inst.id).unwrap();
        for rel in ["config/lien.txt", "mods/lien.jar", "saves/Dehors", "options.txt"] {
            assert!(fs::symlink_metadata(ig.join(rel)).is_err(), "{rel} ne doit pas exister");
        }
        assert!(!snapshot(&ig).iter().any(|(_, c)| c == b"mot de passe"));
        // Dossier de mods lui-même remplacé par un lien : ignoré.
        fs::remove_dir_all(g.join("mods")).unwrap();
        std::os::unix::fs::symlink(e._t.path().join("prism"), g.join("mods")).unwrap();
        let inst2 = run(&e, &ImportParams::default()).unwrap();
        assert_eq!(inst2.mod_count, 0);
    }

    #[test]
    fn instance_sans_dossier_de_jeu_donne_une_instance_vide() {
        let t = tempfile::tempdir().unwrap();
        let d = prism(t.path(), "vide", "Vide", &[("net.minecraft", "1.21.1")]);
        let instances = InstanceStore::new(t.path().join("inst"));
        let store = t.path().join("store");
        fs::create_dir_all(&store).unwrap();
        let index = StoreIndex::open(t.path().join("i.db")).unwrap();
        let det = load_instance(&d, Launcher::Prism).unwrap();
        let i = import_instance(&det, &ImportParams::default(), &instances, &store, &index, None, Arc::new(AtomicBool::new(false))).unwrap();
        assert_eq!(i.mod_count, 0);
    }

    #[test]
    fn curseforge_et_modrinth_s_importent_aussi() {
        let t = tempfile::tempdir().unwrap();
        let store = t.path().join("store");
        fs::create_dir_all(&store).unwrap();
        let index = StoreIndex::open(t.path().join("i.db")).unwrap();
        let instances = InstanceStore::new(t.path().join("inst"));
        let c = cf(&t.path().join("cf"), "x", "Cf Pack", "1.20.1", Some("forge-47.2.0"));
        w(&c.join("mods/a.jar"), b"a");
        w(&c.join("saves/W/level.dat"), b"w");
        w(&c.join("minecraftinstance.json.bak"), b"ne pas importer");
        let mr = t.path().join("mr");
        modrinth_db(&mr, &[("p", "Mr Pack", "1.21.1", "neoforge", Some("21.1.77"))]);
        w(&mr.join("profiles/p/mods/b.jar"), b"b");
        w(&mr.join("profiles/p/profile.json"), b"ne pas importer");
        let go = |d: DetectedInstance| import_instance(&d, &ImportParams::default(), &instances, &store, &index, None, Arc::new(AtomicBool::new(false))).unwrap();
        let a = go(load_instance(&c, Launcher::Curseforge).unwrap());
        let b = go(load_instance(&mr.join("profiles/p"), Launcher::Modrinth).unwrap());
        assert_eq!((a.loader.clone(), a.mod_count), (lone(Loader::Forge, "47.2.0"), 1));
        assert_eq!((b.loader.clone(), b.mod_count), (lone(Loader::NeoForge, "21.1.77"), 1));
        let ga = instances.game_dir(&a.id).unwrap();
        assert!(ga.join("saves/W/level.dat").is_file());
        assert!(!ga.join("minecraftinstance.json").exists() && !ga.join("minecraftinstance.json.bak").exists());
        assert!(!instances.game_dir(&b.id).unwrap().join("profile.json").exists());
    }
}
