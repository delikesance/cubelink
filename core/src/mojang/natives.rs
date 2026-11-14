//! Tâche M3 : extraction des bibliothèques natives (jars/zips) vers un dossier `natives_dir`.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use super::libraries::Library;
use crate::types::{Error, Result};

/// Taille maximale d'une entrée décompressée (128 Mo).
pub const MAX_ENTRY_BYTES: u64 = 128 * 1024 * 1024;
/// Taille maximale totale décompressée (256 Mo).
pub const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtractReport {
    /// Fichiers écrits (nouveaux ou modifiés).
    pub extracted: usize,
    /// Fichiers déjà présents avec le même contenu.
    pub unchanged: usize,
    /// Entrées ignorées (dossiers, exclusions).
    pub skipped: usize,
    /// Octets décompressés traités (hors ignorés).
    pub bytes: u64,
}

fn invalid(msg: impl Into<String>) -> Error {
    Error::Invalid(format!("natives : {}", msg.into()))
}

pub fn extract_natives(
    libs: &[Library],
    libs_dir: &Path,
    natives_dir: &Path,
) -> Result<ExtractReport> {
    extract_with_limits(libs, libs_dir, natives_dir, MAX_ENTRY_BYTES, MAX_TOTAL_BYTES)
}

/// Normalise un nom d'entrée en chemin relatif sûr ; `Err` si le chemin sort du dossier cible.
fn safe_relative(name: &str) -> Result<PathBuf> {
    if name.contains('\0') {
        return Err(invalid(format!("entrée {name:?} : octet nul")));
    }
    if name.starts_with('/') || name.starts_with('\\') {
        return Err(invalid(format!("entrée {name:?} : chemin absolu")));
    }
    let mut out = PathBuf::new();
    for comp in name.split(['/', '\\']) {
        if comp == ".." {
            return Err(invalid(format!("entrée {name:?} : sort du dossier cible")));
        }
        if comp.contains(':') {
            return Err(invalid(format!("entrée {name:?} : préfixe de lecteur")));
        }
        if comp.is_empty() || comp == "." {
            continue;
        }
        out.push(comp);
    }
    if out.as_os_str().is_empty() {
        return Err(invalid(format!("entrée {name:?} : chemin vide")));
    }
    Ok(out)
}

