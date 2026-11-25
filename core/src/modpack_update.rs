//! Mise à jour d'un modpack Modrinth déjà installé (ticket #137) : détection, application, annulation.
//!
//! - [`check`] compare la version installée (`Instance.pack`) aux versions publiées compatibles avec l'instance
//!   (même Minecraft, même loader) et renvoie la plus récente si elle est plus récente que l'installée.
//! - [`apply`] compare l'ANCIEN et le NOUVEAU `.mrpack` (index et overrides) au contenu réel du disque :
//!   un fichier du pack n'est remplacé ou retiré que s'il est encore identique à celui de l'ancien pack. Un fichier
//!   que le joueur a modifié est gardé tel quel (décision : le joueur gagne, sans question) ; un fichier que le
//!   joueur a ajouté, `saves/`, `options.txt`, `servers.dat`... ne sont jamais touchés.
//! - Tout est téléchargé et extrait AVANT la première modification de l'instance (un échec réseau ne change rien).
//!   Ensuite, chaque fichier retiré ou remplacé est déplacé dans `.cubelink-pack-backup/<id>/files/` et un
//!   manifeste décrit l'opération ; `Instance.pack` est mis à jour en dernier. Un échec en cours de route remet tout.
//! - [`undo`] relit le manifeste : retire les fichiers ajoutés (sauf ceux que le joueur a modifiés depuis),
//!   remet les fichiers déplacés, la provenance des mods, l'ancienne origine du pack et l'ancien loader.
//!
//! Hors périmètre (v1) : CurseForge (pas de hash dans `manifest.json`) et les mises à jour qui changent la version
//! de Minecraft ou le loader (une nouvelle instance est plus sûre : mondes et mods ne suivraient pas).

use std::collections::{BTreeMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use futures_util::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha512};
use zip::ZipArchive;

use crate::browse::{self, BrowseKind, Providers};
use crate::instance::InstanceStore;
use crate::mods::{self, Provenance};
use crate::mrpack::{self, FilesProgress, ImportOptions, Layout, MrpackFile, MrpackIndex, OverrideEntry};
use crate::providers::modrinth_dl::{DlOptions, StoredObject};
use crate::store::index::StoreIndex;
use crate::store::link::link_into;
use crate::types::{Error, Instance, Loader, LoaderSpec, PackOrigin, Result};

/// Dossier des sauvegardes, à la racine du dossier de jeu (hors de `mods/`, il couvre aussi `config/`...).
pub const BACKUP_DIR: &str = ".cubelink-pack-backup";
/// Sauvegardes conservées par instance.
const KEEP_BACKUPS: usize = 3;

fn invalid(m: impl Into<String>) -> Error {
    Error::Invalid(m.into())
}

// ---------- Contrat (sérialisé vers l'interface) ----------

/// Mise à jour proposée pour un modpack installé.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct ModpackUpdate {
    /// Identifiant de la version Modrinth à installer.
    pub version_id: String,
    /// Nom affiché de la version (« Mon pack 1.4.2 »).
    pub name: String,
    /// Numéro de version (« 1.4.2 »).
    pub version_number: String,
    /// Date de publication ISO 8601.
    pub published: String,
}

/// Résultat d'une mise à jour appliquée.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct ModpackUpdateResult {
    pub added: u32,
    pub removed: u32,
    pub replaced: u32,
    /// Chemins (relatifs à l'instance) que le joueur a modifiés ou ajoutés lui-même et que la mise à jour n'a pas touchés.
    pub kept: Vec<String>,
    /// Sauvegarde à passer à `modpack_update_undo`.
    pub backup_id: String,
}

// ---------- Détection ----------

