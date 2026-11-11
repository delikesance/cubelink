//! Tâche M4 : index d'assets Mojang et téléchargement des objets (sha1 + taille vérifiés,
//! écriture atomique, reprise, retry, annulation).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::stream::{self, StreamExt};
use serde::Deserialize;

use super::versions::{sha1_hex, AssetIndexRef};
use crate::types::{Error, Result};

pub const DEFAULT_RESOURCES_URL: &str = "https://resources.download.minecraft.net";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AssetObject {
    pub hash: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AssetIndex {
    pub objects: BTreeMap<String, AssetObject>,
    #[serde(default)]
    pub map_to_resources: bool,
    #[serde(default, rename = "virtual")]
    pub virtual_: bool,
}

/// Progression : objets traités (téléchargés ou déjà présents) et octets correspondants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssetProgress {
    pub files_done: usize,
    pub files_total: usize,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

/// Réglages du téléchargement. `cancel` passé à true => `Error::Cancelled`.
#[derive(Debug, Clone)]
pub struct DownloadOptions {
    pub concurrency: usize,
    pub attempts: u32,
    pub retry_delay: Duration,
    pub cancel: Arc<AtomicBool>,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self { concurrency: 8, attempts: 3, retry_delay: Duration::from_millis(500), cancel: Arc::new(AtomicBool::new(false)) }
    }
}

fn net(e: impl std::fmt::Display) -> Error {
    Error::Network(e.to_string())
}

/// Télécharge l'index, vérifie son sha1, le sauvegarde dans `<assets_dir>/indexes/<id>.json`.
pub async fn fetch_asset_index(index_ref: &AssetIndexRef, assets_dir: &Path) -> Result<AssetIndex> {
    if index_ref.id.contains(['/', '\\']) || index_ref.id.contains("..") {
        return Err(Error::Invalid(format!("identifiant d'index {}", index_ref.id)));
    }
    // Index déjà sur disque et conforme au sha1 attendu : aucun accès réseau (lancement plus rapide, et hors ligne).
    let cached = assets_dir.join("indexes").join(format!("{}.json", index_ref.id));
    if let Ok(bytes) = tokio::fs::read(&cached).await {
        if sha1_hex(&bytes).eq_ignore_ascii_case(&index_ref.sha1) {
            if let Ok(index) = serde_json::from_slice::<AssetIndex>(&bytes) {
                return Ok(index);
            }
        }
    }
    let resp = reqwest::get(&index_ref.url).await.map_err(net)?;
    if !resp.status().is_success() {
        return Err(Error::Network(format!("index d'assets {} : HTTP {}", index_ref.id, resp.status())));
    }
    let bytes = resp.bytes().await.map_err(net)?;
    let actual = sha1_hex(&bytes);
    if !actual.eq_ignore_ascii_case(&index_ref.sha1) {
        return Err(Error::Integrity { expected: index_ref.sha1.to_lowercase(), actual });
    }
    let index: AssetIndex = serde_json::from_slice(&bytes)
        .map_err(|e| Error::Network(format!("index d'assets {} invalide : {e}", index_ref.id)))?;
    let dir = assets_dir.join("indexes");
    tokio::fs::create_dir_all(&dir).await?;
    write_atomic(&dir.join(format!("{}.json", index_ref.id)), &bytes).await?;
    Ok(index)
}

async fn write_atomic(dest: &Path, bytes: &[u8]) -> Result<()> {
    let mut part = dest.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    tokio::fs::write(&part, bytes).await?;
    if let Err(e) = tokio::fs::rename(&part, dest).await {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(e.into());
    }
    Ok(())
}

/// Télécharge tous les objets avec les réglages par défaut (hors concurrence).
pub async fn download_assets<F>(
    index: &AssetIndex,
    assets_dir: &Path,
    base_url: Option<&str>,
    progress: F,
    concurrency: usize,
) -> Result<()>
where
    F: Fn(AssetProgress) + Send + Sync,
{
    let options = DownloadOptions { concurrency, ..DownloadOptions::default() };
    download_assets_with(index, assets_dir, base_url, progress, &options).await
}

