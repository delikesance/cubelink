//! « Vider le cache » : supprime du magasin les objets qu'aucune instance n'utilise.
//!
//! Le compteur `refs` de l'index n'est pas fiable pour cela : il monte à chaque installation mais ne redescend pas
//! quand un mod est retiré ou qu'une instance est supprimée. Le critère est donc le fichier lui-même : un objet est
//! « utilisé » si un fichier sous les dossiers d'instances en est un hardlink (même identité de fichier : volume et
//! index sur Windows, périphérique et inode sur Unix). Un objet seulement présent dans le magasin ne sert à personne,
//! et c'est exactement lui dont la suppression libère de la place : un objet lié reste de toute façon sur le disque
//! tant qu'une instance le garde. Une copie (repli quand le hardlink est impossible) n'est pas un lien : l'objet du
//! magasin est alors supprimable, la copie de l'instance n'est pas touchée.
//!
//! Prudence : un objet récent n'est jamais supprimé (fenêtre entre le téléchargement et le placement dans l'instance) ;
//! un dossier d'instance illisible annule l'opération ; un fichier d'instance dont l'identité ne peut pas être lue
//! protège les objets de même taille. Ni mondes, ni instances, ni l'index ne sont touchés.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::store::gc::{prune_empty_dirs, safe_join};
use crate::store::index::{object_rel_path, StoreIndex};
use crate::types::{Error, Result};

/// Dossiers jamais parcourus sous une instance : les mondes, les journaux et les rapports de plantage ne contiennent
/// pas de lien vers le magasin (gain de temps seulement : un lien manqué ne fait jamais supprimer un fichier d'instance).
const SKIPPED_DIRS: [&str; 3] = ["saves", "logs", "crash-reports"];

/// Identité d'un fichier. Sous Unix, `(périphérique, inode)` : aucun descripteur n'est gardé ouvert (un magasin de
/// plusieurs milliers d'objets dépasserait sinon la limite `ulimit -n`). Ailleurs, `same_file::Handle`.
#[cfg(unix)]
type FileId = (u64, u64);
#[cfg(not(unix))]
type FileId = same_file::Handle;

#[cfg(unix)]
fn file_id(path: &Path) -> std::io::Result<FileId> {
    use std::os::unix::fs::MetadataExt;
    let md = fs::metadata(path)?;
    Ok((md.dev(), md.ino()))
}

#[cfg(not(unix))]
fn file_id(path: &Path) -> std::io::Result<FileId> {
    same_file::Handle::from_path(path)
}

/// Options du vidage.
#[derive(Debug, Clone, Default)]
pub struct PurgeOptions {
    /// Ne supprime rien : le rapport décrit ce qui le serait (taille affichée avant de confirmer).
    pub dry_run: bool,
    /// Âge minimal (dernière activité de l'entrée) pour qu'un objet soit supprimé.
    pub min_age: Duration,
}

/// Résultat. En `dry_run`, `removed` et `bytes_freed` décrivent ce qui serait supprimé.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PurgeReport {
    pub removed: Vec<String>,
    /// Octets réellement libérés (taille des fichiers supprimés).
    pub bytes_freed: u64,
    /// Objets conservés parce qu'une instance les utilise (ou peut le faire).
    pub kept_in_use: usize,
    /// Objets conservés parce que trop récents.
    pub kept_recent: usize,
    /// Échecs non fatals : l'objet concerné est conservé.
    pub errors: Vec<String>,
}

fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Parcourt `roots` (dossiers d'instances) et renvoie les objets du magasin qu'ils lient, plus les tailles des
/// fichiers dont l'identité n'a pas pu être lue. Un dossier illisible est une erreur : on ne devine pas.
type Scan = (HashSet<usize>, HashSet<u64>);

fn scan_instances(roots: &[PathBuf], store: &HashMap<FileId, usize>, sizes: &HashSet<u64>) -> Result<Scan> {
    let mut used = HashSet::new();
    let mut unsure = HashSet::new();
    let mut stack: Vec<PathBuf> = roots.to_vec();
    while let Some(dir) = stack.pop() {
        let rd = match fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(Error::Other(format!("dossier {} illisible : {e}", dir.display()))),
        };
        for ent in rd {
            let ent = ent.map_err(|e| Error::Other(format!("dossier {} illisible : {e}", dir.display())))?;
            let Ok(ft) = ent.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            let path = ent.path();
            if ft.is_dir() {
                if ent.file_name().to_str().is_some_and(|n| SKIPPED_DIRS.contains(&n)) {
                    continue;
                }
                stack.push(path);
            } else if ft.is_file() {
                let Ok(md) = ent.metadata() else { continue };
                if !sizes.contains(&md.len()) {
                    continue;
                }
                match file_id(&path) {
                    Ok(h) => {
                        if let Some(&i) = store.get(&h) {
                            used.insert(i);
                        }
                    }
                    Err(_) => {
                        unsure.insert(md.len());
                    }
                }
            }
        }
    }
    Ok((used, unsure))
}

