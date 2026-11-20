//! Objets vivants du magasin : ceux dont un fichier des instances est un lien dur.
//!
//! Le compteur `refs` de l'index n'est pas fiable : les chemins de suppression (mod retiré, ancienne version,
//! sauvegarde, instance supprimée) ne le décrémentent pas tous, et une réinstallation le compte deux fois.
//! La source de vérité est donc le disque : un objet est vivant tant qu'un fichier d'instance a la même identité
//! physique (volume + index de fichier) que lui. Une simple copie (repli sans lien dur, configuration) ne le retient pas.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use crate::store::index::{object_rel_path, StoreIndex};
use crate::types::Result;

/// Dossiers lourds d'une instance où l'on ne cherche pas de lien (mondes, journaux, captures, illustrations).
/// Ignorés seulement à la racine de l'instance et du dossier de jeu : un sous-dossier `logs` d'un mod est parcouru.
const SKIPPED_DIRS: [&str; 5] = ["saves", "logs", "crash-reports", "screenshots", "art"];

/// Dernier niveau (0 = dossier de l'instance, 1 = son dossier de jeu) où `SKIPPED_DIRS` est appliqué.
const SKIP_MAX_DEPTH: usize = 1;

/// Identité physique d'un fichier. Sous Unix : (volume, inode), lue sans garder de descripteur ouvert (un cache de
/// plusieurs milliers d'objets dépasserait sinon la limite de 1024 descripteurs). Ailleurs : `same_file::Handle`.
#[cfg(unix)]
type FileId = (u64, u64);
#[cfg(not(unix))]
type FileId = same_file::Handle;

#[cfg(unix)]
fn file_id(path: &Path) -> std::io::Result<FileId> {
    use std::os::unix::fs::MetadataExt;
    let m = fs::metadata(path)?;
    Ok((m.dev(), m.ino()))
}

#[cfg(not(unix))]
fn file_id(path: &Path) -> std::io::Result<FileId> {
    same_file::Handle::from_path(path)
}

/// Résultat du parcours des instances.
#[derive(Debug, Clone, Default)]
pub struct LiveScan {
    /// Hash -> nombre de fichiers d'instance liés à l'objet.
    pub counts: HashMap<String, u32>,
    /// Faux si une lecture a échoué : l'ensemble vivant peut alors être incomplet et ne doit servir à rien supprimer.
    pub complete: bool,
    /// Raisons des lectures échouées.
    pub errors: Vec<String>,
}

/// Parcourt `instances_root` et compte, pour chaque objet de l'index, les fichiers qui en sont un lien dur.
///
/// Les liens symboliques ne sont pas suivis. Un dossier ou un fichier qui disparaît pendant le parcours est ignoré ;
/// toute autre erreur de lecture passe `complete` à faux.
pub fn live_refs(store_root: &Path, index: &StoreIndex, instances_root: &Path) -> Result<LiveScan> {
    let mut scan = LiveScan { complete: true, ..LiveScan::default() };

    let mut handles: HashMap<FileId, String> = HashMap::new();
    let mut sizes: HashSet<u64> = HashSet::new();
    for e in index.all()? {
        let path = store_root.join(object_rel_path(&e.sha256)?);
        match file_id(&path) {
            Ok(h) => {
                sizes.insert(e.size);
                handles.insert(h, e.sha256);
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => scan.fail(format!("objet {} : {err}", e.sha256)),
        }
    }
    if handles.is_empty() {
        return Ok(scan);
    }

    let rd = match fs::read_dir(instances_root) {
        Ok(rd) => rd,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(scan),
        Err(err) => {
            scan.fail(format!("{} : {err}", instances_root.display()));
            return Ok(scan);
        }
    };
    let ctx = Ctx { handles: &handles, sizes: &sizes };
    for entry in rd {
        match entry {
            Ok(e) if e.file_type().map(|t| t.is_dir()).unwrap_or(false) => walk(&ctx, &e.path(), 0, &mut scan),
            Ok(_) => {}
            Err(err) => scan.fail(format!("{} : {err}", instances_root.display())),
        }
    }
    Ok(scan)
}

struct Ctx<'a> {
    handles: &'a HashMap<FileId, String>,
    sizes: &'a HashSet<u64>,
}

impl LiveScan {
    fn fail(&mut self, msg: String) {
        self.complete = false;
        self.errors.push(msg);
    }
}

fn walk(ctx: &Ctx<'_>, dir: &Path, depth: usize, scan: &mut LiveScan) {
    let rd = match fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(err) if err.kind() == ErrorKind::NotFound => return,
        Err(err) => return scan.fail(format!("{} : {err}", dir.display())),
    };
    for entry in rd {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                scan.fail(format!("{} : {err}", dir.display()));
                continue;
            }
        };
        // `file_type` ne suit pas les liens symboliques : un lien n'est ni un dossier ni un fichier ordinaire.
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            if depth <= SKIP_MAX_DEPTH && SKIPPED_DIRS.iter().any(|n| entry.file_name() == *n) {
                continue;
            }
            walk(ctx, &entry.path(), depth + 1, scan);
        } else if ft.is_file() {
            check_file(ctx, &entry, scan);
        }
    }
}

