//! Reproduction sans réseau du parcours invité (create_guest_instance + install_manifest_files) à partir d'un VRAI
//! dossier de jeu, en lecture seule. Variable : CUBELINK_REAL_GAME_DIR (dossier `minecraft`, `instance.json` à côté) ;
//! sans elle, l'instance « fabulously-optimized » du dossier de données par défaut. Ignoré si absent.
//! Le téléchargement Modrinth (find_unknown_mods) et l'installation du loader/Java (ensure_game) NE sont PAS exercés.

use std::fs;
use std::path::PathBuf;

use core_lib::guest::{create_guest_instance, install_manifest_files};
use core_lib::instance::InstanceStore;
use core_lib::manifest::{build_manifest_with, ManifestOptions};
use core_lib::store::index::{object_rel_path, StoreIndex};
use core_lib::types::Instance;

#[test]
fn parcours_invite_sur_vrai_dossier() {
    let game = std::env::var_os("CUBELINK_REAL_GAME_DIR").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join("app.cubelink.launcher/instances/fabulously-optimized/minecraft")
    });
    let Ok(raw) = fs::read_to_string(game.parent().unwrap().join("instance.json")) else {
        eprintln!("SKIP : {} introuvable", game.display());
        return;
    };
    let inst: Instance = serde_json::from_str(&raw).unwrap();
    let opts = ManifestOptions { include_config: true, include_resourcepacks: false, include_shaderpacks: false, declared_java_major: None };
    // Instantané en lecture seule (le jeu de l'hôte tourne : certains fichiers sont verrouillés, ex. controlify/*.lock).
    let snap_dir = tempfile::tempdir().unwrap();
    let snap = snap_dir.path().join("minecraft");
    let mut locked = vec![];
    copy_tree(&game, &snap, &mut locked);
    eprintln!("instantané : fichiers illisibles (verrouillés) ignorés : {locked:?}");
    let game = snap;
    let mut last = String::new();
    let (m, report) = build_manifest_with(&inst, &game, &opts, &mut |p| last = p.path.to_string())
        .unwrap_or_else(|e| panic!("build_manifest a échoué (dernier fichier traité : {last:?}) : {e:?}"));
    let total: u64 = m.files.iter().map(|f| f.size).sum();
    eprintln!("manifeste : {} fichiers, {total} octets, {} ignorés", m.files.len(), report.skipped.len());
    for f in &m.files {
        eprintln!("  {:>9} {}{}", f.size, f.path, if f.sha1.is_some() { "  [jar mod]" } else { "" });
    }
    for (p, why) in &report.skipped {
        eprintln!("  IGNORÉ {p} : {why}");
    }

    // Réception simulée : chaque fichier copié dans le magasin temporaire à son hash.
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("app.cubelink.launcher").join("store");
    fs::create_dir_all(&root).unwrap();
    let index = StoreIndex::open(t.path().join("index.db")).unwrap();
    for f in &m.files {
        let src = game.join(&f.path);
        let rel = object_rel_path(&f.sha256).unwrap();
        let dst = root.join(&rel);
        fs::create_dir_all(dst.parent().unwrap()).unwrap();
        fs::copy(&src, &dst).unwrap_or_else(|e| panic!("copie {} : {e}", f.path));
        index.insert(&f.sha256, &rel, f.size).unwrap();
    }
    let store = InstanceStore::new(t.path().join("app.cubelink.launcher").join("instances"));
    let g = create_guest_instance(&store, &m, "Hôte", &root, &index).unwrap_or_else(|e| panic!("ÉCHEC create_guest_instance : {e:?}"));
    install_manifest_files(&store, &g, &m, &root, &index).unwrap_or_else(|e| panic!("ÉCHEC install_manifest_files : {e:?}"));
    let gd = store.game_dir(&g.id).unwrap();
    for f in &m.files {
        assert!(gd.join(&f.path).is_file(), "absent chez l'invité : {}", f.path);
    }
    eprintln!("OK : {} fichiers installés chez l'invité ({})", m.files.len(), gd.display());

    // Réessais : suppression puis recréation, et deuxième invité simultané (même manifeste, mêmes objets partagés).
    core_lib::guest::remove_guest_instance(&store, &g.id, &index).unwrap_or_else(|e| panic!("ÉCHEC remove_guest_instance : {e:?}"));
    let g2 = create_guest_instance(&store, &m, "Hôte", &root, &index).unwrap_or_else(|e| panic!("ÉCHEC 2e create : {e:?}"));
    install_manifest_files(&store, &g2, &m, &root, &index).unwrap_or_else(|e| panic!("ÉCHEC 2e install : {e:?}"));
    let g3 = create_guest_instance(&store, &m, "Hôte", &root, &index).unwrap_or_else(|e| panic!("ÉCHEC 3e create : {e:?}"));
    install_manifest_files(&store, &g3, &m, &root, &index).unwrap_or_else(|e| panic!("ÉCHEC 3e install : {e:?}"));
    eprintln!("OK : réessais (suppression/recréation, 2 invités) : {} puis {}", g2.id, g3.id);
}

fn copy_tree(from: &std::path::Path, to: &std::path::Path, locked: &mut Vec<String>) {
    fs::create_dir_all(to).unwrap();
    let Ok(rd) = fs::read_dir(from) else { return };
    for e in rd.flatten() {
        let (p, name) = (e.path(), e.file_name());
        if p.is_dir() {
            if !matches!(name.to_string_lossy().as_ref(), "saves" | "logs" | "crash-reports" | "screenshots" | "downloads" | "local") {
                copy_tree(&p, &to.join(&name), locked);
            }
        } else if let Err(err) = fs::copy(&p, to.join(&name)) {
            locked.push(format!("{} ({err})", p.display()));
        }
    }
}
