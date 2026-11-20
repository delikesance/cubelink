//! Marquer-balayer du magasin sur un vrai disque : vrais fichiers de quelques Mo, vrais liens durs, vraies
//! suppressions d'instance (`InstanceStore::delete`) et de mod (`mods::remove`). Aucun réseau, aucune dépendance Windows.
//!
//! Scénario du ticket #79 : trois faux modpacks installés puis supprimés ; le cache doit se vider de ce qui n'est plus
//! lié nulle part et garder ce qu'une autre instance utilise encore.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use core_lib::instance::InstanceStore;
use core_lib::mods;
use core_lib::store::gc::sweep;
use core_lib::store::hash::sha256_file;
use core_lib::store::index::{object_rel_path, StoreIndex};
use core_lib::store::link::{link_into, LinkKind};
use core_lib::types::{Loader, LoaderSpec};

const MB: usize = 1024 * 1024;

struct Env {
    _d: tempfile::TempDir,
    store: PathBuf,
    index: StoreIndex,
    instances: InstanceStore,
}

fn env() -> Env {
    let d = tempfile::tempdir().unwrap();
    let store = d.path().join("store");
    fs::create_dir_all(&store).unwrap();
    let index = StoreIndex::open(d.path().join("index.sqlite")).unwrap();
    let instances = InstanceStore::new(d.path().join("instances"));
    Env { _d: d, store, index, instances }
}

/// Octets pseudo-aléatoires reproductibles (xorshift) : chaque graine donne un contenu et un hash distincts.
fn blob(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

/// Dépose un objet dans le magasin et dans l'index (comme un téléchargement), sans le lier.
fn put(e: &Env, seed: u64, len: usize) -> (String, PathBuf) {
    let tmp = e.store.join(format!("tmp-{seed}"));
    fs::write(&tmp, blob(seed, len)).unwrap();
    let sha = sha256_file(&tmp).unwrap();
    let rel = object_rel_path(&sha).unwrap();
    let path = e.store.join(&rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::rename(&tmp, &path).unwrap();
    e.index.insert(&sha, &rel, len as u64).unwrap();
    (sha, path)
}

/// Installe l'objet dans `game/mods/<name>` comme le font les installateurs : lien dur puis `add_ref`.
fn install(e: &Env, game: &Path, name: &str, sha: &str, path: &Path) {
    let kind = link_into(game, &format!("mods/{name}"), path).unwrap();
    assert_eq!(kind, LinkKind::HardLink, "le test exige de vrais liens durs");
    e.index.add_ref(sha).unwrap();
}

fn instance(e: &Env, name: &str) -> (String, PathBuf) {
    let i = e.instances.create(name, "1.21.1", LoaderSpec { loader: Loader::Fabric, version: "0.16.0".into() }, 4096).unwrap();
    let game = e.instances.game_dir(&i.id).unwrap();
    (i.id, game)
}

fn on_disk(e: &Env, sha: &str) -> bool {
    e.store.join(object_rel_path(sha).unwrap()).is_file()
}

fn refs(e: &Env, sha: &str) -> Option<u32> {
    e.index.get(sha).unwrap().map(|x| x.refs)
}

fn dir_bytes(dir: &Path) -> u64 {
    fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| match e.metadata() {
                    Ok(m) if m.is_dir() => dir_bytes(&e.path()),
                    Ok(m) => m.len(),
                    Err(_) => 0,
                })
                .sum()
        })
        .unwrap_or(0)
}

fn prefix_dirs(store: &Path) -> usize {
    fs::read_dir(store).unwrap().flatten().filter(|e| e.path().is_dir()).count()
}