/// Version plus récente du modpack, compatible avec l'instance. `Ok(None)` : instance sans origine Modrinth, déjà à
/// jour, ou version installée introuvable (on ne propose jamais un retour en arrière). Une pré-version (beta, alpha)
/// n'est proposée que si la version installée en est déjà une.
pub async fn check(p: &Providers, inst: &Instance) -> Result<Option<ModpackUpdate>> {
    let Some(pack) = inst.pack.as_ref().filter(|o| o.source == "modrinth") else { return Ok(None) };
    let loader = Some(inst.loader.loader).filter(|l| *l != Loader::Vanilla);
    let list = browse::modrinth_versions(p, &pack.project_id, BrowseKind::Modpack, Some(&inst.minecraft), loader).await?;
    let installed = match list.iter().find(|v| v.id == pack.version_id) {
        Some(v) => v.clone(),
        // Retirée de la liste (autre Minecraft, dépubliée) : on n'a de quoi comparer que si l'API la connaît encore.
        None => match p.modrinth.version(&pack.version_id).await {
            Ok(v) => v,
            Err(Error::NotFound(_)) => return Ok(None),
            Err(e) => return Err(e),
        },
    };
    if installed.date_published.is_empty() {
        return Ok(None);
    }
    let installed_is_release = installed.version_type.is_empty() || installed.version_type == "release";
    // `list` est trié du plus récent au plus ancien.
    let newer = list.iter().find(|v| {
        v.id != installed.id
            && !v.files.is_empty()
            && v.date_published > installed.date_published
            && (!installed_is_release || v.version_type.is_empty() || v.version_type == "release")
    });
    Ok(newer.map(|v| ModpackUpdate {
        version_id: v.id.clone(),
        name: if v.name.trim().is_empty() { v.version_number.clone() } else { v.name.clone() },
        version_number: v.version_number.clone(),
        published: v.date_published.clone(),
    }))
}

// ---------- Empreintes ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Alg {
    Sha1,
    Sha512,
}

/// Empreinte d'un fichier du pack. Deux empreintes ne se comparent que si elles ont le même algorithme.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Digest {
    alg: Alg,
    hex: String,
}

impl Digest {
    fn of_index(f: &MrpackFile) -> Digest {
        if f.sha512.is_empty() {
            Digest { alg: Alg::Sha1, hex: f.sha1.to_ascii_lowercase() }
        } else {
            Digest { alg: Alg::Sha512, hex: f.sha512.to_ascii_lowercase() }
        }
    }

    /// Empreinte du fichier du disque avec le même algorithme que `self` (flux, pas de lecture complète en mémoire).
    fn disk_matches(&self, path: &Path) -> Result<bool> {
        Ok(hash_file(path, self.alg)? == self.hex)
    }
}

fn hash_file(path: &Path, alg: Alg) -> Result<String> {
    match alg {
        Alg::Sha1 => Ok(crate::unknown_mods::sha1_file(path)?),
        Alg::Sha512 => {
            let mut f = std::fs::File::open(path)?;
            let mut h = Sha512::new();
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = f.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                h.update(&buf[..n]);
            }
            Ok(hex::encode(h.finalize()))
        }
    }
}

/// SHA-512 de chaque override (même ordre que `entries`), lu en flux, taille bornée par celle déclarée.
fn hash_overrides(archive: &mut ZipArchive<std::fs::File>, entries: &[OverrideEntry]) -> Result<Vec<String>> {
    let mut out = Vec::with_capacity(entries.len());
    for e in entries {
        let entry = archive.by_index(e.index).map_err(|x| invalid(format!("{} : {x}", e.rel)))?;
        let declared = entry.size();
        let mut reader = entry.take(declared + 1);
        let mut h = Sha512::new();
        let mut buf = vec![0u8; 64 * 1024];
        let mut total = 0u64;
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            total += n as u64;
            h.update(&buf[..n]);
        }
        if total > declared {
            return Err(invalid(format!("{} : taille réelle supérieure à celle déclarée (zip bomb)", e.rel)));
        }
        out.push(hex::encode(h.finalize()));
    }
    Ok(out)
}

// ---------- État d'un pack ----------

#[derive(Debug, Clone)]
enum Source {
    /// Fichier de l'index (téléchargé), position dans `MrpackIndex::files`.
    Index(usize),
    /// Entrée de `overrides/` ou `client-overrides/`, position dans la liste des overrides.
    Over(usize),
}

#[derive(Debug, Clone)]
struct Item {
    path: String,
    digest: Digest,
    source: Source,
    /// Projet Modrinth du jar (déduit de l'URL), pour la provenance.
    provenance: Option<Provenance>,
}

/// Contenu effectif d'un pack : chemin (minuscules) -> fichier. Un override remplace le fichier de l'index au même chemin
/// (comme à l'import). Les fichiers que le client ne gère pas (`env.client = unsupported`) n'y figurent pas.
struct State {
    items: BTreeMap<String, Item>,
}

