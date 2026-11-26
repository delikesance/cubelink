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
fn pack(e: &Env, name: &str, version: &str, loader_version: &str, files: &[(&str, &[u8])], overrides: &[(&str, &[u8])]) -> PathBuf {
    let files: Vec<serde_json::Value> = files
        .iter()
        .map(|(p, c)| {
            serde_json::json!({"path": p, "hashes": {"sha512": sha512(c), "sha1": crate::unknown_mods::sha1_hex(c)},
                "downloads": [format!("{}/{}", e.server.uri(), &sha512(c)[..16])], "fileSize": c.len()})
        })
        .collect();
    let index = serde_json::to_vec(&serde_json::json!({
        "formatVersion": 1, "game": "minecraft", "versionId": version, "name": "Mon Pack", "files": files,
        "dependencies": {"minecraft": "1.21.1", "fabric-loader": loader_version}
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

/// Installe `v1` comme le fait « Parcourir » : import, puis origine enregistrée.
async fn install(e: &Env, v1: &Path) -> (Instance, PathBuf) {
    let inst = mrpack::import(v1, &e.instances, &e.store, &e.idx, &opts(), None, Arc::new(AtomicBool::new(false))).await.unwrap();
    let inst = e.instances.set_pack(&inst.id, Some(origin("v1"))).unwrap();
    let g = e.instances.game_dir(&inst.id).unwrap();
    (inst, g)
}

async fn update(e: &Env, id: &str, v1: &Path, v2: &Path) -> Result<ModpackUpdateResult> {
    apply(&e.instances, id, Some(v1), v2, origin("v2"), &CfResolver::new(), &e.store, &e.idx, &opts(), None, Arc::new(AtomicBool::new(false))).await
}

/// Photographie du dossier de jeu : chemin -> contenu, sans le dossier de sauvegarde.
fn snapshot(g: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for en in std::fs::read_dir(dir).unwrap().flatten() {
            let p = en.path();
            let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            if rel == BACKUP_DIR {
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

fn refs(e: &Env) -> BTreeMap<String, u32> {
    e.idx.all().unwrap().into_iter().map(|x| (x.sha256, x.refs)).collect()
}

/// v1 : a1, b, d + config ; v2 : a2 (remplace a1), b (inchangé), c (ajouté), d retiré + config changée.
fn v1v2(e: &Env) -> (PathBuf, PathBuf) {
    let v1 = pack(e, "v1", "v1", "0.16.5", &[("mods/a.jar", A1), ("mods/b.jar", B1), ("mods/d.jar", D1)], &[("overrides/config/x.toml", CFG1), ("overrides/config/y.toml", b"y=1")]);
    let v2 = pack(e, "v2", "v2", "0.16.5", &[("mods/a.jar", A2), ("mods/b.jar", B1), ("mods/c.jar", C1)], &[("overrides/config/x.toml", CFG2), ("overrides/config/y.toml", b"y=1")]);
    (v1, v2)
}

#[tokio::test]
async fn update_adds_replaces_removes_and_keeps_player_files() {
    let e = env().await;
    let (v1, v2) = v1v2(&e);
    let (inst, g) = install(&e, &v1).await;
    // Le joueur : un mod à lui, des mondes, ses options, un fichier de config modifié (y.toml reste celui du pack).
    std::fs::create_dir_all(g.join("saves/Monde")).unwrap();
    std::fs::write(g.join("saves/Monde/level.dat"), b"monde").unwrap();
    std::fs::write(g.join("options.txt"), b"fov:90").unwrap();
    std::fs::write(g.join("mods/perso.jar"), b"mod du joueur").unwrap();

    let r = update(&e, &inst.id, &v1, &v2).await.unwrap();
    assert_eq!((r.added, r.removed, r.replaced), (1, 1, 2), "{r:?}"); // c ajouté ; d retiré ; a et x.toml remplacés
    assert!(r.kept.is_empty(), "{:?}", r.kept);
    assert_eq!(std::fs::read(g.join("mods/a.jar")).unwrap(), A2);
    assert_eq!(std::fs::read(g.join("mods/b.jar")).unwrap(), B1);
    assert_eq!(std::fs::read(g.join("mods/c.jar")).unwrap(), C1);
    assert!(!g.join("mods/d.jar").exists());
    assert_eq!(std::fs::read(g.join("config/x.toml")).unwrap(), CFG2);
    assert_eq!(std::fs::read(g.join("mods/perso.jar")).unwrap(), b"mod du joueur");
    assert_eq!(std::fs::read(g.join("saves/Monde/level.dat")).unwrap(), b"monde");
    assert_eq!(std::fs::read(g.join("options.txt")).unwrap(), b"fov:90");
    // L'ancien état est sauvegardé.
    let b = g.join(BACKUP_DIR).join(&r.backup_id);
    assert_eq!(std::fs::read(b.join("files/mods/a.jar")).unwrap(), A1);
    assert_eq!(std::fs::read(b.join("files/mods/d.jar")).unwrap(), D1);
    assert_eq!(std::fs::read(b.join("files/config/x.toml")).unwrap(), CFG1);
    assert_eq!(e.instances.get(&inst.id).unwrap().pack, Some(origin("v2")));
}

#[tokio::test]
async fn player_modified_config_is_kept_and_reported() {
    let e = env().await;
    let (v1, v2) = v1v2(&e);
    let (inst, g) = install(&e, &v1).await;
    std::fs::write(g.join("config/x.toml"), b"volume=11 (le joueur)\n").unwrap();
    // y.toml : supprimé par le joueur, inchangé dans le pack : on ne le recrée pas.
    std::fs::remove_file(g.join("config/y.toml")).unwrap();
    let r = update(&e, &inst.id, &v1, &v2).await.unwrap();
    assert_eq!(r.kept, vec!["config/x.toml".to_string()]);
    assert_eq!(std::fs::read(g.join("config/x.toml")).unwrap(), b"volume=11 (le joueur)\n");
    assert!(!g.join("config/y.toml").exists());
    // Le mod du pack, lui, est bien mis à jour.
    assert_eq!(std::fs::read(g.join("mods/a.jar")).unwrap(), A2);
}

#[tokio::test]
async fn player_modified_pack_mod_is_not_replaced_or_removed() {
    let e = env().await;
    let (v1, v2) = v1v2(&e);
    let (inst, g) = install(&e, &v1).await;
    // Fichier du pack modifié par le joueur : remplacement et retrait refusés, tout le reste avance.
    std::fs::write(g.join("mods/a.jar"), b"patche par le joueur").unwrap();
    std::fs::write(g.join("mods/d.jar"), b"d aussi").unwrap();
    let r = update(&e, &inst.id, &v1, &v2).await.unwrap();
    assert_eq!(r.kept, vec!["mods/a.jar".to_string(), "mods/d.jar".to_string()]);
    assert_eq!(std::fs::read(g.join("mods/a.jar")).unwrap(), b"patche par le joueur");
    assert_eq!(std::fs::read(g.join("mods/d.jar")).unwrap(), b"d aussi");
    assert_eq!(std::fs::read(g.join("mods/c.jar")).unwrap(), C1);
}

#[tokio::test]
async fn undo_restores_the_exact_tree_pack_origin_and_store_refs() {
    let e = env().await;
    let (v1, v2) = v1v2(&e);
    let (inst, g) = install(&e, &v1).await;
    std::fs::write(g.join("mods/perso.jar"), b"mod du joueur").unwrap();
    std::fs::write(g.join("options.txt"), b"fov:90").unwrap();
    let (before, refs_before) = (snapshot(&g), refs(&e));
    let r = update(&e, &inst.id, &v1, &v2).await.unwrap();
    assert_ne!(snapshot(&g), before);
    let back = undo(&e.instances, &inst.id, &r.backup_id, &e.idx).unwrap();
    assert_eq!(snapshot(&g), before, "l'arbre doit être identique octet pour octet");
    assert_eq!(back.pack, Some(origin("v1")));
    assert!(!g.join(BACKUP_DIR).join(&r.backup_id).exists());
    // Références du magasin : les objets de v2 retombent à leur compte d'avant (zéro ou absents), ceux de v1 reviennent.
    let after = refs(&e);
    for (sha, n) in &refs_before {
        assert_eq!(after.get(sha), Some(n), "références de {sha}");
    }
    for (sha, n) in &after {
        if !refs_before.contains_key(sha) {
            assert_eq!(*n, 0, "objet de la nouvelle version encore référencé : {sha}");
        }
    }
    // Une deuxième annulation ne trouve plus rien.
    assert!(matches!(undo(&e.instances, &inst.id, &r.backup_id, &e.idx), Err(Error::NotFound(_))));
}

#[tokio::test]
async fn undo_keeps_files_the_player_changed_since_the_update() {
    let e = env().await;
    let (v1, v2) = v1v2(&e);
    let (inst, g) = install(&e, &v1).await;
    let r = update(&e, &inst.id, &v1, &v2).await.unwrap();
    std::fs::write(g.join("config/x.toml"), b"volume=5 (apres la mise a jour)\n").unwrap();
    undo(&e.instances, &inst.id, &r.backup_id, &e.idx).unwrap();
    assert_eq!(std::fs::read(g.join("config/x.toml")).unwrap(), b"volume=5 (apres la mise a jour)\n");
    // Le reste est revenu.
    assert_eq!(std::fs::read(g.join("mods/a.jar")).unwrap(), A1);
    assert_eq!(std::fs::read(g.join("mods/d.jar")).unwrap(), D1);
    assert!(!g.join("mods/c.jar").exists());
}

#[tokio::test]
async fn undo_refuses_after_a_newer_update_and_bad_ids() {
    let e = env().await;
    let (v1, v2) = v1v2(&e);
    let (inst, _g) = install(&e, &v1).await;
    let r = update(&e, &inst.id, &v1, &v2).await.unwrap();
    e.instances.set_pack(&inst.id, Some(origin("v3"))).unwrap();
    assert!(matches!(undo(&e.instances, &inst.id, &r.backup_id, &e.idx), Err(Error::Invalid(_))));
    for bad in ["../x", "", "a/b", "a b"] {
        assert!(matches!(undo(&e.instances, &inst.id, bad, &e.idx), Err(Error::Invalid(_))), "{bad:?}");
    }
}

#[tokio::test]
async fn failed_download_leaves_the_instance_untouched() {
    let e = env().await;
    let (v1, _) = v1v2(&e);
    // v2 demande un mod que le faux serveur ne sert pas (404) : rien ne doit bouger.
    let v2 = pack(&e, "v2bad", "v2", "0.16.5", &[("mods/a.jar", A2), ("mods/b.jar", B1), ("mods/zzz.jar", b"introuvable")], &[("overrides/config/x.toml", CFG2)]);
    let (inst, g) = install(&e, &v1).await;
    let before = snapshot(&g);
    assert!(update(&e, &inst.id, &v1, &v2).await.is_err());
    assert_eq!(snapshot(&g), before);
    assert!(!g.join(BACKUP_DIR).exists() || std::fs::read_dir(g.join(BACKUP_DIR)).unwrap().count() == 0);
    assert_eq!(e.instances.get(&inst.id).unwrap().pack, Some(origin("v1")));
}

#[tokio::test]
async fn saves_and_options_in_the_new_overrides_are_never_written() {
    let e = env().await;
    let v1 = pack(&e, "v1", "v1", "0.16.5", &[("mods/a.jar", A1)], &[]);
    let v2 = pack(
        &e,
        "v2",
        "v2",
        "0.16.5",
        &[("mods/a.jar", A1)],
        &[("overrides/saves/Monde/level.dat", b"pack"), ("overrides/options.txt", b"fov:30"), ("overrides/servers.dat", b"s"), ("overrides/config/ok.toml", b"ok=1"), ("client-overrides/logs/latest.log", b"l")],
    );
    let (inst, g) = install(&e, &v1).await;
    std::fs::write(g.join("options.txt"), b"fov:90").unwrap();
    let r = update(&e, &inst.id, &v1, &v2).await.unwrap();
    assert_eq!(r.added, 1);
    assert_eq!(std::fs::read(g.join("options.txt")).unwrap(), b"fov:90");
    assert!(!g.join("saves").exists() && !g.join("servers.dat").exists() && !g.join("logs").exists());
    assert_eq!(std::fs::read(g.join("config/ok.toml")).unwrap(), b"ok=1");
}

#[tokio::test]
async fn player_file_at_a_new_path_wins() {
    let e = env().await;
    let v1 = pack(&e, "v1", "v1", "0.16.5", &[("mods/a.jar", A1)], &[]);
    let v2 = pack(&e, "v2", "v2", "0.16.5", &[("mods/a.jar", A1), ("mods/c.jar", C1)], &[("overrides/config/n.toml", b"pack")]);
    let (inst, g) = install(&e, &v1).await;
    std::fs::write(g.join("mods/c.jar"), b"c du joueur").unwrap();
    std::fs::create_dir_all(g.join("config")).unwrap();
    std::fs::write(g.join("config/n.toml"), b"n du joueur").unwrap();
    let r = update(&e, &inst.id, &v1, &v2).await.unwrap();
    assert_eq!((r.added, r.kept.clone()), (0, vec!["config/n.toml".to_string(), "mods/c.jar".to_string()]));
    assert_eq!(std::fs::read(g.join("mods/c.jar")).unwrap(), b"c du joueur");
    assert_eq!(std::fs::read(g.join("config/n.toml")).unwrap(), b"n du joueur");
}

#[tokio::test]
async fn refuses_minecraft_or_loader_kind_change_and_foreign_project() {
    let e = env().await;
    let (v1, v2) = v1v2(&e);
    let (inst, g) = install(&e, &v1).await;
    let before = snapshot(&g);
    // Autre projet.
    let other = PackOrigin { source: "modrinth".into(), project_id: "AUTRE".into(), version_id: "v2".into() };
    let r = apply(&e.instances, &inst.id, Some(&v1), &v2, other, &CfResolver::new(), &e.store, &e.idx, &opts(), None, Arc::new(AtomicBool::new(false))).await;
    assert!(matches!(r, Err(Error::Invalid(_))));
    // Autre loader (neoforge) : refus.
    let neo = {
        let idx = serde_json::to_vec(&serde_json::json!({"formatVersion": 1, "versionId": "n", "name": "P", "files": [], "dependencies": {"minecraft": "1.21.1", "neoforge": "21.1.1"}})).unwrap();
        let p = e.tmp.path().join("neo.mrpack");
        let mut w = zip::ZipWriter::new(std::fs::File::create(&p).unwrap());
        w.start_file("modrinth.index.json", SimpleFileOptions::default()).unwrap();
        w.write_all(&idx).unwrap();
        w.finish().unwrap();
        p
    };
    let r = update(&e, &inst.id, &v1, &neo).await;
    assert!(matches!(&r, Err(Error::Invalid(m)) if m.contains("loader")), "{r:?}");
    assert_eq!(snapshot(&g), before);
    // Instance sans origine : refus.
    e.instances.set_pack(&inst.id, None).unwrap();
    assert!(matches!(update(&e, &inst.id, &v1, &v2).await, Err(Error::Invalid(_))));
}

#[tokio::test]
async fn loader_version_bump_is_applied_and_undone() {
    let e = env().await;
    let v1 = pack(&e, "v1", "v1", "0.16.5", &[("mods/a.jar", A1)], &[]);
    let v2 = pack(&e, "v2", "v2", "0.16.9", &[("mods/a.jar", A2)], &[]);
    let (inst, _g) = install(&e, &v1).await;
    let r = update(&e, &inst.id, &v1, &v2).await.unwrap();
    assert_eq!(e.instances.get(&inst.id).unwrap().loader.version, "0.16.9");
    let back = undo(&e.instances, &inst.id, &r.backup_id, &e.idx).unwrap();
    assert_eq!(back.loader.version, "0.16.5");
}

#[tokio::test]
async fn malicious_new_pack_is_refused_before_any_change() {
    let e = env().await;
    let v1 = pack(&e, "v1", "v1", "0.16.5", &[("mods/a.jar", A1)], &[]);
    let (inst, g) = install(&e, &v1).await;
    let before = snapshot(&g);
    for (name, entry) in [("trav", "overrides/../evil.txt"), ("exe", "overrides/mods/x.exe")] {
        let v2 = pack(&e, name, "v2", "0.16.5", &[("mods/a.jar", A2)], &[(entry, b"x")]);
        assert!(update(&e, &inst.id, &v1, &v2).await.is_err(), "{entry}");
        assert_eq!(snapshot(&g), before);
    }
    // Fichier d'index hors des dossiers autorisés.
    let v2 = pack(&e, "bad", "v2", "0.16.5", &[("../evil.jar", A2)], &[]);
    assert!(update(&e, &inst.id, &v1, &v2).await.is_err());
    assert_eq!(snapshot(&g), before);
}

#[tokio::test]
async fn revert_restores_the_old_state_from_the_manifest_alone() {
    // `revert` (utilisé aussi par la remise en état après un échec) remet l'ancien état depuis le seul manifeste.
    let e = env().await;
    let (v1, v2) = v1v2(&e);
    let (inst, g) = install(&e, &v1).await;
    let r = update(&e, &inst.id, &v1, &v2).await.unwrap();
    let bdir = g.join(BACKUP_DIR).join(&r.backup_id);
    // On simule l'état d'un arrêt en plein milieu : un des fichiers sauvegardés est revenu à sa place avant le crash.
    let mf: Manifest = serde_json::from_slice(&std::fs::read(bdir.join("manifest.json")).unwrap()).unwrap();
    let (errors, kept) = revert(&g, &bdir, &mf, &e.idx, true);
    assert!(errors.is_empty() && kept.is_empty(), "{errors:?}");
    assert_eq!(std::fs::read(g.join("mods/a.jar")).unwrap(), A1);
}

fn prov(id: &str) -> Provenance {
    Provenance { source: "modrinth".into(), id: id.into() }
}

#[tokio::test]
async fn provenance_follows_the_update_and_the_undo() {
    let e = env().await;
    let (v1, v2) = v1v2(&e);
    let (inst, g) = install(&e, &v1).await;
    mods::record_provenance(&g, &[("a.jar".into(), prov("PA")), ("d.jar".into(), prov("PD")), ("b.jar".into(), prov("PB"))]).unwrap();
    let before = mods::read_provenance(&g);
    // Les URLs de test ne sont pas celles du CDN : on pose la provenance que l'index en aurait déduite.
    let mut p = prepare(&e.instances.get(&inst.id).unwrap(), &v1, &v2, &origin("v2"), &opts()).unwrap();
    for (k, id) in [("mods/a.jar", "PA"), ("mods/c.jar", "PC")] {
        p.new_state.items.get_mut(k).unwrap().provenance = Some(prov(id));
        let i = p.new_idx.files.iter().position(|f| f.path == k).unwrap();
        p.new_idx.files[i].provenance = Some(prov(id));
    }
    for (k, id) in [("mods/a.jar", "PA"), ("mods/d.jar", "PD")] {
        p.old_state.items.get_mut(k).unwrap().provenance = Some(prov(id));
    }
    let inst = e.instances.get(&inst.id).unwrap();
    let r = apply_prepared(&e.instances, &inst, p, origin("v2"), &e.store, &e.idx, &opts(), &None, &Arc::new(AtomicBool::new(false))).await.unwrap();
    let now = mods::read_provenance(&g);
    assert_eq!(now.get("a.jar"), Some(&prov("PA")));
    assert_eq!(now.get("c.jar"), Some(&prov("PC")));
    assert_eq!(now.get("b.jar"), Some(&prov("PB")));
    assert!(!now.contains_key("d.jar"), "le mod retiré n'a plus de provenance");
    undo(&e.instances, &inst.id, &r.backup_id, &e.idx).unwrap();
    assert_eq!(mods::read_provenance(&g), before);
}

#[tokio::test]
async fn disabled_mod_stays_disabled_in_its_new_version() {
    let e = env().await;
    // v2 renomme le jar (autre contenu, autre nom) : même projet PA.
    let v1 = pack(&e, "v1", "v1", "0.16.5", &[("mods/a-1.jar", A1), ("mods/b.jar", B1)], &[]);
    let v2 = pack(&e, "v2", "v2", "0.16.5", &[("mods/a-2.jar", A2), ("mods/b.jar", B1)], &[]);
    let (inst, g) = install(&e, &v1).await;
    std::fs::rename(g.join("mods/a-1.jar"), g.join("mods/a-1.jar.disabled")).unwrap();
    let mut p = prepare(&e.instances.get(&inst.id).unwrap(), &v1, &v2, &origin("v2"), &opts()).unwrap();
    p.old_state.items.get_mut("mods/a-1.jar").unwrap().provenance = Some(prov("PA"));
    p.new_state.items.get_mut("mods/a-2.jar").unwrap().provenance = Some(prov("PA"));
    let i = p.new_idx.files.iter().position(|f| f.path == "mods/a-2.jar").unwrap();
    p.new_idx.files[i].provenance = Some(prov("PA"));
    let inst = e.instances.get(&inst.id).unwrap();
    let r = apply_prepared(&e.instances, &inst, p, origin("v2"), &e.store, &e.idx, &opts(), &None, &Arc::new(AtomicBool::new(false))).await.unwrap();
    assert_eq!((r.added, r.removed), (1, 1));
    assert!(g.join("mods/a-2.jar.disabled").is_file() && !g.join("mods/a-2.jar").exists());
    assert!(!g.join("mods/a-1.jar.disabled").exists());
    undo(&e.instances, &inst.id, &r.backup_id, &e.idx).unwrap();
    assert!(g.join("mods/a-1.jar.disabled").is_file() && !g.join("mods/a-2.jar.disabled").exists());
}

#[tokio::test]
async fn a_newer_player_version_of_the_same_mod_is_not_duplicated() {
    let e = env().await;
    let v1 = pack(&e, "v1", "v1", "0.16.5", &[("mods/a-1.jar", A1)], &[]);
    let v2 = pack(&e, "v2", "v2", "0.16.5", &[("mods/a-2.jar", A2)], &[]);
    let (inst, g) = install(&e, &v1).await;
    // Le joueur avait déjà remplacé le jar du pack par sa propre version du même projet.
    std::fs::remove_file(g.join("mods/a-1.jar")).unwrap();
    std::fs::write(g.join("mods/a-joueur.jar"), b"version du joueur").unwrap();
    mods::record_provenance(&g, &[("a-joueur.jar".into(), prov("PA"))]).unwrap();
    let mut p = prepare(&e.instances.get(&inst.id).unwrap(), &v1, &v2, &origin("v2"), &opts()).unwrap();
    p.new_state.items.get_mut("mods/a-2.jar").unwrap().provenance = Some(prov("PA"));
    let inst = e.instances.get(&inst.id).unwrap();
    let r = apply_prepared(&e.instances, &inst, p, origin("v2"), &e.store, &e.idx, &opts(), &None, &Arc::new(AtomicBool::new(false))).await.unwrap();
    assert_eq!(r.added, 0);
    assert_eq!(r.kept, vec!["mods/a-joueur.jar".to_string()]);
    assert!(!g.join("mods/a-2.jar").exists());
}

#[test]
fn protected_paths() {
    for p in ["saves/x/level.dat", "screenshots/a.png", "logs/latest.log", "options.txt", "servers.dat", "mods/.cubelink-backup/x/a.jar", ".cubelink-pack-backup/x", "Options.TXT"] {
        assert!(protected(p), "{p}");
    }
    for p in ["config/options.txt", "mods/a.jar", "saves", "resourcepacks/x.zip", "config/saves.toml"] {
        assert!(!protected(p), "{p}");
    }
}

// ---------- Détection ----------

fn providers(mr: &MockServer) -> Providers {
    Providers { modrinth: ModrinthClient::with_base_url(format!("{}/v2", mr.uri()), "test"), cf: CfApi::with_base("http://127.0.0.1:1", ""), allow_loopback: true }
}

fn installed_inst(version: &str, source: &str) -> Instance {
    Instance {
        id: "i1".into(),
        name: "Mon Pack".into(),
        minecraft: "1.21.1".into(),
        loader: LoaderSpec { loader: Loader::Fabric, version: "0.16.5".into() },
        ram_mb: 0,
        mod_count: 0,
        java_path: None,
        jvm_args: String::new(),
        wrapper: String::new(),
        art: None,
        pack: Some(PackOrigin { source: source.into(), project_id: "PROJ".into(), version_id: version.into() }),
    }
}

fn ver(id: &str, number: &str, date: &str, kind: &str) -> serde_json::Value {
    serde_json::json!({"id": id, "project_id": "PROJ", "name": format!("Pack {number}"), "version_number": number, "date_published": date,
        "version_type": kind, "game_versions": ["1.21.1"], "loaders": ["fabric"],
        "files": [{"url": "https://cdn.modrinth.com/data/PROJ/versions/x/p.mrpack", "filename": "p.mrpack", "primary": true, "size": 1, "hashes": {"sha1": "a", "sha512": "b"}}]})
}

async fn serve_versions(mr: &MockServer, list: Vec<serde_json::Value>) {
    Mock::given(method("GET")).and(path("/v2/project/PROJ/version")).respond_with(ResponseTemplate::new(200).set_body_json(list)).mount(mr).await;
}

#[tokio::test]
async fn check_proposes_the_newest_release_only() {
    let mr = MockServer::start().await;
    serve_versions(
        &mr,
        vec![
            ver("v3beta", "1.5.0-beta", "2026-05-01T00:00:00Z", "beta"),
            ver("v3", "1.4.2", "2026-04-01T00:00:00Z", "release"),
            ver("v2", "1.4.1", "2026-03-01T00:00:00Z", "release"),
            ver("v1", "1.4.0", "2026-02-01T00:00:00Z", "release"),
        ],
    )
    .await;
    let p = providers(&mr);
    let u = check(&p, &installed_inst("v1", "modrinth")).await.unwrap().unwrap();
    assert_eq!((u.version_id.as_str(), u.version_number.as_str(), u.name.as_str()), ("v3", "1.4.2", "Pack 1.4.2"));
    // Déjà à jour (la bêta plus récente n'est pas proposée) : rien.
    assert_eq!(check(&p, &installed_inst("v3", "modrinth")).await.unwrap(), None);
    // Version installée plus récente que la liste filtrée : jamais de retour en arrière.
    assert_eq!(check(&p, &installed_inst("v3beta", "modrinth")).await.unwrap(), None);
}

#[tokio::test]
async fn check_proposes_a_beta_when_the_installed_one_is_a_beta() {
    let mr = MockServer::start().await;
    serve_versions(&mr, vec![ver("v2b", "2.0-beta.2", "2026-05-01T00:00:00Z", "beta"), ver("v1b", "2.0-beta.1", "2026-04-01T00:00:00Z", "beta")]).await;
    let u = check(&providers(&mr), &installed_inst("v1b", "modrinth")).await.unwrap().unwrap();
    assert_eq!(u.version_id, "v2b");
}

#[tokio::test]
async fn check_is_none_without_modrinth_origin_and_errors_on_network_failure() {
    let mr = MockServer::start().await;
    serve_versions(&mr, vec![ver("v2", "2", "2026-05-01T00:00:00Z", "release")]).await;
    let p = providers(&mr);
    let mut none = installed_inst("v1", "modrinth");
    none.pack = None;
    assert_eq!(check(&p, &none).await.unwrap(), None);
    assert_eq!(check(&p, &installed_inst("123", "curseforge")).await.unwrap(), None);
    // Version installée absente de la liste et inconnue de l'API : on ne propose rien.
    Mock::given(method("GET")).and(path("/v2/version/gone")).respond_with(ResponseTemplate::new(404)).mount(&mr).await;
    assert_eq!(check(&p, &installed_inst("gone", "modrinth")).await.unwrap(), None);
    // Hors ligne : erreur (l'appelant l'ignore).
    let dead = Providers { modrinth: ModrinthClient::with_base_url("http://127.0.0.1:1/v2", "test"), cf: CfApi::with_base("http://127.0.0.1:1", ""), allow_loopback: true };
    assert!(check(&dead, &installed_inst("v1", "modrinth")).await.is_err());
}

#[tokio::test]
async fn check_uses_the_api_date_when_installed_version_is_not_in_the_filtered_list() {
    let mr = MockServer::start().await;
    serve_versions(&mr, vec![ver("v2", "2", "2026-05-01T00:00:00Z", "release"), ver("v0", "0.9", "2025-01-01T00:00:00Z", "release")]).await;
    Mock::given(method("GET"))
        .and(path("/v2/version/vold"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ver("vold", "0.5", "2024-01-01T00:00:00Z", "release")))
        .mount(&mr)
        .await;
    let u = check(&providers(&mr), &installed_inst("vold", "modrinth")).await.unwrap().unwrap();
    assert_eq!(u.version_id, "v2");
}

#[tokio::test]
async fn failure_while_applying_puts_everything_back() {
    let e = env().await;
    let v1 = pack(&e, "v1", "v1", "0.16.5", &[("mods/a.jar", A1), ("mods/d.jar", D1)], &[("overrides/config/x.toml", CFG1)]);
    // v2 : a remplacé, d retiré, x.toml changé, et un nouvel override sous `config/sub/` que le joueur bloque avec un FICHIER `config/sub`.
    let v2 = pack(&e, "v2", "v2", "0.16.5", &[("mods/a.jar", A2)], &[("overrides/config/x.toml", CFG2), ("overrides/config/sub/n.toml", b"n")]);
    let (inst, g) = install(&e, &v1).await;
    std::fs::write(g.join("config/sub"), b"un fichier, pas un dossier").unwrap();
    let (before, refs_before) = (snapshot(&g), refs(&e));
    assert!(update(&e, &inst.id, &v1, &v2).await.is_err());
    assert_eq!(snapshot(&g), before, "l'instance doit être exactement comme avant");
    assert_eq!(e.instances.get(&inst.id).unwrap().pack, Some(origin("v1")));
    assert!(!g.join(BACKUP_DIR).exists() || std::fs::read_dir(g.join(BACKUP_DIR)).unwrap().count() == 0);
    for (sha, n) in &refs_before {
        assert_eq!(refs(&e).get(sha), Some(n));
    }
}

#[tokio::test]
async fn revert_never_deletes_an_original_that_was_not_moved_yet() {
    // Échec pendant les déplacements : le remplacement de a.jar n'a pas commencé, l'original est toujours à sa place.
    let e = env().await;
    let v1 = pack(&e, "v1", "v1", "0.16.5", &[("mods/a.jar", A1), ("mods/d.jar", D1)], &[]);
    let (inst, g) = install(&e, &v1).await;
    let bdir = g.join(BACKUP_DIR).join("test-backup");
    std::fs::create_dir_all(&bdir).unwrap();
    let digest = Digest { alg: Alg::Sha512, hex: sha512(A2) };
    let mf = Manifest {
        id: "test-backup".into(),
        from: inst.pack.clone(),
        to: origin("v2"),
        loader_from: inst.loader.clone(),
        loader_to: inst.loader.clone(),
        // d.jar déjà déplacé, a.jar pas encore : seul d.jar est dans la sauvegarde.
        moved: vec![Moved { rel: "mods/a.jar".into(), provenance: None }, Moved { rel: "mods/d.jar".into(), provenance: None }],
        added: vec![Added { rel: "mods/a.jar".into(), digest, sha256: None, prov_key: None }],
    };
    std::fs::create_dir_all(bdir.join("files/mods")).unwrap();
    std::fs::rename(g.join("mods/d.jar"), bdir.join("files/mods/d.jar")).unwrap();
    let (errors, _) = revert(&g, &bdir, &mf, &e.idx, true);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(std::fs::read(g.join("mods/a.jar")).unwrap(), A1, "l'original ne doit pas être supprimé");
    assert_eq!(std::fs::read(g.join("mods/d.jar")).unwrap(), D1);
}

#[tokio::test]
async fn unreachable_tolerant_file_is_reported_missing_and_the_rest_is_applied() {
    let e = env().await;
    let (v1, _) = v1v2(&e);
    // Mod CurseForge non redistribuable : introuvable (404), marqué « tolérant » comme à l'import CurseForge.
    let v2 = pack(&e, "v2cf", "v2", "0.16.5", &[("mods/a.jar", A2), ("mods/b.jar", B1), ("mods/restreint.jar", b"introuvable")], &[]);
    let (inst, g) = install(&e, &v1).await;
    let mut p = prepare(&e.instances.get(&inst.id).unwrap(), &v1, &v2, &origin("v2"), &opts()).unwrap();
    let i = p.new_idx.files.iter().position(|f| f.path == "mods/restreint.jar").unwrap();
    p.new_idx.files[i].tolerant = true;
    let inst = e.instances.get(&inst.id).unwrap();
    let r = apply_prepared(&e.instances, &inst, p, origin("v2"), &e.store, &e.idx, &opts(), &None, &Arc::new(AtomicBool::new(false))).await.unwrap();
    assert_eq!(r.missing, vec!["mods/restreint.jar".to_string()]);
    assert_eq!(std::fs::read(g.join("mods/a.jar")).unwrap(), A2);
    assert!(!g.join("mods/restreint.jar").exists());
    assert_eq!(e.instances.get(&inst.id).unwrap().pack, Some(origin("v2")));
}
