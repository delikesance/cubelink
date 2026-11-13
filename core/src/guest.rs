//! Tâche N15 : profil invité isolé.
//!
//! Une instance `guest` reçoit uniquement les fichiers du manifeste de l'hôte (mods, config,
//! resourcepacks, shaderpacks), placés par hardlink/copie depuis le cache. Aucun `options.txt`,
//! `saves/` ni compte n'est hérité. Les références du cache sont tracées dans `guest.json`
//! (à côté du dossier de jeu) pour être libérées à la suppression.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::instance::{InstanceKind, InstanceStore};
use crate::manifest::{validate_manifest, ALLOWED_PREFIXES};
use crate::store::index::StoreIndex;
use crate::store::link::link_into;
use crate::types::{Error, Instance, LoaderSpec, Manifest, Result};

const GUEST_META: &str = "guest.json";
const GUEST_SUFFIX: &str = " (invité)";
const MAX_NAME_CHARS: usize = 64;
/// Monde reçu d'un ami : RAM des paramètres globaux, sauf choix explicite avant de rejoindre.
const DEFAULT_GUEST_RAM_MB: u32 = crate::instance::RAM_DEFAULT;

/// Ce que l'appelant doit installer (réseau) avant de lancer le profil invité.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestPlan {
    pub loader: LoaderSpec,
    pub minecraft: String,
    pub java_major: u32,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct GuestMeta {
    host: String,
    /// Hash de chaque fichier dont une référence du cache a été prise.
    refs: Vec<String>,
}

pub fn required_loader(m: &Manifest) -> LoaderSpec {
    m.loader.clone()
}

pub fn required_java_major(m: &Manifest) -> u32 {
    m.java_major
}

pub fn plan(m: &Manifest) -> GuestPlan {
    GuestPlan { loader: required_loader(m), minecraft: m.minecraft.clone(), java_major: required_java_major(m) }
}

/// Crée une instance `guest` vide (dossier de jeu sans aucun fichier hérité).
pub fn create_guest_instance(
    store: &InstanceStore,
    manifest: &Manifest,
    host_label: &str,
    _store_root: &Path,
    _index: &StoreIndex,
) -> Result<Instance> {
    validate_manifest(manifest)?;
    let existing: Vec<String> = store.list().into_iter().map(|i| i.name.to_lowercase()).collect();
    let base = sanitize_name(&manifest.name);
    let mut n = 1u32;
    let name = loop {
        let tail = if n == 1 { GUEST_SUFFIX.to_string() } else { format!("{GUEST_SUFFIX} {n}") };
        let keep = MAX_NAME_CHARS - tail.chars().count();
        let cand = format!("{}{tail}", base.chars().take(keep).collect::<String>().trim_end());
        if !existing.contains(&cand.to_lowercase()) {
            break cand;
        }
        n += 1;
    };
    let inst = store.create_with_kind(
        &name,
        &manifest.minecraft,
        manifest.loader.clone(),
        DEFAULT_GUEST_RAM_MB,
        InstanceKind::Guest,
    )?;
    let meta = GuestMeta { host: host_label.chars().filter(|c| !c.is_control()).take(64).collect(), refs: vec![] };
    if let Err(e) = write_meta(store, &inst.id, &meta) {
        let _ = store.delete(&inst.id);
        return Err(e);
    }
    Ok(inst)
}