fn state_of(pack: &MrpackIndex, overrides: &[OverrideEntry], hashes: &[String]) -> State {
    let mut items = BTreeMap::new();
    for (i, f) in pack.files.iter().enumerate() {
        if f.client_unsupported() {
            continue;
        }
        items.insert(
            f.path.to_ascii_lowercase(),
            Item { path: f.path.clone(), digest: Digest::of_index(f), source: Source::Index(i), provenance: f.provenance.clone() },
        );
    }
    for (i, o) in overrides.iter().enumerate() {
        items.insert(
            o.rel.to_ascii_lowercase(),
            Item { path: o.rel.clone(), digest: Digest { alg: Alg::Sha512, hex: hashes[i].clone() }, source: Source::Over(i), provenance: None },
        );
    }
    State { items }
}

const PROTECTED_DIRS: &[&str] = &["saves", "screenshots", "logs", "crash-reports", "backups"];
const PROTECTED_FILES: &[&str] = &["options.txt", "optionsof.txt", "optionsshaders.txt", "servers.dat", "servers.dat_old", "usercache.json", "usernamecache.json"];

/// Données du joueur : jamais lues ni écrites par une mise à jour, même si le pack en fournit.
fn protected(rel: &str) -> bool {
    let l = rel.to_ascii_lowercase();
    let mut segs = l.split('/');
    let first = segs.next().unwrap_or("");
    (PROTECTED_DIRS.contains(&first) && l.contains('/')) || (!l.contains('/') && PROTECTED_FILES.contains(&first)) || l.split('/').any(|s| s.starts_with(".cubelink"))
}

// ---------- Plan ----------

/// Fichier présent sur le disque : nom réel (avec `.disabled` pour un mod désactivé).
enum Probe {
    Absent,
    File { actual: String, disabled: bool },
    /// Quelque chose d'autre qu'un fichier (dossier, lien) au chemin du pack : on n'y touche pas.
    Other,
}

fn probe(game_dir: &Path, rel: &str) -> Probe {
    let at = |r: &str| game_dir.join(r.split('/').collect::<PathBuf>());
    match std::fs::symlink_metadata(at(rel)) {
        Ok(m) if m.is_file() => return Probe::File { actual: rel.to_string(), disabled: false },
        Ok(_) => return Probe::Other,
        Err(_) => {}
    }
    if mrpack::mod_jar_name(rel).is_some() {
        let dis = format!("{rel}.disabled");
        match std::fs::symlink_metadata(at(&dis)) {
            Ok(m) if m.is_file() => return Probe::File { actual: dis, disabled: true },
            Ok(_) => return Probe::Other,
            Err(_) => {}
        }
    }
    Probe::Absent
}

fn disk_path(game_dir: &Path, rel: &str) -> PathBuf {
    game_dir.join(rel.split('/').collect::<PathBuf>())
}

#[derive(Debug, Clone)]
struct Old {
    /// Nom réel sur le disque.
    actual: String,
    disabled: bool,
    /// Projet du jar de l'ancien pack (pour recréer un jar désactivé sous sa nouvelle version).
    project: Option<String>,
}

#[derive(Debug, Clone)]
enum Action {
    /// Nouveau fichier du pack à poser à `dest`.
    Add { item: Item, dest: String },
    /// Fichier du pack changé : l'ancien (`old`) part dans la sauvegarde, le nouveau est posé à `dest`.
    Replace { item: Item, dest: String, old: Old },
    /// Fichier retiré du pack : il part dans la sauvegarde.
    Remove { old: Old },
}

#[derive(Debug, Default)]
struct Plan {
    actions: Vec<Action>,
    kept: Vec<String>,
}

impl Plan {
    fn counts(&self) -> (u32, u32, u32) {
        let c = |f: fn(&Action) -> bool| self.actions.iter().filter(|a| f(a)).count() as u32;
        (c(|a| matches!(a, Action::Add { .. })), c(|a| matches!(a, Action::Remove { .. })), c(|a| matches!(a, Action::Replace { .. })))
    }
}

