//! Mise à jour d'un modpack installé (ticket #137) sur un vrai disque : un faux Modrinth en loopback sert deux versions
//! d'un modpack (vrais `.mrpack`, vrais jars de test). Parcours complet : installation de la v1 comme « Parcourir »,
//! détection de la v2, mise à jour (le joueur a modifié une config et ajouté un mod), vérification des fichiers, puis
//! annulation et retour à l'état exact d'avant. Aucun accès au vrai Modrinth.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use core_lib::browse::{self, BrowseInstallInput, BrowseKind, BrowseSource, Providers};
use core_lib::instance::InstanceStore;
use core_lib::modpack_update;
use core_lib::mrpack::{self, ImportOptions};
use core_lib::providers::curseforge_api::CfApi;
use core_lib::providers::modrinth_api::ModrinthClient;
use core_lib::providers::modrinth_dl::DlOptions;
use core_lib::store::index::StoreIndex;
use serde_json::json;
use sha2::{Digest, Sha512};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use zip::write::SimpleFileOptions;

const A1: &[u8] = b"jar a 1";
const A2: &[u8] = b"jar a 2";
const B: &[u8] = b"jar b";
const C: &[u8] = b"jar c";

fn sha512(b: &[u8]) -> String {
    hex::encode(Sha512::digest(b))
}

fn mrpack_bytes(server: &str, version: &str, files: &[(&str, &[u8])], overrides: &[(&str, &[u8])]) -> Vec<u8> {
    let files: Vec<_> = files
        .iter()
        .map(|(p, c)| json!({"path": p, "hashes": {"sha512": sha512(c), "sha1": core_lib::unknown_mods::sha1_hex(c)},
            "downloads": [format!("{server}/cdn/{}", &sha512(c)[..16])], "fileSize": c.len()}))
        .collect();
    let index = json!({"formatVersion": 1, "game": "minecraft", "versionId": version, "name": "Pack Test", "files": files,
        "dependencies": {"minecraft": "1.21.1", "fabric-loader": "0.16.5"}});
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    w.start_file("modrinth.index.json", SimpleFileOptions::default()).unwrap();
    w.write_all(index.to_string().as_bytes()).unwrap();
    for (n, c) in overrides {
        w.start_file(*n, SimpleFileOptions::default()).unwrap();
        w.write_all(c).unwrap();
    }
    w.finish().unwrap().into_inner()
}

fn version_json(server: &str, id: &str, number: &str, date: &str, file: &str, bytes: &[u8]) -> serde_json::Value {
    json!({"id": id, "project_id": "PACKPROJ", "name": format!("Pack {number}"), "version_number": number, "date_published": date,
        "version_type": "release", "game_versions": ["1.21.1"], "loaders": ["fabric"], "dependencies": [],
        "files": [{"url": format!("{server}/cdn/{file}"), "filename": file, "primary": true, "size": bytes.len(),
                   "hashes": {"sha1": core_lib::unknown_mods::sha1_hex(bytes), "sha512": sha512(bytes)}}]})
}

