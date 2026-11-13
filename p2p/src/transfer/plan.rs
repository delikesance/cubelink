//! Tâche N10 : plan de transfert côté invité (quels fichiers du manifeste manquent dans le cache).

use std::collections::{HashMap, HashSet};

use core_lib::manifest::validate_manifest;
use core_lib::store::index::StoreIndex;
use core_lib::types::{Error, Manifest, ManifestFile, Result};

/// Plafond par défaut de la taille totale d'un manifeste : 20 Go.
pub const DEFAULT_MAX_TOTAL_BYTES: u64 = 20 * 1024 * 1024 * 1024;

/// Interroge le cache de l'invité.
pub trait CacheLookup {
    /// Vrai seulement si le contenu `sha256` est présent ET de taille `size`.
    fn has(&self, sha256: &str, size: u64) -> bool;
}

/// Adaptateur : `StoreIndex` comme `CacheLookup`. Toute erreur d'index vaut « absent ».
pub struct StoreCache<'a>(pub &'a StoreIndex);

impl CacheLookup for StoreCache<'_> {
    fn has(&self, sha256: &str, size: u64) -> bool {
        matches!(self.0.get(sha256), Ok(Some(e)) if e.size == size)
    }
}

/// Comme `StoreCache`, mais vérifie aussi que l'objet EXISTE sur le disque avec la bonne taille. Un objet que l'index
/// déclare connu alors que son fichier a disparu (suppression d'un fichier invalide, nettoyage manuel, antivirus)
/// est purgé de l'index et compté « à recevoir » : sinon l'installation échouerait ensuite (« absent du cache »).
pub struct DiskCache<'a> {
    pub index: &'a StoreIndex,
    pub root: &'a std::path::Path,
}

impl CacheLookup for DiskCache<'_> {
    fn has(&self, sha256: &str, size: u64) -> bool {
        let Ok(Some(e)) = self.index.get(sha256) else { return false };
        let on_disk = core_lib::store::index::object_rel_path(sha256)
            .ok()
            .and_then(|rel| std::fs::metadata(self.root.join(rel)).ok())
            .map(|m| m.is_file() && m.len() == size);
        if e.size == size && on_disk == Some(true) {
            return true;
        }
        if e.size == size {
            // index OK mais fichier absent ou de taille incorrecte : on oublie l'entrée pour le re-recevoir
            let _ = self.index.remove(sha256);
        }
        false
    }
}

/// Options de calcul.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanOptions {
    pub max_total_bytes: u64,
    /// Trie `to_receive` du plus gros au plus petit (sinon par chemin).
    pub largest_first: bool,
}

impl Default for PlanOptions {
    fn default() -> Self {
        Self { max_total_bytes: DEFAULT_MAX_TOTAL_BYTES, largest_first: false }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// Fichiers déjà en cache (un par chemin).
    pub cached: Vec<ManifestFile>,
    /// Fichiers à recevoir, dédupliqués par sha256.
    pub to_receive: Vec<ManifestFile>,
    /// Tous les (chemin, sha256) à installer, triés par chemin.
    pub installs: Vec<(String, String)>,
    pub cached_count: usize,
    pub to_receive_count: usize,
    pub to_receive_bytes: u64,
    /// Somme des tailles de tous les fichiers du manifeste.
    pub total_bytes: u64,
}

/// Plan avec les options par défaut.
pub fn compute_plan(manifest: &Manifest, cache: &dyn CacheLookup) -> Result<Plan> {
    compute_plan_with(manifest, cache, &PlanOptions::default())
}

pub fn compute_plan_with(manifest: &Manifest, cache: &dyn CacheLookup, opts: &PlanOptions) -> Result<Plan> {
    validate_manifest(manifest)?;

    let mut files: Vec<&ManifestFile> = manifest.files.iter().collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));

    let mut sizes: HashMap<&str, u64> = HashMap::new();
    let mut total_bytes = 0u64;
    for f in &files {
        if let Some(prev) = sizes.insert(f.sha256.as_str(), f.size) {
            if prev != f.size {
                return Err(Error::Invalid(format!(
                    "manifeste : tailles contradictoires pour le même contenu ({:?})",
                    f.path
                )));
            }
        }
        total_bytes = total_bytes.saturating_add(f.size);
    }
    if total_bytes > opts.max_total_bytes {
        return Err(Error::Invalid(format!(
            "le monde est trop volumineux : {total_bytes} octets (maximum autorisé : {} octets)",
            opts.max_total_bytes
        )));
    }

    let mut plan = Plan { total_bytes, ..Plan::default() };
    let mut queued: HashSet<&str> = HashSet::new();
    for f in files {
        plan.installs.push((f.path.clone(), f.sha256.clone()));
        if cache.has(&f.sha256, f.size) {
            plan.cached.push(f.clone());
        } else if queued.insert(f.sha256.as_str()) {
            plan.to_receive.push(f.clone());
        }
    }
    if opts.largest_first {
        plan.to_receive.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.path.cmp(&b.path)));
    }
    plan.cached_count = plan.cached.len();
    plan.to_receive_count = plan.to_receive.len();
    plan.to_receive_bytes = plan.to_receive.iter().map(|f| f.size).sum();
    Ok(plan)
}