#[test]
fn trois_modpacks_installes_puis_supprimes() {
    let e = env();
    let (id1, g1) = instance(&e, "Pack un");
    let (id2, g2) = instance(&e, "Pack deux");
    let (id3, g3) = instance(&e, "Pack trois");

    // Pack 1 : deux jars propres. Pack 2 : un jar propre + un jar partagé avec le pack 3. Pack 3 : le partagé + deux propres.
    let (a1, pa1) = put(&e, 11, 2 * MB);
    let (a2, pa2) = put(&e, 12, 2 * MB);
    let (b1, pb1) = put(&e, 21, 2 * MB);
    let (shared, ps) = put(&e, 99, 3 * MB);
    let (c1, pc1) = put(&e, 31, 2 * MB);
    let (c2, pc2) = put(&e, 32, 2 * MB);
    // .mrpack de Parcourir : dans le cache, jamais lié ni référencé.
    let (mr1, _) = put(&e, 41, MB);
    let (mr2, _) = put(&e, 42, MB);
    let (mr3, _) = put(&e, 43, MB);

    install(&e, &g1, "a1.jar", &a1, &pa1);
    install(&e, &g1, "a2.jar", &a2, &pa2);
    install(&e, &g2, "b1.jar", &b1, &pb1);
    install(&e, &g2, "shared.jar", &shared, &ps);
    install(&e, &g3, "shared.jar", &shared, &ps);
    install(&e, &g3, "c1.jar", &c1, &pc1);
    install(&e, &g3, "c2.jar", &c2, &pc2);
    // Réinstallation du même fichier (lien déjà en place) : add_ref en double, comme browse::install_content.
    install(&e, &g3, "c1.jar", &c1, &pc1);
    assert_eq!(refs(&e, &c1), Some(2), "compteur gonflé par la réinstallation");

    // Un modpack installé par-dessus : sauvegarde d'annulation d'un jar retiré (mod_fix), sous .cubelink.
    link_into(&g3, ".cubelink/backups/b-1/mods/c2.jar", &pc2).unwrap();

    let before = dir_bytes(&e.store);
    let all = [&a1, &a2, &b1, &shared, &c1, &c2, &mr1, &mr2, &mr3];

    // Suppressions comme dans l'application : aucune ne libère de référence.
    e.instances.delete(&id1).unwrap();
    e.instances.delete(&id2).unwrap();
    mods::remove(&g3, "c1.jar").unwrap();
    assert_eq!(refs(&e, &a1), Some(1), "le compteur ne descend jamais tout seul");

    // Un balayage prudent (min_age d'un jour) ne touche à rien : tout est récent.
    let r = sweep(&e.store, &e.index, e.instances.root(), Duration::from_secs(86_400)).unwrap();
    assert!(r.removed.is_empty() && r.errors.is_empty(), "{r:?}");
    assert!(all.iter().all(|s| on_disk(&e, s)));
    // ... mais les compteurs sont recalculés.
    assert_eq!(refs(&e, &a1), Some(0));
    assert_eq!(refs(&e, &b1), Some(0));
    assert_eq!(refs(&e, &shared), Some(1), "encore lié par le pack 3 seulement");
    assert_eq!(refs(&e, &c1), Some(0), "retiré du pack 3 : le compteur gonflé retombe à 0");
    assert_eq!(refs(&e, &c2), Some(2), "jar du pack 3 et sa sauvegarde .cubelink");

    // Balayage sans délai.
    let r = sweep(&e.store, &e.index, e.instances.root(), Duration::ZERO).unwrap();
    assert!(r.errors.is_empty(), "{r:?}");
    let mut removed = r.removed.clone();
    removed.sort();
    let mut expected = vec![a1.clone(), a2.clone(), b1.clone(), c1.clone(), mr1.clone(), mr2.clone(), mr3.clone()];
    expected.sort();
    assert_eq!(removed, expected);
    assert_eq!(r.bytes_freed, (2 + 2 + 2 + 2 + 1 + 1 + 1) as u64 * MB as u64);

    // Disque : objets non liés supprimés, objets encore liés intacts et lisibles par l'instance 3.
    for s in [&a1, &a2, &b1, &c1, &mr1, &mr2, &mr3] {
        assert!(!on_disk(&e, s), "{s} aurait dû disparaître");
        assert!(e.index.get(s).unwrap().is_none(), "entrée d'index orpheline pour {s}");
    }
    for s in [&shared, &c2] {
        assert!(on_disk(&e, s) && e.index.get(s).unwrap().is_some());
    }
    assert_eq!(fs::read(g3.join("mods/shared.jar")).unwrap(), blob(99, 3 * MB));
    assert_eq!(fs::read(g3.join("mods/c2.jar")).unwrap(), blob(32, 2 * MB));
    assert_eq!(prefix_dirs(&e.store), 2, "dossiers de préfixe vides retirés (il reste ceux de 2 objets)");
    let after = dir_bytes(&e.store);
    assert_eq!(before - after, r.bytes_freed, "du avant/après");

    // Idempotence.
    let r2 = sweep(&e.store, &e.index, e.instances.root(), Duration::ZERO).unwrap();
    assert!(r2.removed.is_empty() && r2.errors.is_empty(), "{r2:?}");
    assert_eq!(dir_bytes(&e.store), after);

    // Suppression de la dernière instance : tout le cache part, même le jar encore sauvegardé.
    e.instances.delete(&id3).unwrap();
    let r3 = sweep(&e.store, &e.index, e.instances.root(), Duration::ZERO).unwrap();
    assert_eq!(r3.removed.len(), 2, "{r3:?}");
    assert_eq!(dir_bytes(&e.store), 0);
    assert_eq!(prefix_dirs(&e.store), 0);
}

#[test]
fn une_copie_ne_retient_pas_l_objet_et_l_index_illisible_ne_supprime_rien() {
    let e = env();
    let (_id, g) = instance(&e, "Copie");
    let (sha, p) = put(&e, 7, MB);
    fs::create_dir_all(g.join("mods")).unwrap();
    fs::copy(&p, g.join("mods/copie.jar")).unwrap();
    // Racine d'instances illisible (un fichier à la place du dossier) : balayage refusé, rien ne bouge.
    let bad = e._d.path().join("pas-un-dossier");
    fs::write(&bad, b"x").unwrap();
    let r = sweep(&e.store, &e.index, &bad, Duration::ZERO).unwrap();
    assert!(r.removed.is_empty() && !r.errors.is_empty());
    assert!(on_disk(&e, &sha));
    // Avec une racine valide, la copie ne compte pas : l'objet du cache est balayé, la copie de l'instance reste.
    let r = sweep(&e.store, &e.index, e.instances.root(), Duration::ZERO).unwrap();
    assert_eq!(r.removed, vec![sha.clone()]);
    assert!(!on_disk(&e, &sha));
    assert_eq!(fs::read(g.join("mods/copie.jar")).unwrap(), blob(7, MB));
}
