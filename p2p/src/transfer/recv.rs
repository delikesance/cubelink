//! Tâche N12 : côté invité du transfert. Reçoit les fichiers de `plan.to_receive` par un
//! `BlobTransport`, VÉRIFIE sha256 et taille du manifeste, puis installe atomiquement dans le magasin.
//! Un fichier altéré n'est jamais inséré dans le cache (supprimé, `Error::Integrity`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use core_lib::store::hash::verify_file;
use core_lib::store::index::{object_rel_path, StoreIndex};
use core_lib::types::{Error, ManifestFile, Result, TransferPhase, TransferProgress};
use futures_util::stream::{self, StreamExt};

use super::plan::Plan;
use super::send::{BlobTransport, FetchRequest};

/// Options de réception.
#[derive(Debug, Clone)]
pub struct RecvOptions {
    /// Transferts de fichiers simultanés (au moins 1).
    pub concurrency: usize,
    /// Taille maximale d'un fichier.
    pub max_file_bytes: u64,
    /// Taille totale maximale à recevoir.
    pub max_total_bytes: u64,
    /// Point d'extension N13 (resume.rs) : contenus déjà reçus à ne pas redemander.
    pub skip_sha256: HashSet<String>,
}

impl Default for RecvOptions {
    fn default() -> Self {
        Self {
            concurrency: 4,
            max_file_bytes: 2 * 1024 * 1024 * 1024,
            max_total_bytes: super::plan::DEFAULT_MAX_TOTAL_BYTES,
            skip_sha256: HashSet::new(),
        }
    }
}

/// Bilan d'une réception réussie.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RecvSummary {
    pub files: u32,
    pub bytes: u64,
}

/// Supprime le fichier temporaire sauf s'il a été installé.
struct TempGuard(Option<PathBuf>);

impl TempGuard {
    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for TempGuard {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

struct Shared<'a> {
    start: Instant,
    bytes: AtomicU64,
    files: AtomicU64,
    files_total: u32,
    bytes_total: u64,
    via_relay: bool,
    progress: &'a (dyn Fn(TransferProgress) + Send + Sync),
}

impl Shared<'_> {
    fn emit(&self, phase: TransferPhase, error: Option<String>) {
        let bytes = self.bytes.load(Ordering::Relaxed);
        let secs = self.start.elapsed().as_secs_f64();
        let speed = if secs > 0.0 { (bytes as f64 / secs) as u64 } else { 0 };
        (self.progress)(TransferProgress {
            phase,
            files_done: self.files.load(Ordering::Relaxed) as u32,
            files_total: self.files_total,
            bytes_done: bytes,
            bytes_total: self.bytes_total,
            speed_bps: speed,
            via_relay: self.via_relay,
            error,
        });
    }
}

/// Le sha1 annoncé par l'hôte est une revendication : si présent, il doit correspondre au contenu
/// reçu (sinon un hôte pourrait faire passer un jar quelconque pour un mod connu du catalogue).
pub(crate) fn verify_claimed_sha1(path: &Path, claimed: Option<&str>) -> Result<()> {
    match claimed {
        Some(h) => core_lib::unknown_mods::verify_sha1_file(path, h),
        None => Ok(()),
    }
}

/// Reçoit tous les fichiers de `plan.to_receive` depuis `peer`.
#[allow(clippy::too_many_arguments)]
pub async fn receive_plan<T: BlobTransport>(
    plan: &Plan,
    peer: &str,
    transport: &T,
    store_root: &Path,
    index: &StoreIndex,
    opts: &RecvOptions,
    progress: &(dyn Fn(TransferProgress) + Send + Sync),
    cancel: &AtomicBool,
) -> Result<RecvSummary> {
    let todo: Vec<&ManifestFile> =
        plan.to_receive.iter().filter(|f| !opts.skip_sha256.contains(&f.sha256)).collect();
    let bytes_total: u64 = todo.iter().map(|f| f.size).sum();
    let shared = Shared {
        start: Instant::now(),
        bytes: AtomicU64::new(0),
        files: AtomicU64::new(0),
        files_total: todo.len() as u32,
        bytes_total,
        via_relay: false,
        progress,
    };
    let result = run(&todo, peer, transport, store_root, index, opts, &shared, cancel).await;
    match &result {
        Ok(_) => shared.emit(TransferPhase::Done, None),
        Err(Error::Cancelled) => shared.emit(TransferPhase::Paused, None),
        Err(e) => shared.emit(TransferPhase::Failed, Some(e.to_string())),
    }
    result
}

