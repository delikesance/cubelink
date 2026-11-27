//! Reproduit sous Linux l'étape de `e2e/10-browse.spec.ts` : un mod installé depuis Modrinth (comme dépendance), puis
//! le même mod installé depuis CurseForge sur la même instance. Le jar doit rester dans `mods/`.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};

use core_lib::browse::{BrowseInstallInput, BrowseKind, BrowseSource};
use core_lib::types::{Loader, LoaderSpec};

mod common;
use common::Node;

/// Arrête le faux serveur même si le test échoue (sinon il garde la sortie ouverte).
struct KillOnDrop(std::process::Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn meme_mod_modrinth_puis_curseforge_reste_installe() {
    if Command::new("node")
        .arg("--version")
        .stdout(Stdio::null())
        .status()
        .is_err()
    {
        eprintln!("node absent : test ignoré");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let mut child = Command::new("node")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests-fixtures/browse/server.mjs"))
        .args(["--port", "0"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let _server = KillOnDrop(child);
    let mut line = String::new();
    BufReader::new(stdout).read_line(&mut line).unwrap();
    let base = serde_json::from_str::<serde_json::Value>(&line).unwrap()["base"]
        .as_str()
        .unwrap()
        .to_string();
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
    let inst = st
        .instances
        .create(
            "Monde de test",
            "1.21.1",
            LoaderSpec {
                loader: Loader::Fabric,
                version: "0.16.0".into(),
            },
            2048,
        )
        .unwrap();
    let mods = st.instances.game_dir(&inst.id).unwrap().join("mods");
    let list = || {
        let mut v: Vec<String> = std::fs::read_dir(&mods)
            .map(|d| {
                d.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    };
    let install = |source, id: &str| BrowseInstallInput {
        source,
        id: id.into(),
        kind: BrowseKind::Mod,
        version_id: None,
        instance_id: Some(inst.id.clone()),
        ram_mb: None,
    };
    st.browse_install(install(BrowseSource::Modrinth, "mod-0"))
        .await
        .unwrap();
    eprintln!("après Modrinth : {:?}", list());
    let r = st
        .browse_install(install(BrowseSource::Curseforge, "1001"))
        .await;
    eprintln!(
        "après CurseForge : {:?} -> {:?}",
        list(),
        r.as_ref().map(|_| ())
    );
    r.unwrap();
    assert!(
        list().iter().any(|f| f.starts_with("mod-1-1.0.0")),
        "{:?}",
        list()
    );
}
