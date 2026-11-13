//! Métadonnées d'un mod lues dans son `.jar` (hors ligne) : identifiant, version, dépendances, jars imbriqués.
//!
//! Formats : `fabric.mod.json`, `quilt.mod.json`, `META-INF/neoforge.mods.toml`, `META-INF/mods.toml`, `mcmod.info`.
//! Jars imbriqués : `jars` (Fabric / Quilt) et `META-INF/jarjar/metadata.json` (Forge / NeoForge), sur deux niveaux.
//! Cache mémoire par chemin + empreinte (taille, date).

use std::collections::HashMap;
use std::io::{Cursor, Read, Seek};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::Value;

use crate::version_range::Flavor;

/// Descripteurs de métadonnées lus au plus à cette taille.
const MAX_META_BYTES: u64 = 1024 * 1024;
/// Jar imbriqué lu en mémoire au plus à cette taille.
const MAX_NESTED_BYTES: u64 = 8 * 1024 * 1024;
/// Nombre maximal de jars imbriqués ouverts pour un jar racine.
const MAX_NESTED_JARS: u32 = 128;
/// Niveaux d'imbrication suivis (Fabric API : racine -> modules).
const NESTED_DEPTH: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoaderKind {
    Fabric,
    Quilt,
    Forge,
    NeoForge,
    /// `mcmod.info` : vieux Forge, aucune règle de dépendance.
    Legacy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepKind {
    Required,
    /// Fabric / Quilt `breaks`.
    Breaks,
    /// Forge / NeoForge `type = "incompatible"`.
    Incompatible,
    /// Fabric `conflicts`, NeoForge `discouraged` (avertissement).
    Conflicts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dep {
    pub id: String,
    /// Plage brute (`*` ou vide : toutes versions). Un tableau Fabric est joint par ` || `.
    pub range: String,
    pub kind: DepKind,
    /// `BOTH`, `CLIENT` ou `SERVER` (vide : `BOTH`).
    pub side: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModMeta {
    pub id: String,
    pub name: Option<String>,
    /// `None` : version inconnue (modèle non résolu, absente).
    pub version: Option<String>,
    pub kind: LoaderKind,
    /// Autres ids fournis (`provides`), avec leur version propre le cas échéant.
    pub provides: Vec<(String, Option<String>)>,
    pub deps: Vec<Dep>,
}

impl ModMeta {
    pub fn flavor(&self) -> Flavor {
        match self.kind {
            LoaderKind::Fabric | LoaderKind::Quilt => Flavor::Fabric,
            _ => Flavor::Maven,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JarMeta {
    /// Mods déclarés à la racine du jar (tous formats confondus).
    pub mods: Vec<ModMeta>,
    /// Ids fournis par les jars imbriqués (id, version).
    pub nested: Vec<(String, Option<String>)>,
    pub has_fabric: bool,
    pub has_quilt: bool,
    /// `META-INF/mods.toml`.
    pub has_forge: bool,
    /// `META-INF/neoforge.mods.toml`.
    pub has_neoforge: bool,
    pub has_legacy: bool,
}

impl JarMeta {
    pub fn is_empty(&self) -> bool {
        !(self.has_fabric || self.has_quilt || self.has_forge || self.has_neoforge || self.has_legacy)
    }
}

/// Repli d'un jar imbriqué illisible : (artifact, version).
type Nested = (String, Option<String>);

type Cache = Mutex<HashMap<String, (String, Option<Arc<JarMeta>>)>>;
static CACHE: OnceLock<Cache> = OnceLock::new();

fn stamp(meta: &std::fs::Metadata) -> String {
    let ns = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
    format!("{}-{ns}", meta.len())
}

/// Métadonnées du jar `path` (cache chemin + taille + date). `None` si le jar est illisible.
pub fn jar_meta(path: &Path) -> Option<Arc<JarMeta>> {
    let meta = std::fs::metadata(path).ok().filter(|m| m.is_file())?;
    let key = path.display().to_string();
    let st = stamp(&meta);
    let cache = CACHE.get_or_init(Default::default);
    if let Some((s, v)) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        if *s == st {
            return v.clone();
        }
    }
    let v = read_jar(path).map(Arc::new);
    cache.lock().unwrap_or_else(|e| e.into_inner()).insert(key, (st, v.clone()));
    v
}

fn read_jar(path: &Path) -> Option<JarMeta> {
    let f = std::fs::File::open(path).ok()?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(f)).ok()?;
    let mut budget = MAX_NESTED_JARS;
    Some(scan(&mut zip, NESTED_DEPTH, &mut budget))
}

fn read_entry<R: Read + Seek>(zip: &mut zip::ZipArchive<R>, name: &str, max: u64) -> Option<Vec<u8>> {
    let name = name.trim_start_matches('/');
    if name.is_empty() || name.contains("..") {
        return None;
    }
    let f = zip.by_name(name).ok()?;
    if f.size() > max {
        return None;
    }
    let mut buf = Vec::with_capacity(f.size() as usize);
    f.take(max + 1).read_to_end(&mut buf).ok()?;
    (buf.len() as u64 <= max).then_some(buf)
}

fn manifest_version(zip: &mut zip::ZipArchive<impl Read + Seek>) -> Option<String> {
    let b = read_entry(zip, "META-INF/MANIFEST.MF", MAX_META_BYTES)?;
    let text = String::from_utf8_lossy(&b);
    text.lines().find_map(|l| {
        let v = l.strip_prefix("Implementation-Version:")?.trim();
        (!v.is_empty()).then(|| v.to_string())
    })
}

fn scan<R: Read + Seek>(zip: &mut zip::ZipArchive<R>, depth: u32, budget: &mut u32) -> JarMeta {
    let mut jm = JarMeta::default();
    let mut nested_paths: Vec<(String, Option<Nested>)> = Vec::new();

    if let Some(b) = read_entry(zip, "fabric.mod.json", MAX_META_BYTES) {
        jm.has_fabric = true;
        if let Ok(v) = serde_json::from_slice::<Value>(&b) {
            if let Some(m) = parse_fabric(&v) {
                jm.mods.push(m);
            }
            for j in v.get("jars").and_then(|j| j.as_array()).into_iter().flatten() {
                if let Some(f) = j.get("file").and_then(|f| f.as_str()) {
                    nested_paths.push((f.to_string(), None));
                }
            }
        }
    }
    if let Some(b) = read_entry(zip, "quilt.mod.json", MAX_META_BYTES) {
        jm.has_quilt = true;
        if let Ok(v) = serde_json::from_slice::<Value>(&b) {
            if let Some(m) = parse_quilt(&v) {
                jm.mods.push(m);
            }
            for j in v.pointer("/quilt_loader/jars").and_then(|j| j.as_array()).into_iter().flatten() {
                if let Some(f) = j.as_str() {
                    nested_paths.push((f.to_string(), None));
                }
            }
        }
    }
    let mf = manifest_version(zip);
    if let Some(b) = read_entry(zip, "META-INF/neoforge.mods.toml", MAX_META_BYTES) {
        jm.has_neoforge = true;
        jm.mods.extend(parse_toml(&String::from_utf8_lossy(&b), LoaderKind::NeoForge, mf.as_deref()));
    }
    if let Some(b) = read_entry(zip, "META-INF/mods.toml", MAX_META_BYTES) {
        jm.has_forge = true;
        jm.mods.extend(parse_toml(&String::from_utf8_lossy(&b), LoaderKind::Forge, mf.as_deref()));
    }
    if let Some(b) = read_entry(zip, "mcmod.info", MAX_META_BYTES) {
        jm.has_legacy = true;
        if let Ok(v) = serde_json::from_slice::<Value>(&b) {
            jm.mods.extend(parse_mcmod(&v));
        }
    }
    if let Some(b) = read_entry(zip, "META-INF/jarjar/metadata.json", MAX_META_BYTES) {
        if let Ok(v) = serde_json::from_slice::<Value>(&b) {
            for j in v.get("jars").and_then(|j| j.as_array()).into_iter().flatten() {
                let Some(path) = j.get("path").and_then(|p| p.as_str()) else { continue };
                let artifact = j.pointer("/identifier/artifact").and_then(|a| a.as_str()).map(String::from);
                let ver = j.pointer("/version/artifactVersion").and_then(|a| a.as_str()).map(String::from);
                nested_paths.push((path.to_string(), artifact.map(|a| (a, ver))));
            }
        }
    }

    if depth == 0 {
        return jm;
    }
    for (path, fallback) in nested_paths {
        if *budget == 0 {
            break;
        }
        *budget -= 1;
        let Some(bytes) = read_entry(zip, &path, MAX_NESTED_BYTES) else {
            if let Some((a, v)) = fallback {
                jm.nested.push((a, v));
            }
            continue;
        };
        let Ok(mut inner) = zip::ZipArchive::new(Cursor::new(bytes)) else {
            if let Some((a, v)) = fallback {
                jm.nested.push((a, v));
            }
            continue;
        };
        let sub = scan(&mut inner, depth - 1, budget);
        if sub.mods.is_empty() {
            if let Some((a, v)) = fallback {
                jm.nested.push((a, v));
            }
        }
        for m in &sub.mods {
            jm.nested.push((m.id.clone(), m.version.clone()));
            for (p, pv) in &m.provides {
                jm.nested.push((p.clone(), pv.clone().or_else(|| m.version.clone())));
            }
        }
        jm.nested.extend(sub.nested);
    }
    jm
}

fn clean_version(v: Option<&Value>) -> Option<String> {
    let s = v?.as_str()?.trim();
    (!s.is_empty() && !s.contains("${")).then(|| s.to_string())
}

/// Plage Fabric / Quilt : chaîne, ou tableau de chaînes (OU).
fn range_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim().to_string(),
        Value::Array(a) => {
            let parts: Vec<&str> = a.iter().filter_map(|x| x.as_str()).map(str::trim).collect();
            if parts.is_empty() {
                "*".into()
            } else {
                parts.join(" || ")
            }
        }
        _ => "*".into(),
    }
}

fn provides_of(v: Option<&Value>) -> Vec<(String, Option<String>)> {
    v.and_then(|p| p.as_array())
        .into_iter()
        .flatten()
        .filter_map(|p| match p {
            Value::String(s) => Some((s.clone(), None)),
            Value::Object(_) => Some((p.get("id")?.as_str()?.to_string(), clean_version(p.get("version")))),
            _ => None,
        })
        .collect()
}

fn fabric_deps(v: &Value, key: &str, kind: DepKind, out: &mut Vec<Dep>) {
    if let Some(o) = v.get(key).and_then(|d| d.as_object()) {
        for (id, r) in o {
            out.push(Dep { id: id.clone(), range: range_of(r), kind, side: String::new() });
        }
    }
}

fn parse_fabric(v: &Value) -> Option<ModMeta> {
    let id = v.get("id")?.as_str()?.to_string();
    let mut deps = Vec::new();
    fabric_deps(v, "depends", DepKind::Required, &mut deps);
    fabric_deps(v, "breaks", DepKind::Breaks, &mut deps);
    fabric_deps(v, "conflicts", DepKind::Conflicts, &mut deps);
    Some(ModMeta {
        name: v.get("name").and_then(|n| n.as_str()).filter(|n| !n.is_empty()).map(String::from),
        version: clean_version(v.get("version")),
        kind: LoaderKind::Fabric,
        provides: provides_of(v.get("provides")),
        deps,
        id,
    })
}

fn quilt_deps(list: Option<&Value>, kind: DepKind, out: &mut Vec<Dep>) {
    for d in list.and_then(|l| l.as_array()).into_iter().flatten() {
        match d {
            Value::String(id) => out.push(Dep { id: id.clone(), range: "*".into(), kind, side: String::new() }),
            Value::Object(o) => {
                if o.get("optional").and_then(|b| b.as_bool()) == Some(true) || o.contains_key("unless") {
                    continue;
                }
                let Some(id) = o.get("id").and_then(|i| i.as_str()) else { continue };
                let range = match o.get("versions") {
                    Some(r @ (Value::String(_) | Value::Array(_))) => range_of(r),
                    _ => "*".into(),
                };
                out.push(Dep { id: id.to_string(), range, kind, side: String::new() });
            }
            _ => {}
        }
    }
}

fn parse_quilt(v: &Value) -> Option<ModMeta> {
    let q = v.get("quilt_loader")?;
    let id = q.get("id")?.as_str()?.to_string();
    let mut deps = Vec::new();
    quilt_deps(q.get("depends"), DepKind::Required, &mut deps);
    quilt_deps(q.get("breaks"), DepKind::Breaks, &mut deps);
    Some(ModMeta {
        name: q.pointer("/metadata/name").and_then(|n| n.as_str()).filter(|n| !n.is_empty()).map(String::from),
        version: clean_version(q.get("version")),
        kind: LoaderKind::Quilt,
        provides: provides_of(q.get("provides")),
        deps,
        id,
    })
}

fn parse_mcmod(v: &Value) -> Vec<ModMeta> {
    let list = v.as_array().or_else(|| v.get("modList").and_then(|l| l.as_array()));
    list.into_iter()
        .flatten()
        .filter_map(|m| {
            let id = m.get("modid")?.as_str()?.to_string();
            Some(ModMeta {
                name: m.get("name").and_then(|n| n.as_str()).filter(|n| !n.is_empty()).map(String::from),
                version: clean_version(m.get("version")),
                kind: LoaderKind::Legacy,
                provides: Vec::new(),
                deps: Vec::new(),
                id,
            })
        })
        .collect()
}

fn toml_version(v: Option<&toml::Value>, manifest: Option<&str>) -> Option<String> {
    let s = v?.as_str()?.trim();
    if s == "${file.jarVersion}" {
        return manifest.map(String::from);
    }
    (!s.is_empty() && !s.contains("${")).then(|| s.to_string())
}

fn parse_toml(text: &str, kind: LoaderKind, manifest: Option<&str>) -> Vec<ModMeta> {
    let Ok(doc) = text.parse::<toml::Table>() else { return Vec::new() };
    let mut out: Vec<ModMeta> = doc
        .get("mods")
        .and_then(|m| m.as_array())
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let id = m.get("modId")?.as_str()?.trim().to_string();
            (!id.is_empty()).then(|| ModMeta {
                name: m.get("displayName").and_then(|n| n.as_str()).map(str::trim).filter(|n| !n.is_empty()).map(String::from),
                version: toml_version(m.get("version"), manifest),
                kind,
                provides: Vec::new(),
                deps: Vec::new(),
                id,
            })
        })
        .collect();
    if let Some(deps) = doc.get("dependencies").and_then(|d| d.as_table()) {
        for (owner, list) in deps {
            let Some(m) = out.iter_mut().find(|m| &m.id == owner) else { continue };
            for d in list.as_array().into_iter().flatten() {
                let Some(id) = d.get("modId").and_then(|i| i.as_str()) else { continue };
                let ty = d.get("type").and_then(|t| t.as_str()).map(str::to_ascii_lowercase);
                let dep_kind = match ty.as_deref() {
                    Some("required") => DepKind::Required,
                    Some("incompatible") => DepKind::Incompatible,
                    Some("discouraged") => DepKind::Conflicts,
                    Some(_) => continue,
                    None => match d.get("mandatory").and_then(|b| b.as_bool()) {
                        Some(true) => DepKind::Required,
                        Some(false) => continue,
                        None if kind == LoaderKind::NeoForge => DepKind::Required,
                        None => continue,
                    },
                };
                m.deps.push(Dep {
                    id: id.to_string(),
                    range: d.get("versionRange").and_then(|r| r.as_str()).unwrap_or("").trim().to_string(),
                    kind: dep_kind,
                    side: d.get("side").and_then(|s| s.as_str()).unwrap_or("").to_ascii_uppercase(),
                });
            }
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;

    pub fn zip_bytes(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (n, b) in files {
            w.start_file(*n, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(b).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    pub fn jar(dir: &Path, name: &str, files: &[(&str, &[u8])]) -> std::path::PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, zip_bytes(files)).unwrap();
        p
    }

    #[test]
    fn fabric_complet() {
        let d = tempfile::tempdir().unwrap();
        let j = jar(
            d.path(),
            "a.jar",
            &[(
                "fabric.mod.json",
                br#"{"id":"a","version":"1.2","name":"Alpha","provides":["a-old",{"id":"a2","version":"3"}],
                  "depends":{"fabricloader":">=0.15","flywheel":["1.0","2.0"]},"breaks":{"b":"*"},"conflicts":{"c":"<2"}}"#,
            )],
        );
        let m = jar_meta(&j).unwrap();
        assert!(m.has_fabric);
        let a = &m.mods[0];
        assert_eq!((a.id.as_str(), a.name.as_deref(), a.version.as_deref()), ("a", Some("Alpha"), Some("1.2")));
        assert_eq!(a.provides, vec![("a-old".to_string(), None), ("a2".to_string(), Some("3".to_string()))]);
        let fly = a.deps.iter().find(|d| d.id == "flywheel").unwrap();
        assert_eq!(fly.range, "1.0 || 2.0");
        assert_eq!(a.deps.iter().find(|d| d.id == "b").unwrap().kind, DepKind::Breaks);
        assert_eq!(a.deps.iter().find(|d| d.id == "c").unwrap().kind, DepKind::Conflicts);
    }

    #[test]
    fn quilt() {
        let d = tempfile::tempdir().unwrap();
        let j = jar(
            d.path(),
            "q.jar",
            &[(
                "quilt.mod.json",
                br#"{"quilt_loader":{"id":"q","version":"1.0","metadata":{"name":"Quilty"},
                "depends":["plain",{"id":"x","versions":">=2"},{"id":"opt","optional":true}],"breaks":[{"id":"bad","versions":"*"}]}}"#,
            )],
        );
        let m = jar_meta(&j).unwrap();
        let q = &m.mods[0];
        assert_eq!(q.name.as_deref(), Some("Quilty"));
        let ids: Vec<_> = q.deps.iter().map(|d| (d.id.as_str(), d.kind)).collect();
        assert_eq!(ids, vec![("plain", DepKind::Required), ("x", DepKind::Required), ("bad", DepKind::Breaks)]);
    }

    #[test]
    fn neoforge_plusieurs_mods_et_version_du_manifest() {
        let d = tempfile::tempdir().unwrap();
        let toml = br#"
modLoader="javafml"
[[mods]]
modId="one"
version="${file.jarVersion}"
displayName="One"
[[mods]]
modId="two"
version="${other}"
[[dependencies.one]]
modId="neoforge"
type="required"
versionRange="[21,)"
side="BOTH"
[[dependencies.one]]
modId="flywheel"
versionRange="[1.0,)"
[[dependencies.one]]
modId="opt"
type="optional"
[[dependencies.one]]
modId="bad"
type="incompatible"
[[dependencies.one]]
modId="meh"
type="discouraged"
"#;
        let j = jar(d.path(), "n.jar", &[("META-INF/neoforge.mods.toml", toml), ("META-INF/MANIFEST.MF", b"Manifest-Version: 1.0\r\nImplementation-Version: 4.5.6\r\n")]);
        let m = jar_meta(&j).unwrap();
        assert!(m.has_neoforge && !m.has_forge);
        assert_eq!(m.mods.len(), 2);
        assert_eq!(m.mods[0].version.as_deref(), Some("4.5.6"));
        assert_eq!(m.mods[1].version, None);
        let kinds: Vec<_> = m.mods[0].deps.iter().map(|d| (d.id.as_str(), d.kind)).collect();
        assert_eq!(kinds, vec![("neoforge", DepKind::Required), ("flywheel", DepKind::Required), ("bad", DepKind::Incompatible), ("meh", DepKind::Conflicts)]);
    }

    #[test]
    fn forge_mandatory_et_mcmod() {
        let d = tempfile::tempdir().unwrap();
        let toml = b"[[mods]]\nmodId=\"f\"\n[[dependencies.f]]\nmodId=\"a\"\nmandatory=true\nversionRange=\"[1,)\"\n[[dependencies.f]]\nmodId=\"b\"\nmandatory=false\n";
        let j = jar(d.path(), "f.jar", &[("META-INF/mods.toml", toml)]);
        let m = jar_meta(&j).unwrap();
        assert!(m.has_forge);
        assert_eq!(m.mods[0].deps.len(), 1);
        let o = jar(d.path(), "o.jar", &[("mcmod.info", br#"[{"modid":"old","name":"Old","version":"1"}]"#)]);
        let m = jar_meta(&o).unwrap();
        assert!(m.has_legacy);
        assert_eq!(m.mods[0].kind, LoaderKind::Legacy);
        assert!(m.mods[0].deps.is_empty());
    }

    #[test]
    fn jars_imbriques_fabric_deux_niveaux() {
        let d = tempfile::tempdir().unwrap();
        let leaf = zip_bytes(&[("fabric.mod.json", br#"{"id":"leaf","version":"9"}"#)]);
        let mid = zip_bytes(&[("fabric.mod.json", br#"{"id":"mid","version":"2","provides":["mid-alias"],"jars":[{"file":"META-INF/jars/leaf.jar"}]}"#), ("META-INF/jars/leaf.jar", &leaf)]);
        let j = jar(d.path(), "api.jar", &[("fabric.mod.json", br#"{"id":"api","version":"1","jars":[{"file":"META-INF/jars/mid.jar"}]}"#), ("META-INF/jars/mid.jar", &mid)]);
        let m = jar_meta(&j).unwrap();
        let ids: Vec<&str> = m.nested.iter().map(|(i, _)| i.as_str()).collect();
        assert!(ids.contains(&"mid") && ids.contains(&"mid-alias") && ids.contains(&"leaf"));
    }

    #[test]
    fn jarjar_neoforge_et_repli_sur_l_artifact() {
        let d = tempfile::tempdir().unwrap();
        let inner = zip_bytes(&[("META-INF/mods.toml", b"[[mods]]\nmodId=\"inner_id\"\nversion=\"1.5\"\n")]);
        let meta = br#"{"jars":[
          {"identifier":{"group":"g","artifact":"ignored-name"},"version":{"artifactVersion":"1.5"},"path":"META-INF/jarjar/inner.jar"},
          {"identifier":{"group":"g","artifact":"lib"},"version":{"artifactVersion":"7.0"},"path":"META-INF/jarjar/lib.jar"}]}"#;
        let j = jar(d.path(), "j.jar", &[("META-INF/jarjar/metadata.json", meta), ("META-INF/jarjar/inner.jar", &inner), ("META-INF/jarjar/lib.jar", b"pas un zip")]);
        let m = jar_meta(&j).unwrap();
        assert!(m.nested.contains(&("inner_id".to_string(), Some("1.5".to_string()))));
        assert!(m.nested.contains(&("lib".to_string(), Some("7.0".to_string()))));
        assert!(m.is_empty());
    }

    #[test]
    fn jar_sans_metadonnees_et_jar_illisible() {
        let d = tempfile::tempdir().unwrap();
        let j = jar(d.path(), "x.jar", &[("a.txt", b"x")]);
        assert!(jar_meta(&j).unwrap().is_empty());
        std::fs::write(d.path().join("bad.jar"), b"nope").unwrap();
        assert!(jar_meta(&d.path().join("bad.jar")).is_none());
    }
}
