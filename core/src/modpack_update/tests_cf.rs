//! Tests de la mise à jour d'un modpack CurseForge : détection, application, garde anti-doublon, refus.

use super::*;
use std::io::Write;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use zip::write::SimpleFileOptions;

use crate::browse::Providers;
use crate::providers::curseforge_api::CfApi;
use crate::providers::modrinth_api::ModrinthClient;

const A1: &[u8] = b"mod a version 1";
const A2: &[u8] = b"mod a version 2";
const B1: &[u8] = b"mod b";
const D1: &[u8] = b"mod d retire";
const X1: &[u8] = b"mod x sans empreinte";
const CFG1: &[u8] = b"volume=1\n";
const CFG2: &[u8] = b"volume=2\n";
const PROJECT: &str = "100";

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
    Env { instances: InstanceStore::new(tmp.path().join("instances")), tmp, store, idx, server: MockServer::start().await }
}

fn opts() -> ImportOptions {
    ImportOptions {
        allowed_hosts: vec!["127.0.0.1".into()],
        allow_http: true,
        dl: DlOptions { retry_delay: std::time::Duration::from_millis(5), attempts: 1, ..Default::default() },
        ..Default::default()
    }
}

fn resolver(e: &Env) -> CfResolver {
    CfResolver::with_base_url(e.server.uri()).with_api(e.server.uri(), "k")
}

fn origin(v: &str) -> PackOrigin {
    PackOrigin { source: "curseforge".into(), project_id: PROJECT.into(), version_id: v.into() }
}

/// Fichier de mod CurseForge tel que l'API le décrit.
struct CfMod {
    id: u64,
    name: &'static str,
    content: &'static [u8],
    /// `false` : pas d'URL de distribution (mod non redistribuable).
    has_url: bool,
    /// `false` : l'API ne donne pas de SHA-1.
    has_sha1: bool,
}

fn cf_mod(id: u64, name: &'static str, content: &'static [u8]) -> CfMod {
    CfMod { id, name, content, has_url: true, has_sha1: true }
}

/// Sert l'API de résolution groupée et le CDN pour `mods`.
async fn mount_mods(e: &Env, mods: &[&CfMod]) {
    let data: Vec<serde_json::Value> = mods
        .iter()
        .map(|m| {
            let url = m.has_url.then(|| format!("{}/files/{}/{}", e.server.uri(), m.id, m.name));
            let hashes = if m.has_sha1 { vec![serde_json::json!({"value": crate::unknown_mods::sha1_hex(m.content), "algo": 1})] } else { vec![] };
            serde_json::json!({"id": m.id, "fileName": m.name, "downloadUrl": url, "fileLength": m.content.len(), "hashes": hashes})
        })
        .collect();
    Mock::given(method("POST")).and(path("/v1/mods/files")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "data": data }))).mount(&e.server).await;
    for m in mods.iter().filter(|m| m.has_url) {
        Mock::given(method("GET")).and(path(format!("/files/{}/{}", m.id, m.name))).respond_with(ResponseTemplate::new(200).set_body_bytes(m.content.to_vec())).mount(&e.server).await;
    }
}

/// Zip CurseForge : `manifest.json` (Minecraft `mc`, Fabric 0.16.5) et overrides.
fn cf_zip(e: &Env, name: &str, mc: &str, mods: &[&CfMod], overrides: &[(&str, &[u8])]) -> PathBuf {
    let files: Vec<serde_json::Value> = mods.iter().map(|m| serde_json::json!({"projectID": 500 + m.id, "fileID": m.id, "required": true})).collect();
    let manifest = serde_json::to_vec(&serde_json::json!({
        "minecraft": {"version": mc, "modLoaders": [{"id": "fabric-0.16.5", "primary": true}]},
        "manifestType": "minecraftModpack", "name": "CF Pack", "overrides": "overrides", "files": files
    }))
    .unwrap();
    let p = e.tmp.path().join(format!("{name}.zip"));
    let mut w = zip::ZipWriter::new(std::fs::File::create(&p).unwrap());
    w.start_file("manifest.json", SimpleFileOptions::default()).unwrap();
    w.write_all(&manifest).unwrap();
    for (n, c) in overrides {
        w.start_file(*n, SimpleFileOptions::default()).unwrap();
        w.write_all(c).unwrap();
    }
    w.finish().unwrap();
    p
}

