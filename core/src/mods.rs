//! Gestion des mods d'une instance : liste, activation, suppression, ajout manuel, provenance.
//!
//! La provenance est écrite dans `<instance>/.cubelink-mods.json` (hors du dossier de jeu, donc jamais
//! partagée en P2P) : `{ "<fichier>": { "source": "curseforge"|"modrinth"|"custom", "id": "..." } }`.
//! La clé est le nom du fichier ACTIVÉ (sans `.disabled`) : activer / désactiver ne la modifie pas.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::types::{Error, Result};

pub const PROVENANCE_FILE: &str = ".cubelink-mods.json";
/// Taille maximale d'un mod ajouté à la main.
pub const MAX_CUSTOM_BYTES: u64 = 512 * 1024 * 1024;
const DISABLED: &str = ".disabled";
/// Extensions exécutables refusées dans un nom de fichier.
const DANGEROUS_EXT: &[&str] = &["exe", "bat", "cmd", "dll", "ps1", "com", "scr", "msi", "vbs", "lnk", "sh"];

static LOCK: Mutex<()> = Mutex::new(());

fn guard() -> std::sync::MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModEntry {
    /// Nom actuel du fichier (finit par `.disabled` si le mod est désactivé).
    pub file: String,
    pub enabled: bool,
    pub size: u64,
    /// Ajouté à la main par l'utilisateur.
    pub custom: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    pub source: String,
    pub id: String,
}

fn invalid(msg: impl Into<String>) -> Error {
    Error::Invalid(msg.into())
}

/// Nom de fichier simple : pas de séparateur, `..`, `:`, caractère de contrôle, extension exécutable.
pub fn validate_file_name(name: &str) -> Result<()> {
    let bad = name.is_empty()
        || name.len() > 200
        || name.starts_with('.')
        || name.contains("..")
        || name.contains(['/', '\\', ':', '\0', '*', '?', '"', '<', '>', '|'])
        || name.chars().any(|c| c.is_control())
        || name.ends_with('.')
        || name.ends_with(' ');
    if bad {
        return Err(invalid(format!("nom de fichier invalide : {name:?}")));
    }
    let base = name.strip_suffix(DISABLED).unwrap_or(name);
    if let Some((_, ext)) = base.rsplit_once('.') {
        if DANGEROUS_EXT.iter().any(|d| ext.eq_ignore_ascii_case(d)) {
            return Err(invalid(format!("extension refusée : {name:?}")));
        }
    }
    Ok(())
}

fn lower(s: &str) -> String {
    s.to_ascii_lowercase()
}

/// `Some(base)` si le nom finit par `.jar` ou `.jar.disabled`, `base` étant le nom activé.
fn jar_base(name: &str) -> Option<String> {
    let l = lower(name);
    if l.ends_with(".jar") {
        Some(name.to_string())
    } else if l.ends_with(".jar.disabled") {
        Some(name[..name.len() - DISABLED.len()].to_string())
    } else {
        None
    }
}

fn validate_mod_name(name: &str) -> Result<String> {
    validate_file_name(name)?;
    jar_base(name).ok_or_else(|| invalid("seuls les fichiers .jar (ou .jar.disabled) sont des mods"))
}

fn mods_dir(game_dir: &Path) -> PathBuf {
    game_dir.join("mods")
}

fn prov_path(game_dir: &Path) -> PathBuf {
    game_dir.parent().unwrap_or(game_dir).join(PROVENANCE_FILE)
}