/// Version complète : annulation, tentatives et délai réglables.
pub async fn download_assets_with<F>(
    index: &AssetIndex,
    assets_dir: &Path,
    base_url: Option<&str>,
    progress: F,
    options: &DownloadOptions,
) -> Result<()>
where
    F: Fn(AssetProgress) + Send + Sync,
{
    let base = base_url.unwrap_or(DEFAULT_RESOURCES_URL).trim_end_matches('/');
    let client = reqwest::Client::new();

    // Dédoublonnage : plusieurs noms peuvent pointer vers le même objet.
    let mut seen = HashSet::new();
    let mut objects = Vec::new();
    for obj in index.objects.values() {
        let h = obj.hash.to_ascii_lowercase();
        if h.len() != 40 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::Invalid(format!("hash d'asset {}", obj.hash)));
        }
        if seen.insert(h.clone()) {
            objects.push((h, obj.size));
        }
    }
    let files_total = objects.len();
    let bytes_total: u64 = objects.iter().map(|(_, s)| s).sum();
    let files_done = AtomicUsize::new(0);
    let bytes_done = AtomicU64::new(0);

    let mut results = stream::iter(objects)
        .map(|(hash, size)| {
            let client = &client;
            async move {
                fetch_object(client, base, assets_dir, &hash, size, options).await?;
                Ok::<u64, Error>(size)
            }
        })
        .buffer_unordered(options.concurrency.max(1));

    while let Some(r) = results.next().await {
        let size = r?;
        let f = files_done.fetch_add(1, Ordering::SeqCst) + 1;
        let b = bytes_done.fetch_add(size, Ordering::SeqCst) + size;
        progress(AssetProgress { files_done: f, files_total, bytes_done: b, bytes_total });
    }
    Ok(())
}