/// Installe `v1` comme « Parcourir » : import CurseForge, puis origine enregistrée.
async fn install(e: &Env, v1: &Path) -> (Instance, PathBuf) {
    let r = mrpack::import_curseforge(v1, &resolver(e), &e.instances, &e.store, &e.idx, &opts(), None, Arc::new(AtomicBool::new(false))).await.unwrap();
    let inst = e.instances.set_pack(&r.instance.id, Some(origin("1"))).unwrap();
    let g = e.instances.game_dir(&inst.id).unwrap();
    (inst, g)
}

async fn update_with(e: &Env, cf: &CfResolver, id: &str, old: Option<&Path>, new: &Path) -> Result<ModpackUpdateResult> {
    apply(&e.instances, id, old, new, origin("2"), cf, &e.store, &e.idx, &opts(), None, Arc::new(AtomicBool::new(false))).await
}

async fn update(e: &Env, id: &str, v1: &Path, v2: &Path) -> Result<ModpackUpdateResult> {
    update_with(e, &resolver(e), id, Some(v1), v2).await
}


fn err_text(r: Result<ModpackUpdateResult>) -> String {
    r.expect_err("la mise à jour aurait dû être refusée").to_string()
}

// ---------- Application ----------

#[tokio::test]
async fn update_replaces_adds_and_removes_and_keeps_player_files() {
    let e = env().await;
    let (a1, a2, b, d) = (cf_mod(1, "a-1.jar", A1), cf_mod(2, "a-2.jar", A2), cf_mod(3, "b.jar", B1), cf_mod(4, "d.jar", D1));
    mount_mods(&e, &[&a1, &a2, &b, &d]).await;
    let v1 = cf_zip(&e, "v1", "1.21.1", &[&a1, &d], &[("overrides/config/x.toml", CFG1)]);
    let v2 = cf_zip(&e, "v2", "1.21.1", &[&a2, &b], &[("overrides/config/x.toml", CFG2)]);
    let (inst, g) = install(&e, &v1).await;
    std::fs::write(g.join("mods/perso.jar"), b"du joueur").unwrap();

    let r = update(&e, &inst.id, &v1, &v2).await.unwrap();

    assert_eq!((r.added, r.removed, r.replaced), (2, 2, 1));
    assert!(r.missing.is_empty(), "{:?}", r.missing);
    assert!(!g.join("mods/a-1.jar").exists(), "le jar renommé ne doit pas rester en double");
    assert_eq!(std::fs::read(g.join("mods/a-2.jar")).unwrap(), A2);
    assert_eq!(std::fs::read(g.join("mods/b.jar")).unwrap(), B1);
    assert!(!g.join("mods/d.jar").exists());
    assert_eq!(std::fs::read(g.join("mods/perso.jar")).unwrap(), b"du joueur");
    assert_eq!(std::fs::read(g.join("config/x.toml")).unwrap(), CFG2);
    assert_eq!(e.instances.get(&inst.id).unwrap().pack, Some(origin("2")));
}

#[tokio::test]
async fn update_keeps_a_config_the_player_modified() {
    let e = env().await;
    let a1 = cf_mod(1, "a-1.jar", A1);
    mount_mods(&e, &[&a1]).await;
    let v1 = cf_zip(&e, "v1", "1.21.1", &[&a1], &[("overrides/config/x.toml", CFG1)]);
    let v2 = cf_zip(&e, "v2", "1.21.1", &[&a1], &[("overrides/config/x.toml", CFG2)]);
    let (inst, g) = install(&e, &v1).await;
    std::fs::write(g.join("config/x.toml"), b"volume=11\n").unwrap();

    let r = update(&e, &inst.id, &v1, &v2).await.unwrap();

    assert_eq!(r.kept, vec!["config/x.toml".to_string()]);
    assert_eq!(std::fs::read(g.join("config/x.toml")).unwrap(), b"volume=11\n");
}