/// Compare l'ancien pack, le nouveau pack et le disque. Lit le disque (empreintes) ; ne modifie rien.
fn plan(old: &State, new: &State, game_dir: &Path) -> Result<Plan> {
    let mut out = Plan::default();
    let mut kept: Vec<String> = Vec::new();
    for (key, n) in &new.items {
        if protected(&n.path) {
            continue;
        }
        let old_item = old.items.get(key);
        if old_item.is_some_and(|o| o.digest == n.digest) {
            continue; // inchangé entre les deux versions : le disque reste comme il est
        }
        match (probe(game_dir, &n.path), old_item) {
            (Probe::Absent, _) => out.actions.push(Action::Add { item: n.clone(), dest: n.path.clone() }),
            (Probe::Other, _) => kept.push(n.path.clone()),
            (Probe::File { actual, disabled }, o) => {
                let on_disk = disk_path(game_dir, &actual);
                if n.digest.disk_matches(&on_disk)? {
                    continue; // déjà la bonne version
                }
                match o {
                    Some(o) if o.digest.disk_matches(&on_disk)? => out.actions.push(Action::Replace {
                        item: n.clone(),
                        dest: actual.clone(),
                        old: Old { actual, disabled, project: o.provenance.as_ref().map(|p| p.id.clone()) },
                    }),
                    // Modifié par le joueur, ou fichier du joueur au même chemin : il garde sa version.
                    _ => kept.push(n.path.clone()),
                }
            }
        }
    }
    for (key, o) in &old.items {
        if new.items.contains_key(key) || protected(&o.path) {
            continue;
        }
        match probe(game_dir, &o.path) {
            Probe::Absent => {}
            Probe::Other => kept.push(o.path.clone()),
            Probe::File { actual, disabled } => {
                if o.digest.disk_matches(&disk_path(game_dir, &actual))? {
                    out.actions.push(Action::Remove { old: Old { actual, disabled, project: o.provenance.as_ref().map(|p| p.id.clone()) } });
                } else {
                    kept.push(o.path.clone());
                }
            }
        }
    }

    // Un mod désactivé par le joueur reste désactivé dans sa nouvelle version (même projet, autre nom de fichier).
    let disabled_projects: HashSet<String> =
        out.actions.iter().filter_map(|a| match a {
            Action::Remove { old } | Action::Replace { old, .. } if old.disabled => old.project.clone(),
            _ => None,
        }).collect();
    // Noms (de base) des jars qui quittent `mods/` avec cette mise à jour.
    let leaving: HashSet<String> = out
        .actions
        .iter()
        .filter_map(|a| match a {
            Action::Remove { old } | Action::Replace { old, .. } => mrpack::mod_jar_name(old.actual.trim_end_matches(".disabled")).map(str::to_string),
            _ => None,
        })
        .collect();
    let mut actions = Vec::with_capacity(out.actions.len());
    for a in std::mem::take(&mut out.actions) {
        match a {
            Action::Add { item, dest } => {
                let name = mrpack::mod_jar_name(&item.path).map(str::to_string);
                if let (Some(name), Some(p)) = (&name, &item.provenance) {
                    // Le joueur a déjà une autre version de ce mod (installée depuis le catalogue, par exemple) : l'ajouter
                    // donnerait deux jars du même mod, que le jeu refuse. Il garde la sienne.
                    let other = mods::files_of_project(game_dir, &p.source, &p.id)
                        .into_iter()
                        .find(|f| f != name && !leaving.contains(f) && !matches!(probe(game_dir, &format!("mods/{f}")), Probe::Absent));
                    if let Some(other) = other {
                        kept.push(format!("mods/{other}"));
                        continue;
                    }
                    if p.source == "modrinth" && disabled_projects.contains(&p.id) {
                        actions.push(Action::Add { item: item.clone(), dest: format!("{dest}.disabled") });
                        continue;
                    }
                }
                actions.push(Action::Add { item, dest });
            }
            other => actions.push(other),
        }
    }
    out.actions = actions;
    kept.sort();
    kept.dedup();
    out.kept = kept;
    Ok(out)
}

