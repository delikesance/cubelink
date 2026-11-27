//! Mise à jour d'un modpack sur l'AppState réel (vraies commandes, vrai disque, vrai magasin) contre le faux catalogue
//! `tests-fixtures/browse/server.mjs` (node), Modrinth puis CurseForge : installation depuis le catalogue, relevé daté,
//! détection, mise à jour sans retélécharger l'ancien pack, fichiers du joueur gardés, mod à poser, annulation.
//!
//! Remplace sous Linux l'E2E `e2e/20-modpack-update.spec.ts` et `21-modpack-update-cf` (CDP = WebView2, Windows seulement). Ignoré (avec un
//! message) quand `node` est absent. Une seule fonction de test : les surcharges de test sont lues une fois par processus.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use core_lib::browse::{BrowseInstallInput, BrowseKind, BrowseSource};
use core_lib::modpack_update::{needs_old_pack, RECORD_FILE};

mod common;
use common::Node;

struct Catalogue(Child);
impl Drop for Catalogue {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// Démarre le faux catalogue et renvoie son URL de base.
fn start_catalogue() -> (Catalogue, String) {
    let mut child = Command::new("node")
        .arg(root().join("tests-fixtures/browse/server.mjs"))
        .args(["--port", "0"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let base = serde_json::from_str::<serde_json::Value>(&line).unwrap()["base"].as_str().unwrap().to_string();
    (Catalogue(child), base)
}

fn put(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn modpack_installe_mis_a_jour_avec_le_releve_puis_annule() {
    if Command::new("node").arg("--version").stdout(Stdio::null()).status().is_err() {
        eprintln!("node absent : test de mise à jour de modpack ignoré");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let (_catalogue, base) = start_catalogue();
    for (k, v) in [
        ("CUBELINK_TEST_MODE", "1"),
        ("CUBELINK_TEST_MODRINTH_API", base.as_str()),
        ("CUBELINK_TEST_CF_API", base.as_str()),
        ("CUBELINK_SKIP_JAVA_PROBE", "1"),
    ] {
        std::env::set_var(k, v);
    }
    let alex = Node::start(&tmp.path().join("data"), "Alex").await;
    let st = &alex.st;

    // Installation de la v1 depuis le catalogue : le relevé est écrit et daté de l'origine du pack.
    let input = BrowseInstallInput {
        source: BrowseSource::Modrinth,
        id: "modpack-7".into(),
        kind: BrowseKind::Modpack,
        version_id: Some("modpack-7-v1".into()),
        instance_id: None,
        ram_mb: None,
    };
    let inst = st.browse_install(input).await.unwrap().instance;
    let game = st.instances.game_dir(&inst.id).unwrap();
    assert!(game.join(RECORD_FILE).is_file(), "relevé absent après l'installation");
    assert!(!needs_old_pack(&st.instances, &inst.id).unwrap(), "relevé non utilisable (origine absente ?)");
    assert_eq!(read(&game.join("config/hello.txt")), "hello");

    // Le joueur : fichier du pack modifié, mod à lui, options et monde.
    put(&game.join("config/hello.txt"), "hello du joueur");
    put(&game.join("mods/perso.jar"), "perso");
    put(&game.join("options.txt"), "fov:90");
    put(&game.join("saves/Monde/level.dat"), "niveau");

    let up = st.modpack_update_check(&inst.id).await.unwrap().expect("la 1.1.0 doit être proposée");
    assert_eq!((up.version_id.as_str(), up.version_number.as_str()), ("modpack-7-v2", "1.1.0"));

    let r = st.modpack_update_apply(&inst.id, &up.version_id).await.unwrap();
    assert_eq!(read(&game.join("config/added.txt")), "ajoute en 1.1.0");
    assert_eq!(read(&game.join("config/hello.txt")), "hello du joueur");
    assert!(r.kept.contains(&"config/hello.txt".to_string()), "{:?}", r.kept);
    assert_eq!(read(&game.join("mods/perso.jar")), "perso");
    assert_eq!(read(&game.join("options.txt")), "fov:90");
    assert_eq!(read(&game.join("saves/Monde/level.dat")), "niveau");
    assert_eq!(st.instances.get(&inst.id).unwrap().pack.unwrap().version_id, "modpack-7-v2");
    // Relevé réécrit pour la v2 : la prochaine mise à jour s'en servira.
    assert!(!needs_old_pack(&st.instances, &inst.id).unwrap());
    assert!(read(&game.join(RECORD_FILE)).contains("config/added.txt"));

    // Annulation : retour exact, relevé de la v1 remis, la mise à jour est de nouveau proposée.
    st.modpack_update_undo(&inst.id, &r.backup_id).await.unwrap();
    assert!(!game.join("config/added.txt").exists());
    assert_eq!(read(&game.join("config/hello.txt")), "hello du joueur");
    assert_eq!(st.instances.get(&inst.id).unwrap().pack.unwrap().version_id, "modpack-7-v1");
    assert!(!needs_old_pack(&st.instances, &inst.id).unwrap());
    assert!(!read(&game.join(RECORD_FILE)).contains("config/added.txt"));
    let again = st.modpack_update_check(&inst.id).await.unwrap();
    assert_eq!(again.map(|u| u.version_id), Some("modpack-7-v2".to_string()));

    curseforge(st).await;
}

/// CurseForge (projet 5000 du faux catalogue) : le pack serveur plus récent est ignoré, le mod non redistribuable est
/// signalé sans bloquer, le relevé évite l'ancien zip.
async fn curseforge(st: &cubelink_app::state::AppState) {
    let input = BrowseInstallInput {
        source: BrowseSource::Curseforge,
        id: "5000".into(),
        kind: BrowseKind::Modpack,
        version_id: Some("5001".into()),
        instance_id: None,
        ram_mb: None,
    };
    let inst = st.browse_install(input).await.unwrap().instance;
    let game = st.instances.game_dir(&inst.id).unwrap();
    assert!(!needs_old_pack(&st.instances, &inst.id).unwrap(), "relevé CurseForge non utilisable");
    assert!(game.join("mods/cf-mod-a-1.jar").is_file());
    put(&game.join("config/cf-keep.txt"), "keep du joueur");
    put(&game.join("mods/perso.jar"), "perso");

    let up = st.modpack_update_check(&inst.id).await.unwrap().expect("la 1.1.0 doit être proposée");
    assert_eq!(up.version_id, "5002", "le pack serveur 5003 ne doit pas être proposé");

    let r = st.modpack_update_apply(&inst.id, &up.version_id).await.unwrap();
    assert!(!game.join("mods/cf-mod-a-1.jar").exists());
    assert!(game.join("mods/cf-mod-a-2.jar").is_file());
    assert!(game.join("mods/cf-mod-b.jar").is_file());
    assert_eq!(r.missing, vec!["mods/cf-restreint.jar".to_string()]);
    assert_eq!(read(&game.join("config/cf-upd.txt")), "upd v2");
    assert_eq!(read(&game.join("config/added-cf.txt")), "ajoute en 1.1.0");
    assert_eq!(read(&game.join("config/cf-keep.txt")), "keep du joueur");
    assert_eq!(read(&game.join("mods/perso.jar")), "perso");
    assert_eq!(st.instances.get(&inst.id).unwrap().pack.unwrap().version_id, "5002");

    st.modpack_update_undo(&inst.id, &r.backup_id).await.unwrap();
    assert!(game.join("mods/cf-mod-a-1.jar").is_file());
    assert!(!game.join("mods/cf-mod-a-2.jar").exists());
    assert_eq!(read(&game.join("config/cf-upd.txt")), "upd v1");
    assert_eq!(read(&game.join("config/cf-keep.txt")), "keep du joueur");
    assert_eq!(st.instances.get(&inst.id).unwrap().pack.unwrap().version_id, "5001");
}
