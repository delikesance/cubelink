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

fn net(e: reqwest::Error) -> Error {
    crate::http::net_error(e)
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
    let resp = crate::http::client().get(&index_ref.url).send().await.map_err(net)?;
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

/// Nom d'asset sûr : chemin relatif sans `..`, `\\`, `:` ni segment vide (un index corrompu ne doit pas sortir du dossier).
fn safe_asset_name(name: &str) -> Result<()> {
    let bad = name.is_empty()
        || name.starts_with('/')
        || name.contains('\\')
        || name.contains(':')
        || name.split('/').any(|s| s.is_empty() || s == "..");
    if bad {
        Err(Error::Invalid(format!("nom d'asset {name}")))
    } else {
        Ok(())
    }
}

/// Copie `src` vers `dest` (atomique via `.part`), sauf si `dest` a déjà la bonne taille.
fn copy_named(src: &Path, dest: &Path, size: u64) -> Result<()> {
    if let Ok(meta) = std::fs::metadata(dest) {
        if meta.is_file() && meta.len() == size {
            return Ok(());
        }
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut part = dest.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    if let Err(e) = std::fs::copy(src, &part).and_then(|_| std::fs::rename(&part, dest)) {
        let _ = std::fs::remove_file(&part);
        return Err(e.into());
    }
    Ok(())
}

fn copy_all(index: &AssetIndex, assets_dir: &Path, dest_root: &Path, cancel: &AtomicBool) -> Result<()> {
    for (name, obj) in &index.objects {
        if cancel.load(Ordering::SeqCst) {
            return Err(Error::Cancelled);
        }
        safe_asset_name(name)?;
        let h = obj.hash.to_ascii_lowercase();
        if h.len() != 40 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::Invalid(format!("hash d'asset {}", obj.hash)));
        }
        let src = assets_dir.join("objects").join(&h[..2]).join(&h);
        if !src.is_file() {
            return Err(Error::NotFound(format!("objet d'asset {name} ({h})")));
        }
        let dest = name.split('/').fold(dest_root.to_path_buf(), |p, seg| p.join(seg));
        copy_named(&src, &dest, obj.size)?;
    }
    Ok(())
}

/// Les vieux clients lisent les sons par nom, pas par hash. Une fois les objets téléchargés :
/// - `virtual` (1.6 à 1.7.2, index `legacy`) : copie vers `<assets>/virtual/<index_id>/<nom>` ; renvoie ce dossier,
///   à passer en `${game_assets}` ;
/// - `map_to_resources` (avant 1.6, index `pre-1.6`) : copie vers `<game_dir>/resources/<nom>`.
///
/// Sans ces drapeaux, ne fait rien et renvoie `None`. Idempotent (taille vérifiée), annulable entre les fichiers.
/// Bloquant : à lancer dans `spawn_blocking`.
pub fn materialize_legacy_assets(
    index: &AssetIndex,
    index_id: &str,
    assets_dir: &Path,
    game_dir: &Path,
    cancel: &AtomicBool,
) -> Result<Option<PathBuf>> {
    if index_id.is_empty() || index_id.contains(['/', '\\']) || index_id.contains("..") {
        return Err(Error::Invalid(format!("identifiant d'index {index_id}")));
    }
    let mut virtual_dir = None;
    if index.virtual_ {
        let dir = assets_dir.join("virtual").join(index_id);
        copy_all(index, assets_dir, &dir, cancel)?;
        virtual_dir = Some(dir);
    }
    if index.map_to_resources {
        copy_all(index, assets_dir, &game_dir.join("resources"), cancel)?;
    }
    Ok(virtual_dir)
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
    download_assets_client(&crate::http::client(), index, assets_dir, base_url, progress, options).await
}

