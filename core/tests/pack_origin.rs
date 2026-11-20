//! Origine d'un modpack (ticket #84) sur un vrai disque : un vrai `.mrpack` (zip) importé, reconnu par son SHA-1
//! auprès d'un faux Modrinth en loopback, enregistré dans `instance.json`, puis utilisé pour l'illustration.
//! Aucun accès au vrai Modrinth, aucune dépendance Windows.

use std::io::Write as _;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use core_lib::art::lookup_and_apply;
use core_lib::browse::{resolve_pack_origin, Providers};
use core_lib::instance::InstanceStore;
use core_lib::mrpack::{self, ImportOptions};
use core_lib::providers::curseforge_api::CfApi;
use core_lib::providers::modrinth_api::ModrinthClient;
use core_lib::store::index::StoreIndex;
use core_lib::types::PackOrigin;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use zip::write::SimpleFileOptions;

const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3, 4];

fn write_mrpack(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let p = dir.join(format!("{name}.mrpack"));
    let mut w = zip::ZipWriter::new(std::fs::File::create(&p).unwrap());
    let index = json!({"formatVersion": 1, "game": "minecraft", "versionId": "v-libre-du-createur", "name": "Mon Pack", "files": [],
                       "dependencies": {"minecraft": "1.21.1", "fabric-loader": "0.16.5"}});
    w.start_file("modrinth.index.json", SimpleFileOptions::default()).unwrap();
    w.write_all(index.to_string().as_bytes()).unwrap();
    w.start_file("overrides/config/x.toml", SimpleFileOptions::default()).unwrap();
    w.write_all(b"a=1").unwrap();
    w.finish().unwrap();
    p
}

#[tokio::test]
async fn imported_mrpack_gets_exact_origin_and_art_not_the_homonym() {
    let d = tempfile::tempdir().unwrap();
    let store = d.path().join("store");
    std::fs::create_dir_all(&store).unwrap();
    let index = StoreIndex::open(store.join("index.db")).unwrap();
    let instances = InstanceStore::new(d.path().join("instances"));
    let (mr, cf) = (MockServer::start().await, MockServer::start().await);
    let p = Providers { modrinth: ModrinthClient::with_base_url(format!("{}/v2", mr.uri()), "test"), cf: CfApi::with_base(cf.uri(), "k"), allow_loopback: true };

    // Vrai .mrpack sur le disque, importé par le vrai flux d'import.
    let file = write_mrpack(d.path(), "pack");
    let opts = ImportOptions { allowed_hosts: vec!["127.0.0.1".into()], allow_http: true, ..Default::default() };
    let inst = mrpack::import(&file, &instances, &store, &index, &opts, None, Arc::new(AtomicBool::new(false))).await.unwrap();
    assert_eq!(inst.pack, None);
    assert_eq!(std::fs::read(instances.game_dir(&inst.id).unwrap().join("config/x.toml")).unwrap(), b"a=1");

    // Faux Modrinth : le fichier est la version VER1 du projet VRAI ; la recherche par nom désignerait un homonyme.
    let sha1 = core_lib::unknown_mods::sha1_hex(&std::fs::read(&file).unwrap());
    Mock::given(method("GET"))
        .and(path(format!("/v2/version_file/{sha1}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "VER1", "project_id": "VRAI", "version_number": "2.0", "files": [], "dependencies": []})))
        .mount(&mr)
        .await;
    Mock::given(method("GET"))
        .and(path("/v2/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"hits": [{"project_id": "HOMONYME", "slug": "h", "title": "Mon Pack", "author": "x"}], "total_hits": 1})))
        .expect(0)
        .mount(&mr)
        .await;
    Mock::given(method("GET"))
        .and(path("/v2/project/VRAI"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "VRAI", "title": "Le vrai pack", "icon_url": format!("{}/img/icon", mr.uri())})))
        .mount(&mr)
        .await;
    Mock::given(method("GET")).and(path("/img/icon")).respond_with(ResponseTemplate::new(200).set_body_bytes(PNG.to_vec())).mount(&mr).await;

    let origin = resolve_pack_origin(&p, &file).await.expect("le fichier est connu de Modrinth");
    assert_eq!(origin, PackOrigin { source: "modrinth".into(), project_id: "VRAI".into(), version_id: "VER1".into() });
    instances.set_pack(&inst.id, Some(origin.clone())).unwrap();

    // Illustration : projet exact, sans recherche par nom (le faux serveur échoue si /v2/search est appelé).
    let with_art = lookup_and_apply(&p, &instances, &inst.id, true).await.unwrap();
    let art = with_art.art.clone().unwrap();
    assert_eq!((art.source.as_str(), art.project_id.as_str(), art.title.as_str()), ("modrinth", "VRAI", "Le vrai pack"));
    assert!(instances.art_dir(&inst.id).unwrap().join(art.icon.as_deref().unwrap()).is_file());

    // Sur le disque : l'origine et l'art cohabitent dans instance.json, y compris après un rechargement.
    let raw: serde_json::Value = serde_json::from_slice(&std::fs::read(d.path().join("instances").join(&inst.id).join("instance.json")).unwrap()).unwrap();
    assert_eq!(raw["pack"], json!({"source": "modrinth", "project_id": "VRAI", "version_id": "VER1"}));
    assert_eq!(raw["art"]["project_id"], "VRAI");
    let reloaded = InstanceStore::new(d.path().join("instances")).get(&inst.id).unwrap();
    assert_eq!((reloaded.pack, reloaded.art.map(|a| a.project_id)), (Some(origin), Some("VRAI".to_string())));

    // Un .mrpack que Modrinth ne connaît pas (404) : pas d'origine, et l'import n'en dépend pas.
    let other = write_mrpack(d.path(), "maison");
    let unknown_mr = MockServer::start().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(404)).mount(&unknown_mr).await;
    let p2 = Providers { modrinth: ModrinthClient::with_base_url(format!("{}/v2", unknown_mr.uri()), "test"), cf: CfApi::with_base(cf.uri(), "k"), allow_loopback: true };
    assert_eq!(resolve_pack_origin(&p2, &other).await, None);
}
