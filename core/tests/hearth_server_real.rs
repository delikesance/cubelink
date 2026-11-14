//! Essai réel du serveur dédié de Hearth : installe un vrai serveur (vanilla puis Fabric 1.21.1), le démarre, fait
//! un point de reprise, l'arrête et vérifie qu'aucun processus Java ne reste. Réseau et Java 21 requis ; l'EULA de
//! Minecraft est acceptée par le test (c'est un essai jetable dans un dossier temporaire).
//!
//! `cargo test -p cubelink-core --test hearth_server_real -- --ignored --nocapture`

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use core_lib::hearth::server::{
    resolve_java, DedicatedServer, InstallRequest, InstalledServer, ServerConfig, ServerInstaller,
};
use core_lib::hearth::world::{changed_since, copy_world, is_world_dir, Snapshot};
use core_lib::types::{Loader, LoaderSpec};

const MC: &str = "1.21.1";

/// Processus dont la ligne de commande mentionne `needle` (Linux : `/proc`). Vide ailleurs.
fn processes_mentioning(needle: &str) -> Vec<u32> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir("/proc") else { return out };
    let me = std::process::id();
    for e in rd.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
        if pid == me {
            continue;
        }
        if let Ok(raw) = std::fs::read(e.path().join("cmdline")) {
            if String::from_utf8_lossy(&raw).contains(needle) {
                out.push(pid);
            }
        }
    }
    out
}

fn kill_pid(pid: u32) {
    let _ = std::process::Command::new("kill").args(["-9", &pid.to_string()]).status();
}

fn region_files(world: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(world.join("region")) else { return Vec::new() };
    rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "mca")).collect()
}

async fn run_loader(loader: Loader, root: &Path, cache: &Path) {
    let name = format!("{loader:?}");
    let dir = root.join(name.to_lowercase()).join("server");
    let java_root = root.join("java");
    let t0 = Instant::now();
    let installer = ServerInstaller::new();
    let req = InstallRequest {
        dir: dir.clone(),
        minecraft: MC.into(),
        loader: LoaderSpec { loader, version: String::new() },
        java_root: java_root.clone(),
        java_override: None,
        jar_cache: Some(cache.to_path_buf()),
        progress: Some(std::sync::Arc::new(|m: &str| println!("  [installation] {m}"))),
    };
    let installed: InstalledServer = installer.install(&req).await.expect("installation du serveur");
    println!("[{name}] installé en {:?} : {:?} (java {})", t0.elapsed(), installed.args, installed.java_major);
    assert!(installed.required_files.iter().all(|f| dir.join(f).is_file()));

    let java = resolve_java(&java_root, installed.java_major, None).await.expect("Java");
    let mut cfg = ServerConfig::new(&dir, &java, 1536);
    cfg.accept_eula = true; // essai : l'EULA est acceptée par le test
    cfg.ready_timeout = Duration::from_secs(420);
    let t1 = Instant::now();
    let server = DedicatedServer::start(&cfg, &installed).await.unwrap_or_else(|e| panic!("démarrage : {e}"));
    println!("[{name}] prêt en {:?} (pid {}, port {})", t1.elapsed(), server.pid(), server.port());
    let pid = server.pid();
    assert!(server.is_running());

    // Point de reprise : save-off, save-all flush, copie du monde (callback), save-on.
    let world = server.world_dir();
    let snap0 = Snapshot::capture(&world).ok();
    let copy_dst = root.join(name.to_lowercase()).join("copie");
    let copy_dst2 = copy_dst.clone();
    let world2 = world.clone();
    let files = server
        .checkpoint(|| async move {
            let w = world2.clone();
            let report = tokio::task::spawn_blocking(move || copy_world(&w, &copy_dst2))
                .await
                .map_err(|e| core_lib::types::Error::Other(e.to_string()))??;
            Ok(report.files)
        })
        .await
        .expect("point de reprise");
    println!("[{name}] point de reprise : {files} fichiers copiés");
    assert!(files > 0);
    assert!(is_world_dir(&world), "level.dat attendu dans {}", world.display());
    assert!(is_world_dir(&copy_dst));
    assert!(!copy_dst.join("session.lock").exists(), "session.lock ne doit pas être copié");
    let regions = region_files(&world);
    assert!(!regions.is_empty(), "aucun fichier région dans {}", world.display());
    assert!(!region_files(&copy_dst).is_empty());
    println!("[{name}] {} fichier(s) région", regions.len());

    // Instantané : après le point de reprise, la liste des fichiers modifiés se calcule sur le vrai monde.
    let snap1 = Snapshot::capture(&world).expect("instantané");
    assert!(snap1.files.contains_key("level.dat"));
    assert!(!snap1.files.contains_key("session.lock"));
    if let Some(s0) = snap0 {
        let (diff, _) = changed_since(&s0, &world).expect("diff");
        println!("[{name}] depuis l'instantané initial : +{} ~{} -{}", diff.added.len(), diff.modified.len(), diff.removed.len());
    }

    // Un second point de reprise consécutif fonctionne aussi (la sauvegarde automatique a bien été rétablie).
    server.checkpoint(|| async { Ok(()) }).await.expect("second point de reprise");

    let t2 = Instant::now();
    let out = server.stop(Duration::from_secs(90)).await;
    println!("[{name}] arrêt en {:?} : {out:?}", t2.elapsed());
    assert!(!out.forced, "arrêt forcé : {:?}", server.recent_lines(20));
    assert_eq!(out.exit_code, Some(0));
    assert!(!server.is_running());

    // Aucun java du test ne doit rester.
    let dir_str = dir.display().to_string();
    let mut left = Vec::new();
    for _ in 0..20 {
        left = processes_mentioning(&dir_str);
        if left.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    for p in &left {
        kill_pid(*p);
    }
    assert!(left.is_empty(), "processus restants : {left:?} (tués)");
    assert!(!Path::new(&format!("/proc/{pid}")).exists() || cfg!(not(target_os = "linux")));
}

#[tokio::test]
#[ignore = "réseau, Java 21 et plusieurs minutes : à lancer avec --ignored"]
async fn vanilla_puis_fabric_1_21_1() {
    let root = tempfile::tempdir().unwrap();
    let cache = root.path().join("cache");
    for loader in [Loader::Vanilla, Loader::Fabric] {
        run_loader(loader, root.path(), &cache).await;
    }
    let left = processes_mentioning(&root.path().display().to_string());
    for p in &left {
        kill_pid(*p);
    }
    assert!(left.is_empty(), "processus restants : {left:?}");
}