fn extract_with_limits(
    libs: &[Library],
    libs_dir: &Path,
    natives_dir: &Path,
    max_entry: u64,
    max_total: u64,
) -> Result<ExtractReport> {
    let mut report = ExtractReport::default();
    for lib in libs.iter().filter(|l| l.is_native) {
        let jar = libs_dir.join(&lib.path);
        let file = match fs::File::open(&jar) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::NotFound(format!(
                    "jar natif {} ({})",
                    lib.name,
                    jar.display()
                )))
            }
            Err(e) => return Err(e.into()),
        };
        let mut zip = zip::ZipArchive::new(file)
            .map_err(|e| invalid(format!("archive {} illisible : {e}", lib.name)))?;
        for i in 0..zip.len() {
            let mut entry = zip
                .by_index(i)
                .map_err(|e| invalid(format!("archive {} : {e}", lib.name)))?;
            let name = entry
                .name()
                .map_err(|e| invalid(format!("archive {} : nom d'entrée illisible : {e}", lib.name)))?
                .into_owned();
            let rel = safe_relative(&name)?;
            if let Some(mode) = entry.unix_mode() {
                if mode & 0o170000 == 0o120000 {
                    return Err(invalid(format!("entrée {name:?} : lien symbolique")));
                }
            }
            if entry.is_dir() {
                report.skipped += 1;
                continue;
            }
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            if lib
                .extract_excludes
                .iter()
                .any(|p| rel_str.starts_with(p.trim_start_matches('/')))
            {
                report.skipped += 1;
                continue;
            }
            if entry.size() > max_entry {
                return Err(invalid(format!("entrée {name:?} trop grande")));
            }
            let mut data = Vec::new();
            (&mut entry).take(max_entry + 1).read_to_end(&mut data)?;
            let len = data.len() as u64;
            if len > max_entry {
                return Err(invalid(format!("entrée {name:?} trop grande")));
            }
            report.bytes += len;
            if report.bytes > max_total {
                return Err(invalid("taille totale décompressée excessive"));
            }
            let dest = natives_dir.join(&rel);
            if fs::read(&dest).map(|d| d == data).unwrap_or(false) {
                report.unchanged += 1;
                continue;
            }
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut tmp_name = dest.file_name().unwrap_or_default().to_os_string();
            tmp_name.push(".tmp");
            let tmp = dest.with_file_name(tmp_name);
            let res = (|| -> std::io::Result<()> {
                let mut f = fs::File::create(&tmp)?;
                f.write_all(&data)?;
                f.sync_all()?;
                fs::rename(&tmp, &dest)
            })();
            if let Err(e) = res {
                let _ = fs::remove_file(&tmp);
                return Err(e.into());
            }
            report.extracted += 1;
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::write::SimpleFileOptions;

    fn lib(path: &str, excludes: &[&str]) -> Library {
        Library {
            name: format!("test:{path}"),
            path: path.to_string(),
            url: String::new(),
            sha1: None,
            size: None,
            is_native: true,
            extract_excludes: excludes.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn make_zip(dir: &Path, rel: &str, entries: &[(&str, &[u8])]) {
        let p = dir.join(rel);
        let mut w = zip::ZipWriter::new(fs::File::create(p).unwrap());
        for (n, d) in entries {
            w.start_file(*n, SimpleFileOptions::default()).unwrap();
            w.write_all(d).unwrap();
        }
        w.finish().unwrap();
    }

    fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let libs = t.path().join("libs");
        let nat = t.path().join("natives");
        fs::create_dir_all(&libs).unwrap();
        (t, libs, nat)
    }

    #[test]
    fn nominal_and_excludes() {
        let (_t, libs, nat) = setup();
        make_zip(
            &libs,
            "a.jar",
            &[("lwjgl.dll", b"dll"), ("sub/x.so", b"so"), ("META-INF/MANIFEST.MF", b"m")],
        );
        let l = [lib("a.jar", &["META-INF/"])];
        let r = extract_natives(&l, &libs, &nat).unwrap();
        assert_eq!(r.extracted, 2);
        assert_eq!(r.skipped, 1);
        assert_eq!(fs::read(nat.join("lwjgl.dll")).unwrap(), b"dll");
        assert_eq!(fs::read(nat.join("sub/x.so")).unwrap(), b"so");
        assert!(!nat.join("META-INF").exists());
    }

    #[test]
    fn non_native_ignored() {
        let (_t, libs, nat) = setup();
        let mut l = lib("missing.jar", &[]);
        l.is_native = false;
        assert_eq!(extract_natives(&[l], &libs, &nat).unwrap(), ExtractReport::default());
    }

    #[test]
    fn zip_slip_rejected() {
        let (t, libs, nat) = setup();
        make_zip(&libs, "a.jar", &[("../evil.dll", b"x")]);
        let e = extract_natives(&[lib("a.jar", &[])], &libs, &nat).unwrap_err();
        assert!(matches!(e, Error::Invalid(_)));
        assert!(!t.path().join("evil.dll").exists());
        assert!(!nat.join("evil.dll").exists());
    }

    #[test]
    fn absolute_and_drive_rejected() {
        for n in ["/abs.dll", "\\abs.dll", "C:/abs.dll", "a/..\\b.dll"] {
            let (_t, libs, nat) = setup();
            make_zip(&libs, "a.jar", &[(n, b"x")]);
            let e = extract_natives(&[lib("a.jar", &[])], &libs, &nat).unwrap_err();
            assert!(matches!(e, Error::Invalid(_)), "{n}");
        }
    }

    #[test]
    fn missing_jar() {
        let (_t, libs, nat) = setup();
        match extract_natives(&[lib("nope.jar", &[])], &libs, &nat) {
            Err(Error::NotFound(m)) => assert!(m.contains("test:nope.jar")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn idempotent() {
        let (_t, libs, nat) = setup();
        make_zip(&libs, "a.jar", &[("a.dll", b"abc")]);
        let l = [lib("a.jar", &[])];
        assert_eq!(extract_natives(&l, &libs, &nat).unwrap().extracted, 1);
        let r = extract_natives(&l, &libs, &nat).unwrap();
        assert_eq!((r.extracted, r.unchanged), (0, 1));
        fs::write(nat.join("a.dll"), b"zzz").unwrap();
        assert_eq!(extract_natives(&l, &libs, &nat).unwrap().extracted, 1);
        assert_eq!(fs::read(nat.join("a.dll")).unwrap(), b"abc");
    }

    #[test]
    fn size_limits() {
        let (_t, libs, nat) = setup();
        make_zip(&libs, "a.jar", &[("a.dll", &[0u8; 100]), ("b.dll", &[1u8; 100])]);
        let l = [lib("a.jar", &[])];
        let e = extract_with_limits(&l, &libs, &nat, 50, 1000).unwrap_err();
        assert!(matches!(e, Error::Invalid(_)));
        let e = extract_with_limits(&l, &libs, &nat, 100, 150).unwrap_err();
        assert!(matches!(e, Error::Invalid(_)));
        assert!(extract_with_limits(&l, &libs, &nat, 100, 200).is_ok());
    }
}