/// Place les fichiers du manifeste dans le dossier de jeu de l'invité. Tout ou rien.
pub fn install_manifest_files(
    store: &InstanceStore,
    instance: &Instance,
    manifest: &Manifest,
    store_root: &Path,
    index: &StoreIndex,
) -> Result<()> {
    validate_manifest(manifest)?;
    if store.kind(&instance.id)? != InstanceKind::Guest {
        return Err(Error::Invalid("installation réservée aux instances invitées".into()));
    }
    let mut meta = read_meta(store, &instance.id)?;
    if !meta.refs.is_empty() {
        return Err(Error::Invalid("fichiers déjà installés pour cet invité".into()));
    }
    let game_dir = store.game_dir(&instance.id)?;
    fs::create_dir_all(&game_dir)?;
    let canon_game = fs::canonicalize(&game_dir)?;

    // Phase 1 : tout vérifier avant de toucher au disque.
    let mut plan: Vec<(&str, &str, PathBuf)> = Vec::new();
    for f in &manifest.files {
        let first = f.path.split('/').next().unwrap_or("");
        if !ALLOWED_PREFIXES.contains(&first) {
            return Err(Error::Invalid(format!("chemin hors périmètre invité : {}", f.path)));
        }
        let entry = index
            .get(&f.sha256)?
            .ok_or_else(|| Error::NotFound(format!("{} absent du cache (sha256 {})", f.path, f.sha256)))?;
        if entry.rel_path.contains("..") || entry.rel_path.starts_with(['/', '\\']) || entry.rel_path.contains(':') {
            return Err(Error::Invalid(format!("chemin d'objet suspect dans l'index : {}", entry.rel_path)));
        }
        let src = store_root.join(&entry.rel_path);
        let md = fs::metadata(&src)
            .map_err(|_| Error::NotFound(format!("{} : objet {} absent du disque", f.path, f.sha256)))?;
        if !md.is_file() || md.len() != f.size || entry.size != f.size {
            return Err(Error::Invalid(format!("{} : taille du cache différente du manifeste", f.path)));
        }
        plan.push((&f.path, &f.sha256, src));
    }

    // Phase 2 : liens, avec rollback.
    let mut created: Vec<PathBuf> = Vec::new();
    let mut reffed: Vec<&str> = Vec::new();
    let res = (|| -> Result<()> {
        for (rel, sha, src) in &plan {
            let dst = check_dest(&canon_game, rel)?;
            link_into(&canon_game, rel, src)?;
            created.push(dst);
            index.add_ref(sha)?;
            reffed.push(sha);
        }
        meta.refs = reffed.iter().map(|s| s.to_string()).collect();
        write_meta(store, &instance.id, &meta)
    })();
    if let Err(e) = res {
        for p in &created {
            let _ = fs::remove_file(p);
        }
        for sha in &reffed {
            let _ = index.release_ref(sha);
        }
        return Err(e);
    }
    Ok(())
}

/// Libère les références du cache puis supprime l'instance. Refuse toute instance `own`.
pub fn remove_guest_instance(store: &InstanceStore, id: &str, index: &StoreIndex) -> Result<()> {
    if store.kind(id)? != InstanceKind::Guest {
        return Err(Error::Invalid(format!("{id} n'est pas une instance invitée : suppression refusée")));
    }
    let meta = read_meta(store, id)?;
    for sha in &meta.refs {
        match index.release_ref(sha) {
            Ok(_) | Err(Error::NotFound(_)) => {}
            Err(e) => return Err(e),
        }
    }
    store.delete(id)
}

/// Vérifie que la destination reste dans le game_dir : pas de lien symbolique sur le chemin.
fn check_dest(canon_game: &Path, rel: &str) -> Result<PathBuf> {
    let mut cur = canon_game.to_path_buf();
    for seg in rel.split('/') {
        cur.push(seg);
        if let Ok(md) = fs::symlink_metadata(&cur) {
            if md.file_type().is_symlink() {
                return Err(Error::Invalid(format!("lien symbolique refusé : {rel}")));
            }
        }
    }
    if let Some(parent) = cur.parent() {
        if let Ok(real) = fs::canonicalize(parent) {
            if !real.starts_with(canon_game) {
                return Err(Error::Invalid(format!("écriture hors du dossier de jeu refusée : {rel}")));
            }
        }
    }
    Ok(cur)
}

fn sanitize_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { ' ' } else { c })
        .collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ").trim_end_matches('.').trim().to_string();
    if s.is_empty() {
        "Modpack".into()
    } else {
        s
    }
}

/// Vrai si l'instance a été créée pour rejoindre un ami (modpack temporaire, absent de la liste de l'utilisateur).
pub fn is_guest_instance(store: &InstanceStore, id: &str) -> bool {
    meta_path(store, id).map(|p| p.exists()).unwrap_or(false)
}

fn meta_path(store: &InstanceStore, id: &str) -> Result<PathBuf> {
    let gd = store.game_dir(id)?;
    Ok(gd.parent().map(|p| p.join(GUEST_META)).unwrap_or_else(|| PathBuf::from(GUEST_META)))
}

fn read_meta(store: &InstanceStore, id: &str) -> Result<GuestMeta> {
    let bytes = fs::read(meta_path(store, id)?)?;
    serde_json::from_slice(&bytes).map_err(|e| Error::Invalid(format!("guest.json corrompu : {e}")))
}

