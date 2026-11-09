//! Tâche S4 : nettoyage du magasin (objets à 0 référence, orphelins, dossiers de préfixe vides).

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::store::index::{object_rel_path, StoreIndex};
use crate::types::{Error, Result};

/// Options du nettoyage.
#[derive(Debug, Clone, Default)]
pub struct GcOptions {
    /// Ne supprime rien : le rapport liste ce qui le serait.
    pub dry_run: bool,
    /// Âge minimal (depuis la dernière activité de l'entrée / la date du fichier) pour être supprimé.
    pub min_age: Duration,
    /// Supprime aussi les fichiers présents sur disque mais absents de l'index (si assez anciens).
    pub remove_orphans: bool,
}

/// Résultat du nettoyage. En `dry_run`, `removed` et `bytes_freed` décrivent ce qui serait supprimé.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GcReport {
    /// Hashes des objets supprimés.
    pub removed: Vec<String>,
    pub bytes_freed: u64,
    /// Objets non référencés mais trop récents.
    pub skipped_recent: usize,
    /// Échecs non fatals (l'entrée d'index est alors conservée).
    pub errors: Vec<String>,
    /// Fichiers du magasin absents de l'index.
    pub orphans: Vec<PathBuf>,
    /// Orphelins effectivement supprimés.
    pub removed_orphans: usize,
    /// Dossiers de préfixe vides supprimés.
    pub removed_dirs: usize,
}

/// Joint `rel` à `root` en refusant tout ce qui n'est pas un composant normal (`..`, absolu, préfixe).
fn safe_join(root: &Path, rel: &str) -> Result<PathBuf> {
    let mut out = root.to_path_buf();
    let mut any = false;
    for c in Path::new(rel).components() {
        match c {
            Component::Normal(p) => {
                out.push(p);
                any = true;
            }
            _ => return Err(Error::Invalid(format!("chemin hors du magasin : {rel:?}"))),
        }
    }
    if !any || rel.contains('\\') {
        return Err(Error::Invalid(format!("chemin hors du magasin : {rel:?}")));
    }
    Ok(out)
}

fn is_hex2(name: &str) -> bool {
    name.len() == 2 && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Sous-dossiers (non symboliques) de `dir`, triés par nom.
fn subdirs(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(rd) = fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<_> = rd
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| Some((e.file_name().into_string().ok()?, e.path())))
        .collect();
    v.sort();
    v
}

/// Nettoie le magasin `store_root` selon `index` et `opts`.
pub fn collect(store_root: &Path, index: &StoreIndex, opts: &GcOptions) -> Result<GcReport> {
    let mut rep = GcReport::default();
    let now = now_secs();
    let min_age = opts.min_age.as_secs() as i64;

    for entry in index.unreferenced()? {
        // Relecture : un objet référencé entre-temps n'est jamais touché.
        match index.get(&entry.sha256)? {
            Some(cur) if cur.refs == 0 => {}
            _ => continue,
        }
        if now - entry.last_used.max(entry.created_at) < min_age {
            rep.skipped_recent += 1;
            continue;
        }
        let expected = object_rel_path(&entry.sha256)?;
        if entry.rel_path != expected {
            rep.errors.push(format!(
                "{} : chemin d'index {:?} différent de {expected:?}, ignoré",
                entry.sha256, entry.rel_path
            ));
            continue;
        }
        let path = match safe_join(store_root, &expected) {
            Ok(p) => p,
            Err(e) => {
                rep.errors.push(format!("{} : {e}", entry.sha256));
                continue;
            }
        };
        if !opts.dry_run {
            if let Err(e) = fs::remove_file(&path) {
                rep.errors.push(format!("{} : suppression de {} : {e}", entry.sha256, path.display()));
                continue;
            }
            if let Err(e) = index.remove(&entry.sha256) {
                rep.errors.push(format!("{} : index : {e}", entry.sha256));
                continue;
            }
        }
        rep.bytes_freed += entry.size;
        rep.removed.push(entry.sha256);
    }

    // Parcours du magasin : orphelins et dossiers vides (uniquement `xx/yy/` sous la racine).
    for (_, d1) in subdirs(store_root).into_iter().filter(|(n, _)| is_hex2(n)) {
        for (_, d2) in subdirs(&d1).into_iter().filter(|(n, _)| is_hex2(n)) {
            scan_leaf(&d2, index, opts, now, min_age, &mut rep);
            if !opts.dry_run && fs::remove_dir(&d2).is_ok() {
                rep.removed_dirs += 1;
            }
        }
        if !opts.dry_run && fs::remove_dir(&d1).is_ok() {
            rep.removed_dirs += 1;
        }
    }
    Ok(rep)
}

