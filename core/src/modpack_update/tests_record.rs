//! Tests du chantier « relevé » : ancien état tiré du relevé, sans retélécharger l'ancien pack.

use super::*;
use std::io::Write;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use zip::write::SimpleFileOptions;

const A1: &[u8] = b"mod a version 1";
const A2: &[u8] = b"mod a version 2";
const B1: &[u8] = b"mod b";
const C1: &[u8] = b"mod c ajoute";
const D1: &[u8] = b"mod d retire";
const CFG1: &[u8] = b"volume=1\n";
const CFG2: &[u8] = b"volume=2\n";

fn sha512(b: &[u8]) -> String {
    hex::encode(Sha512::digest(b))
}

struct Env {
    tmp: tempfile::TempDir,
    instances: InstanceStore,
    store: PathBuf,
    idx: StoreIndex,
    server: MockServer,
}

async fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("store");
    std::fs::create_dir_all(&store).unwrap();
    let idx = StoreIndex::open(store.join("index.db")).unwrap();
    let server = MockServer::start().await;
    for c in [A1, A2, B1, C1, D1] {
        Mock::given(method("GET")).and(path(format!("/{}", &sha512(c)[..16]))).respond_with(ResponseTemplate::new(200).set_body_bytes(c.to_vec())).mount(&server).await;
    }
    Env { instances: InstanceStore::new(tmp.path().join("instances")), tmp, store, idx, server }
}

fn opts() -> ImportOptions {
    ImportOptions {
        allowed_hosts: vec!["127.0.0.1".into()],
        allow_http: true,
        dl: DlOptions { retry_delay: std::time::Duration::from_millis(5), attempts: 1, ..Default::default() },
        ..Default::default()
    }
}

/// `.mrpack` : `files` = (chemin, contenu servi par le faux serveur), `overrides` = (chemin dans le zip, contenu).
fn pack(e: &Env, name: &str, version: &str, files: &[(&str, &[u8])], overrides: &[(&str, &[u8])]) -> PathBuf {
    let files: Vec<serde_json::Value> = files
        .iter()
        .map(|(p, c)| {
            serde_json::json!({"path": p, "hashes": {"sha512": sha512(c), "sha1": crate::unknown_mods::sha1_hex(c)},
                "downloads": [format!("{}/{}", e.server.uri(), &sha512(c)[..16])], "fileSize": c.len()})
        })
        .collect();
    let index = serde_json::to_vec(&serde_json::json!({
        "formatVersion": 1, "game": "minecraft", "versionId": version, "name": "Mon Pack", "files": files,
        "dependencies": {"minecraft": "1.21.1", "fabric-loader": "0.16.5"}
    }))
    .unwrap();
    let p = e.tmp.path().join(format!("{name}.mrpack"));
    let mut w = zip::ZipWriter::new(std::fs::File::create(&p).unwrap());
    w.start_file("modrinth.index.json", SimpleFileOptions::default()).unwrap();
    w.write_all(&index).unwrap();
    for (n, c) in overrides {
        w.start_file(*n, SimpleFileOptions::default()).unwrap();
        w.write_all(c).unwrap();
    }
    w.finish().unwrap();
    p
}

fn origin(v: &str) -> PackOrigin {
    PackOrigin { source: "modrinth".into(), project_id: "PROJ".into(), version_id: v.into() }
}

fn prov(id: &str) -> Provenance {
    Provenance { source: "modrinth".into(), id: id.into() }
}

fn not_cancelled() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

/// Installe `v1` comme le fait « Parcourir » : import, origine enregistrée, puis posée dans le relevé.
async fn install(e: &Env, v1: &Path) -> (Instance, PathBuf) {
    let inst = mrpack::import(v1, &e.instances, &e.store, &e.idx, &opts(), None, not_cancelled()).await.unwrap();
    let inst = e.instances.set_pack(&inst.id, Some(origin("v1"))).unwrap();
    let g = e.instances.game_dir(&inst.id).unwrap();
    stamp_origin(&g, &origin("v1")).unwrap();
    (inst, g)
}

async fn update(e: &Env, id: &str, old: Option<&Path>, new: &Path) -> Result<ModpackUpdateResult> {
    apply(&e.instances, id, old, new, origin("v2"), &CfResolver::new(), &e.store, &e.idx, &opts(), None, not_cancelled()).await
}

fn record_json(g: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(g.join(RECORD_FILE)).unwrap()).unwrap()
}

fn write_record(g: &Path, rec: &serde_json::Value) {
    std::fs::write(g.join(RECORD_FILE), serde_json::to_vec(rec).unwrap()).unwrap();
}

/// Chemin -> empreinte du relevé.
fn digests(g: &Path) -> BTreeMap<String, String> {
    record_json(g)["files"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), v["digest"]["hex"].as_str().unwrap().to_string())).collect()
}