// ---------- Sauvegarde ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Moved {
    /// Nom réel sur le disque, relatif à l'instance.
    rel: String,
    /// Provenance du jar (clé = nom activé), remise à l'annulation.
    provenance: Option<Provenance>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Added {
    rel: String,
    /// Empreinte du fichier posé : si le joueur l'a modifié depuis, l'annulation n'y touche pas.
    digest: Digest,
    /// Objet du magasin lié (référence relâchée à l'annulation).
    sha256: Option<String>,
    /// Clé de provenance (nom activé du jar) enregistrée pour ce fichier.
    prov_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Manifest {
    id: String,
    from: Option<PackOrigin>,
    to: PackOrigin,
    loader_from: LoaderSpec,
    loader_to: LoaderSpec,
    moved: Vec<Moved>,
    added: Vec<Added>,
}

fn valid_backup_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn new_backup_id() -> String {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    format!("{ms:015}-{:03}", N.fetch_add(1, Ordering::Relaxed) % 1000)
}

fn backup_root(game_dir: &Path) -> PathBuf {
    game_dir.join(BACKUP_DIR)
}

fn write_manifest(bdir: &Path, m: &Manifest) -> Result<()> {
    std::fs::create_dir_all(bdir)?;
    let json = serde_json::to_vec_pretty(m).map_err(|e| Error::Other(e.to_string()))?;
    let tmp = bdir.join("manifest.json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, bdir.join("manifest.json"))?;
    Ok(())
}

fn prune_backups(game_dir: &Path) {
    let Ok(rd) = std::fs::read_dir(backup_root(game_dir)) else { return };
    let mut dirs: Vec<String> = rd.flatten().filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().into_string().ok()).collect();
    dirs.sort();
    while dirs.len() > KEEP_BACKUPS {
        let old = dirs.remove(0);
        let _ = std::fs::remove_dir_all(backup_root(game_dir).join(old));
    }
}

/// Clé de provenance (nom activé) d'un jar de `mods/` à partir de son nom réel sur le disque.
fn jar_key(rel: &str) -> Option<String> {
    mrpack::mod_jar_name(rel.trim_end_matches(".disabled")).map(str::to_string)
}

/// Défait l'opération décrite par `mf` : retire les fichiers ajoutés puis remet les fichiers déplacés. Tolère une
/// opération inachevée (fichier pas encore déplacé ou pas encore posé). `force` : retire les ajouts sans comparer leur
/// empreinte (échec en cours d'application : ce sont nos propres fichiers). Renvoie (erreurs, fichiers du joueur gardés).
fn revert(game_dir: &Path, bdir: &Path, mf: &Manifest, index: &StoreIndex, force: bool) -> (Vec<String>, Vec<String>) {
    let mut errors = Vec::new();
    let mut player_kept = Vec::new();
    let mut skip_restore: HashSet<String> = HashSet::new();
    let mut forget: Vec<String> = Vec::new();
    let backed_up = |rel: &str| bdir.join("files").join(rel.split('/').collect::<PathBuf>()).is_file();
    let moved_rels: HashSet<&str> = mf.moved.iter().map(|m| m.rel.as_str()).collect();
    for a in mf.added.iter().rev() {
        // Remplacement inachevé : l'ancien fichier n'a pas encore été déplacé, il est toujours à sa place. Ce n'est pas
        // notre ajout, on n'y touche pas.
        if moved_rels.contains(a.rel.as_str()) && !backed_up(&a.rel) {
            continue;
        }
        let path = disk_path(game_dir, &a.rel);
        match std::fs::symlink_metadata(&path) {
            Err(_) => {}
            Ok(m) if !m.is_file() => {
                errors.push(format!("{} : n'est plus un fichier", a.rel));
                skip_restore.insert(a.rel.clone());
            }
            Ok(_) => {
                let untouched = force || a.digest.disk_matches(&path).unwrap_or(false);
                if !untouched {
                    // Le joueur a modifié ce fichier depuis la mise à jour : il garde sa version.
                    player_kept.push(a.rel.clone());
                    skip_restore.insert(a.rel.clone());
                    continue;
                }
                match std::fs::remove_file(&path) {
                    Ok(()) => {
                        if let Some(s) = &a.sha256 {
                            let _ = index.release_ref(s);
                        }
                    }
                    Err(e) => {
                        errors.push(format!("{} : {e}", a.rel));
                        skip_restore.insert(a.rel.clone());
                        continue;
                    }
                }
            }
        }
        if let Some(k) = &a.prov_key {
            forget.push(k.clone());
        }
    }
    if !forget.is_empty() {
        if let Err(e) = mods::forget_provenance(game_dir, &forget) {
            errors.push(format!("provenance des mods : {e}"));
        }
    }
    let mut restore_prov: Vec<(String, Provenance)> = Vec::new();
    for m in mf.moved.iter().rev() {
        if skip_restore.contains(&m.rel) {
            continue;
        }
        let src = bdir.join("files").join(m.rel.split('/').collect::<PathBuf>());
        if !src.is_file() {
            continue; // jamais déplacé (opération inachevée) ou déjà remis
        }
        let dst = disk_path(game_dir, &m.rel);
        if dst.exists() {
            errors.push(format!("{} existe déjà", m.rel));
            continue;
        }
        let res = dst.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::rename(&src, &dst));
        match res {
            Ok(()) => {
                if let (Some(p), Some(k)) = (&m.provenance, jar_key(&m.rel)) {
                    restore_prov.push((k, p.clone()));
                }
            }
            Err(e) => errors.push(format!("{} : {e}", m.rel)),
        }
    }
    if !restore_prov.is_empty() {
        if let Err(e) = mods::record_provenance(game_dir, &restore_prov) {
            errors.push(format!("provenance des mods : {e}"));
        }
    }
    (errors, player_kept)
}