fn scan_leaf(dir: &Path, index: &StoreIndex, opts: &GcOptions, now: i64, min_age: i64, rep: &mut GcReport) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut files: Vec<_> = rd.flatten().collect();
    files.sort_by_key(|e| e.file_name());
    for e in files {
        // Ni dossiers ni liens symboliques : seulement des fichiers ordinaires.
        if !e.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        let indexed = matches!(index.contains(&name), Ok(true));
        if indexed {
            continue;
        }
        let path = e.path();
        rep.orphans.push(path.clone());
        if !opts.remove_orphans || opts.dry_run {
            continue;
        }
        let age_ok = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| now - (d.as_secs() as i64) >= min_age)
            .unwrap_or(false);
        if !age_ok {
            rep.skipped_recent += 1;
            continue;
        }
        match fs::remove_file(&path) {
            Ok(()) => rep.removed_orphans += 1,
            Err(err) => rep.errors.push(format!("orphelin {} : {err}", path.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(n: u32) -> String {
        format!("{n:064x}")
    }

    struct Env {
        _d: tempfile::TempDir,
        root: PathBuf,
        idx: StoreIndex,
    }

    fn env() -> Env {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("store");
        fs::create_dir_all(&root).unwrap();
        let idx = StoreIndex::open(d.path().join("index.db")).unwrap();
        Env { _d: d, root, idx }
    }

    /// Crée l'objet sur disque et dans l'index.
    fn put(e: &Env, n: u32, size: usize) -> (String, PathBuf) {
        let hash = h(n);
        let rel = object_rel_path(&hash).unwrap();
        let p = e.root.join(&rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, vec![0u8; size]).unwrap();
        e.idx.insert(&hash, &rel, size as u64).unwrap();
        (hash, p)
    }

    fn now_opts() -> GcOptions {
        GcOptions::default()
    }

    #[test]
    fn referenced_is_never_removed() {
        let e = env();
        let (a, pa) = put(&e, 1, 10);
        let (_b, pb) = put(&e, 2, 20);
        e.idx.add_ref(&a).unwrap();
        let r = collect(&e.root, &e.idx, &now_opts()).unwrap();
        assert!(pa.exists());
        assert!(!pb.exists());
        assert_eq!(r.removed, vec![h(2)]);
        assert_eq!(r.bytes_freed, 20);
        assert!(e.idx.contains(&a).unwrap());
        assert!(!e.idx.contains(&h(2)).unwrap());
        assert!(r.errors.is_empty());
    }

    #[test]
    fn dry_run_deletes_nothing() {
        let e = env();
        let (b, pb) = put(&e, 2, 5);
        let r = collect(&e.root, &e.idx, &GcOptions { dry_run: true, ..now_opts() }).unwrap();
        assert_eq!(r.removed, vec![b.clone()]);
        assert_eq!(r.bytes_freed, 5);
        assert!(pb.exists());
        assert!(e.idx.contains(&b).unwrap());
        assert_eq!(r.removed_dirs, 0);
    }

    #[test]
    fn min_age_protects_recent() {
        let e = env();
        let (b, pb) = put(&e, 3, 5);
        let opts = GcOptions { min_age: Duration::from_secs(3600), ..now_opts() };
        let r = collect(&e.root, &e.idx, &opts).unwrap();
        assert!(r.removed.is_empty());
        assert_eq!(r.skipped_recent, 1);
        assert!(pb.exists() && e.idx.contains(&b).unwrap());
    }

    #[test]
    fn missing_file_is_tolerated() {
        let e = env();
        let (a, pa) = put(&e, 1, 5);
        let (b, pb) = put(&e, 2, 7);
        fs::remove_file(&pa).unwrap();
        let r = collect(&e.root, &e.idx, &now_opts()).unwrap();
        assert_eq!(r.errors.len(), 1);
        assert!(e.idx.contains(&a).unwrap(), "entrée conservée");
        assert_eq!(r.removed, vec![b.clone()]);
        assert!(!pb.exists() && !e.idx.contains(&b).unwrap());
    }

    #[test]
    fn orphans_reported_and_removed_on_request() {
        let e = env();
        let (_a, _) = put(&e, 1, 5);
        let orphan = e.root.join(object_rel_path(&h(0xabcd)).unwrap());
        fs::create_dir_all(orphan.parent().unwrap()).unwrap();
        fs::write(&orphan, b"x").unwrap();
        let junk = e.root.join("00").join("00").join("download.part");
        fs::write(&junk, b"y").unwrap();
        let r = collect(&e.root, &e.idx, &GcOptions { dry_run: false, ..now_opts() }).unwrap();
        assert_eq!(r.orphans.len(), 2);
        assert!(orphan.exists() && junk.exists());
        assert_eq!(r.removed_orphans, 0);

        let r = collect(&e.root, &e.idx, &GcOptions { remove_orphans: true, ..now_opts() }).unwrap();
        assert_eq!(r.removed_orphans, 2);
        assert!(!orphan.exists() && !junk.exists());
    }

    #[test]
    fn empty_prefix_dirs_are_cleaned() {
        let e = env();
        let (_b, pb) = put(&e, 0x1234, 5);
        let d2 = pb.parent().unwrap().to_path_buf();
        let d1 = d2.parent().unwrap().to_path_buf();
        // dossier vide préexistant
        fs::create_dir_all(e.root.join("ff").join("ee")).unwrap();
        let r = collect(&e.root, &e.idx, &now_opts()).unwrap();
        assert!(!d2.exists() && !d1.exists());
        assert!(!e.root.join("ff").exists());
        assert!(e.root.exists());
        assert!(r.removed_dirs >= 4);
    }

    #[test]
    fn non_empty_dir_is_kept() {
        let e = env();
        let (a, pa) = put(&e, 1, 5);
        e.idx.add_ref(&a).unwrap();
        collect(&e.root, &e.idx, &now_opts()).unwrap();
        assert!(pa.exists());
    }

    #[test]
    fn malicious_path_is_refused() {
        let e = env();
        let outside = e.root.parent().unwrap().join("victim.txt");
        fs::write(&outside, b"precious").unwrap();
        let hash = h(9);
        e.idx.insert(&hash, "../victim.txt", 8).unwrap();
        let r = collect(&e.root, &e.idx, &now_opts()).unwrap();
        assert!(outside.exists());
        assert!(r.removed.is_empty());
        assert_eq!(r.errors.len(), 1);
        assert!(e.idx.contains(&hash).unwrap());
    }

    #[test]
    fn safe_join_rejects_escapes() {
        let root = Path::new("/r");
        for bad in ["../x", "a/../../x", "/abs", "", "a\\b", "."] {
            assert!(safe_join(root, bad).is_err(), "{bad}");
        }
        assert_eq!(safe_join(root, "ab/cd/x").unwrap(), root.join("ab").join("cd").join("x"));
    }
}