/// Supprime (ou, en `dry_run`, compte) les objets de `store_root` qu'aucun fichier sous `instance_roots` ne lie.
pub fn purge_unused(store_root: &Path, index: &StoreIndex, instance_roots: &[PathBuf], opts: &PurgeOptions) -> Result<PurgeReport> {
    let mut rep = PurgeReport::default();
    let now = now_secs();
    let min_age = opts.min_age.as_secs() as i64;
    let entries = index.all()?;

    // Identité de chaque objet présent sur disque. Une entrée sans fichier est un reste d'index : elle est retirée,
    // sans rien libérer.
    let mut handles: HashMap<FileId, usize> = HashMap::new();
    let mut paths: Vec<Option<PathBuf>> = Vec::with_capacity(entries.len());
    let mut stale: Vec<usize> = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        let expected = object_rel_path(&e.sha256)?;
        let path = match (e.rel_path == expected).then(|| safe_join(store_root, &expected)) {
            Some(Ok(p)) => p,
            _ => {
                rep.errors.push(format!("{} : chemin d'index {:?} invalide, ignoré", e.sha256, e.rel_path));
                paths.push(None);
                continue;
            }
        };
        match file_id(&path) {
            Ok(h) => {
                handles.insert(h, i);
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => stale.push(i),
            Err(err) => rep.errors.push(format!("{} : lecture de {} : {err}", e.sha256, path.display())),
        }
        paths.push(Some(path));
    }

    let sizes: HashSet<u64> = entries.iter().map(|e| e.size).collect();
    let (used, unsure) = scan_instances(instance_roots, &handles, &sizes)?;

    let mut present: Vec<usize> = handles.values().copied().collect();
    present.sort_unstable();
    // Sous Windows, les identités tiennent des fichiers ouverts : les relâcher avant de supprimer.
    drop(handles);
    for i in present {
        let e = &entries[i];
        if used.contains(&i) || unsure.contains(&e.size) {
            rep.kept_in_use += 1;
            continue;
        }
        if now - e.last_used.max(e.created_at) < min_age {
            rep.kept_recent += 1;
            continue;
        }
        let Some(path) = &paths[i] else { continue };
        if !opts.dry_run {
            if let Err(err) = fs::remove_file(path) {
                rep.errors.push(format!("{} : suppression de {} : {err}", e.sha256, path.display()));
                continue;
            }
            if let Err(err) = index.remove(&e.sha256) {
                rep.errors.push(format!("{} : index : {err}", e.sha256));
                continue;
            }
        }
        rep.bytes_freed += e.size;
        rep.removed.push(e.sha256.clone());
    }
    if !opts.dry_run {
        for i in stale {
            let _ = index.remove(&entries[i].sha256);
        }
        prune_empty_dirs(store_root);
    }
    rep.removed.sort();
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::link::{copy_into, link_into, LinkKind};

    fn h(n: u32) -> String {
        format!("{n:064x}")
    }

    struct Env {
        _d: tempfile::TempDir,
        root: PathBuf,
        instances: PathBuf,
        idx: StoreIndex,
    }

    fn env() -> Env {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("store");
        let instances = d.path().join("instances");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(instances.join("a").join("minecraft").join("mods")).unwrap();
        let idx = StoreIndex::open(d.path().join("index.sqlite")).unwrap();
        Env { _d: d, root, instances, idx }
    }

    /// Objet du magasin (disque + index) de `size` octets, contenu propre à `n`.
    fn put(e: &Env, n: u32, size: usize) -> (String, PathBuf) {
        let hash = h(n);
        let rel = object_rel_path(&hash).unwrap();
        let p = e.root.join(&rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, vec![n as u8; size]).unwrap();
        e.idx.insert(&hash, &rel, size as u64).unwrap();
        (hash, p)
    }

    fn game(e: &Env) -> PathBuf {
        e.instances.join("a").join("minecraft")
    }

    fn all() -> PurgeOptions {
        PurgeOptions { dry_run: false, min_age: Duration::ZERO }
    }

    fn run(e: &Env, o: &PurgeOptions) -> PurgeReport {
        purge_unused(&e.root, &e.idx, std::slice::from_ref(&e.instances), o).unwrap()
    }

    #[test]
    fn objet_non_lie_supprime_et_octets_exacts() {
        let e = env();
        let (a, pa) = put(&e, 1, 10);
        let (b, pb) = put(&e, 2, 25);
        let r = run(&e, &all());
        assert_eq!(r.removed.len(), 2);
        assert_eq!(r.bytes_freed, 35);
        assert!(!pa.exists() && !pb.exists());
        assert!(!e.idx.contains(&a).unwrap() && !e.idx.contains(&b).unwrap());
        assert!(!pa.parent().unwrap().exists(), "dossier xx/yy vide retiré");
        assert!(r.errors.is_empty());
    }

    #[test]
    fn objet_hardlinke_conserve_meme_si_refs_a_zero() {
        let e = env();
        let (a, pa) = put(&e, 1, 10);
        let (b, pb) = put(&e, 2, 20);
        // refs reste à 0 partout : le critère est le lien réel, pas le compteur.
        let kind = link_into(&game(&e), "mods/a.jar", &pa).unwrap();
        assert_eq!(kind, LinkKind::HardLink);
        let r = run(&e, &all());
        assert_eq!(r.removed, vec![b.clone()]);
        assert_eq!(r.bytes_freed, 20);
        assert_eq!(r.kept_in_use, 1);
        assert!(pa.exists() && e.idx.contains(&a).unwrap());
        assert!(!pb.exists());
        assert_eq!(fs::read(game(&e).join("mods/a.jar")).unwrap(), vec![1u8; 10]);
    }

    #[test]
    fn refs_non_nul_mais_plus_aucun_lien_est_supprime() {
        // Mod retiré de l'instance sans release_ref : le compteur est resté à 1, l'objet ne sert plus.
        let e = env();
        let (a, pa) = put(&e, 1, 10);
        e.idx.add_ref(&a).unwrap();
        let r = run(&e, &all());
        assert_eq!(r.removed, vec![a]);
        assert!(!pa.exists());
    }

    #[test]
    fn copie_d_instance_ne_protege_pas_l_objet_et_survit() {
        let e = env();
        let (_a, pa) = put(&e, 1, 10);
        assert_eq!(copy_into(&game(&e), "mods/a.jar", &pa).unwrap(), LinkKind::Copy);
        let r = run(&e, &all());
        assert_eq!(r.bytes_freed, 10);
        assert!(!pa.exists());
        assert_eq!(fs::read(game(&e).join("mods/a.jar")).unwrap(), vec![1u8; 10], "la copie de l'instance est intacte");
    }

    #[test]
    fn objet_recent_conserve() {
        let e = env();
        let (a, pa) = put(&e, 1, 10);
        let r = run(&e, &PurgeOptions { dry_run: false, min_age: Duration::from_secs(900) });
        assert!(r.removed.is_empty());
        assert_eq!(r.kept_recent, 1);
        assert!(pa.exists() && e.idx.contains(&a).unwrap());
    }

    #[test]
    fn dry_run_ne_supprime_rien_et_annonce_les_memes_octets() {
        let e = env();
        let (a, pa) = put(&e, 1, 10);
        let (_b, pb) = put(&e, 2, 25);
        link_into(&game(&e), "mods/a.jar", &pa).unwrap();
        let plan = run(&e, &PurgeOptions { dry_run: true, ..all() });
        assert!(pa.exists() && pb.exists() && e.idx.contains(&a).unwrap());
        assert_eq!(plan.bytes_freed, 25);
        let real = run(&e, &all());
        assert_eq!(real.bytes_freed, plan.bytes_freed);
        assert_eq!(real.removed, plan.removed);
    }

    #[test]
    fn lien_symbolique_non_suivi_et_mondes_ignores() {
        let e = env();
        let (_a, pa) = put(&e, 1, 10);
        let (_b, pb) = put(&e, 2, 11);
        fs::create_dir_all(game(&e).join("saves/w")).unwrap();
        // Un hardlink dans un monde n'est pas compté (dossier ignoré) : seul un cas tordu, sans conséquence pour l'instance.
        fs::hard_link(&pb, game(&e).join("saves/w/x.bin")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&pa, game(&e).join("mods/sym.jar")).unwrap();
        #[cfg(not(unix))]
        let _ = pa; // pas de lien symbolique sans droits d'administrateur sous Windows
        let r = run(&e, &all());
        assert_eq!(r.removed.len(), 2);
        assert!(game(&e).join("saves/w/x.bin").exists());
    }

    #[test]
    fn entree_sans_fichier_nettoyee_sans_rien_liberer() {
        let e = env();
        let (a, pa) = put(&e, 1, 10);
        fs::remove_file(&pa).unwrap();
        let r = run(&e, &all());
        assert_eq!(r.bytes_freed, 0);
        assert!(!e.idx.contains(&a).unwrap());
    }

    #[test]
    fn dossier_d_instance_inexistant_ou_vide_tolere() {
        let e = env();
        let (_a, pa) = put(&e, 1, 10);
        let r = purge_unused(&e.root, &e.idx, &[e.instances.join("absent")], &all()).unwrap();
        assert_eq!(r.bytes_freed, 10);
        assert!(!pa.exists());
    }

    #[cfg(unix)]
    #[test]
    fn dossier_d_instance_illisible_annule_tout() {
        use std::os::unix::fs::PermissionsExt;
        let e = env();
        let (a, pa) = put(&e, 1, 10);
        let locked = game(&e).join("mods");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let denied = fs::read_dir(&locked).is_err(); // root lit tout : le test ne prouve rien dans ce cas
        let res = purge_unused(&e.root, &e.idx, std::slice::from_ref(&e.instances), &all());
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        if denied {
            assert!(res.is_err());
            assert!(pa.exists() && e.idx.contains(&a).unwrap());
        }
    }
}