/// Cœur de [`download_assets_with`] avec un client fourni (les tests y injectent des délais courts).
async fn download_assets_client<F>(
    client: &reqwest::Client,
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

    #[tokio::test]
    async fn objet_qui_ne_repond_pas_echoue_en_network_apres_les_tentatives() {
        let server = MockServer::start().await;
        let cs = contents(1);
        let h = obj(&cs[0]).0;
        Mock::given(method("GET"))
            .and(path(format!("/{}/{}", &h[..2], h)))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(cs[0].clone()).set_delay(Duration::from_secs(5)))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let client = crate::http::builder_with(Duration::from_secs(5), Duration::from_millis(200)).build().unwrap();
        let o = DownloadOptions { attempts: 2, ..opts() };
        let t = std::time::Instant::now();
        let err = download_assets_client(&client, &index_of(&cs), dir.path(), Some(&server.uri()), |_| {}, &o).await.unwrap_err();
        assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
        assert!(matches!(&err, Error::Network(m) if m.contains("délai dépassé")), "{err:?}");
    }

    fn fake_store(dir: &Path, files: &[(&str, &[u8])]) -> AssetIndex {
        let mut objects = BTreeMap::new();
        for (name, c) in files {
            let (h, o) = obj(c);
            let d = dir.join("objects").join(&h[..2]);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(&h), c).unwrap();
            objects.insert(name.to_string(), o);
        }
        AssetIndex { objects, map_to_resources: false, virtual_: false }
    }

    fn no_cancel() -> AtomicBool {
        AtomicBool::new(false)
    }

    #[test]
    fn virtual_copie_les_objets_par_nom() {
        let dir = tempfile::tempdir().unwrap();
        let (a, g) = (dir.path().join("assets"), dir.path().join("game"));
        let mut idx = fake_store(&a, &[("minecraft/sounds/x.ogg", b"son"), ("alias.ogg", b"son"), ("lang/fr.lang", b"fr")]);
        idx.virtual_ = true;
        let r = materialize_legacy_assets(&idx, "legacy", &a, &g, &no_cancel()).unwrap();
        assert_eq!(r, Some(a.join("virtual/legacy")));
        assert_eq!(std::fs::read(a.join("virtual/legacy/minecraft/sounds/x.ogg")).unwrap(), b"son");
        assert_eq!(std::fs::read(a.join("virtual/legacy/alias.ogg")).unwrap(), b"son");
        assert_eq!(std::fs::read(a.join("virtual/legacy/lang/fr.lang")).unwrap(), b"fr");
        assert!(!g.join("resources").exists());
    }

    #[test]
    fn map_to_resources_copie_dans_le_dossier_de_jeu() {
        let dir = tempfile::tempdir().unwrap();
        let (a, g) = (dir.path().join("assets"), dir.path().join("game"));
        let mut idx = fake_store(&a, &[("newsound/step/grass1.ogg", b"pas")]);
        idx.map_to_resources = true;
        assert_eq!(materialize_legacy_assets(&idx, "pre-1.6", &a, &g, &no_cancel()).unwrap(), None);
        assert_eq!(std::fs::read(g.join("resources/newsound/step/grass1.ogg")).unwrap(), b"pas");
        assert!(!a.join("virtual").exists());
    }

    #[test]
    fn index_moderne_ne_cree_rien() {
        let dir = tempfile::tempdir().unwrap();
        let (a, g) = (dir.path().join("assets"), dir.path().join("game"));
        let idx = fake_store(&a, &[("x.ogg", b"x")]);
        assert_eq!(materialize_legacy_assets(&idx, "17", &a, &g, &no_cancel()).unwrap(), None);
        assert!(!a.join("virtual").exists() && !g.exists());
    }

    #[test]
    fn copie_idempotente_et_repare_une_taille_fausse() {
        let dir = tempfile::tempdir().unwrap();
        let (a, g) = (dir.path().join("assets"), dir.path().join("game"));
        let mut idx = fake_store(&a, &[("a.ogg", b"contenu")]);
        idx.virtual_ = true;
        let d = a.join("virtual/legacy/a.ogg");
        materialize_legacy_assets(&idx, "legacy", &a, &g, &no_cancel()).unwrap();
        // Même taille : conservé tel quel (non réécrit).
        std::fs::write(&d, b"CONTENU").unwrap();
        materialize_legacy_assets(&idx, "legacy", &a, &g, &no_cancel()).unwrap();
        assert_eq!(std::fs::read(&d).unwrap(), b"CONTENU");
        // Taille fausse : réparé, sans reste de .part.
        std::fs::write(&d, b"tronq").unwrap();
        materialize_legacy_assets(&idx, "legacy", &a, &g, &no_cancel()).unwrap();
        assert_eq!(std::fs::read(&d).unwrap(), b"contenu");
        assert!(!part_of(&d).exists());
    }

    #[test]
    fn nom_piege_refuse_sans_rien_ecrire() {
        for bad in ["../evade.ogg", "/abs.ogg", "a\\b.ogg", "c:x", "a//b", "a/../b"] {
            let dir = tempfile::tempdir().unwrap();
            let (a, g) = (dir.path().join("assets"), dir.path().join("game"));
            let mut idx = fake_store(&a, &[(bad, b"x")]);
            idx.virtual_ = true;
            idx.map_to_resources = true;
            let err = materialize_legacy_assets(&idx, "legacy", &a, &g, &no_cancel()).unwrap_err();
            assert!(matches!(err, Error::Invalid(_)), "{bad} : {err:?}");
            assert!(!dir.path().join("evade.ogg").exists());
            assert!(!g.exists());
        }
    }

    #[test]
    fn identifiant_d_index_piege_refuse() {
        let dir = tempfile::tempdir().unwrap();
        let mut idx = fake_store(dir.path(), &[("a", b"x")]);
        idx.virtual_ = true;
        for bad in ["", "../x", "a/b", "a\\b"] {
            let err = materialize_legacy_assets(&idx, bad, dir.path(), dir.path(), &no_cancel()).unwrap_err();
            assert!(matches!(err, Error::Invalid(_)), "{bad}");
        }
    }

    #[test]
    fn objet_manquant_donne_une_erreur_claire() {
        let dir = tempfile::tempdir().unwrap();
        let mut idx = fake_store(dir.path(), &[("a.ogg", b"x")]);
        idx.virtual_ = true;
        let h = &idx.objects["a.ogg"].hash;
        std::fs::remove_file(dir.path().join("objects").join(&h[..2]).join(h)).unwrap();
        let err = materialize_legacy_assets(&idx, "legacy", dir.path(), dir.path(), &no_cancel()).unwrap_err();
        assert!(matches!(err, Error::NotFound(_)), "{err:?}");
    }

    #[test]
    fn annulation_entre_les_fichiers() {
        let dir = tempfile::tempdir().unwrap();
        let mut idx = fake_store(dir.path(), &[("a.ogg", b"x")]);
        idx.virtual_ = true;
        let c = AtomicBool::new(true);
        let err = materialize_legacy_assets(&idx, "legacy", dir.path(), dir.path(), &c).unwrap_err();
        assert!(matches!(err, Error::Cancelled));
    }

    /// Bout en bout sur vrai disque et vrai HTTP (loopback) : index servi, objets téléchargés, puis rangés par nom.
    #[tokio::test]
    async fn telechargement_puis_arborescence_legacy() {
        let server = MockServer::start().await;
        let cs: Vec<Vec<u8>> = vec![b"un".to_vec(), b"deux".to_vec()];
        for c in &cs {
            serve(&server, c).await;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut idx = index_of(&cs);
        idx.virtual_ = true;
        idx.map_to_resources = true;
        download_assets_with(&idx, dir.path(), Some(&server.uri()), |_| {}, &opts()).await.unwrap();
        let g = dir.path().join("game");
        let v = materialize_legacy_assets(&idx, "legacy", dir.path(), &g, &no_cancel()).unwrap().unwrap();
        assert_eq!(std::fs::read(v.join("f1")).unwrap(), b"deux");
        assert_eq!(std::fs::read(g.join("resources/f0")).unwrap(), b"un");
    }
}
