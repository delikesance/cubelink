//! Fichiers de l'hôte modifiés entre le calcul du manifeste et le transfert (config réécrite par le jeu,
//! mod remplacé) : l'invité ne doit jamais recevoir un fichier qui ne correspond pas au manifeste annoncé.

use std::time::Duration;

use core_lib::types::{TransferPhase, TransferProgress};
use cubelink_app::p2p_backend::P2pBackend;

mod common;
use common::{befriend, world, Node};

/// L'instance de `world` plus un fichier de config (`config/mod.json`).
fn world_with_config(host: &Node) -> String {
    let id = world(host);
    let cfg = host.st.instances.game_dir(&id).unwrap().join("config");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(cfg.join("mod.json"), b"{\"v\":1}").unwrap();
    id
}

/// Lance un join et renvoie la dernière progression terminale (Done ou Failed).
async fn join(guest: &Node, host_key: &str) -> TransferProgress {
    let before = guest.progress().len();
    let plan = guest.st.join_plan(host_key).await.unwrap();
    guest.st.join_start(host_key, None, &plan.manifest_hash, true).await.unwrap();
    for _ in 0..300 {
        let p = guest.progress();
        if let Some(t) = p[before..].iter().find(|p| matches!(p.phase, TransferPhase::Done | TransferPhase::Failed)) {
            return t.clone();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("transfert non terminé");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn config_reecrite_par_le_jeu_apres_le_manifeste_ne_fait_pas_echouer() {
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let host = Node::start(da.path(), "Hote").await;
    let guest = Node::start(db.path(), "Invite").await;
    befriend(&guest, &host).await;
    let id = world_with_config(&host);
    host.be.host_start_sharing(id.clone(), 0).await.unwrap();
    // Le jeu démarre et réécrit sa config APRÈS le calcul du manifeste.
    let gd = host.st.instances.game_dir(&id).unwrap();
    std::fs::write(gd.join("config/mod.json"), b"{\"v\":2,\"window\":[1,2,3]}").unwrap();

    let t = join(&guest, &host.key()).await;
    assert_eq!(t.phase, TransferPhase::Done, "{t:?}");
    let g = guest.st.instances.list().into_iter().next().expect("instance invitée");
    let content = std::fs::read(guest.st.instances.game_dir(&g.id).unwrap().join("config/mod.json")).unwrap();
    assert_eq!(content, b"{\"v\":1}", "l'invité reçoit exactement le fichier annoncé (instantané)");
    host.be.shutdown().await;
    guest.be.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mod_remplace_apres_le_manifeste_echec_clair_puis_reessai_avec_manifeste_frais() {
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let host = Node::start(da.path(), "Hote").await;
    let guest = Node::start(db.path(), "Invite").await;
    befriend(&guest, &host).await;
    let id = world_with_config(&host);
    host.be.host_start_sharing(id.clone(), 0).await.unwrap();
    let gd = host.st.instances.game_dir(&id).unwrap();
    // Un mod est mis à jour pendant que le partage est actif : le manifeste annoncé est périmé.
    std::fs::write(gd.join("mods/a.jar"), vec![9u8; 6000]).unwrap();

    // Jamais de fichier non conforme : le premier essai échoue avec une raison lisible (pas d'« empreinte »).
    let t = join(&guest, &host.key()).await;
    assert_eq!(t.phase, TransferPhase::Failed, "{t:?}");
    let msg = t.error.clone().unwrap_or_default();
    assert!(msg.contains("a changé chez ton ami"), "{msg}");
    assert!(guest.st.instances.list().is_empty(), "rien d'installé après un échec");

    // L'hôte a recalculé son manifeste : « Réessayer » reprend un état frais et réussit.
    for _ in 0..50 {
        let plan = guest.st.join_plan(&host.key()).await.unwrap();
        if plan.manifest.files.iter().any(|f| f.path == "mods/a.jar" && f.size == 6000) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let t = join(&guest, &host.key()).await;
    assert_eq!(t.phase, TransferPhase::Done, "{t:?}");
    let g = guest.st.instances.list().into_iter().next().expect("instance invitée");
    assert_eq!(std::fs::read(guest.st.instances.game_dir(&g.id).unwrap().join("mods/a.jar")).unwrap(), vec![9u8; 6000]);
    host.be.shutdown().await;
    guest.be.shutdown().await;
}
