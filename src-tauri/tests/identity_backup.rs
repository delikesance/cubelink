//! Sauvegarde et restauration d'identité sur de vrais dossiers de données : deux nœuds démarrés par `AppState::new`,
//! Argon2id au coût réel (64 Mio x 3), vrai disque. Le « PC d'arrivée » démarre avec une identité neuve, restaure la
//! sauvegarde du premier, redémarre et doit retrouver la même clé publique et les mêmes amis.

use core_lib::types::{Friend, TrustLevel};

mod common;
use common::Node;

const PASS: &str = "une phrase de passe assez longue";

fn friend(c: char, name: &str) -> Friend {
    Friend { public_key: c.to_string().repeat(64), name: name.into(), trust: TrustLevel::Always, added_at: 7 }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn l_identite_et_les_amis_passent_sur_un_autre_pc() {
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let backup = da.path().join("moi.cubelink-id");

    // PC d'origine : une identité, deux amis, une sauvegarde.
    let a = Node::start(da.path(), "Alice").await;
    a.st.friends.add_friend(&friend('a', "Alice la copine")).unwrap();
    a.st.friends.add_friend(&friend('b', "Bob")).unwrap();
    let a_key = a.key();
    let exported = a.st.identity_export_to(&backup, PASS).unwrap();
    assert_eq!(exported, a_key);
    assert!(a.st.identity_export_to(&da.path().join("court.cubelink-id"), "court").is_err());
    a.be.shutdown().await;
    drop(a);

    // PC d'arrivée : identité neuve, un ami à lui.
    let b = Node::start(db.path(), "Bob").await;
    let b_key = b.key();
    assert_ne!(b_key, a_key);
    b.st.friends.add_friend(&friend('c', "Carole")).unwrap();

    // Mauvaise phrase, fichier qui n'est pas une sauvegarde : refus, rien n'est déposé.
    let err = b.st.identity_stage_import(&backup, "pas la bonne phrase du tout").unwrap_err();
    assert!(err.contains("Phrase de passe incorrecte"), "{err}");
    let junk = db.path().join("junk.cubelink-id");
    std::fs::write(&junk, b"rien a voir avec une sauvegarde, un simple texte qui est assez long").unwrap();
    assert!(b.st.identity_stage_import(&junk, PASS).unwrap_err().contains("pas une sauvegarde"));
    assert!(b.st.identity_check_restart().is_err(), "rien en attente");

    // Pendant une partie : refusé.
    *b.st.game.lock().unwrap() = Some("inst".into());
    assert!(b.st.identity_stage_import(&backup, PASS).unwrap_err().contains("Ferme Minecraft"));
    *b.st.game.lock().unwrap() = None;

    // Import : dépose, ne change rien tant que l'app tourne.
    assert_eq!(b.st.identity_stage_import(&backup, PASS).unwrap(), a_key);
    assert_eq!(b.key(), b_key);
    b.st.identity_check_restart().unwrap();
    b.be.shutdown().await;
    drop(b);

    // Redémarrage : même clé publique, amis restaurés, anciens fichiers gardés en .bak.
    let b2 = Node::start(db.path(), "Bob").await;
    assert_eq!(b2.key(), a_key);
    let names: Vec<String> = b2.st.friends.list_friends().unwrap().into_iter().map(|f| f.name).collect();
    assert_eq!(names, vec!["Alice la copine".to_string(), "Bob".to_string()]);
    let p2p = &b2.st.paths.p2p;
    let files: Vec<String> = std::fs::read_dir(p2p).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert!(files.iter().any(|f| f.starts_with("identity.key.bak-")), "{files:?}");
    assert!(files.iter().any(|f| f.starts_with("friends.db.bak-")), "{files:?}");
    assert!(!files.iter().any(|f| f.starts_with("import-pending")), "{files:?}");
    assert!(b2.st.identity_check_restart().is_err(), "restauration appliquée une seule fois");
    let bak = files.iter().find(|f| f.starts_with("identity.key.bak-")).unwrap();
    assert_eq!(hex::encode(std::fs::read(p2p.join(bak)).unwrap()).len(), 64);
    b2.be.shutdown().await;
}