// ---------- Application ----------

/// Télécharge dans le magasin (sans toucher à l'instance) les fichiers d'index à poser. Clé : chemin du fichier.
async fn download_all(
    pack: &MrpackIndex,
    wanted: &[&Item],
    store_root: &Path,
    index: &StoreIndex,
    opts: &ImportOptions,
    progress: &Option<FilesProgress>,
    cancel: &Arc<AtomicBool>,
) -> Result<BTreeMap<String, StoredObject>> {
    let files: Vec<(&str, &MrpackFile)> = wanted
        .iter()
        .filter_map(|it| match it.source {
            Source::Index(i) => Some((it.path.as_str(), &pack.files[i])),
            Source::Over(_) => None,
        })
        .collect();
    let total = files.len() as u32;
    let dl = DlOptions { cancel: cancel.clone(), progress: None, ..opts.dl.clone() };
    let mut done = 0u32;
    let mut got = BTreeMap::new();
    let mut results = stream::iter(files.iter().copied())
        .map(|(path, f)| {
            let dl = &dl;
            async move { (path, mrpack::fetch_file(f, store_root, index, dl).await) }
        })
        .buffer_unordered(opts.concurrency.max(1));
    while let Some((path, res)) = results.next().await {
        got.insert(path.to_string(), res?);
        done += 1;
        if let Some(p) = progress {
            p(done, total);
        }
    }
    drop(results);
    if cancel.load(Ordering::SeqCst) {
        return Err(Error::Cancelled);
    }
    Ok(got)
}

/// Les deux packs lus et validés : tout ce qu'il faut pour planifier, sans avoir rien téléchargé ni modifié.
struct Prepared {
    old_state: State,
    new_state: State,
    new_idx: MrpackIndex,
    new_zip: ZipArchive<std::fs::File>,
    new_over: Vec<OverrideEntry>,
}

fn prepare(inst: &Instance, old_pack: &Path, new_pack: &Path, new_origin: &PackOrigin, opts: &ImportOptions) -> Result<Prepared> {
    let Some(current) = inst.pack.as_ref().filter(|o| o.source == "modrinth") else {
        return Err(invalid("cette instance n'a pas d'origine Modrinth connue : la mise à jour n'est pas possible"));
    };
    if new_origin.source != "modrinth" || new_origin.project_id != current.project_id {
        return Err(invalid("cette version n'appartient pas au modpack installé"));
    }
    let mut old_zip = mrpack::open_zip(old_pack)?;
    let mut new_zip = mrpack::open_zip(new_pack)?;
    let old_idx = mrpack::parse_index(old_pack)?;
    let new_idx = mrpack::parse_index(new_pack)?;
    for f in &new_idx.files {
        for u in &f.downloads {
            mrpack::check_url(u, opts)?;
        }
    }
    if new_idx.minecraft != inst.minecraft || new_idx.loader.loader != inst.loader.loader {
        return Err(invalid(
            "cette version change la version de Minecraft ou le loader : installe-la comme nouvelle instance pour garder tes mondes et tes mods",
        ));
    }
    let layout = Layout::mrpack();
    let old_over = mrpack::plan_overrides(&mut old_zip, opts, &layout)?;
    let new_over = mrpack::plan_overrides(&mut new_zip, opts, &layout)?;
    let old_hashes = hash_overrides(&mut old_zip, &old_over)?;
    let new_hashes = hash_overrides(&mut new_zip, &new_over)?;
    Ok(Prepared {
        old_state: state_of(&old_idx, &old_over, &old_hashes),
        new_state: state_of(&new_idx, &new_over, &new_hashes),
        new_idx,
        new_zip,
        new_over,
    })
}