fn check_file(ctx: &Ctx<'_>, entry: &fs::DirEntry, scan: &mut LiveScan) {
    let Ok(meta) = entry.metadata() else { return };
    if !ctx.sizes.contains(&meta.len()) {
        return;
    }
    // Un lien dur vers le magasin a au moins deux liens (l'objet lui-même + celui-ci).
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() < 2 {
            return;
        }
    }
    let path = entry.path();
    match file_id(&path) {
        Ok(h) => {
            if let Some(sha) = ctx.handles.get(&h) {
                *scan.counts.entry(sha.clone()).or_insert(0) += 1;
            }
        }
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => scan.fail(format!("{} : {err}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::link::link_into;

    struct Env {
        _d: tempfile::TempDir,
        store: std::path::PathBuf,
        instances: std::path::PathBuf,
        idx: StoreIndex,
    }

    fn env() -> Env {
        let d = tempfile::tempdir().unwrap();
        let store = d.path().join("store");
        let instances = d.path().join("instances");
        fs::create_dir_all(&store).unwrap();
        fs::create_dir_all(&instances).unwrap();
        let idx = StoreIndex::open(d.path().join("index.db")).unwrap();
        Env { _d: d, store, instances, idx }
    }

    fn put(e: &Env, n: u32, content: &[u8]) -> (String, std::path::PathBuf) {
        let sha = format!("{n:064x}");
        let rel = object_rel_path(&sha).unwrap();
        let p = e.store.join(&rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, content).unwrap();
        e.idx.insert(&sha, &rel, content.len() as u64).unwrap();
        (sha, p)
    }

    fn inst(e: &Env, name: &str) -> std::path::PathBuf {
        let g = e.instances.join(name).join("minecraft");
        fs::create_dir_all(&g).unwrap();
        g
    }

    #[test]
    fn counts_hardlinks_across_instances() {
        let e = env();
        let (a, pa) = put(&e, 1, b"aaaa");
        let (b, _) = put(&e, 2, b"bbbbb");
        let (c, _) = put(&e, 3, b"cccccc");
        let (g1, g2) = (inst(&e, "i1"), inst(&e, "i2"));
        link_into(&g1, "mods/a.jar", &pa).unwrap();
        link_into(&g2, "mods/a.jar", &pa).unwrap();
        link_into(&g2, "resourcepacks/a.zip", &pa).unwrap();
        let s = live_refs(&e.store, &e.idx, &e.instances).unwrap();
        assert!(s.complete, "{:?}", s.errors);
        assert_eq!(s.counts.get(&a), Some(&3));
        assert!(!s.counts.contains_key(&b) && !s.counts.contains_key(&c));
    }

    #[test]
    fn copy_is_not_live_and_symlink_not_followed() {
        let e = env();
        let (a, pa) = put(&e, 1, b"aaaa");
        let (b, pb) = put(&e, 2, b"bbbbb");
        let g = inst(&e, "i1");
        fs::create_dir_all(g.join("mods")).unwrap();
        fs::copy(&pa, g.join("mods/copie.jar")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&pb, g.join("mods/lien.jar")).unwrap();
        let s = live_refs(&e.store, &e.idx, &e.instances).unwrap();
        assert!(s.complete);
        assert!(!s.counts.contains_key(&a), "une copie ne retient pas l'objet");
        #[cfg(unix)]
        assert!(!s.counts.contains_key(&b), "un lien symbolique n'est pas suivi");
        let _ = (b, pb);
    }

    #[test]
    fn heavy_dirs_are_skipped_but_nested_ones_are_not() {
        let e = env();
        let (a, pa) = put(&e, 1, b"aaaa");
        let g = inst(&e, "i1");
        link_into(&g, "saves/monde/a.jar", &pa).unwrap();
        link_into(&g, "logs/a.jar", &pa).unwrap();
        let s = live_refs(&e.store, &e.idx, &e.instances).unwrap();
        assert!(!s.counts.contains_key(&a));
        link_into(&g, "mods/logs/a.jar", &pa).unwrap();
        link_into(&g, ".cubelink/backups/b1/mods/a.jar", &pa).unwrap();
        let s = live_refs(&e.store, &e.idx, &e.instances).unwrap();
        assert_eq!(s.counts.get(&a), Some(&2));
    }

    #[test]
    fn missing_instances_dir_is_complete_and_empty() {
        let e = env();
        put(&e, 1, b"aaaa");
        let s = live_refs(&e.store, &e.idx, &e.instances.join("nope")).unwrap();
        assert!(s.complete && s.counts.is_empty());
    }

    #[test]
    fn unreadable_instances_root_makes_scan_incomplete() {
        let e = env();
        put(&e, 1, b"aaaa");
        // Une racine qui n'est pas un dossier : erreur de lecture autre que « introuvable ».
        let file = e.instances.join("fichier");
        fs::write(&file, b"x").unwrap();
        let s = live_refs(&e.store, &e.idx, &file).unwrap();
        assert!(!s.complete && !s.errors.is_empty());
    }
}