pub fn read_provenance(game_dir: &Path) -> BTreeMap<String, Provenance> {
    std::fs::read(prov_path(game_dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn write_provenance(game_dir: &Path, map: &BTreeMap<String, Provenance>) -> Result<()> {
    let path = prov_path(game_dir);
    if map.is_empty() {
        // Rien à retenir : pas de fichier (l'état d'une instance sans provenance reste identique).
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        };
    }
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let json = serde_json::to_vec_pretty(map).map_err(|e| Error::Other(e.to_string()))?;
    std::fs::write(&tmp, json)?;
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

/// Enregistre la provenance de fichiers (clés : noms activés).
pub fn record_provenance(game_dir: &Path, entries: &[(String, Provenance)]) -> Result<()> {
    let _g = guard();
    let mut map = read_provenance(game_dir);
    for (f, p) in entries {
        map.insert(f.clone(), p.clone());
    }
    write_provenance(game_dir, &map)
}

fn forget(game_dir: &Path, base: &str) -> Result<()> {
    let mut map = read_provenance(game_dir);
    if map.remove(base).is_some() {
        write_provenance(game_dir, &map)?;
    }
    Ok(())
}

/// Oublie la provenance de fichiers (noms activés ou désactivés), sans toucher aux fichiers.
pub fn forget_provenance(game_dir: &Path, files: &[String]) -> Result<()> {
    let _g = guard();
    let mut map = read_provenance(game_dir);
    let mut changed = false;
    for f in files {
        let base = jar_base(f).unwrap_or_else(|| f.clone());
        changed |= map.remove(&base).is_some();
    }
    if changed {
        write_provenance(game_dir, &map)?;
    }
    Ok(())
}

/// Liste les `.jar` et `.jar.disabled` de `mods/`, triés par nom.
pub fn list(game_dir: &Path) -> Result<Vec<ModEntry>> {
    let prov = read_provenance(game_dir);
    let mut out = Vec::new();
    let rd = match std::fs::read_dir(mods_dir(game_dir)) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e.into()),
    };
    for e in rd.flatten() {
        let Ok(name) = e.file_name().into_string() else { continue };
        if name.starts_with('.') {
            continue;
        }
        let Some(base) = jar_base(&name) else { continue };
        let Ok(meta) = e.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let custom = prov.get(&base).is_some_and(|p| p.source == "custom");
        out.push(ModEntry { enabled: name == base, file: name, size: meta.len(), custom });
    }
    out.sort_by_key(|m| lower(&m.file));
    Ok(out)
}

/// Icône d'un mod de `mods/` (data URL), `None` s'il n'en a pas. `file` : nom actuel du fichier.
pub fn icon(game_dir: &Path, file: &str) -> Result<Option<String>> {
    validate_mod_name(file)?;
    Ok(crate::mod_icon::icon_data_url(&mods_dir(game_dir).join(file)))
}

/// Active ou désactive un mod (renomme `.jar` <-> `.jar.disabled`). `file` : nom actuel ou nom activé.
pub fn toggle(game_dir: &Path, file: &str, enabled: bool) -> Result<()> {
    let base = validate_mod_name(file)?;
    let _g = guard();
    let dir = mods_dir(game_dir);
    let on = dir.join(&base);
    let off = dir.join(format!("{base}{DISABLED}"));
    let (from, to) = if enabled { (off, on) } else { (on, off) };
    if !from.is_file() {
        if to.is_file() {
            return Ok(()); // déjà dans l'état voulu
        }
        return Err(Error::NotFound(format!("mod {base}")));
    }
    if to.exists() {
        return Err(invalid(format!("{} existe déjà : supprime l'un des deux fichiers", to.file_name().and_then(|n| n.to_str()).unwrap_or(""))));
    }
    std::fs::rename(from, to)?;
    Ok(())
}

/// Supprime un mod (activé ou désactivé) et sa provenance.
pub fn remove(game_dir: &Path, file: &str) -> Result<()> {
    let base = validate_mod_name(file)?;
    let _g = guard();
    let dir = mods_dir(game_dir);
    let mut removed = false;
    for p in [dir.join(&base), dir.join(format!("{base}{DISABLED}"))] {
        if p.is_file() {
            std::fs::remove_file(p)?;
            removed = true;
        }
    }
    if !removed {
        return Err(Error::NotFound(format!("mod {base}")));
    }
    forget(game_dir, &base)
}

/// Premier nom libre (`a.jar`, `a-1.jar`, `a-2.jar`…), en tenant compte des fichiers désactivés.
fn unique_name(dir: &Path, base: &str) -> String {
    let taken = |n: &str| dir.join(n).exists() || dir.join(format!("{n}{DISABLED}")).exists();
    if !taken(base) {
        return base.to_string();
    }
    let stem = &base[..base.len() - ".jar".len()];
    (1..)
        .map(|i| format!("{stem}-{i}.jar"))
        .find(|n| !taken(n))
        .unwrap_or_else(|| base.to_string())
}

/// Copie un `.jar` local dans `mods/` (jamais d'écrasement : suffixe `-1`, `-2`…) et le marque `custom`.
pub fn add(game_dir: &Path, src: &Path) -> Result<ModEntry> {
    let name = src.file_name().and_then(|n| n.to_str()).ok_or_else(|| invalid("nom de fichier illisible"))?;
    validate_file_name(name)?;
    if !lower(name).ends_with(".jar") {
        return Err(invalid("seuls les fichiers .jar peuvent être ajoutés comme mods"));
    }
    let meta = std::fs::metadata(src).map_err(|_| Error::NotFound("fichier introuvable".into()))?;
    if !meta.is_file() {
        return Err(invalid("ce n'est pas un fichier"));
    }
    if meta.len() > MAX_CUSTOM_BYTES {
        return Err(invalid("fichier trop gros (512 Mio maximum)"));
    }
    let _g = guard();
    let dir = mods_dir(game_dir);
    std::fs::create_dir_all(&dir)?;
    let final_name = unique_name(&dir, name);
    let tmp = dir.join(format!(".{final_name}.cubelink-add-{}.tmp", std::process::id()));
    if let Err(e) = std::fs::copy(src, &tmp) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    if let Err(e) = std::fs::rename(&tmp, dir.join(&final_name)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    let mut map = read_provenance(game_dir);
    map.insert(final_name.clone(), Provenance { source: "custom".into(), id: String::new() });
    write_provenance(game_dir, &map)?;
    Ok(ModEntry { file: final_name, enabled: true, size: meta.len(), custom: true })
}

/// Fichiers de `mods/` provenant du projet `source`/`id` (noms activés), pour les mises à jour.
pub fn files_of_project(game_dir: &Path, source: &str, id: &str) -> Vec<String> {
    read_provenance(game_dir).into_iter().filter(|(_, p)| p.source == source && p.id == id).map(|(f, _)| f).collect()
}

/// Supprime les anciens fichiers d'un projet (hors `keep`), activés ou désactivés, et leur provenance.
pub fn remove_old_versions(game_dir: &Path, source: &str, id: &str, keep: &[String]) -> Result<()> {
    let _g = guard();
    let mut map = read_provenance(game_dir);
    let dir = mods_dir(game_dir);
    let old: Vec<String> = map.iter().filter(|(f, p)| p.source == source && p.id == id && !keep.contains(f)).map(|(f, _)| f.clone()).collect();
    for f in &old {
        if validate_file_name(f).is_err() {
            continue;
        }
        for p in [dir.join(f), dir.join(format!("{f}{DISABLED}"))] {
            if p.is_file() {
                let _ = std::fs::remove_file(p);
            }
        }
        map.remove(f);
    }
    if !old.is_empty() {
        write_provenance(game_dir, &map)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let game = d.path().join("inst").join("minecraft");
        std::fs::create_dir_all(game.join("mods")).unwrap();
        (d, game)
    }

    fn put(game: &Path, name: &str, body: &[u8]) {
        std::fs::write(game.join("mods").join(name), body).unwrap();
    }

    #[test]
    fn list_filters_sorts_and_flags() {
        let (_d, g) = setup();
        put(&g, "b.jar", b"12");
        put(&g, "A.jar.disabled", b"1");
        put(&g, "notes.txt", b"x");
        put(&g, ".hidden.jar", b"x");
        std::fs::create_dir(g.join("mods").join("dir.jar")).unwrap();
        record_provenance(&g, &[("b.jar".into(), Provenance { source: "custom".into(), id: String::new() })]).unwrap();
        let l = list(&g).unwrap();
        assert_eq!(l.len(), 2);
        assert_eq!((l[0].file.as_str(), l[0].enabled, l[0].size, l[0].custom), ("A.jar.disabled", false, 1, false));
        assert_eq!((l[1].file.as_str(), l[1].enabled, l[1].custom), ("b.jar", true, true));
        assert!(list(&g.join("nope")).unwrap().is_empty());
    }

    #[test]
    fn toggle_roundtrip_and_errors() {
        let (_d, g) = setup();
        put(&g, "a.jar", b"1");
        toggle(&g, "a.jar", false).unwrap();
        assert!(g.join("mods/a.jar.disabled").is_file() && !g.join("mods/a.jar").exists());
        toggle(&g, "a.jar.disabled", false).unwrap(); // déjà désactivé : sans effet
        toggle(&g, "a.jar.disabled", true).unwrap();
        assert!(g.join("mods/a.jar").is_file());
        assert!(matches!(toggle(&g, "zz.jar", true), Err(Error::NotFound(_))));
        put(&g, "a.jar.disabled", b"2"); // les deux existent
        assert!(toggle(&g, "a.jar", false).is_err());
        for bad in ["../a.jar", "a/b.jar", "a\\b.jar", "..", "", "x.txt", "C:a.jar", ".jar", "a.jar\0"] {
            assert!(toggle(&g, bad, true).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn remove_deletes_both_and_provenance() {
        let (_d, g) = setup();
        put(&g, "a.jar.disabled", b"1");
        record_provenance(&g, &[("a.jar".into(), Provenance { source: "modrinth".into(), id: "p".into() })]).unwrap();
        remove(&g, "a.jar.disabled").unwrap();
        assert!(list(&g).unwrap().is_empty());
        assert!(read_provenance(&g).is_empty());
        assert!(matches!(remove(&g, "a.jar"), Err(Error::NotFound(_))));
        assert!(remove(&g, "../x.jar").is_err());
    }

    #[test]
    fn add_never_overwrites_and_marks_custom() {
        let (d, g) = setup();
        let src = d.path().join("MyMod.jar");
        std::fs::write(&src, b"abc").unwrap();
        let a = add(&g, &src).unwrap();
        assert_eq!((a.file.as_str(), a.enabled, a.size, a.custom), ("MyMod.jar", true, 3, true));
        std::fs::write(&src, b"abcd").unwrap();
        let b = add(&g, &src).unwrap();
        assert_eq!(b.file, "MyMod-1.jar");
        toggle(&g, "MyMod-1.jar", false).unwrap();
        assert_eq!(add(&g, &src).unwrap().file, "MyMod-2.jar");
        assert_eq!(std::fs::read(g.join("mods/MyMod.jar")).unwrap(), b"abc");
        let l = list(&g).unwrap();
        assert!(l.iter().all(|m| m.custom));
        assert_eq!(read_provenance(&g)["MyMod.jar"], Provenance { source: "custom".into(), id: String::new() });
        assert_eq!(std::fs::read_dir(g.join("mods")).unwrap().count(), 3, "aucun temporaire résiduel");
    }

    #[test]
    fn add_rejects_bad_input() {
        let (d, g) = setup();
        let txt = d.path().join("a.txt");
        std::fs::write(&txt, b"x").unwrap();
        assert!(add(&g, &txt).is_err());
        let exe = d.path().join("a.jar.exe");
        std::fs::write(&exe, b"x").unwrap();
        assert!(add(&g, &exe).is_err());
        assert!(matches!(add(&g, &d.path().join("absent.jar")), Err(Error::NotFound(_))));
        let dir = d.path().join("d.jar");
        std::fs::create_dir(&dir).unwrap();
        assert!(add(&g, &dir).is_err());
        assert!(add(&g, Path::new("")).is_err());
    }

    #[test]
    fn add_refuses_over_512_mib() {
        let (d, g) = setup();
        let big = d.path().join("big.jar");
        let f = std::fs::File::create(&big).unwrap();
        f.set_len(MAX_CUSTOM_BYTES + 1).unwrap(); // fichier creux
        drop(f);
        let e = add(&g, &big).unwrap_err();
        assert!(e.to_string().contains("512"), "{e}");
        assert!(list(&g).unwrap().is_empty());
    }

    #[test]
    fn project_updates_remove_old_files() {
        let (_d, g) = setup();
        put(&g, "a-1.jar", b"1");
        put(&g, "a-2.jar", b"2");
        put(&g, "other.jar", b"3");
        let p = |id: &str| Provenance { source: "modrinth".into(), id: id.into() };
        record_provenance(&g, &[("a-1.jar".into(), p("A")), ("a-2.jar".into(), p("A")), ("other.jar".into(), p("B"))]).unwrap();
        remove_old_versions(&g, "modrinth", "A", &["a-2.jar".to_string()]).unwrap();
        let names: Vec<_> = list(&g).unwrap().into_iter().map(|m| m.file).collect();
        assert_eq!(names, vec!["a-2.jar", "other.jar"]);
        assert_eq!(files_of_project(&g, "modrinth", "A"), vec!["a-2.jar"]);
    }

    #[test]
    fn name_validation() {
        for ok in ["a.jar", "My Mod [1.20]+x.jar", "a.jar.disabled", "x.zip"] {
            assert!(validate_file_name(ok).is_ok(), "{ok}");
        }
        for bad in ["a..jar", "a.exe", "run.bat.disabled", "a/b", ".h", "a:b.jar", "x\n.jar"] {
            assert!(validate_file_name(bad).is_err(), "{bad:?}");
        }
    }
}