/// Applique la mise à jour de `old_pack` vers `new_pack` sur l'instance `instance_id`.
///
/// `new_origin` : projet et version Modrinth de `new_pack` (enregistrés dans `Instance.pack` en dernier).
/// Les futurs de ce module lisent et écrivent le disque de façon synchrone : l'appelant l'exécute hors du fil
/// principal de l'interface (`spawn_blocking`).
#[allow(clippy::too_many_arguments)]
pub async fn apply(
    instances: &InstanceStore,
    instance_id: &str,
    old_pack: &Path,
    new_pack: &Path,
    new_origin: PackOrigin,
    store_root: &Path,
    index: &StoreIndex,
    opts: &ImportOptions,
    progress: Option<FilesProgress>,
    cancel: Arc<AtomicBool>,
) -> Result<ModpackUpdateResult> {
    let inst = instances.get(instance_id)?;
    let prepared = prepare(&inst, old_pack, new_pack, &new_origin, opts)?;
    apply_prepared(instances, &inst, prepared, new_origin, store_root, index, opts, &progress, &cancel).await
}

#[allow(clippy::too_many_arguments)]
async fn apply_prepared(
    instances: &InstanceStore,
    inst: &Instance,
    prepared: Prepared,
    new_origin: PackOrigin,
    store_root: &Path,
    index: &StoreIndex,
    opts: &ImportOptions,
    progress: &Option<FilesProgress>,
    cancel: &Arc<AtomicBool>,
) -> Result<ModpackUpdateResult> {
    let Prepared { old_state, new_state, new_idx, mut new_zip, new_over } = prepared;
    let (old_state, new_state, new_idx, new_zip, new_over) = (&old_state, &new_state, &new_idx, &mut new_zip, &new_over[..]);
    let game_dir = instances.game_dir(&inst.id)?;
    let disk_prov = mods::read_provenance(&game_dir);
    let plan = plan(old_state, new_state, &game_dir)?;
    let (added, removed, replaced) = plan.counts();

    let id = new_backup_id();
    let bdir = backup_root(&game_dir).join(&id);
    // Phase A : tout ce qui est nécessaire est téléchargé et extrait hors de l'instance. Un échec ici ne change rien.
    let wanted: Vec<&Item> = plan
        .actions
        .iter()
        .filter_map(|a| match a {
            Action::Add { item, .. } | Action::Replace { item, .. } => Some(item),
            Action::Remove { .. } => None,
        })
        .collect();
    let staged = (|| -> Result<()> {
        let subset: Vec<OverrideEntry> = wanted
            .iter()
            .filter_map(|it| match it.source {
                Source::Over(i) => Some(OverrideEntry { index: new_over[i].index, rel: new_over[i].rel.clone() }),
                Source::Index(_) => None,
            })
            .collect();
        if !subset.is_empty() {
            mrpack::extract_overrides(new_zip, &subset, &bdir.join("staged"), opts, cancel)?;
        }
        Ok(())
    })();
    let objects = match staged {
        Ok(()) => download_all(new_idx, &wanted, store_root, index, opts, progress, cancel).await,
        Err(e) => Err(e),
    };
    let objects = match objects {
        Ok(o) => o,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&bdir);
            return Err(e);
        }
    };

    // Phase B : modification de l'instance, manifeste écrit avant le premier déplacement.
    let new_loader = LoaderSpec { loader: new_idx.loader.loader, version: new_idx.loader.version.clone() };
    let mut mf = Manifest { id: id.clone(), from: inst.pack.clone(), to: new_origin.clone(), loader_from: inst.loader.clone(), loader_to: new_loader.clone(), moved: Vec::new(), added: Vec::new() };
    for a in &plan.actions {
        match a {
            Action::Remove { old } | Action::Replace { old, .. } => mf.moved.push(Moved {
                rel: old.actual.clone(),
                provenance: jar_key(&old.actual).and_then(|k| disk_prov.get(&k).cloned()),
            }),
            Action::Add { .. } => {}
        }
        match a {
            Action::Add { item, dest } | Action::Replace { item, dest, .. } => mf.added.push(Added {
                rel: dest.clone(),
                digest: item.digest.clone(),
                sha256: objects.get(&item.path).map(|o| o.sha256.clone()),
                prov_key: jar_key(dest).filter(|_| item.provenance.is_some()),
            }),
            Action::Remove { .. } => {}
        }
    }
    let fail = |e: Error, mf: &Manifest| -> Error {
        let (errors, _) = revert(&game_dir, &bdir, mf, index, true);
        if errors.is_empty() {
            let _ = std::fs::remove_dir_all(&bdir);
        } else {
            tracing::warn!("mise à jour du modpack : remise en état incomplète ({}) ; sauvegarde conservée : {}", errors.join(" ; "), bdir.display());
        }
        e
    };
    if let Err(e) = write_manifest(&bdir, &mf) {
        return Err(fail(e, &mf));
    }
    let run = || -> Result<()> {
        for m in &mf.moved {
            let src = disk_path(&game_dir, &m.rel);
            let dst = bdir.join("files").join(m.rel.split('/').collect::<PathBuf>());
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::rename(&src, &dst)?;
        }
        let forget: Vec<String> = mf.moved.iter().filter_map(|m| jar_key(&m.rel)).collect();
        mods::forget_provenance(&game_dir, &forget)?;
        let mut record: Vec<(String, Provenance)> = Vec::new();
        for a in &plan.actions {
            let (Action::Add { item, dest } | Action::Replace { item, dest, .. }) = a else { continue };
            match item.source {
                Source::Index(_) => {
                    let obj = objects.get(&item.path).ok_or_else(|| Error::Other(format!("{} : fichier non téléchargé", item.path)))?;
                    link_into(&game_dir, dest, &store_root.join(&obj.rel_path))?;
                    index.add_ref(&obj.sha256)?;
                }
                Source::Over(_) => {
                    let from = bdir.join("staged").join(item.path.split('/').collect::<PathBuf>());
                    let to = disk_path(&game_dir, dest);
                    if let Some(parent) = to.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::rename(&from, &to)?;
                }
            }
            if let (Some(k), Some(p)) = (jar_key(dest), &item.provenance) {
                record.push((k, p.clone()));
            }
        }
        mods::record_provenance(&game_dir, &record)?;
        let _ = std::fs::remove_dir_all(bdir.join("staged"));
        instances.set_pack(&inst.id, Some(new_origin.clone()))?;
        if new_loader != inst.loader {
            instances.set_loader(&inst.id, new_loader.clone())?;
        }
        Ok(())
    };
    if let Err(e) = run() {
        // Ce qui n'a pas été posé n'existe pas encore : `revert` tolère une opération inachevée.
        let _ = instances.set_pack(&inst.id, inst.pack.clone());
        let _ = instances.set_loader(&inst.id, inst.loader.clone());
        return Err(fail(e, &mf));
    }
    prune_backups(&game_dir);
    Ok(ModpackUpdateResult { added, removed, replaced, kept: plan.kept, backup_id: id })
}