fn expected(files: &[(&str, &[u8])]) -> BTreeMap<String, String> {
    files.iter().map(|(p, c)| (p.to_string(), sha512(c))).collect()
}

/// v1 : a1, b, d + config ; v2 : a2 (remplace a1), b (inchangé), c (ajouté), d retiré + config changée.
fn v1v2(e: &Env) -> (PathBuf, PathBuf) {
    let v1 = pack(e, "v1", "v1", &[("mods/a.jar", A1), ("mods/b.jar", B1), ("mods/d.jar", D1)], &[("overrides/config/x.toml", CFG1), ("overrides/config/y.toml", b"y=1")]);
    let v2 = pack(e, "v2", "v2", &[("mods/a.jar", A2), ("mods/b.jar", B1), ("mods/c.jar", C1)], &[("overrides/config/x.toml", CFG2), ("overrides/config/y.toml", b"y=1")]);
    (v1, v2)
}

fn v1_files() -> [(&'static str, &'static [u8]); 5] {
    [("mods/a.jar", A1), ("mods/b.jar", B1), ("mods/d.jar", D1), ("config/x.toml", CFG1), ("config/y.toml", b"y=1")]
}

fn v2_files() -> [(&'static str, &'static [u8]); 5] {
    [("mods/a.jar", A2), ("mods/b.jar", B1), ("mods/c.jar", C1), ("config/x.toml", CFG2), ("config/y.toml", b"y=1")]
}

#[tokio::test]
async fn install_writes_the_expected_digests_and_origin() {
    let e = env().await;
    let (v1, _) = v1v2(&e);
    let (_, g) = install(&e, &v1).await;
    let rec = record_json(&g);
    assert_eq!(rec["format"], 1);
    assert_eq!(rec["origin"]["version_id"], "v1");
    assert_eq!(digests(&g), expected(&v1_files()));
    assert_eq!(rec["files"]["mods/a.jar"]["digest"]["alg"], "sha512");
}

#[tokio::test]
async fn install_record_prefers_the_override_at_the_same_path() {
    let e = env().await;
    let v1 = pack(&e, "v1", "v1", &[("config/x.toml", A1), ("mods/b.jar", B1)], &[("overrides/config/x.toml", CFG1)]);
    let (_, g) = install(&e, &v1).await;
    assert_eq!(digests(&g), expected(&[("config/x.toml", CFG1), ("mods/b.jar", B1)]));
}

#[tokio::test]
async fn update_without_old_pack_keeps_player_files_and_applies_the_diff() {
    let e = env().await;
    let (v1, v2) = v1v2(&e);
    let (inst, g) = install(&e, &v1).await;
    std::fs::write(g.join("mods/perso.jar"), b"mod du joueur").unwrap();
    std::fs::write(g.join("config/x.toml"), b"volume=11 (le joueur)\n").unwrap();
    assert!(!needs_old_pack(&e.instances, &inst.id).unwrap());
    let r = update(&e, &inst.id, None, &v2).await.unwrap();
    assert_eq!((r.added, r.removed, r.replaced), (1, 1, 1), "{r:?}"); // c ajouté, d retiré, a remplacé
    assert_eq!(r.kept, vec!["config/x.toml".to_string()]);
    assert_eq!(std::fs::read(g.join("mods/a.jar")).unwrap(), A2);
    assert_eq!(std::fs::read(g.join("mods/c.jar")).unwrap(), C1);
    assert!(!g.join("mods/d.jar").exists());
    assert_eq!(std::fs::read(g.join("mods/perso.jar")).unwrap(), b"mod du joueur");
    assert_eq!(std::fs::read(g.join("config/x.toml")).unwrap(), b"volume=11 (le joueur)\n");
}

#[tokio::test]
async fn stale_record_requires_the_old_pack() {
    let e = env().await;
    let (v1, v2) = v1v2(&e);
    let (inst, _) = install(&e, &v1).await;
    e.instances.set_pack(&inst.id, Some(origin("autre"))).unwrap();
    assert!(needs_old_pack(&e.instances, &inst.id).unwrap());
    assert!(update(&e, &inst.id, None, &v2).await.is_err());
}

#[tokio::test]
async fn record_without_origin_is_not_used() {
    let e = env().await;
    let (v1, _) = v1v2(&e);
    let inst = mrpack::import(&v1, &e.instances, &e.store, &e.idx, &opts(), None, not_cancelled()).await.unwrap();
    e.instances.set_pack(&inst.id, Some(origin("v1"))).unwrap();
    assert!(needs_old_pack(&e.instances, &inst.id).unwrap());
}