#[tokio::test]
async fn non_redistributable_mod_is_reported_missing_and_the_rest_is_applied() {
    let e = env().await;
    let (a1, a2) = (cf_mod(1, "a-1.jar", A1), cf_mod(2, "a-2.jar", A2));
    let restricted = CfMod { has_url: false, ..cf_mod(5, "restreint.jar", b"interdit") };
    mount_mods(&e, &[&a1, &a2, &restricted]).await;
    let v1 = cf_zip(&e, "v1", "1.21.1", &[&a1], &[]);
    let v2 = cf_zip(&e, "v2", "1.21.1", &[&a2, &restricted], &[]);
    let (inst, g) = install(&e, &v1).await;

    let r = update(&e, &inst.id, &v1, &v2).await.unwrap();

    assert_eq!(r.missing, vec!["mods/restreint.jar".to_string()]);
    assert_eq!(std::fs::read(g.join("mods/a-2.jar")).unwrap(), A2);
    assert!(!g.join("mods/restreint.jar").exists());
    assert_eq!(e.instances.get(&inst.id).unwrap().pack, Some(origin("2")));
}

#[tokio::test]
async fn unknown_fingerprint_jar_on_disk_that_the_new_pack_drops_blocks_the_update() {
    let e = env().await;
    let (a1, a2) = (cf_mod(1, "a-1.jar", A1), cf_mod(2, "a-2.jar", A2));
    let x = CfMod { has_url: false, has_sha1: false, ..cf_mod(6, "x.jar", X1) };
    mount_mods(&e, &[&a1, &a2, &x]).await;
    let v1 = cf_zip(&e, "v1", "1.21.1", &[&a1, &x], &[]);
    let v2 = cf_zip(&e, "v2", "1.21.1", &[&a2], &[]);
    let (inst, g) = install(&e, &v1).await;
    std::fs::write(g.join("mods/x.jar"), X1).unwrap();
    let before = std::fs::read_dir(g.join("mods")).unwrap().count();
    // Par `apply` lui-même : la garde est branchée avant tout téléchargement.
    let msg = update(&e, &inst.id, &v1, &v2).await.expect_err("doublon à refuser").to_string();

    assert!(msg.contains("mods/x.jar"), "{msg}");
    assert!(!g.join("mods/a-2.jar").exists());
    assert_eq!(std::fs::read_dir(g.join("mods")).unwrap().count(), before);

    // Le fichier n'est plus sur le disque : plus rien ne bloque.
    std::fs::remove_file(g.join("mods/x.jar")).unwrap();
    guard(&e, &inst, &g, &v1, &v2).await.unwrap();
    update(&e, &inst.id, &v1, &v2).await.unwrap();
    assert_eq!(std::fs::read(g.join("mods/a-2.jar")).unwrap(), A2);
}

/// Ce que `apply` fait avant de planifier : préparation CurseForge puis garde anti-doublon.
async fn guard(e: &Env, inst: &Instance, game_dir: &Path, old: &Path, new: &Path) -> Result<()> {
    let prepared = curseforge::prepare(inst, None, Some(old), new, &origin("2"), &resolver(e), &opts()).await?;
    curseforge::refuse_duplicates(&prepared, game_dir)
}