fn snapshot(g: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for en in std::fs::read_dir(dir).unwrap().flatten() {
            let p = en.path();
            let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            if rel == modpack_update::BACKUP_DIR {
                continue;
            }
            if p.is_dir() {
                walk(root, &p, out);
            } else {
                out.insert(rel, std::fs::read(&p).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(g, g, &mut out);
    out
}

#[tokio::test]
async fn catalogue_modpack_is_detected_updated_and_undone_on_a_real_disk() {
    let d = tempfile::tempdir().unwrap();
    let store = d.path().join("store");
    std::fs::create_dir_all(&store).unwrap();
    let index = StoreIndex::open(store.join("index.db")).unwrap();
    let instances = InstanceStore::new(d.path().join("instances"));
    let mr = MockServer::start().await;
    let cf = MockServer::start().await;
    let base = mr.uri();
    let p = Providers { modrinth: ModrinthClient::with_base_url(format!("{base}/v2"), "test"), cf: CfApi::with_base(cf.uri(), "k"), allow_loopback: true };

    // Catalogue : deux versions du projet, chacune un vrai .mrpack, plus les jars servis par le « CDN ».
    let v1 = mrpack_bytes(&base, "1.0.0", &[("mods/a.jar", A1), ("mods/b.jar", B)], &[("overrides/config/pack.toml", b"mode=normal\n")]);
    let v2 = mrpack_bytes(&base, "1.1.0", &[("mods/a.jar", A2), ("mods/c.jar", C)], &[("overrides/config/pack.toml", b"mode=hard\n")]);
    for (name, bytes) in [("pack-1.0.0.mrpack", &v1), ("pack-1.1.0.mrpack", &v2)] {
        Mock::given(method("GET")).and(path(format!("/cdn/{name}"))).respond_with(ResponseTemplate::new(200).set_body_bytes(bytes.clone())).mount(&mr).await;
    }
    for c in [A1, A2, B, C] {
        Mock::given(method("GET")).and(path(format!("/cdn/{}", &sha512(c)[..16]))).respond_with(ResponseTemplate::new(200).set_body_bytes(c.to_vec())).mount(&mr).await;
    }
    let ver1 = version_json(&base, "VER100", "1.0.0", "2026-01-01T00:00:00Z", "pack-1.0.0.mrpack", &v1);
    let ver2 = version_json(&base, "VER110", "1.1.0", "2026-03-01T00:00:00Z", "pack-1.1.0.mrpack", &v2);
    Mock::given(method("GET")).and(path("/v2/version/VER100")).respond_with(ResponseTemplate::new(200).set_body_json(&ver1)).mount(&mr).await;
    Mock::given(method("GET")).and(path("/v2/version/VER110")).respond_with(ResponseTemplate::new(200).set_body_json(&ver2)).mount(&mr).await;
    Mock::given(method("GET")).and(path("/v2/project/PACKPROJ/version")).respond_with(ResponseTemplate::new(200).set_body_json(json!([ver2, ver1]))).mount(&mr).await;

    let opts = ImportOptions {
        allowed_hosts: vec!["127.0.0.1".into()],
        allow_http: true,
        dl: DlOptions { retry_delay: std::time::Duration::from_millis(5), attempts: 1, ..Default::default() },
        ..Default::default()
    };
    let dl = DlOptions::default();
    let progress = |_: u32, _: u32, _: &str| {};
    let input = |v: &str| BrowseInstallInput { source: BrowseSource::Modrinth, id: "PACKPROJ".into(), kind: BrowseKind::Modpack, version_id: Some(v.into()), instance_id: None, ram_mb: None };

    // Installation de la v1 comme « Parcourir » : récupération, import, origine enregistrée.
    let tmp = d.path().join("tmp");
    let (f1, origin1) = browse::fetch_modpack(&p, &input("VER100"), &store, &index, &tmp.join("a"), &dl, &progress).await.unwrap();
    assert_eq!(origin1.version_id, "VER100");
    let inst = mrpack::import(&f1, &instances, &store, &index, &opts, None, Arc::new(AtomicBool::new(false))).await.unwrap();
    let inst = instances.set_pack(&inst.id, Some(origin1)).unwrap();
    let g = instances.game_dir(&inst.id).unwrap();
    assert_eq!(std::fs::read(g.join("mods/a.jar")).unwrap(), A1);

    // Détection : la v1.1.0 est proposée ; une fois installée, plus rien.
    let up = modpack_update::check(&p, &inst).await.unwrap().expect("une mise à jour est disponible");
    assert_eq!((up.version_id.as_str(), up.version_number.as_str()), ("VER110", "1.1.0"));

    // Le joueur : un mod à lui, ses mondes, ses options.
    std::fs::write(g.join("mods/perso.jar"), b"perso").unwrap();
    std::fs::create_dir_all(g.join("saves/Monde")).unwrap();
    std::fs::write(g.join("saves/Monde/level.dat"), b"niveau").unwrap();
    std::fs::write(g.join("options.txt"), b"fov:90").unwrap();
    let before = snapshot(&g);

    // Mise à jour.
    let (old_file, _) = browse::fetch_modpack(&p, &input("VER100"), &store, &index, &tmp.join("old"), &dl, &progress).await.unwrap();
    let (new_file, new_origin) = browse::fetch_modpack(&p, &input(&up.version_id), &store, &index, &tmp.join("new"), &dl, &progress).await.unwrap();
    let r = modpack_update::apply(&instances, &inst.id, Some(&old_file), &new_file, new_origin.clone(), &core_lib::providers::curseforge::CfResolver::new(), &store, &index, &opts, None, Arc::new(AtomicBool::new(false))).await.unwrap();
    assert_eq!((r.added, r.removed, r.replaced), (1, 1, 2), "{r:?}"); // c ajouté ; b retiré ; a et pack.toml remplacés
    assert!(r.kept.is_empty());
    assert_eq!(std::fs::read(g.join("mods/a.jar")).unwrap(), A2);
    assert_eq!(std::fs::read(g.join("mods/c.jar")).unwrap(), C);
    assert!(!g.join("mods/b.jar").exists());
    assert_eq!(std::fs::read(g.join("config/pack.toml")).unwrap(), b"mode=hard\n");
    assert_eq!(std::fs::read(g.join("mods/perso.jar")).unwrap(), b"perso");
    assert_eq!(std::fs::read(g.join("saves/Monde/level.dat")).unwrap(), b"niveau");
    assert_eq!(std::fs::read(g.join("options.txt")).unwrap(), b"fov:90");
    let updated = instances.get(&inst.id).unwrap();
    assert_eq!(updated.pack, Some(new_origin));
    assert_eq!(updated.mod_count, 3); // a, c, perso
    // Le pack est à jour : plus de proposition (la liste du faux catalogue est triée, VER110 en tête).
    assert_eq!(modpack_update::check(&p, &updated).await.unwrap(), None);
    let raw: serde_json::Value = serde_json::from_slice(&std::fs::read(d.path().join("instances").join(&inst.id).join("instance.json")).unwrap()).unwrap();
    assert_eq!(raw["pack"]["version_id"], "VER110");

    // Annulation : arbre identique octet pour octet, ancienne origine revenue, la proposition réapparaît.
    let back = modpack_update::undo(&instances, &inst.id, &r.backup_id, &index).unwrap();
    assert_eq!(snapshot(&g), before);
    assert_eq!(back.pack.unwrap().version_id, "VER100");
    assert!(modpack_update::check(&p, &instances.get(&inst.id).unwrap()).await.unwrap().is_some());
    let leftovers: Vec<PathBuf> = std::fs::read_dir(g.join(modpack_update::BACKUP_DIR)).map(|r| r.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}