/// Résumé compatible `JoinPlan` : (en cache, à recevoir, octets à recevoir).
pub fn to_join_summary(plan: &Plan) -> (usize, usize, u64) {
    (plan.cached_count, plan.to_receive_count, plan.to_receive_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_lib::types::{Loader, LoaderSpec};

    #[test]
    fn objet_en_index_mais_absent_du_disque_est_redemande_puis_installable() {
        use core_lib::guest::{create_guest_instance, install_manifest_files};
        use core_lib::instance::InstanceStore;
        use core_lib::store::index::object_rel_path;
        use sha2::{Digest, Sha256};

        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("store");
        std::fs::create_dir_all(&root).unwrap();
        let index = StoreIndex::open(t.path().join("index.db")).unwrap();
        let put = |data: &[u8]| {
            let sha = hex::encode(Sha256::digest(data));
            let rel = object_rel_path(&sha).unwrap();
            std::fs::create_dir_all(root.join(&rel).parent().unwrap()).unwrap();
            std::fs::write(root.join(&rel), data).unwrap();
            index.insert(&sha, &rel, data.len() as u64).unwrap();
            ManifestFile { path: String::new(), size: data.len() as u64, sha256: sha, sha1: None, cf_fingerprint: None }
        };
        let (d1, d2) = (b"premier fichier".as_slice(), b"second fichier, un peu plus long".as_slice());
        let f1 = ManifestFile { path: "config/a.json".into(), ..put(d1) };
        let f2 = ManifestFile { path: "config/b.json".into(), ..put(d2) };
        let m = manifest(vec![f1.clone(), f2.clone()]);

        // l'objet 2 disparaît du disque, l'index le déclare toujours connu
        std::fs::remove_file(root.join(object_rel_path(&f2.sha256).unwrap())).unwrap();
        assert!(StoreCache(&index).has(&f2.sha256, f2.size), "l'ancien comportement se fie à l'index seul");
        let plan = compute_plan(&m, &DiskCache { index: &index, root: &root }).unwrap();
        assert_eq!((plan.cached_count, plan.to_receive_count), (1, 1));
        assert_eq!(plan.to_receive[0].path, "config/b.json");
        assert!(index.get(&f2.sha256).unwrap().is_none(), "l'entrée orpheline est purgée de l'index");
        // objet de taille incorrecte : même traitement
        let rel1 = object_rel_path(&f1.sha256).unwrap();
        std::fs::write(root.join(&rel1), b"tronque").unwrap();
        let plan = compute_plan(&m, &DiskCache { index: &index, root: &root }).unwrap();
        assert_eq!(plan.to_receive_count, 2);

        // réception simulée des deux, puis l'installation réussit
        put(d1);
        put(d2);
        let plan = compute_plan(&m, &DiskCache { index: &index, root: &root }).unwrap();
        assert_eq!(plan.to_receive_count, 0);
        let m = Manifest { files: vec![f1, f2], ..m };
        let store = InstanceStore::new(t.path().join("instances"));
        let g = create_guest_instance(&store, &m, "Hôte", &root, &index).unwrap();
        install_manifest_files(&store, &g, &m, &root, &index).unwrap();
        assert!(store.game_dir(&g.id).unwrap().join("config/b.json").is_file());
    }

    #[derive(Default)]
    struct FakeCache(HashSet<(String, u64)>);

    impl CacheLookup for FakeCache {
        fn has(&self, sha256: &str, size: u64) -> bool {
            self.0.contains(&(sha256.to_string(), size))
        }
    }

    fn h(n: usize) -> String {
        format!("{n:064x}")
    }

    fn file(path: &str, n: usize, size: u64) -> ManifestFile {
        ManifestFile { path: path.into(), size, sha256: h(n), sha1: None, cf_fingerprint: None }
    }

    fn manifest(files: Vec<ManifestFile>) -> Manifest {
        Manifest {
            name: "Monde".into(),
            minecraft: "1.21.1".into(),
            loader: LoaderSpec { loader: Loader::NeoForge, version: "21.1.77".into() },
            java_major: 21,
            files,
        }
    }

    #[test]
    fn ninety_cached_three_to_receive() {
        let mut files: Vec<_> = (0..93).map(|i| file(&format!("mods/m{i:03}.jar"), i + 1, 1000)).collect();
        files.reverse();
        let cache = FakeCache((0..90).map(|i| (h(i + 1), 1000)).collect());
        let p = compute_plan(&manifest(files), &cache).unwrap();
        assert_eq!((p.cached_count, p.to_receive_count), (90, 3));
        assert_eq!(p.to_receive_bytes, 3000);
        assert_eq!(p.total_bytes, 93_000);
        assert_eq!(p.installs.len(), 93);
        assert_eq!(to_join_summary(&p), (90, 3, 3000));
    }

    #[test]
    fn duplicate_hash_received_once_installed_twice() {
        let m = manifest(vec![file("mods/a.jar", 1, 10), file("mods/b.jar", 1, 10)]);
        let p = compute_plan(&m, &FakeCache::default()).unwrap();
        assert_eq!(p.to_receive_count, 1);
        assert_eq!(p.to_receive_bytes, 10);
        assert_eq!(p.total_bytes, 20);
        assert_eq!(p.installs.len(), 2);
    }

    #[test]
    fn same_hash_different_size_rejected() {
        let m = manifest(vec![file("mods/a.jar", 1, 10), file("mods/b.jar", 1, 11)]);
        assert!(matches!(compute_plan(&m, &FakeCache::default()), Err(Error::Invalid(_))));
    }

    #[test]
    fn size_mismatch_is_not_cached() {
        let m = manifest(vec![file("mods/a.jar", 1, 10)]);
        let cache = FakeCache([(h(1), 99)].into_iter().collect());
        let p = compute_plan(&m, &cache).unwrap();
        assert_eq!((p.cached_count, p.to_receive_count), (0, 1));
    }

    #[test]
    fn hostile_manifest_rejected() {
        let m = manifest(vec![file("../evil.jar", 1, 10)]);
        assert!(matches!(compute_plan(&m, &FakeCache::default()), Err(Error::Invalid(_))));
    }

    #[test]
    fn cap_enforced_in_french() {
        let m = manifest(vec![file("mods/a.jar", 1, 600), file("mods/b.jar", 2, 600)]);
        let opts = PlanOptions { max_total_bytes: 1000, ..PlanOptions::default() };
        match compute_plan_with(&m, &FakeCache::default(), &opts) {
            Err(Error::Invalid(msg)) => assert!(msg.contains("trop volumineux")),
            other => panic!("attendu Invalid, obtenu {other:?}"),
        }
        let ok = PlanOptions { max_total_bytes: 1200, ..PlanOptions::default() };
        assert!(compute_plan_with(&m, &FakeCache::default(), &ok).is_ok());
    }

    #[test]
    fn empty_manifest_gives_empty_plan() {
        let p = compute_plan(&manifest(vec![]), &FakeCache::default()).unwrap();
        assert_eq!(p, Plan::default());
        assert_eq!(to_join_summary(&p), (0, 0, 0));
    }

    #[test]
    fn deterministic_and_largest_first() {
        let a = manifest(vec![file("mods/a.jar", 1, 5), file("mods/b.jar", 2, 50), file("mods/c.jar", 3, 20)]);
        let mut rev = a.clone();
        rev.files.reverse();
        let cache = FakeCache::default();
        assert_eq!(compute_plan(&a, &cache).unwrap(), compute_plan(&rev, &cache).unwrap());
        let opts = PlanOptions { largest_first: true, ..PlanOptions::default() };
        let p = compute_plan_with(&rev, &cache, &opts).unwrap();
        let sizes: Vec<u64> = p.to_receive.iter().map(|f| f.size).collect();
        assert_eq!(sizes, vec![50, 20, 5]);
        assert_eq!(p, compute_plan_with(&a, &cache, &opts).unwrap());
    }
}
