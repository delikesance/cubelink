//! Un mod du pack dont l'identifiant interne (modid) est déjà sur le disque dans un jar du joueur n'est pas posé en double.

use super::*;
use crate::mod_meta::tests::zip_bytes;

fn fabric_jar(modid: &str, version: &str) -> Vec<u8> {
    let json = format!(r#"{{"id":"{modid}","version":"{version}"}}"#);
    zip_bytes(&[("fabric.mod.json", json.as_bytes())])
}

async fn serve(e: &Env, bytes: &[u8]) {
    Mock::given(method("GET"))
        .and(path(format!("/{}", &sha512(bytes)[..16])))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes.to_vec()))
        .mount(&e.server)
        .await;
}

#[tokio::test]
async fn pack_mod_with_the_modid_of_a_player_jar_is_not_installed_twice() {
    let e = env().await;
    let from_pack = fabric_jar("sodium", "1.0");
    serve(&e, &from_pack).await;
    let v1 = pack(&e, "v1", "v1", "0.16.5", &[("mods/b.jar", B1)], &[]);
    let v2 = pack(
        &e,
        "v2",
        "v2",
        "0.16.5",
        &[("mods/b.jar", B1), ("mods/sodium-cf.jar", &from_pack)],
        &[],
    );
    let (inst, g) = install(&e, &v1).await;
    // Le joueur a mis la version Modrinth du même mod, sous un autre nom de fichier.
    let player = fabric_jar("sodium", "2.0");
    std::fs::write(g.join("mods/sodium-modrinth.jar"), &player).unwrap();

    let r = update(&e, &inst.id, &v1, &v2).await.unwrap();

    assert_eq!(r.added, 0, "{r:?}");
    assert_eq!(r.kept, vec!["mods/sodium-modrinth.jar".to_string()]);
    assert!(!g.join("mods/sodium-cf.jar").exists());
    assert_eq!(
        std::fs::read(g.join("mods/sodium-modrinth.jar")).unwrap(),
        player
    );
}

#[tokio::test]
async fn pack_mod_with_a_new_modid_is_still_installed() {
    let e = env().await;
    let from_pack = fabric_jar("lithium", "1.0");
    serve(&e, &from_pack).await;
    let v1 = pack(&e, "v1", "v1", "0.16.5", &[("mods/b.jar", B1)], &[]);
    let v2 = pack(
        &e,
        "v2",
        "v2",
        "0.16.5",
        &[("mods/b.jar", B1), ("mods/lithium.jar", &from_pack)],
        &[],
    );
    let (inst, g) = install(&e, &v1).await;
    std::fs::write(g.join("mods/sodium.jar"), fabric_jar("sodium", "2.0")).unwrap();

    let r = update(&e, &inst.id, &v1, &v2).await.unwrap();

    assert_eq!((r.added, r.kept.len()), (1, 0), "{r:?}");
    assert!(g.join("mods/lithium.jar").exists());
}