#[allow(clippy::too_many_arguments)]
async fn run<T: BlobTransport>(
    todo: &[&ManifestFile],
    peer: &str,
    transport: &T,
    store_root: &Path,
    index: &StoreIndex,
    opts: &RecvOptions,
    shared: &Shared<'_>,
    cancel: &AtomicBool,
) -> Result<RecvSummary> {
    if let Some(f) = todo.iter().find(|f| f.size > opts.max_file_bytes) {
        return Err(Error::Invalid(format!(
            "fichier trop volumineux : {:?} ({} octets, maximum {})",
            f.path, f.size, opts.max_file_bytes
        )));
    }
    if shared.bytes_total > opts.max_total_bytes {
        return Err(Error::Invalid(format!(
            "transfert trop volumineux : {} octets (maximum {})",
            shared.bytes_total, opts.max_total_bytes
        )));
    }
    let tmp_dir = store_root.join(".tmp");
    tokio::fs::create_dir_all(&tmp_dir).await?;
    shared.emit(TransferPhase::Preparing, None);

    let mut results = stream::iter(todo.iter().copied())
        .map(|f| receive_one(f, peer, transport, store_root, &tmp_dir, index, shared, cancel))
        .buffer_unordered(opts.concurrency.max(1));
    let mut summary = RecvSummary::default();
    while let Some(r) = results.next().await {
        let size = r?;
        summary.files += 1;
        summary.bytes += size;
    }
    Ok(summary)
}