#[tokio::test]
async fn unknown_fingerprint_jar_still_in_the_new_pack_does_not_block() {
    let e = env().await;
    let (a1, a2) = (cf_mod(1, "a-1.jar", A1), cf_mod(2, "a-2.jar", A2));
    let x = CfMod { has_url: false, has_sha1: false, ..cf_mod(6, "x.jar", X1) };
    mount_mods(&e, &[&a1, &a2, &x]).await;
    let v1 = cf_zip(&e, "v1", "1.21.1", &[&a1, &x], &[]);
    let v2 = cf_zip(&e, "v2", "1.21.1", &[&a2, &x], &[]);
    let (inst, g) = install(&e, &v1).await;
    std::fs::write(g.join("mods/x.jar"), X1).unwrap();

    guard(&e, &inst, &g, &v1, &v2).await.unwrap();
    update(&e, &inst.id, &v1, &v2).await.unwrap();

    assert_eq!(std::fs::read(g.join("mods/x.jar")).unwrap(), X1);
    assert_eq!(std::fs::read(g.join("mods/a-2.jar")).unwrap(), A2);
}

#[tokio::test]
async fn update_without_key_nor_record_asks_to_reinstall() {
    let e = env().await;
    let a1 = cf_mod(1, "a-1.jar", A1);
    mount_mods(&e, &[&a1]).await;
    let v1 = cf_zip(&e, "v1", "1.21.1", &[&a1], &[]);
    let v2 = cf_zip(&e, "v2", "1.21.1", &[&a1], &[("overrides/config/x.toml", CFG2)]);
    let (inst, _) = install(&e, &v1).await;
    let no_key = CfResolver::with_base_url(e.server.uri());

    let msg = err_text(update_with(&e, &no_key, &inst.id, Some(&v1), &v2).await);

    assert!(msg.contains("réinstalle"), "{msg}");
}

#[tokio::test]
async fn update_refuses_a_new_minecraft_version() {
    let e = env().await;
    let a1 = cf_mod(1, "a-1.jar", A1);
    mount_mods(&e, &[&a1]).await;
    let v1 = cf_zip(&e, "v1", "1.21.1", &[&a1], &[]);
    let v2 = cf_zip(&e, "v2", "1.20.1", &[&a1], &[]);
    let (inst, _) = install(&e, &v1).await;

    let msg = err_text(update(&e, &inst.id, &v1, &v2).await);

    assert!(msg.contains("Minecraft"), "{msg}");
}

#[tokio::test]
async fn recorded_state_is_used_without_reading_the_old_pack() {
    let e = env().await;
    let (a1, a2) = (cf_mod(1, "a-1.jar", A1), cf_mod(2, "a-2.jar", A2));
    mount_mods(&e, &[&a1, &a2]).await;
    let v1 = cf_zip(&e, "v1", "1.21.1", &[&a1], &[]);
    let v2 = cf_zip(&e, "v2", "1.21.1", &[&a2], &[]);
    let (inst, _) = install(&e, &v1).await;
    let batch_calls = || async { e.server.received_requests().await.unwrap().iter().filter(|r| r.url.path() == "/v1/mods/files").count() };
    let before = batch_calls().await;
    let recorded = State {
        items: BTreeMap::from([(
            "mods/a-1.jar".to_string(),
            Item { path: "mods/a-1.jar".into(), digest: Digest { alg: Alg::Sha1, hex: crate::unknown_mods::sha1_hex(A1) }, source: Source::Index(0), provenance: None },
        )]),
    };

    let p = curseforge::prepare(&inst, Some(recorded), Some(&v1), &v2, &origin("2"), &resolver(&e), &opts()).await.unwrap();

    assert_eq!(p.old_state.items.len(), 1);
    assert_eq!(batch_calls().await - before, 1, "seul le nouveau pack doit être résolu");
    assert!(p.opts.is_some(), "les CDN CurseForge doivent être autorisés");
}

// ---------- Détection ----------

fn cf_file(id: u64, date: &str, release_type: u32, server_pack: bool, mc: &str) -> serde_json::Value {
    serde_json::json!({"id": id, "modId": 100, "displayName": format!("Pack {id}"), "fileName": format!("pack-{id}.zip"), "fileDate": date,
        "downloadUrl": format!("https://edge.forgecdn.net/files/0/{id}/pack-{id}.zip"), "releaseType": release_type,
        "isServerPack": server_pack, "gameVersions": [mc, "Fabric"]})
}

