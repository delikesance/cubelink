//! Le balayage ne doit pas garder un descripteur ouvert par objet du cache : un cache de plus de 1024 objets
//! (quelques modpacks) ferait sinon échouer le parcours avec EMFILE, et rien ne serait jamais libéré.
//! Processus à part (un test unique) : la limite de descripteurs est abaissée pour tout le processus.
#![cfg(unix)]

use std::fs;

use core_lib::store::index::{object_rel_path, StoreIndex};
use core_lib::store::link::link_into;
use core_lib::store::live::live_refs;

#[test]
fn live_refs_does_not_hold_one_descriptor_per_object() {
    const N: u32 = 1300;
    let d = tempfile::tempdir().unwrap();
    let store = d.path().join("store");
    let instances = d.path().join("instances");
    let game = instances.join("i1").join("minecraft");
    fs::create_dir_all(&store).unwrap();
    fs::create_dir_all(&game).unwrap();
    let idx = StoreIndex::open(d.path().join("index.db")).unwrap();
    for n in 0..N {
        let sha = format!("{n:064x}");
        let rel = object_rel_path(&sha).unwrap();
        let p = store.join(&rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        let body = format!("objet {n}");
        fs::write(&p, &body).unwrap();
        idx.insert(&sha, &rel, body.len() as u64).unwrap();
        if n % 2 == 0 {
            link_into(&game, &format!("mods/m{n}.jar"), &p).unwrap();
        }
    }

    let mut lim = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    unsafe {
        assert_eq!(libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim), 0);
        let low = libc::rlimit { rlim_cur: 256.min(lim.rlim_max), rlim_max: lim.rlim_max };
        assert_eq!(libc::setrlimit(libc::RLIMIT_NOFILE, &low), 0);
    }
    let scan = live_refs(&store, &idx, &instances).unwrap();
    assert!(scan.complete, "{:?}", scan.errors);
    assert_eq!(scan.counts.len(), (N / 2) as usize);
}