async fn fetch_object(
    client: &reqwest::Client,
    base: &str,
    assets_dir: &Path,
    hash: &str,
    size: u64,
    options: &DownloadOptions,
) -> Result<()> {
    let check_cancel = || {
        if options.cancel.load(Ordering::SeqCst) {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    };
    check_cancel()?;
    let prefix = &hash[..2];
    let dest = assets_dir.join("objects").join(prefix).join(hash);

    // Déjà présent : sauté d'après la taille seule (écriture atomique à l'installation ; relire et hacher les milliers
    // d'objets à chaque lancement coûtait plusieurs secondes). Taille différente (tronqué, corrompu) : supprimé puis retéléchargé.
    if let Ok(meta) = tokio::fs::metadata(&dest).await {
        if meta.is_file() && meta.len() == size {
            return Ok(());
        }
        let _ = tokio::fs::remove_file(&dest).await;
    }
    tokio::fs::create_dir_all(assets_dir.join("objects").join(prefix)).await?;
    let url = format!("{base}/{prefix}/{hash}");

    let mut last = Error::Network(format!("{url} : aucune tentative"));
    for attempt in 0..options.attempts.max(1) {
        check_cancel()?;
        if attempt > 0 {
            tokio::time::sleep(options.retry_delay).await;
            check_cancel()?;
        }
        match try_download(client, &url).await {
            Ok(bytes) => {
                check_cancel()?;
                let actual = sha1_hex(&bytes);
                if actual != hash {
                    return Err(Error::Integrity { expected: hash.to_string(), actual });
                }
                if bytes.len() as u64 != size {
                    return Err(Error::Integrity {
                        expected: format!("{size} octets"),
                        actual: format!("{} octets", bytes.len()),
                    });
                }
                return write_atomic(&dest, &bytes).await;
            }
            Err(e @ Error::NotFound(_)) => return Err(e),
            Err(e) => last = e,
        }
    }
    Err(last)
}

async fn try_download(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let resp = client.get(url).send().await.map_err(net)?;
    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(Error::NotFound(url.to_string()));
    }
    if !status.is_success() {
        return Err(Error::Network(format!("{url} : HTTP {status}")));
    }
    Ok(resp.bytes().await.map_err(net)?.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn obj(content: &[u8]) -> (String, AssetObject) {
        let hash = sha1_hex(content);
        (hash.clone(), AssetObject { hash, size: content.len() as u64 })
    }

    fn opts() -> DownloadOptions {
        DownloadOptions { concurrency: 4, attempts: 3, retry_delay: Duration::from_millis(5), cancel: Arc::new(AtomicBool::new(false)) }
    }

    fn index_of(contents: &[Vec<u8>]) -> AssetIndex {
        let objects = contents.iter().enumerate().map(|(i, c)| (format!("f{i}"), obj(c).1)).collect();
        AssetIndex { objects, map_to_resources: false, virtual_: false }
    }

    async fn serve(server: &MockServer, content: &[u8]) {
        let (h, _) = obj(content);
        Mock::given(method("GET"))
            .and(path(format!("/{}/{}", &h[..2], h)))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(content.to_vec()))
            .mount(server)
            .await;
    }

    fn dest(dir: &Path, hash: &str) -> PathBuf {
        dir.join("objects").join(&hash[..2]).join(hash)
    }

    fn part_of(p: &Path) -> PathBuf {
        let mut s = p.as_os_str().to_owned();
        s.push(".part");
        PathBuf::from(s)
    }

    fn contents(n: usize) -> Vec<Vec<u8>> {
        (0..n).map(|i| format!("asset-{i}").into_bytes()).collect()
    }

    #[tokio::test]
    async fn nominal_20_objects() {
        let server = MockServer::start().await;
        let cs = contents(20);
        for c in &cs {
            serve(&server, c).await;
        }
        let dir = tempfile::tempdir().unwrap();
        let calls = std::sync::Mutex::new(Vec::new());
        download_assets(&index_of(&cs), dir.path(), Some(&server.uri()), |p| calls.lock().unwrap().push(p), 8)
            .await
            .unwrap();
        for c in &cs {
            assert_eq!(std::fs::read(dest(dir.path(), &sha1_hex(c))).unwrap(), *c);
        }
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 20);
        let last = calls.last().unwrap();
        assert_eq!((last.files_done, last.files_total), (20, 20));
        assert_eq!(last.bytes_done, last.bytes_total);
    }

    #[tokio::test]
    async fn existing_valid_object_is_skipped() {
        let server = MockServer::start().await; // aucun mock : toute requête = 404
        let cs = contents(1);
        let dir = tempfile::tempdir().unwrap();
        let d = dest(dir.path(), &sha1_hex(&cs[0]));
        std::fs::create_dir_all(d.parent().unwrap()).unwrap();
        std::fs::write(&d, &cs[0]).unwrap();
        download_assets_with(&index_of(&cs), dir.path(), Some(&server.uri()), |_| {}, &opts()).await.unwrap();
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn corrupted_object_is_redownloaded() {
        let server = MockServer::start().await;
        let cs = contents(1);
        serve(&server, &cs[0]).await;
        let dir = tempfile::tempdir().unwrap();
        let d = dest(dir.path(), &sha1_hex(&cs[0]));
        std::fs::create_dir_all(d.parent().unwrap()).unwrap();
        std::fs::write(&d, b"corrompu").unwrap();
        download_assets_with(&index_of(&cs), dir.path(), Some(&server.uri()), |_| {}, &opts()).await.unwrap();
        assert_eq!(std::fs::read(&d).unwrap(), cs[0]);
    }

    #[tokio::test]
    async fn wrong_sha1_is_rejected_and_not_kept() {
        let server = MockServer::start().await;
        let good = b"attendu".to_vec();
        let (h, _) = obj(&good);
        Mock::given(method("GET"))
            .and(path(format!("/{}/{}", &h[..2], h)))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"autre chose".to_vec()))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let err = download_assets_with(&index_of(&[good]), dir.path(), Some(&server.uri()), |_| {}, &opts())
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Integrity { .. }), "{err:?}");
        assert!(!dest(dir.path(), &h).exists());
        assert!(!part_of(&dest(dir.path(), &h)).exists());
    }

    #[tokio::test]
    async fn transient_500_is_retried() {
        let server = MockServer::start().await;
        let c = b"retry".to_vec();
        let (h, _) = obj(&c);
        let p = format!("/{}/{}", &h[..2], h);
        Mock::given(method("GET")).and(path(p.clone())).respond_with(ResponseTemplate::new(500)).up_to_n_times(1).mount(&server).await;
        Mock::given(method("GET")).and(path(p)).respond_with(ResponseTemplate::new(200).set_body_bytes(c.clone())).mount(&server).await;
        let dir = tempfile::tempdir().unwrap();
        download_assets_with(&index_of(std::slice::from_ref(&c)), dir.path(), Some(&server.uri()), |_| {}, &opts()).await.unwrap();
        assert_eq!(std::fs::read(dest(dir.path(), &h)).unwrap(), c);
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn persistent_500_fails_after_three_attempts() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(500)).mount(&server).await;
        let dir = tempfile::tempdir().unwrap();
        let err = download_assets_with(&index_of(&contents(1)), dir.path(), Some(&server.uri()), |_| {}, &opts())
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Network(_)));
        assert_eq!(server.received_requests().await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn cancellation_returns_cancelled() {
        let server = MockServer::start().await;
        let cs = contents(5);
        for c in &cs {
            serve(&server, c).await;
        }
        let dir = tempfile::tempdir().unwrap();
        let o = opts();
        o.cancel.store(true, Ordering::SeqCst);
        let err = download_assets_with(&index_of(&cs), dir.path(), Some(&server.uri()), |_| {}, &o).await.unwrap_err();
        assert!(matches!(err, Error::Cancelled));
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn cancellation_during_download() {
        let server = MockServer::start().await;
        let cs = contents(10);
        for c in &cs {
            serve(&server, c).await;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut o = opts();
        o.concurrency = 1;
        let cancel = o.cancel.clone();
        let err = download_assets_with(&index_of(&cs), dir.path(), Some(&server.uri()), move |_| cancel.store(true, Ordering::SeqCst), &o)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Cancelled));
    }

    #[tokio::test]
    async fn leftover_part_file_is_overwritten() {
        let server = MockServer::start().await;
        let cs = contents(1);
        serve(&server, &cs[0]).await;
        let dir = tempfile::tempdir().unwrap();
        let d = dest(dir.path(), &sha1_hex(&cs[0]));
        std::fs::create_dir_all(d.parent().unwrap()).unwrap();
        std::fs::write(part_of(&d), b"moitie").unwrap();
        download_assets_with(&index_of(&cs), dir.path(), Some(&server.uri()), |_| {}, &opts()).await.unwrap();
        assert_eq!(std::fs::read(&d).unwrap(), cs[0]);
        assert!(!part_of(&d).exists());
    }

    #[tokio::test]
    async fn index_fetched_verified_and_saved() {
        let server = MockServer::start().await;
        let body = br#"{"objects":{"a/b.txt":{"hash":"da39a3ee5e6b4b0d3255bfef95601890afd80709","size":0}},"virtual":true}"#.to_vec();
        Mock::given(method("GET")).and(path("/idx.json")).respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone())).mount(&server).await;
        let r = AssetIndexRef { id: "17".into(), url: format!("{}/idx.json", server.uri()), sha1: sha1_hex(&body), size: body.len() as u64 };
        let dir = tempfile::tempdir().unwrap();
        let idx = fetch_asset_index(&r, dir.path()).await.unwrap();
        assert_eq!(idx.objects.len(), 1);
        assert!(idx.virtual_ && !idx.map_to_resources);
        assert_eq!(std::fs::read(dir.path().join("indexes/17.json")).unwrap(), body);
    }

    #[tokio::test]
    async fn index_with_wrong_sha1_is_rejected() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/idx.json")).respond_with(ResponseTemplate::new(200).set_body_bytes(b"{\"objects\":{}}".to_vec())).mount(&server).await;
        let r = AssetIndexRef { id: "17".into(), url: format!("{}/idx.json", server.uri()), sha1: "0".repeat(40), size: 14 };
        let dir = tempfile::tempdir().unwrap();
        let err = fetch_asset_index(&r, dir.path()).await.unwrap_err();
        assert!(matches!(err, Error::Integrity { .. }));
        assert!(!dir.path().join("indexes/17.json").exists());
    }
}