#[tokio::test]
async fn record_with_a_path_escaping_the_instance_is_ignored_entirely() {
    let e = env().await;
    let (v1, _) = v1v2(&e);
    let (inst, g) = install(&e, &v1).await;
    let mut rec = record_json(&g);
    rec["files"]["../x"] = rec["files"]["mods/a.jar"].clone();
    write_record(&g, &rec);
    assert!(needs_old_pack(&e.instances, &inst.id).unwrap());
}

#[tokio::test]
async fn corrupt_record_is_ignored() {
    let e = env().await;
    let (v1, v2) = v1v2(&e);
    let (inst, g) = install(&e, &v1).await;
    std::fs::write(g.join(RECORD_FILE), b"{ pas du json").unwrap();
    assert!(needs_old_pack(&e.instances, &inst.id).unwrap());
    assert!(update(&e, &inst.id, None, &v2).await.is_err());
}

#[tokio::test]
async fn record_follows_the_update_and_the_undo() {
    let e = env().await;
    let (v1, v2) = v1v2(&e);
    let (inst, g) = install(&e, &v1).await;
    let before = std::fs::read(g.join(RECORD_FILE)).unwrap();
    let r = update(&e, &inst.id, None, &v2).await.unwrap();
    assert_eq!(digests(&g), expected(&v2_files()));
    assert_eq!(record_json(&g)["origin"]["version_id"], "v2");
    undo(&e.instances, &inst.id, &r.backup_id, &e.idx).unwrap();
    assert_eq!(std::fs::read(g.join(RECORD_FILE)).unwrap(), before);
    assert_eq!(digests(&g), expected(&v1_files()));
}

#[tokio::test]
async fn undo_of_a_backup_without_record_copy_removes_the_record() {
    let e = env().await;
    let (v1, v2) = v1v2(&e);
    let (inst, g) = install(&e, &v1).await;
    let r = update(&e, &inst.id, None, &v2).await.unwrap();
    std::fs::remove_file(g.join(BACKUP_DIR).join(&r.backup_id).join("record.json")).unwrap();
    undo(&e.instances, &inst.id, &r.backup_id, &e.idx).unwrap();
    assert!(!g.join(RECORD_FILE).exists());
}

#[tokio::test]
async fn failed_update_leaves_the_record_untouched() {
    let e = env().await;
    let (v1, _) = v1v2(&e);
    let (inst, g) = install(&e, &v1).await;
    let before = std::fs::read(g.join(RECORD_FILE)).unwrap();
    // Contenu inconnu du faux serveur : le téléchargement échoue avant toute modification.
    let v2 = pack(&e, "v2", "v2", &[("mods/z.jar", b"inconnu du serveur")], &[]);
    assert!(update(&e, &inst.id, None, &v2).await.is_err());
    assert_eq!(std::fs::read(g.join(RECORD_FILE)).unwrap(), before);
    assert_eq!(e.instances.get(&inst.id).unwrap().pack, Some(origin("v1")));
}

#[tokio::test]
async fn disabled_mod_from_the_record_stays_disabled_in_its_new_version() {
    let e = env().await;
    let v1 = pack(&e, "v1", "v1", &[("mods/a-1.jar", A1), ("mods/b.jar", B1)], &[]);
    let v2 = pack(&e, "v2", "v2", &[("mods/a-2.jar", A2), ("mods/b.jar", B1)], &[]);
    let (inst, g) = install(&e, &v1).await;
    // La provenance vient du relevé (écrite à la main : le faux serveur n'est pas le CDN Modrinth).
    let mut rec = record_json(&g);
    rec["files"]["mods/a-1.jar"]["provenance"] = serde_json::json!({"source": "modrinth", "id": "PA"});
    write_record(&g, &rec);
    std::fs::rename(g.join("mods/a-1.jar"), g.join("mods/a-1.jar.disabled")).unwrap();
    let inst = e.instances.get(&inst.id).unwrap();
    let recorded = record::old_state(&g, &inst).unwrap();
    let mut p = prepare_modrinth(&inst, Some(recorded), None, &v2, &origin("v2"), &opts()).unwrap();
    p.new_state.items.get_mut("mods/a-2.jar").unwrap().provenance = Some(prov("PA"));
    let i = p.new_idx.files.iter().position(|f| f.path == "mods/a-2.jar").unwrap();
    p.new_idx.files[i].provenance = Some(prov("PA"));
    let r = apply_prepared(&e.instances, &inst, p, origin("v2"), &e.store, &e.idx, &opts(), &None, &not_cancelled()).await.unwrap();
    assert_eq!((r.added, r.removed), (1, 1));
    assert!(g.join("mods/a-2.jar.disabled").is_file() && !g.join("mods/a-2.jar").exists());
    assert!(!g.join("mods/a-1.jar.disabled").exists());
}