/// Annule une mise à jour : retire les fichiers ajoutés, remet ceux qui avaient été déplacés, l'ancienne origine du pack
/// et l'ancien loader. Refuse si une mise à jour plus récente a eu lieu depuis. Un fichier que le joueur a modifié
/// depuis la mise à jour reste tel quel. Renvoie l'instance.
pub fn undo(instances: &InstanceStore, instance_id: &str, backup_id: &str, index: &StoreIndex) -> Result<Instance> {
    if !valid_backup_id(backup_id) {
        return Err(invalid("identifiant de sauvegarde invalide"));
    }
    let inst = instances.get(instance_id)?;
    let game_dir = instances.game_dir(instance_id)?;
    let bdir = backup_root(&game_dir).join(backup_id);
    let raw = std::fs::read(bdir.join("manifest.json")).map_err(|_| Error::NotFound("sauvegarde introuvable (déjà annulée ou supprimée)".into()))?;
    let mf: Manifest = serde_json::from_slice(&raw).map_err(|e| invalid(format!("sauvegarde illisible : {e}")))?;
    if inst.pack.as_ref() != Some(&mf.to) {
        return Err(invalid("une mise à jour plus récente a eu lieu depuis : annule d'abord celle-là"));
    }
    let (errors, player_kept) = revert(&game_dir, &bdir, &mf, index, false);
    if !errors.is_empty() {
        return Err(Error::Other(format!("annulation incomplète ({}) ; la sauvegarde est conservée dans {BACKUP_DIR}/{backup_id}", errors.join(" ; "))));
    }
    if !player_kept.is_empty() {
        tracing::info!("annulation : fichiers modifiés depuis la mise à jour, laissés tels quels : {}", player_kept.join(", "));
    }
    instances.set_pack(instance_id, mf.from.clone())?;
    let updated = if mf.loader_from != inst.loader { instances.set_loader(instance_id, mf.loader_from.clone())? } else { instances.get(instance_id)? };
    let _ = std::fs::remove_dir_all(&bdir);
    Ok(updated)
}

#[cfg(test)]
mod tests;