#[allow(clippy::too_many_arguments)]
async fn receive_one<T: BlobTransport>(
    f: &ManifestFile,
    peer: &str,
    transport: &T,
    store_root: &Path,
    tmp_dir: &Path,
    index: &StoreIndex,
    shared: &Shared<'_>,
    cancel: &AtomicBool,
) -> Result<u64> {
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }
    let rel = object_rel_path(&f.sha256)?;
    let tmp = tmp_dir.join(format!("{}.{:016x}.part", f.sha256, rand::random::<u64>()));
    let mut guard = TempGuard(Some(tmp.clone()));

    let req = FetchRequest { sha256: f.sha256.clone(), size: f.size, dest: tmp.clone(), resume_from: 0 };
    let on_bytes = |n: u64| {
        shared.bytes.fetch_add(n, Ordering::Relaxed);
        shared.emit(TransferPhase::Receiving, None);
    };
    let before = shared.bytes.load(Ordering::Relaxed);
    transport.fetch(peer, &req, &on_bytes, cancel).await?;
    // Le transport peut avoir compté des octets d'un fichier finalement rejeté : on les retire.
    let counted = shared.bytes.load(Ordering::Relaxed).saturating_sub(before);

    let reject = |e: Error| {
        shared.bytes.fetch_sub(counted.min(shared.bytes.load(Ordering::Relaxed)), Ordering::Relaxed);
        e
    };

    // Vérifications : taille PUIS sha256 du manifeste.
    let actual_size = tokio::fs::metadata(&tmp).await?.len();
    if actual_size != f.size {
        return Err(reject(Error::Integrity {
            expected: format!("{} octets", f.size),
            actual: format!("{actual_size} octets"),
        }));
    }
    shared.emit(TransferPhase::Verifying, None);
    let (vtmp, vsha, vsha1) = (tmp.clone(), f.sha256.clone(), f.sha1.clone());
    tokio::task::spawn_blocking(move || verify_file(&vtmp, &vsha).and_then(|()| verify_claimed_sha1(&vtmp, vsha1.as_deref())))
        .await
        .map_err(|e| Error::Other(format!("vérification : {e}")))?
        .map_err(reject)?;

    // Installation atomique puis enregistrement dans l'index.
    let dest = store_root.join(&rel);
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::rename(&tmp, &dest).await?;
    guard.disarm();
    index.insert(&f.sha256, &rel, f.size)?;
    shared.files.fetch_add(1, Ordering::Relaxed);
    shared.emit(TransferPhase::Receiving, None);
    Ok(f.size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transfer::send::{
        AllowList, BlobTransport, Host, IrohTransport, MemNetwork, MemTransport, ServeIndex, ServeLimits,
    };
    use core_lib::types::{Loader, LoaderSpec, Manifest};
    use sha2::{Digest, Sha256};
    use std::sync::{Arc, Mutex};

    fn sha(data: &[u8]) -> String {
        hex::encode(Sha256::digest(data))
    }

    struct Fixture {
        game: tempfile::TempDir,
        store: tempfile::TempDir,
        index: StoreIndex,
        manifest: Manifest,
        plan: Plan,
        contents: Vec<Vec<u8>>,
    }

    fn fixture() -> Fixture {
        let game = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(game.path().join("mods")).unwrap();
        let mut files = Vec::new();
        let mut contents = Vec::new();
        for i in 0..5u8 {
            let data: Vec<u8> = (0..(1000 + i as usize * 300)).map(|j| (j as u8).wrapping_mul(i + 3)).collect();
            let path = format!("mods/m{i}.jar");
            std::fs::write(game.path().join(&path), &data).unwrap();
            files.push(ManifestFile { path, size: data.len() as u64, sha256: sha(&data), sha1: Some(core_lib::unknown_mods::sha1_hex(&data)) });
            contents.push(data);
        }
        let manifest = Manifest {
            name: "M".into(),
            minecraft: "1.21.1".into(),
            loader: LoaderSpec { loader: Loader::Fabric, version: "0.16.0".into() },
            java_major: 21,
            files,
        };
        let index = StoreIndex::open(store.path().join("index.db")).unwrap();
        let plan = crate::transfer::plan::compute_plan(&manifest, &crate::transfer::plan::StoreCache(&index)).unwrap();
        Fixture { game, store, index, manifest, plan, contents }
    }

    async fn nodes(fx: &Fixture, allowed: &[&str]) -> (MemTransport, MemTransport) {
        let net = MemNetwork::new();
        let idx = ServeIndex::from_manifest(&fx.manifest, fx.game.path()).unwrap();
        let host = Host::new(idx, Arc::new(AllowList(allowed.iter().map(|s| s.to_string()).collect())), ServeLimits::default());
        let a = MemTransport::new(net.clone(), "hote");
        a.serve(host).await.unwrap();
        (a, MemTransport::new(net, "invite"))
    }

    fn quiet(_: TransferProgress) {}

    #[tokio::test]
    async fn nominal_five_files() {
        let fx = fixture();
        let (_a, b) = nodes(&fx, &["invite"]).await;
        let events = Mutex::new(Vec::new());
        let cb = |p: TransferProgress| events.lock().unwrap().push(p);
        let cancel = AtomicBool::new(false);
        let s = receive_plan(&fx.plan, "hote", &b, fx.store.path(), &fx.index, &RecvOptions::default(), &cb, &cancel)
            .await
            .unwrap();
        assert_eq!(s.files, 5);
        assert_eq!(s.bytes, fx.plan.to_receive_bytes);
        for (f, data) in fx.manifest.files.iter().zip(&fx.contents) {
            let e = fx.index.get(&f.sha256).unwrap().unwrap();
            assert_eq!(std::fs::read(fx.store.path().join(&e.rel_path)).unwrap(), *data);
        }
        let ev = events.lock().unwrap();
        let last = ev.last().unwrap();
        assert_eq!((last.phase, last.files_done, last.files_total), (TransferPhase::Done, 5, 5));
        assert_eq!(last.bytes_done, fx.plan.to_receive_bytes);
        // plus rien à recevoir
        let again = crate::transfer::plan::compute_plan(&fx.manifest, &crate::transfer::plan::StoreCache(&fx.index)).unwrap();
        assert_eq!(again.to_receive_count, 0);
    }

    #[tokio::test]
    async fn tampered_file_rejected_and_not_cached() {
        let fx = fixture();
        let (_a, b) = nodes(&fx, &["invite"]).await;
        // l'hôte altère un fichier (même taille) après la construction du manifeste
        let mut bad = fx.contents[2].clone();
        bad[0] ^= 0xff;
        std::fs::write(fx.game.path().join("mods/m2.jar"), &bad).unwrap();
        let cancel = AtomicBool::new(false);
        let opts = RecvOptions { concurrency: 1, ..RecvOptions::default() };
        let r = receive_plan(&fx.plan, "hote", &b, fx.store.path(), &fx.index, &opts, &quiet, &cancel).await;
        assert!(matches!(r, Err(Error::Integrity { .. })), "{r:?}");
        let sha_bad = &fx.manifest.files[2].sha256;
        assert!(fx.index.get(sha_bad).unwrap().is_none());
        assert!(!fx.store.path().join(object_rel_path(sha_bad).unwrap()).exists());
        // aucun résidu temporaire
        let left = std::fs::read_dir(fx.store.path().join(".tmp")).unwrap().count();
        assert_eq!(left, 0);
    }

    #[tokio::test]
    async fn wrong_claimed_sha1_rejected_and_not_cached() {
        let mut fx = fixture();
        // sha256 et taille corrects, mais sha1 annoncé mensonger
        fx.plan.to_receive[3].sha1 = Some("0".repeat(40));
        let (_a, b) = nodes(&fx, &["invite"]).await;
        let cancel = AtomicBool::new(false);
        let opts = RecvOptions { concurrency: 1, ..RecvOptions::default() };
        let r = receive_plan(&fx.plan, "hote", &b, fx.store.path(), &fx.index, &opts, &quiet, &cancel).await;
        assert!(matches!(r, Err(Error::Integrity { .. })), "{r:?}");
        let sha_bad = &fx.manifest.files[3].sha256;
        assert!(fx.index.get(sha_bad).unwrap().is_none());
        assert!(!fx.store.path().join(object_rel_path(sha_bad).unwrap()).exists());
        assert_eq!(std::fs::read_dir(fx.store.path().join(".tmp")).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn wrong_size_rejected() {
        let fx = fixture();
        let (_a, b) = nodes(&fx, &["invite"]).await;
        std::fs::write(fx.game.path().join("mods/m1.jar"), b"court").unwrap();
        let cancel = AtomicBool::new(false);
        let r = receive_plan(&fx.plan, "hote", &b, fx.store.path(), &fx.index, &RecvOptions::default(), &quiet, &cancel).await;
        assert!(matches!(r, Err(Error::Integrity { .. })), "{r:?}");
        assert!(fx.index.get(&fx.manifest.files[1].sha256).unwrap().is_none());
    }

    #[tokio::test]
    async fn unauthorized_peer_refused() {
        let fx = fixture();
        let (_a, b) = nodes(&fx, &["quelquun-d-autre"]).await;
        let cancel = AtomicBool::new(false);
        let r = receive_plan(&fx.plan, "hote", &b, fx.store.path(), &fx.index, &RecvOptions::default(), &quiet, &cancel).await;
        assert!(matches!(r, Err(Error::Auth(_))), "{r:?}");
        assert_eq!(fx.index.total_size().unwrap(), 0);
    }

    #[tokio::test]
    async fn hash_outside_manifest_refused() {
        let fx = fixture();
        let (_a, b) = nodes(&fx, &["invite"]).await;
        let evil = b"pas dans le manifeste";
        let plan = Plan {
            to_receive: vec![ManifestFile { path: "mods/x.jar".into(), size: evil.len() as u64, sha256: sha(evil), sha1: None }],
            ..Plan::default()
        };
        let cancel = AtomicBool::new(false);
        let r = receive_plan(&plan, "hote", &b, fx.store.path(), &fx.index, &RecvOptions::default(), &quiet, &cancel).await;
        assert!(matches!(r, Err(Error::NotFound(_))), "{r:?}");
    }

    #[tokio::test]
    async fn cancellation_stops_transfer() {
        let fx = fixture();
        let (_a, b) = nodes(&fx, &["invite"]).await;
        let cancel = AtomicBool::new(true);
        let events = Mutex::new(Vec::new());
        let cb = |p: TransferProgress| events.lock().unwrap().push(p);
        let r = receive_plan(&fx.plan, "hote", &b, fx.store.path(), &fx.index, &RecvOptions::default(), &cb, &cancel).await;
        assert!(matches!(r, Err(Error::Cancelled)));
        assert_eq!(fx.index.total_size().unwrap(), 0);
        assert_eq!(events.lock().unwrap().last().unwrap().phase, TransferPhase::Paused);
    }

    #[tokio::test]
    async fn size_limits_enforced() {
        let fx = fixture();
        let (_a, b) = nodes(&fx, &["invite"]).await;
        let cancel = AtomicBool::new(false);
        let per_file = RecvOptions { max_file_bytes: 1100, ..RecvOptions::default() };
        let r = receive_plan(&fx.plan, "hote", &b, fx.store.path(), &fx.index, &per_file, &quiet, &cancel).await;
        assert!(matches!(r, Err(Error::Invalid(_))));
        let total = RecvOptions { max_total_bytes: 1000, ..RecvOptions::default() };
        let r = receive_plan(&fx.plan, "hote", &b, fx.store.path(), &fx.index, &total, &quiet, &cancel).await;
        assert!(matches!(r, Err(Error::Invalid(_))));
        assert_eq!(fx.index.total_size().unwrap(), 0);
    }

    #[tokio::test]
    async fn skip_set_is_honoured() {
        let fx = fixture();
        let (_a, b) = nodes(&fx, &["invite"]).await;
        let cancel = AtomicBool::new(false);
        let skip: HashSet<String> = fx.manifest.files.iter().take(2).map(|f| f.sha256.clone()).collect();
        let opts = RecvOptions { skip_sha256: skip, ..RecvOptions::default() };
        let s = receive_plan(&fx.plan, "hote", &b, fx.store.path(), &fx.index, &opts, &quiet, &cancel).await.unwrap();
        assert_eq!(s.files, 3);
    }


    /// Hôte MALVEILLANT (binaire modifié, sans la garde de l'hôte légitime) : répond au contrôle avec le BLAKE3
    /// d'un contenu de même taille mais altéré et le sert via iroh-blobs. L'invité doit le rejeter par sa propre
    /// vérification sha256 (`Integrity`), sans rien mettre en cache ni laisser de résidu.
    #[derive(Clone)]
    struct HostileControl {
        store: iroh_blobs::store::fs::FsStore,
        evil_dir: std::path::PathBuf,
    }

    impl std::fmt::Debug for HostileControl {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("HostileControl")
        }
    }

    impl iroh::protocol::ProtocolHandler for HostileControl {
        async fn accept(&self, conn: iroh::endpoint::Connection) -> std::result::Result<(), iroh::protocol::AcceptError> {
            while let Ok((mut send, mut recv)) = conn.accept_bi().await {
                let this = self.clone();
                tokio::spawn(async move {
                    let mut sha = [0u8; 64];
                    if recv.read_exact(&mut sha).await.is_err() {
                        return;
                    }
                    let sha = String::from_utf8_lossy(&sha).into_owned();
                    let tag = this
                        .store
                        .blobs()
                        .add_path_with_opts(iroh_blobs::api::blobs::AddPathOptions {
                            path: this.evil_dir.join(&sha),
                            format: iroh_blobs::BlobFormat::Raw,
                            mode: iroh_blobs::api::blobs::ImportMode::Copy,
                        })
                        .await
                        .unwrap();
                    let mut msg = vec![0u8];
                    msg.extend_from_slice(tag.hash.as_bytes());
                    let _ = send.write_all(&msg).await;
                    let _ = send.finish();
                    let _ = recv.read_to_end(16).await;
                });
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn hostile_iroh_host_serving_altered_bytes_is_rejected_by_the_guest() {
        use iroh::endpoint::presets;
        use iroh::protocol::Router;
        use iroh::{Endpoint, RelayMode};

        let fx = fixture();
        let dir = tempfile::tempdir().unwrap();
        // Contenus altérés (même taille), un fichier par sha256 annoncé.
        let evil_dir = dir.path().join("evil");
        std::fs::create_dir_all(&evil_dir).unwrap();
        for (f, data) in fx.manifest.files.iter().zip(&fx.contents) {
            let mut bad = data.clone();
            bad[0] ^= 0xff;
            std::fs::write(evil_dir.join(&f.sha256), &bad).unwrap();
        }
        let ep_a = Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled).bind().await.unwrap();
        let ep_b = Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled).bind().await.unwrap();
        let store = iroh_blobs::store::fs::FsStore::load(dir.path().join("host-blobs")).await.unwrap();
        let router = Router::builder(ep_a.clone())
            .accept(crate::transfer::send::CONTROL_ALPN, HostileControl { store: store.clone(), evil_dir })
            .accept(iroh_blobs::ALPN, iroh_blobs::BlobsProtocol::new(&store, None))
            .spawn();
        let b = IrohTransport::new(ep_b, dir.path().join("b")).await.unwrap();
        let peer = b.add_peer(ep_a.addr());
        let cancel = AtomicBool::new(false);
        let opts = RecvOptions { concurrency: 1, ..RecvOptions::default() };
        let r = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            receive_plan(&fx.plan, &peer, &b, fx.store.path(), &fx.index, &opts, &quiet, &cancel),
        )
        .await
        .expect("délai dépassé");
        assert!(matches!(r, Err(Error::Integrity { .. })), "{r:?}");
        // Rien dans le cache, rien d'installé, aucun résidu.
        for f in &fx.manifest.files {
            assert!(fx.index.get(&f.sha256).unwrap().is_none());
            assert!(!fx.store.path().join(object_rel_path(&f.sha256).unwrap()).exists());
        }
        assert_eq!(std::fs::read_dir(fx.store.path().join(".tmp")).unwrap().count(), 0);
        let _ = router.shutdown().await;
    }

    /// Intégration iroh locale : deux endpoints, relais désactivé, adresses IP locales (comme le spike N1).
    #[tokio::test]
    #[ignore = "ouvre de vrais sockets UDP locaux ; lancer avec --ignored"]
    async fn iroh_local_transfer() {
        use iroh::endpoint::presets;
        use iroh::{Endpoint, RelayMode};

        let fx = fixture();
        let dir = tempfile::tempdir().unwrap();
        let ep_a = Endpoint::builder(presets::Minimal)
            .relay_mode(RelayMode::Disabled)
            .alpns(vec![crate::transfer::send::CONTROL_ALPN.to_vec(), iroh_blobs::ALPN.to_vec()])
            .bind()
            .await
            .unwrap();
        let ep_b = Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled).bind().await.unwrap();
        let guest_id = ep_b.id().to_string();
        let a = IrohTransport::new(ep_a, dir.path().join("a")).await.unwrap();
        let b = IrohTransport::new(ep_b, dir.path().join("b")).await.unwrap();
        let idx = ServeIndex::from_manifest(&fx.manifest, fx.game.path()).unwrap();
        a.serve(Host::new(idx, Arc::new(AllowList(vec![guest_id])), ServeLimits::default())).await.unwrap();
        let peer = b.add_peer(a.addr());
        let cancel = AtomicBool::new(false);
        let s = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            receive_plan(&fx.plan, &peer, &b, fx.store.path(), &fx.index, &RecvOptions::default(), &quiet, &cancel),
        )
        .await
        .expect("délai dépassé")
        .unwrap();
        assert_eq!(s.files, 5);
        assert!(!b.via_relay(&peer));
        a.shutdown().await;
    }
}
