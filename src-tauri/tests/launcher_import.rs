//! Ticket #138 sur l'AppState réel : détection puis import d'instances Prism, CurseForge App et Modrinth App fabriquées
//! sur le vrai disque (dossier de test `CUBELINK_TEST_LAUNCHER_HOME`), vrai magasin, vrai index, jeu préparé
//! (`ensure_game`) contre le faux miroir Mojang (`tests-fixtures/mojang/server.mjs`, node), puis lancement avec le faux
//! java jusqu'à son témoin de lancement : le classpath de l'instance importée doit être complet.
//!
//! Ignoré (avec un message) quand `node` est absent. Une seule fonction de test : les surcharges de test sont lues une
//! fois par processus.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use core_lib::import_launcher::Launcher;
use core_lib::store::hash::sha256_file;
use core_lib::store::link::same_file;
use core_lib::types::Loader;
use zip::write::SimpleFileOptions;

mod common;
use common::{wait_for, Node};

struct Mirror(Child);
impl Drop for Mirror {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn jar(path: &Path, id: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut w = zip::ZipWriter::new(fs::File::create(path).unwrap());
    w.start_file("fabric.mod.json", SimpleFileOptions::default()).unwrap();
    w.write_all(format!("{{\"schemaVersion\":1,\"id\":\"{id}\",\"version\":\"1.0.0\",\"name\":\"{id}\"}}").as_bytes()).unwrap();
    w.finish().unwrap();
}

fn put(path: &Path, content: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn tree(dir: &Path) -> Vec<(PathBuf, String)> {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn import_depuis_prism_curseforge_et_modrinth_jusqu_au_lancement() {
    if Command::new("node").arg("--version").stdout(Stdio::null()).status().is_err() {
        eprintln!("node absent : test de l'import d'instances ignoré");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    // Faux miroir Mojang / Fabric / Adoptium.
    let mut child = Command::new("node")
        .arg(root().join("tests-fixtures/mojang/server.mjs"))
        .args(["--port", "0"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let _mirror = Mirror(child);
    let base = serde_json::from_str::<serde_json::Value>(&line).unwrap()["base"].as_str().unwrap().to_string();
    // Faux java exécutable (copie : le dépôt ne garde pas le bit d'exécution).
    let fj = tmp.path().join("fj");
    fs::create_dir_all(&fj).unwrap();
    let fake_java = if cfg!(windows) { "fake-java.cmd" } else { "fake-java.sh" };
    for f in [fake_java, "fake-java.mjs"] {
        fs::copy(root().join("tests-fixtures/mojang").join(f), fj.join(f)).unwrap();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(fj.join(fake_java), fs::Permissions::from_mode(0o755)).unwrap();
    }
    let home = tmp.path().join("home");
    for (k, v) in [
        ("CUBELINK_TEST_MODE", "1"),
        ("CUBELINK_MOJANG_META_URL", base.as_str()),
        ("CUBELINK_FABRIC_META_URL", base.as_str()),
        ("CUBELINK_ADOPTIUM_URL", base.as_str()),
        // Recherche d'illustration : catalogues injoignables (refus immédiat), l'import doit réussir quand même.
        ("CUBELINK_TEST_MODRINTH_API", "http://127.0.0.1:1"),
        ("CUBELINK_TEST_CF_API", "http://127.0.0.1:1"),
        ("CUBELINK_SKIP_JAVA_PROBE", "1"),
        ("CUBELINK_JAVA_PATH", fj.join(fake_java).to_str().unwrap()),
        ("CUBELINK_TEST_LAUNCHER_HOME", home.to_str().unwrap()),
    ] {
        std::env::set_var(k, v);
    }

    // --- Prism (Flatpak) : Fabric 0.16.5 sur 1.21.1-test, mods, config, monde, options.
    let prism = home.join(".var/app/org.prismlauncher.PrismLauncher/data/PrismLauncher/instances/Aventure");
    put(&prism.join("instance.cfg"), b"[General]\nInstanceType=OneSix\nname=Aventure entre amis\n");
    put(
        &prism.join("mmc-pack.json"),
        br#"{"formatVersion":1,"components":[{"uid":"net.minecraft","version":"1.21.1-test"},{"uid":"net.fabricmc.fabric-loader","version":"0.16.5"}]}"#,
    );
    jar(&prism.join(".minecraft/mods/alpha.jar"), "alpha");
    put(&prism.join(".minecraft/config/alpha.toml"), b"volume = 3\n");
    put(&prism.join(".minecraft/saves/Survie/level.dat"), &[1u8; 4000]);
    put(&prism.join(".minecraft/options.txt"), b"renderDistance:12\n");
    // --- CurseForge App : vanilla 1.20.4-test.
    let cf = home.join("curseforge/minecraft/Instances/Vanille CF");
    put(&cf.join("minecraftinstance.json"), br#"{"name":"Vanille CF","gameVersion":"1.20.4-test","baseModLoader":null}"#);
    put(&cf.join("saves/Monde/level.dat"), &[2u8; 1000]);
    // --- Modrinth App : Fabric sans version dans la base (la recommandée du miroir : 0.16.5).
    let mr = home.join(".local/share/ModrinthApp");
    fs::create_dir_all(mr.join("profiles/Profil Mr")).unwrap();
    {
        let c = rusqlite::Connection::open(mr.join("app.db")).unwrap();
        c.execute_batch("CREATE TABLE profiles (path TEXT PRIMARY KEY, name TEXT, game_version TEXT, mod_loader TEXT, mod_loader_version TEXT)").unwrap();
        c.execute("INSERT INTO profiles VALUES ('Profil Mr', 'Profil Modrinth', '1.21.1-test', 'fabric', NULL)", []).unwrap();
    }
    jar(&mr.join("profiles/Profil Mr/mods/beta.jar"), "beta");
    let (before_prism, before_cf, before_mr_db) = (tree(&prism), tree(&cf), fs::read(mr.join("app.db")).unwrap());

    let data = tmp.path().join("data");
    let alex = Node::start(&data, "Alex").await;
    let st = &alex.st;

    // Détection dans les dossiers habituels (profil de test).
    let found = st.launcher_import_scan(None).await.unwrap();
    let mut names: Vec<(String, Launcher)> = found.iter().map(|d| (d.name.clone(), d.launcher)).collect();
    names.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        names,
        [
            ("Aventure entre amis".to_string(), Launcher::Prism),
            ("Profil Modrinth".to_string(), Launcher::Modrinth),
            ("Vanille CF".to_string(), Launcher::Curseforge)
        ]
    );
    // Chemin manuel : le dossier d'une instance Prism choisie seule.
    let manual = st.launcher_import_scan(Some(prism.display().to_string())).await.unwrap();
    assert_eq!(manual.len(), 1);
    assert!(st.launcher_import_scan(Some(home.join("nexiste-pas").display().to_string())).await.is_err());

    let by = |l: Launcher| found.iter().find(|d| d.launcher == l).unwrap();
    // Un chemin qui n'est pas une instance du lanceur annoncé est refusé, rien n'est créé.
    let e = st.launcher_import_run(&by(Launcher::Prism).path, Launcher::Curseforge, None, None).await.unwrap_err();
    assert!(e.contains("instance"), "{e}");
    assert!(st.instances_list().is_empty());

    let a = st.launcher_import_run(&by(Launcher::Prism).path, Launcher::Prism, None, None).await.unwrap();
    let b = st.launcher_import_run(&by(Launcher::Curseforge).path, Launcher::Curseforge, Some("Mon CF".into()), None).await.unwrap();
    let c = st.launcher_import_run(&by(Launcher::Modrinth).path, Launcher::Modrinth, None, Some(3072)).await.unwrap();
    assert_eq!((a.name.as_str(), a.minecraft.as_str(), a.loader.loader, a.loader.version.as_str()), ("Aventure entre amis", "1.21.1-test", Loader::Fabric, "0.16.5"));
    assert_eq!((b.name.as_str(), b.loader.loader), ("Mon CF", Loader::Vanilla));
    assert_eq!((c.loader.loader, c.loader.version.as_str(), c.ram_mb), (Loader::Fabric, "0.16.5", 3072));
    assert_eq!(st.instances_list().len(), 3);

    // Fichiers, liens physiques vers le magasin, rien de lié à la source.
    let ga = st.instances.game_dir(&a.id).unwrap();
    assert_eq!(fs::read(ga.join("config/alpha.toml")).unwrap(), b"volume = 3\n");
    assert_eq!(fs::read(ga.join("saves/Survie/level.dat")).unwrap(), vec![1u8; 4000]);
    assert_eq!(fs::read(ga.join("options.txt")).unwrap(), b"renderDistance:12\n");
    let obj = st.paths.store.join(core_lib::store::index::object_rel_path(&sha256_file(&ga.join("mods/alpha.jar")).unwrap()).unwrap());
    assert!(same_file(&obj, &ga.join("mods/alpha.jar")));
    assert!(!same_file(&prism.join(".minecraft/mods/alpha.jar"), &ga.join("mods/alpha.jar")));
    assert!(st.instances.game_dir(&c.id).unwrap().join("mods/beta.jar").is_file());
    assert!(st.instances.game_dir(&b.id).unwrap().join("saves/Monde/level.dat").is_file());
    // Progression émise.
    assert!(alex.count("browse://progress") >= 3);
    // Sources intactes.
    assert_eq!(tree(&prism), before_prism);
    assert_eq!(tree(&cf), before_cf);
    assert_eq!(fs::read(mr.join("app.db")).unwrap(), before_mr_db);
    assert!(!mr.join("app.db-wal").exists() && !mr.join("app.db-shm").exists());

    // L'instance importée se lance (faux java) : le classpath préparé par ensure_game est complet et les mods sont là.
    st.instances_launch(&a.id).await.unwrap();
    let witness = ga.join(".cubelink-fake-launch.json");
    wait_for("témoin de lancement", 60, || witness.is_file() && fs::read_to_string(&witness).map(|s| s.contains("\"done\"")).unwrap_or(false)).await;
    let w: serde_json::Value = serde_json::from_str(&fs::read_to_string(&witness).unwrap()).unwrap();
    assert_eq!(w["classpath_missing"], serde_json::json!([]), "{w}");
    assert!(ga.join("mods/alpha.jar").is_file());
    st.instances_kill(&a.id).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
}