fn write_meta(store: &InstanceStore, id: &str, meta: &GuestMeta) -> Result<()> {
    let p = meta_path(store, id)?;
    let tmp = p.with_extension("json.tmp");
    let data = serde_json::to_vec_pretty(meta).map_err(|e| Error::Other(e.to_string()))?;
    fs::write(&tmp, data)?;
    fs::rename(&tmp, &p)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::hash::sha256_file;
    use crate::store::index::object_rel_path;
    use crate::store::link::same_file;
    use crate::types::{Loader, ManifestFile};

    struct Env {
        _t: tempfile::TempDir,
        store: InstanceStore,
        root: PathBuf,
        index: StoreIndex,
    }

    fn env() -> Env {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("cache");
        fs::create_dir_all(&root).unwrap();
        let index = StoreIndex::open(t.path().join("index.db")).unwrap();
        Env { store: InstanceStore::new(t.path().join("instances")), root, index, _t: t }
    }

    fn cache(e: &Env, data: &[u8]) -> ManifestFile {
        let tmp = e.root.join("tmp.bin");
        fs::write(&tmp, data).unwrap();
        let sha = sha256_file(&tmp).unwrap();
        let rel = object_rel_path(&sha).unwrap();
        let dst = e.root.join(&rel);
        fs::create_dir_all(dst.parent().unwrap()).unwrap();
        fs::rename(&tmp, &dst).unwrap();
        e.index.insert(&sha, &rel, data.len() as u64).unwrap();
        ManifestFile { path: String::new(), size: data.len() as u64, sha256: sha, sha1: None }
    }

    fn mf(e: &Env, items: &[(&str, &[u8])]) -> Manifest {
        let files = items.iter().map(|(p, d)| ManifestFile { path: p.to_string(), ..cache(e, d) }).collect();
        Manifest {
            name: "Create: Aero".into(),
            minecraft: "1.21.1".into(),
            loader: LoaderSpec { loader: Loader::NeoForge, version: "21.1.77".into() },
            java_major: 21,
            files,
        }
    }

    fn snapshot(p: &Path) -> Vec<(String, Vec<u8>)> {
        fn walk(base: &Path, p: &Path, out: &mut Vec<(String, Vec<u8>)>) {
            for en in fs::read_dir(p).unwrap().flatten() {
                let path = en.path();
                if path.is_dir() {
                    walk(base, &path, out);
                } else {
                    out.push((path.strip_prefix(base).unwrap().to_string_lossy().into(), fs::read(&path).unwrap()));
                }
            }
        }
        let mut v = Vec::new();
        walk(p, p, &mut v);
        v.sort();
        v
    }

    #[test]
    fn create_and_install_hardlink() {
        let e = env();
        let m = mf(&e, &[("mods/a.jar", b"aaa"), ("config/x.toml", b"x=1")]);
        let g = create_guest_instance(&e.store, &m, "Alice", &e.root, &e.index).unwrap();
        assert_eq!(g.name, "Create Aero (invité)");
        assert_eq!(e.store.kind(&g.id).unwrap(), InstanceKind::Guest);
        install_manifest_files(&e.store, &g, &m, &e.root, &e.index).unwrap();
        let gd = e.store.game_dir(&g.id).unwrap();
        assert_eq!(fs::read(gd.join("mods/a.jar")).unwrap(), b"aaa");
        let obj = e.root.join(object_rel_path(&m.files[0].sha256).unwrap());
        assert!(same_file(&obj, &gd.join("mods/a.jar")));
        assert!(!gd.join("options.txt").exists() && !gd.join("saves").exists());
        assert_eq!(e.index.get(&m.files[0].sha256).unwrap().unwrap().refs, 1);
        let p = plan(&m);
        assert_eq!((p.java_major, p.minecraft.as_str()), (21, "1.21.1"));
    }

    #[test]
    fn host_instance_untouched_and_no_name_collision() {
        let e = env();
        let m = mf(&e, &[("mods/a.jar", b"aaa")]);
        let own = e.store.create("Create Aero (invité)", "1.21.1", m.loader.clone(), 4096).unwrap();
        let gd = e.store.game_dir(&own.id).unwrap();
        fs::write(gd.join("options.txt"), "fov:90").unwrap();
        fs::create_dir_all(gd.join("saves/w")).unwrap();
        fs::write(gd.join("saves/w/level.dat"), "L").unwrap();
        let own_dir = e.store.root().join(&own.id);
        let before = snapshot(&own_dir);
        let g = create_guest_instance(&e.store, &m, "H", &e.root, &e.index).unwrap();
        assert_ne!(g.name, own.name);
        install_manifest_files(&e.store, &g, &m, &e.root, &e.index).unwrap();
        assert_eq!(snapshot(&own_dir), before);
        assert_eq!(e.store.kind(&own.id).unwrap(), InstanceKind::Own);
    }

    #[test]
    fn hostile_manifest_rejected() {
        let e = env();
        let ok = mf(&e, &[("mods/a.jar", b"aaa")]);
        let g = create_guest_instance(&e.store, &ok, "H", &e.root, &e.index).unwrap();
        for bad in ["../evil.jar", "mods/../../x", "options.txt", "saves/w/level.dat", "/etc/passwd", "C:/x.jar"] {
            let mut m = ok.clone();
            m.files[0].path = bad.into();
            assert!(install_manifest_files(&e.store, &g, &m, &e.root, &e.index).is_err(), "{bad}");
            assert!(create_guest_instance(&e.store, &m, "H", &e.root, &e.index).is_err(), "{bad}");
        }
        assert!(!e.store.game_dir(&g.id).unwrap().join("mods/a.jar").exists());
    }

    #[test]
    fn missing_object_rolls_back() {
        let e = env();
        let mut m = mf(&e, &[("mods/a.jar", b"aaa"), ("mods/b.jar", b"bbb")]);
        // b.jar : indexé mais absent du disque.
        fs::remove_file(e.root.join(object_rel_path(&m.files[1].sha256).unwrap())).unwrap();
        let g = create_guest_instance(&e.store, &m, "H", &e.root, &e.index).unwrap();
        let err = install_manifest_files(&e.store, &g, &m, &e.root, &e.index).unwrap_err();
        assert!(matches!(err, Error::NotFound(ref s) if s.contains("mods/b.jar")), "{err}");
        let gd = e.store.game_dir(&g.id).unwrap();
        assert!(!gd.join("mods/a.jar").exists());
        assert_eq!(e.index.get(&m.files[0].sha256).unwrap().unwrap().refs, 0);
        // Hash inconnu de l'index.
        m.files[1].sha256 = "1".repeat(64);
        assert!(matches!(install_manifest_files(&e.store, &g, &m, &e.root, &e.index), Err(Error::NotFound(_))));
        assert!(!gd.join("mods/a.jar").exists());
    }

    #[test]
    fn symlink_escape_refused_and_rolled_back() {
        let e = env();
        let m = mf(&e, &[("mods/a.jar", b"aaa"), ("config/x.toml", b"x")]);
        let g = create_guest_instance(&e.store, &m, "H", &e.root, &e.index).unwrap();
        let gd = e.store.game_dir(&g.id).unwrap();
        let outside = e.root.join("outside");
        fs::create_dir_all(&outside).unwrap();
        #[cfg(unix)]
        let ok = std::os::unix::fs::symlink(&outside, gd.join("config")).is_ok();
        #[cfg(windows)]
        let ok = std::os::windows::fs::symlink_dir(&outside, gd.join("config")).is_ok();
        if !ok {
            return; // pas le droit de créer des liens symboliques
        }
        assert!(install_manifest_files(&e.store, &g, &m, &e.root, &e.index).is_err());
        assert!(!outside.join("x.toml").exists());
        assert!(!gd.join("mods/a.jar").exists());
        assert_eq!(e.index.get(&m.files[0].sha256).unwrap().unwrap().refs, 0);
    }

    #[test]
    fn remove_refused_for_own_and_releases_refs() {
        let e = env();
        let m = mf(&e, &[("mods/a.jar", b"aaa")]);
        let own = e.store.create("Mien", "1.21.1", m.loader.clone(), 4096).unwrap();
        assert!(matches!(remove_guest_instance(&e.store, &own.id, &e.index), Err(Error::Invalid(_))));
        assert!(e.store.get(&own.id).is_ok());
        let g = create_guest_instance(&e.store, &m, "H", &e.root, &e.index).unwrap();
        install_manifest_files(&e.store, &g, &m, &e.root, &e.index).unwrap();
        assert_eq!(e.index.get(&m.files[0].sha256).unwrap().unwrap().refs, 1);
        remove_guest_instance(&e.store, &g.id, &e.index).unwrap();
        assert_eq!(e.index.get(&m.files[0].sha256).unwrap().unwrap().refs, 0);
        assert!(e.store.get(&g.id).is_err());
        assert!(e.root.join(object_rel_path(&m.files[0].sha256).unwrap()).exists());
    }

    #[test]
    fn two_guests_share_cache() {
        let e = env();
        let m = mf(&e, &[("mods/a.jar", b"aaa")]);
        let g1 = create_guest_instance(&e.store, &m, "H", &e.root, &e.index).unwrap();
        let g2 = create_guest_instance(&e.store, &m, "H", &e.root, &e.index).unwrap();
        assert_ne!(g1.name, g2.name);
        install_manifest_files(&e.store, &g1, &m, &e.root, &e.index).unwrap();
        install_manifest_files(&e.store, &g2, &m, &e.root, &e.index).unwrap();
        let sha = &m.files[0].sha256;
        assert_eq!(e.index.get(sha).unwrap().unwrap().refs, 2);
        let a = e.store.game_dir(&g1.id).unwrap().join("mods/a.jar");
        let b = e.store.game_dir(&g2.id).unwrap().join("mods/a.jar");
        assert!(same_file(&a, &b));
        remove_guest_instance(&e.store, &g1.id, &e.index).unwrap();
        assert_eq!(e.index.get(sha).unwrap().unwrap().refs, 1);
    }
}