async fn mount_files(s: &MockServer, files: Vec<serde_json::Value>) {
    for f in &files {
        Mock::given(method("GET")).and(path(format!("/v1/mods/100/files/{}", f["id"]))).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "data": f }))).mount(s).await;
    }
    Mock::given(method("GET")).and(path("/v1/mods/100/files")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "data": files }))).mount(s).await;
}

fn cf_providers(s: &MockServer, key: &str) -> Providers {
    Providers { modrinth: ModrinthClient::with_base_url("http://127.0.0.1:1/v2", "test"), cf: CfApi::with_base(s.uri(), key), allow_loopback: true }
}

fn installed(file_id: &str) -> Instance {
    Instance {
        id: "i1".into(),
        name: "CF Pack".into(),
        minecraft: "1.21.1".into(),
        loader: LoaderSpec { loader: Loader::Fabric, version: "0.16.5".into() },
        ram_mb: 0,
        mod_count: 0,
        java_path: None,
        jvm_args: String::new(),
        wrapper: String::new(),
        art: None,
        pack: Some(origin(file_id)),
    }
}

#[tokio::test]
async fn check_proposes_the_newest_compatible_non_server_release() {
    let s = MockServer::start().await;
    mount_files(
        &s,
        vec![
            cf_file(1, "2024-01-01T00:00:00Z", 1, false, "1.21.1"),
            cf_file(2, "2024-03-01T00:00:00Z", 1, false, "1.21.1"),
            cf_file(3, "2024-04-01T00:00:00Z", 2, false, "1.21.1"),
            cf_file(4, "2024-05-01T00:00:00Z", 1, true, "1.21.1"),
            cf_file(5, "2024-06-01T00:00:00Z", 1, false, "1.20.1"),
        ],
    )
    .await;

    let u = check(&cf_providers(&s, "k"), &installed("1")).await.unwrap().unwrap();

    assert_eq!(u.version_id, "2");
    assert_eq!(u.published, "2024-03-01T00:00:00Z");
}

#[tokio::test]
async fn check_proposes_a_beta_only_when_the_installed_one_is_a_beta() {
    let s = MockServer::start().await;
    mount_files(&s, vec![cf_file(1, "2024-01-01T00:00:00Z", 2, false, "1.21.1"), cf_file(3, "2024-04-01T00:00:00Z", 3, false, "1.21.1")]).await;

    let u = check(&cf_providers(&s, "k"), &installed("1")).await.unwrap().unwrap();

    assert_eq!(u.version_id, "3");
}

#[tokio::test]
async fn check_is_none_when_up_to_date_or_the_newest_is_a_server_pack() {
    let s = MockServer::start().await;
    mount_files(&s, vec![cf_file(1, "2024-01-01T00:00:00Z", 1, false, "1.21.1"), cf_file(3, "2024-04-01T00:00:00Z", 2, false, "1.21.1"), cf_file(4, "2024-05-01T00:00:00Z", 1, true, "1.21.1")]).await;
    let p = cf_providers(&s, "k");

    assert_eq!(check(&p, &installed("1")).await.unwrap(), None);
    assert_eq!(check(&p, &installed("3")).await.unwrap(), None, "jamais plus ancienne que l'installée");
}

#[tokio::test]
async fn check_is_none_without_api_key_and_never_calls_the_api() {
    let s = MockServer::start().await;
    mount_files(&s, vec![cf_file(1, "2024-01-01T00:00:00Z", 1, false, "1.21.1"), cf_file(2, "2024-03-01T00:00:00Z", 1, false, "1.21.1")]).await;

    assert_eq!(check(&cf_providers(&s, ""), &installed("1")).await.unwrap(), None);
    assert!(s.received_requests().await.unwrap().is_empty());
}
