//! Ticket #138 sur un vrai disque : un dossier Prism et un dossier CurseForge fabriqués comme chez un joueur
//! (HOME temporaire, vrais jars zip), détectés par `scan_known`, importés avec un vrai `InstanceStore`, un vrai index
//! SQLite et un vrai magasin. Aucun réseau. Vérifie : détection, instance relue depuis le disque, liens physiques
//! vers le magasin, source octet pour octet intacte, index cohérent avec les liens, et un balayage du magasin qui
//! garde ce que l'instance utilise.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use core_lib::import_launcher::{import_instance, load_instance, scan_known, ImportParams, Launcher, Roots};
use core_lib::instance::InstanceStore;
use core_lib::mods;
use core_lib::store::gc::sweep;
use core_lib::store::hash::sha256_file;
use core_lib::store::index::{object_rel_path, StoreIndex};
use core_lib::store::link::same_file;
use core_lib::store::live::live_refs;
use core_lib::types::Loader;
use zip::write::SimpleFileOptions;

/// Vrai jar : un zip avec `fabric.mod.json`, relu par le code de Cubelink.
fn jar(path: &Path, id: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut w = zip::ZipWriter::new(fs::File::create(path).unwrap());
    w.start_file("fabric.mod.json", SimpleFileOptions::default()).unwrap();
    w.write_all(format!("{{\"schemaVersion\":1,\"id\":\"{id}\",\"version\":\"1.0.0\",\"name\":\"{id}\"}}").as_bytes()).unwrap();
    w.start_file("data.bin", SimpleFileOptions::default()).unwrap();
    w.write_all(&vec![7u8; 200_000]).unwrap();
    w.finish().unwrap();
}

fn put(path: &Path, content: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn tree_hash(dir: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap().flatten() {
            if e.file_type().unwrap().is_dir() {
                stack.push(e.path());
            } else {
                out.push((e.path(), sha256_file(&e.path()).unwrap()));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn prism_et_curseforge_importes_depuis_un_home_reel() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path().join("home");
    // Prism (installation native Linux).
    let prism = home.join(".local/share/PrismLauncher/instances/Aventure");
    put(&prism.join("instance.cfg"), b"[General]\nInstanceType=OneSix\nname=Aventure entre amis\n");
    put(
        &prism.join("mmc-pack.json"),
        br#"{"formatVersion":1,"components":[{"uid":"net.minecraft","version":"1.21.1"},{"uid":"net.fabricmc.fabric-loader","version":"0.16.5"}]}"#,
    );
    jar(&prism.join(".minecraft/mods/alpha.jar"), "alpha");
    jar(&prism.join(".minecraft/mods/beta.jar"), "beta");
    put(&prism.join(".minecraft/config/alpha.toml"), b"volume = 3\n");
    put(&prism.join(".minecraft/saves/Survie/level.dat"), &[1u8; 5000]);
    put(&prism.join(".minecraft/options.txt"), b"renderDistance:12\n");
    // CurseForge App.
    let cf = home.join("curseforge/minecraft/Instances/Forge SMP");
    put(&cf.join("minecraftinstance.json"), br#"{"name":"Forge SMP","gameVersion":"1.20.1","baseModLoader":{"name":"forge-47.2.0","minecraftVersion":"1.20.1"}}"#);
    jar(&cf.join("mods/alpha.jar"), "alpha");
    put(&cf.join("saves/Monde/level.dat"), &[2u8; 3000]);

    let roots = Roots { appdata: None, home: Some(home.clone()), data_home: Some(home.join(".local/share")) };
    let found = scan_known(&roots);
    assert_eq!(found.len(), 2, "{found:?}");
    let p = found.iter().find(|d| d.launcher == Launcher::Prism).unwrap();
    assert_eq!((p.name.as_str(), p.minecraft.as_str(), p.mod_count, p.has_saves), ("Aventure entre amis", "1.21.1", 2, true));
    assert_eq!(p.loader.loader, Loader::Fabric);

    let before_prism = tree_hash(&prism);
    let before_cf = tree_hash(&cf);

    let store = t.path().join("data/store");
    fs::create_dir_all(&store).unwrap();
    let index = StoreIndex::open(t.path().join("data/index.sqlite")).unwrap();
    let instances_root = t.path().join("data/instances");
    let instances = InstanceStore::new(&instances_root);
    let go = |d: &core_lib::import_launcher::DetectedInstance| {
        // Comme la commande : on relit depuis le chemin, pas depuis l'objet reçu.
        let det = load_instance(Path::new(&d.path), d.launcher).unwrap();
        import_instance(&det, &ImportParams::default(), &instances, &store, &index, None, Arc::new(AtomicBool::new(false))).unwrap()
    };
    let a = go(p);
    let b = go(found.iter().find(|d| d.launcher == Launcher::Curseforge).unwrap());

    // L'instance est relue depuis le disque par un autre InstanceStore, comme au prochain démarrage.
    let reread = InstanceStore::new(&instances_root).get(&a.id).unwrap();
    assert_eq!((reread.name.as_str(), reread.minecraft.as_str()), ("Aventure entre amis", "1.21.1"));
    assert_eq!(reread.loader.version, "0.16.5");
    let ga = instances.game_dir(&a.id).unwrap();
    let gb = instances.game_dir(&b.id).unwrap();
    // Les jars sont de vrais mods pour Cubelink.
    let listed: Vec<String> = mods::list(&ga).unwrap().into_iter().map(|m| m.file).collect();
    assert_eq!(listed, ["alpha.jar", "beta.jar"]);
    assert_eq!(fs::read(ga.join("config/alpha.toml")).unwrap(), b"volume = 3\n");
    assert_eq!(fs::read(ga.join("saves/Survie/level.dat")).unwrap(), vec![1u8; 5000]);
    assert_eq!(fs::read(ga.join("options.txt")).unwrap(), b"renderDistance:12\n");
    // Le même jar chez deux lanceurs : un seul objet dans le magasin, lié aux deux instances.
    let sha = sha256_file(&ga.join("mods/alpha.jar")).unwrap();
    assert_eq!(sha, sha256_file(&gb.join("mods/alpha.jar")).unwrap());
    let obj = store.join(object_rel_path(&sha).unwrap());
    assert!(same_file(&obj, &ga.join("mods/alpha.jar")) && same_file(&obj, &gb.join("mods/alpha.jar")));
    assert!(!same_file(&prism.join(".minecraft/mods/alpha.jar"), &ga.join("mods/alpha.jar")));
    assert!(!same_file(&prism.join(".minecraft/options.txt"), &ga.join("options.txt")));
    assert_eq!(index.get(&sha).unwrap().unwrap().refs, 2);
    // Index et liens physiques disent la même chose.
    let live = live_refs(&store, &index, &instances_root).unwrap();
    assert!(live.complete);
    for (sha, n) in &live.counts {
        assert_eq!(index.get(sha).unwrap().unwrap().refs, *n);
    }
    // Un balayage du magasin ne retire rien de ce que les instances utilisent.
    let rep = sweep(&store, &index, &instances_root, Duration::ZERO).unwrap();
    assert!(rep.removed.is_empty(), "{rep:?}");
    assert!(ga.join("mods/beta.jar").is_file() && obj.is_file());
    // Les deux sources sont octet pour octet identiques.
    assert_eq!(tree_hash(&prism), before_prism);
    assert_eq!(tree_hash(&cf), before_cf);
}
