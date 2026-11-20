//! #72 : un échec de préparation du jeu (réseau, Java) après une création ou un import ne supprime plus l'instance.
//! Vrai AppState, vrai disque (dossier temporaire), vrai HTTP loopback : le « Mojang » du mode test est un port fermé,
//! donc `ensure_game` échoue réellement, comme avec une coupure réseau.

use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex, Once};

use core_lib::types::{Loader, LoaderSpec};
use cubelink_app::p2p_backend::UnavailableBackend;
use cubelink_app::state::{AppState, EventSink};
use zip::write::SimpleFileOptions;

type Events = Arc<Mutex<Vec<(String, serde_json::Value)>>>;

/// Les surcharges de test sont lues une seule fois par processus : on les pose avant le premier appel.
fn mode_test_mojang_coupe() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        std::env::set_var("CUBELINK_TEST_MODE", "1");
        // Port 1 en loopback : connexion refusée tout de suite.
        std::env::set_var("CUBELINK_MOJANG_META_URL", "http://127.0.0.1:1");
    });
}

fn etat(dir: &Path) -> (Arc<AppState>, Events) {
    mode_test_mojang_coupe();
    let events: Events = Arc::default();
    let ev = events.clone();
    let sink: EventSink = Arc::new(move |n, v| ev.lock().unwrap().push((n.to_string(), v)));
    (Arc::new(AppState::new(dir, sink, Arc::new(UnavailableBackend)).unwrap()), events)
}

fn zip_brut(dir: &Path, entrees: &[(&str, &[u8])]) -> std::path::PathBuf {
    let p = dir.join("pack.zip");
    let mut w = zip::ZipWriter::new(std::fs::File::create(&p).unwrap());
    for (n, c) in entrees {
        w.start_file(*n, SimpleFileOptions::default()).unwrap();
        w.write_all(c).unwrap();
    }
    w.finish().unwrap();
    p
}

fn noms(ev: &Events, nom: &str) -> Vec<serde_json::Value> {
    ev.lock().unwrap().iter().filter(|(n, _)| n == nom).map(|(_, v)| v.clone()).collect()
}

fn vanilla() -> LoaderSpec {
    LoaderSpec { loader: Loader::Vanilla, version: String::new() }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn creation_garde_l_instance_quand_la_preparation_echoue() {
    let dir = tempfile::tempdir().unwrap();
    let (st, ev) = etat(dir.path());

    let inst = st.instances_create("Survie".into(), "1.21.1".into(), vanilla(), 4096).await.expect("la création réussit malgré l'échec de préparation");

    assert_eq!(st.instances_list().len(), 1, "l'instance est conservée");
    assert_eq!(st.instances_list()[0].id, inst.id);
    let deferred = noms(&ev, "import://deferred");
    assert_eq!(deferred.len(), 1, "l'échec est annoncé : {:?}", ev.lock().unwrap());
    assert_eq!(deferred[0]["instance_id"], inst.id);
    assert!(!deferred[0]["message"].as_str().unwrap_or("").is_empty());
    // La progression n'est plus muette : la première étape de la préparation est annoncée.
    let prog = noms(&ev, "import://progress");
    assert!(prog.iter().any(|p| p["message"].as_str().is_some_and(|m| !m.is_empty())), "progression absente : {prog:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_garde_l_instance_et_ses_fichiers_quand_la_preparation_echoue() {
    let dir = tempfile::tempdir().unwrap();
    let pack = tempfile::tempdir().unwrap();
    let (st, ev) = etat(dir.path());
    let zip = zip_brut(pack.path(), &[("mods/a.jar", b"contenu du mod A"), ("config/x.txt", b"reglage")]);

    let inst = st
        .instances_import_mrpack(&zip.display().to_string(), Some("Pack".into()), Some("1.21.1".into()), Some(vanilla()), None)
        .await
        .expect("l'import réussit malgré l'échec de préparation");

    assert_eq!(st.instances_list().len(), 1, "l'instance importée est conservée");
    let game = st.instances.game_dir(&inst.id).unwrap();
    assert_eq!(std::fs::read(game.join("mods/a.jar")).unwrap(), b"contenu du mod A", "les mods déjà installés sont intacts");
    assert!(game.join("config/x.txt").exists());
    let deferred = noms(&ev, "import://deferred");
    assert_eq!(deferred.len(), 1);
    assert_eq!(deferred[0]["instance_id"], inst.id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_invalide_echoue_toujours_sans_laisser_d_instance() {
    let dir = tempfile::tempdir().unwrap();
    let pack = tempfile::tempdir().unwrap();
    let (st, ev) = etat(dir.path());
    let faux = pack.path().join("casse.mrpack");
    std::fs::write(&faux, b"pas un zip").unwrap();

    let e = st.instances_import_mrpack(&faux.display().to_string(), None, None, None, None).await;
    assert!(e.is_err(), "un import invalide reste une erreur");
    assert!(st.instances_list().is_empty(), "aucune instance ajoutée");
    assert!(noms(&ev, "import://deferred").is_empty(), "pas d'avertissement « différé » pour un vrai échec d'import");
}
