//! Icône d'un mod, lue dans son `.jar` : `fabric.mod.json` / `quilt.mod.json` (`icon`),
//! `META-INF/neoforge.mods.toml` / `mods.toml` (`logoFile`), `mcmod.info` (`logoFile`), sinon `pack.png`.
//!
//! Renvoie une data URL réduite à [`ICON_PX`] px. Cache mémoire par chemin + empreinte (taille, date).

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::Mutex;

use crate::art::{data_url, make_thumbnail, sniff};

/// Côté maximal de l'icône renvoyée.
pub const ICON_PX: u32 = 64;
/// Une icône plus lourde que ça dans le jar est ignorée.
const MAX_ICON_BYTES: u64 = 2 * 1024 * 1024;
/// Descripteurs de métadonnées lus au plus à cette taille.
const MAX_META_BYTES: u64 = 256 * 1024;

type Cache = Mutex<HashMap<String, (String, Option<String>)>>;
static CACHE: std::sync::OnceLock<Cache> = std::sync::OnceLock::new();

fn stamp(meta: &std::fs::Metadata) -> String {
    let ns = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
    format!("{}-{ns}", meta.len())
}

/// Icône du jar `path` en data URL, `None` s'il n'en a pas (ou si le jar est illisible).
pub fn icon_data_url(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok().filter(|m| m.is_file())?;
    let key = path.display().to_string();
    let st = stamp(&meta);
    let cache = CACHE.get_or_init(Default::default);
    if let Some((s, v)) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        if *s == st {
            return v.clone();
        }
    }
    let v = read_icon(path);
    cache.lock().unwrap_or_else(|e| e.into_inner()).insert(key, (st, v.clone()));
    v
}

fn read_entry<R: Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>, name: &str, max: u64) -> Option<Vec<u8>> {
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

/// `icon` d'un descripteur Fabric / Quilt : chaîne, ou objet `{ "taille": "chemin" }` (on prend la plus grande ≤ 256).
fn json_icon(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Object(m) => {
            let mut sizes: Vec<(u32, &str)> = m.iter().filter_map(|(k, v)| Some((k.parse().ok()?, v.as_str()?))).collect();
            sizes.sort_by_key(|(k, _)| *k);
            sizes.iter().rev().find(|(k, _)| *k <= 256).or(sizes.first()).map(|(_, p)| p.to_string())
        }
        _ => None,
    }
}

/// Valeur de `logoFile = "x.png"` dans un `mods.toml` (sans dépendance TOML : une ligne suffit).
fn toml_logo(text: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let l = l.trim();
        let rest = l.strip_prefix("logoFile")?.trim_start().strip_prefix('=')?.trim();
        let q = rest.chars().next().filter(|c| *c == '"' || *c == '\'')?;
        let inner = &rest[1..];
        Some(inner[..inner.find(q)?].to_string())
    })
}

fn candidates<R: Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>) -> Vec<String> {
    let mut out = Vec::new();
    for desc in ["fabric.mod.json", "quilt.mod.json"] {
        if let Some(b) = read_entry(zip, desc, MAX_META_BYTES) {
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&b) {
                let icon = v.get("icon").or_else(|| v.pointer("/quilt_loader/metadata/icon"));
                if let Some(p) = icon.and_then(json_icon) {
                    out.push(p);
                }
            }
        }
    }
    for desc in ["META-INF/neoforge.mods.toml", "META-INF/mods.toml"] {
        if let Some(b) = read_entry(zip, desc, MAX_META_BYTES) {
            if let Some(p) = toml_logo(&String::from_utf8_lossy(&b)) {
                out.push(p);
            }
        }
    }
    if let Some(b) = read_entry(zip, "mcmod.info", MAX_META_BYTES) {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&b) {
            let first = v.as_array().and_then(|a| a.first()).or_else(|| v.pointer("/modList/0"));
            if let Some(p) = first.and_then(|m| m.get("logoFile")).and_then(|l| l.as_str()).filter(|s| !s.is_empty()) {
                out.push(p.to_string());
            }
        }
    }
    out.push("pack.png".into());
    out
}

fn read_icon(path: &Path) -> Option<String> {
    let f = std::fs::File::open(path).ok()?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(f)).ok()?;
    for name in candidates(&mut zip) {
        let Some(bytes) = read_entry(&mut zip, &name, MAX_ICON_BYTES) else { continue };
        let Some((mime, _)) = sniff(&bytes) else { continue };
        return Some(match make_thumbnail(&bytes, ICON_PX) {
            Some((m, small)) => data_url(m, &small),
            None => data_url(mime, &bytes),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3];

    fn jar(dir: &Path, name: &str, files: &[(&str, &[u8])]) -> std::path::PathBuf {
        let p = dir.join(name);
        let mut w = zip::ZipWriter::new(std::fs::File::create(&p).unwrap());
        for (n, b) in files {
            w.start_file(*n, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(b).unwrap();
        }
        w.finish().unwrap();
        p
    }

    #[test]
    fn icones_fabric_neoforge_mcmod_et_repli() {
        let d = tempfile::tempdir().unwrap();
        let fab = jar(d.path(), "f.jar", &[("fabric.mod.json", br#"{"icon":{"16":"s.png","128":"assets/m/icon.png"}}"#), ("assets/m/icon.png", PNG)]);
        assert!(icon_data_url(&fab).unwrap().starts_with("data:image/png;base64,"));
        let neo = jar(d.path(), "n.jar", &[("META-INF/neoforge.mods.toml", b"[[mods]]\n  logoFile = \"logo.png\"\n"), ("logo.png", PNG)]);
        assert!(icon_data_url(&neo).is_some());
        let old = jar(d.path(), "o.jar", &[("mcmod.info", br#"[{"modid":"x","logoFile":"/logo.png"}]"#), ("logo.png", PNG)]);
        assert!(icon_data_url(&old).is_some());
        let pack = jar(d.path(), "p.jar", &[("pack.png", PNG)]);
        assert!(icon_data_url(&pack).is_some());
        let none = jar(d.path(), "x.jar", &[("fabric.mod.json", br#"{"icon":"../evil.png"}"#), ("a.txt", b"x")]);
        assert!(icon_data_url(&none).is_none());
        std::fs::write(d.path().join("bad.jar"), b"pas un zip").unwrap();
        assert!(icon_data_url(&d.path().join("bad.jar")).is_none());
    }

    #[test]
    fn toml_logo_lit_les_guillemets() {
        assert_eq!(toml_logo("logoFile='a.png'").as_deref(), Some("a.png"));
        assert_eq!(toml_logo("# rien\nlogoFile=\"b c.png\" # commentaire").as_deref(), Some("b c.png"));
        assert_eq!(toml_logo("logoFileX=\"no\""), None);
    }
}
