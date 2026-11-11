//! Tâche P2m : téléchargement d'un fichier Modrinth via le magasin adressé par contenu.
//!
//! Flux : alias (sha512/sha1 Modrinth -> sha256) -> cache ; sinon téléchargement en flux vers un
//! temporaire du magasin, une seule passe de hash (sha1 + sha256 + sha512), vérification contre
//! Modrinth, rename atomique vers `object_rel_path(sha256)`, `insert` dans l'index.
//!
//! Limite : `StoreIndex` n'a pas de table d'alias (et on ne le modifie pas) ; les alias sont donc
//! dans `store_root/aliases.json`, écrit atomiquement. Le crate `sha1` n'est pas une dépendance :
//! une implémentation SHA-1 minimale est incluse ici.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use sha2::{Digest, Sha256, Sha512};
use tokio::io::AsyncWriteExt;

use crate::providers::modrinth_api::VersionFile;
use crate::store::index::{object_rel_path, StoreIndex};
use crate::store::link::{link_into, LinkKind};
use crate::types::{Error, Result};

/// Objet présent dans le magasin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredObject {
    pub sha256: String,
    pub rel_path: String,
    pub size: u64,
    pub from_cache: bool,
}

/// Callback de progression : (octets reçus, octets attendus ; 0 si inconnu).
pub type ProgressFn = Arc<dyn Fn(u64, u64) + Send + Sync>;

#[derive(Clone)]
pub struct DlOptions {
    pub attempts: u32,
    pub retry_delay: Duration,
    pub cancel: Arc<AtomicBool>,
    pub progress: Option<ProgressFn>,
}

impl Default for DlOptions {
    fn default() -> Self {
        Self { attempts: 3, retry_delay: Duration::from_millis(500), cancel: Arc::new(AtomicBool::new(false)), progress: None }
    }
}

/// Clients HTTP partagés : réutilisent les connexions (pas de nouveau handshake TLS par fichier).
fn build_client(follow_redirects: bool) -> reqwest::Client {
    let mut b = reqwest::Client::builder().connect_timeout(Duration::from_secs(15)).pool_max_idle_per_host(32);
    if !follow_redirects {
        b = b.redirect(reqwest::redirect::Policy::none());
    }
    b.build().unwrap_or_default()
}

fn shared_client() -> &'static reqwest::Client {
    static C: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    C.get_or_init(|| build_client(true))
}

fn shared_client_no_redirect() -> &'static reqwest::Client {
    static C: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    C.get_or_init(|| build_client(false))
}

fn net(e: impl std::fmt::Display) -> Error {
    Error::Network(e.to_string())
}

// ---------- SHA-1 minimal ----------

struct Sha1 {
    h: [u32; 5],
    buf: Vec<u8>,
    len: u64,
}

impl Sha1 {
    fn new() -> Self {
        Self { h: [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0], buf: Vec::with_capacity(64), len: 0 }
    }

    fn block(h: &mut [u32; 5], b: &[u8]) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([b[4 * i], b[4 * i + 1], b[4 * i + 2], b[4 * i + 3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut bb, mut c, mut d, mut e] = *h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((bb & c) | (!bb & d), 0x5A827999u32),
                20..=39 => (bb ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((bb & c) | (bb & d) | (c & d), 0x8F1BBCDC),
                _ => (bb ^ c ^ d, 0xCA62C1D6),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*wi);
            e = d;
            d = c;
            c = bb.rotate_left(30);
            bb = a;
            a = t;
        }
        for (x, y) in h.iter_mut().zip([a, bb, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }

    fn update(&mut self, data: &[u8]) {
        self.len += data.len() as u64;
        self.buf.extend_from_slice(data);
        let full = self.buf.len() / 64 * 64;
        for chunk in self.buf[..full].chunks(64) {
            Self::block(&mut self.h, chunk);
        }
        self.buf.drain(..full);
    }

    fn finish(mut self) -> String {
        let bits = self.len * 8;
        let mut tail = std::mem::take(&mut self.buf);
        tail.push(0x80);
        while tail.len() % 64 != 56 {
            tail.push(0);
        }
        tail.extend_from_slice(&bits.to_be_bytes());
        for chunk in tail.chunks(64) {
            Self::block(&mut self.h, chunk);
        }
        self.h.iter().map(|x| format!("{x:08x}")).collect()
    }
}

// ---------- Alias ----------

static ALIAS_LOCK: Mutex<()> = Mutex::new(());
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn alias_path(store_root: &Path) -> PathBuf {
    store_root.join("aliases.json")
}

fn read_aliases(store_root: &Path) -> BTreeMap<String, String> {
    std::fs::read(alias_path(store_root)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn add_aliases(store_root: &Path, keys: &[String], sha256: &str) -> Result<()> {
    let _g = ALIAS_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut map = read_aliases(store_root);
    for k in keys {
        map.insert(k.clone(), sha256.to_string());
    }
    let tmp = store_root.join(format!(".aliases.{}-{}.tmp", std::process::id(), TMP_COUNTER.fetch_add(1, Ordering::Relaxed)));
    let json = serde_json::to_vec_pretty(&map).map_err(|e| Error::Other(e.to_string()))?;
    std::fs::write(&tmp, json)?;
    if let Err(e) = std::fs::rename(&tmp, alias_path(store_root)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

/// Clés d'alias, par ordre de préférence (sha512 puis sha1).
fn alias_keys(file: &VersionFile) -> Vec<String> {
    let mut v = Vec::new();
    if !file.hashes.sha512.is_empty() {
        v.push(format!("sha512:{}", file.hashes.sha512.to_ascii_lowercase()));
    }
    if !file.hashes.sha1.is_empty() {
        v.push(format!("sha1:{}", file.hashes.sha1.to_ascii_lowercase()));
    }
    v
}

fn cached(file: &VersionFile, store_root: &Path, index: &StoreIndex) -> Result<Option<StoredObject>> {
    let aliases = read_aliases(store_root);
    for key in alias_keys(file) {
        let Some(sha) = aliases.get(&key) else { continue };
        let Some(entry) = index.get(sha)? else { continue };
        let path = store_root.join(&entry.rel_path);
        let ok = std::fs::metadata(&path).map(|m| m.is_file() && m.len() == entry.size).unwrap_or(false)
            && (file.size == 0 || file.size == entry.size);
        if ok {
            return Ok(Some(StoredObject { sha256: entry.sha256, rel_path: entry.rel_path, size: entry.size, from_cache: true }));
        }
    }
    Ok(None)
}

// ---------- Téléchargement ----------

/// Télécharge `file` dans le magasin (ou le retrouve dans le cache sans réseau).
pub async fn fetch_into_store(file: &VersionFile, store_root: &Path, index: &StoreIndex, opts: &DlOptions) -> Result<StoredObject> {
    if file.hashes.sha512.is_empty() && file.hashes.sha1.is_empty() {
        return Err(Error::Invalid("aucun hash Modrinth à vérifier".into()));
    }
    check_cancel(opts)?;
    if let Some(o) = cached(file, store_root, index)? {
        return Ok(o);
    }
    let tmp_dir = store_root.join(".tmp");
    tokio::fs::create_dir_all(&tmp_dir).await?;
    let client = shared_client().clone();
    let attempts = opts.attempts.max(1);
    let mut last = Error::Network("aucune tentative".into());
    for attempt in 0..attempts {
        check_cancel(opts)?;
        if attempt > 0 {
            tokio::time::sleep(opts.retry_delay).await;
            check_cancel(opts)?;
        }
        let tmp = tmp_dir.join(format!("{}.part", uuid::Uuid::new_v4()));
        let res = download_once(&client, &file.url, file.size, &tmp, opts, None).await;
        match res {
            Ok(d) => {
                let r = finalize(file, store_root, index, &tmp, d);
                let _ = tokio::fs::remove_file(&tmp).await;
                return r;
            }
            Err(e) => {
                let _ = tokio::fs::remove_file(&tmp).await;
                match e {
                    Error::Network(ref m) if !m.starts_with("HTTP 4") => last = e,
                    other => return Err(other),
                }
            }
        }
    }
    Err(last)
}

fn check_cancel(opts: &DlOptions) -> Result<()> {
    if opts.cancel.load(Ordering::SeqCst) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

struct Digests {
    sha1: String,
    sha256: String,
    sha512: String,
    size: u64,
}

async fn download_once(client: &reqwest::Client, url: &str, size_hint: u64, tmp: &Path, opts: &DlOptions, max_bytes: Option<u64>) -> Result<Digests> {
    let resp = client.get(url).send().await.map_err(net)?;
    let status = resp.status();
    if !status.is_success() {
        return Err(Error::Network(format!("HTTP {} pour {}", status.as_u16(), url)));
    }
    if let (Some(max), Some(len)) = (max_bytes, resp.content_length()) {
        if len > max {
            return Err(Error::Invalid(format!("fichier trop gros ({len} octets, maximum {max})")));
        }
    }
    let total = if size_hint > 0 { size_hint } else { resp.content_length().unwrap_or(0) };
    let mut out = tokio::fs::File::create(tmp).await?;
    let (mut s1, mut s256, mut s512) = (Sha1::new(), Sha256::new(), Sha512::new());
    let mut size = 0u64;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        check_cancel(opts)?;
        let chunk = chunk.map_err(net)?;
        s1.update(&chunk);
        s256.update(&chunk);
        s512.update(&chunk);
        out.write_all(&chunk).await?;
        size += chunk.len() as u64;
        if max_bytes.is_some_and(|m| size > m) {
            return Err(Error::Invalid(format!("fichier trop gros (plus de {} octets)", max_bytes.unwrap_or(0))));
        }
        if let Some(p) = &opts.progress {
            p(size, total);
        }
    }
    out.flush().await?;
    drop(out);
    check_cancel(opts)?;
    Ok(Digests { sha1: s1.finish(), sha256: hex::encode(s256.finalize()), sha512: hex::encode(s512.finalize()), size })
}

fn finalize(file: &VersionFile, store_root: &Path, index: &StoreIndex, tmp: &Path, d: Digests) -> Result<StoredObject> {
    if file.size > 0 && file.size != d.size {
        return Err(Error::Integrity { expected: format!("{} octets", file.size), actual: format!("{} octets", d.size) });
    }
    let (exp, act) = if !file.hashes.sha512.is_empty() {
        (file.hashes.sha512.to_ascii_lowercase(), d.sha512.clone())
    } else {
        (file.hashes.sha1.to_ascii_lowercase(), d.sha1.clone())
    };
    if exp != act {
        return Err(Error::Integrity { expected: exp, actual: act });
    }
    let rel = object_rel_path(&d.sha256)?;
    let dest = store_root.join(&rel);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Téléchargement concurrent du même hash : si la destination existe déjà, c'est le même contenu.
    if !dest.exists() {
        if let Err(e) = std::fs::rename(tmp, &dest) {
            if !dest.exists() {
                return Err(e.into());
            }
        }
    }
    index.insert(&d.sha256, &rel, d.size)?;
    add_aliases(store_root, &alias_keys(file), &d.sha256)?;
    Ok(StoredObject { sha256: d.sha256, rel_path: rel, size: d.size, from_cache: false })
}

/// Télécharge `url` SANS valeur de hash attendue (ex. CurseForge sans clé API) : calcule les
/// empreintes pendant le flux et place l'objet dans le magasin sous son sha256. Aucune
/// vérification d'intégrité contre une référence : l'appelant doit avoir validé l'URL (HTTPS, hôte).
/// Le client ne suit aucune redirection. `max_bytes` borne la taille du fichier.
pub async fn fetch_into_store_unverified(url: &str, max_bytes: u64, store_root: &Path, index: &StoreIndex, opts: &DlOptions) -> Result<StoredObject> {
    check_cancel(opts)?;
    let tmp_dir = store_root.join(".tmp");
    tokio::fs::create_dir_all(&tmp_dir).await?;
    let client = shared_client_no_redirect().clone();
    let attempts = opts.attempts.max(1);
    let mut last = Error::Network("aucune tentative".into());
    for attempt in 0..attempts {
        check_cancel(opts)?;
        if attempt > 0 {
            tokio::time::sleep(opts.retry_delay).await;
            check_cancel(opts)?;
        }
        let tmp = tmp_dir.join(format!("{}.part", uuid::Uuid::new_v4()));
        let res = download_once(&client, url, 0, &tmp, opts, Some(max_bytes)).await;
        match res {
            Ok(d) => {
                let r = place_object(store_root, index, &tmp, &d);
                let _ = tokio::fs::remove_file(&tmp).await;
                return r;
            }
            Err(e) => {
                let _ = tokio::fs::remove_file(&tmp).await;
                match e {
                    Error::Network(ref m) if !m.starts_with("HTTP 4") => last = e,
                    other => return Err(other),
                }
            }
        }
    }
    Err(last)
}

/// Rename atomique vers `object_rel_path(sha256)` + inscription dans l'index (sans alias).
fn place_object(store_root: &Path, index: &StoreIndex, tmp: &Path, d: &Digests) -> Result<StoredObject> {
    let rel = object_rel_path(&d.sha256)?;
    let dest = store_root.join(&rel);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if !dest.exists() {
        if let Err(e) = std::fs::rename(tmp, &dest) {
            if !dest.exists() {
                return Err(e.into());
            }
        }
    }
    index.insert(&d.sha256, &rel, d.size)?;
    Ok(StoredObject { sha256: d.sha256.clone(), rel_path: rel, size: d.size, from_cache: false })
}

// ---------- Installation ----------

fn validate_filename(name: &str) -> Result<()> {
    let bad = name.is_empty()
        || name == "."
        || name.contains("..")
        || name.contains(['/', '\\', ':', '\0'])
        || name.chars().any(|c| c.is_control());
    if bad {
        Err(Error::Invalid(format!("nom de fichier invalide : {name:?}")))
    } else {
        Ok(())
    }
}

/// Place l'objet dans `instance_game_dir/mods/<filename>` (hardlink, sinon copie) et incrémente
/// son compteur de références.
pub fn install_to_instance(
    object: &StoredObject,
    store_root: &Path,
    index: &StoreIndex,
    instance_game_dir: &Path,
    filename: &str,
) -> Result<LinkKind> {
    validate_filename(filename)?;
    let src = store_root.join(&object.rel_path);
    let kind = link_into(instance_game_dir, &format!("mods/{filename}"), &src)?;
    index.add_ref(&object.sha256)?;
    Ok(kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::modrinth_api::FileHashes;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn sha1_of(b: &[u8]) -> String {
        let mut s = Sha1::new();
        s.update(b);
        s.finish()
    }

    fn file_for(server: &MockServer, content: &[u8]) -> VersionFile {
        VersionFile {
            url: format!("{}/f.jar", server.uri()),
            filename: "f.jar".into(),
            primary: true,
            size: content.len() as u64,
            hashes: FileHashes { sha1: sha1_of(content), sha512: hex::encode(Sha512::digest(content)) },
        }
    }

    fn opts() -> DlOptions {
        DlOptions { retry_delay: Duration::from_millis(5), ..Default::default() }
    }

    fn setup() -> (tempfile::TempDir, StoreIndex) {
        let dir = tempfile::tempdir().unwrap();
        let idx = StoreIndex::open(dir.path().join("index.db")).unwrap();
        (dir, idx)
    }

    /// Nombre d'objets (fichiers nommés par un hash de 64 caractères) dans le magasin.
    fn cache_files(root: &Path) -> usize {
        fn walk(p: &Path, n: &mut usize) {
            for e in std::fs::read_dir(p).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, n);
                } else if p.file_name().is_some_and(|f| f.len() == 64) {
                    *n += 1;
                }
            }
        }
        let mut n = 0;
        walk(root, &mut n);
        n
    }

    #[test]
    fn sha1_known_vectors() {
        assert_eq!(sha1_of(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(sha1_of(b""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        let mut s = Sha1::new();
        for _ in 0..1000 {
            s.update(b"a");
        }
        assert_eq!(s.finish(), "291e9a6c66994949b57ba5e650361e98fc36b1ba");
    }

    #[tokio::test]
    async fn nominal_then_cache_without_request() {
        let server = MockServer::start().await;
        let content = b"contenu du mod".to_vec();
        Mock::given(method("GET")).and(path("/f.jar")).respond_with(ResponseTemplate::new(200).set_body_bytes(content.clone())).expect(1).mount(&server).await;
        let (dir, idx) = setup();
        let f = file_for(&server, &content);
        let seen = Arc::new(AtomicU64::new(0));
        let s2 = seen.clone();
        let o = DlOptions { progress: Some(Arc::new(move |d, _| s2.store(d, Ordering::SeqCst))), ..opts() };
        let a = fetch_into_store(&f, dir.path(), &idx, &o).await.unwrap();
        assert!(!a.from_cache);
        assert_eq!(a.size, content.len() as u64);
        assert_eq!(seen.load(Ordering::SeqCst), content.len() as u64);
        assert_eq!(a.sha256, hex::encode(Sha256::digest(&content)));
        assert_eq!(std::fs::read(dir.path().join(&a.rel_path)).unwrap(), content);
        assert!(idx.contains(&a.sha256).unwrap());
        let b = fetch_into_store(&f, dir.path(), &idx, &opts()).await.unwrap();
        assert!(b.from_cache);
        assert_eq!(a.sha256, b.sha256);
    }

    #[tokio::test]
    async fn tampered_file_rejected_and_not_cached() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/f.jar")).respond_with(ResponseTemplate::new(200).set_body_bytes(b"altere!".to_vec())).mount(&server).await;
        let (dir, idx) = setup();
        let f = file_for(&server, b"origin!");
        let err = fetch_into_store(&f, dir.path(), &idx, &opts()).await.unwrap_err();
        assert!(matches!(err, Error::Integrity { .. }), "{err:?}");
        assert_eq!(cache_files(dir.path()), 0);
        assert_eq!(idx.total_size().unwrap(), 0);
        assert_eq!(std::fs::read_dir(dir.path().join(".tmp")).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn wrong_size_rejected() {
        let server = MockServer::start().await;
        let content = b"12345678".to_vec();
        Mock::given(method("GET")).and(path("/f.jar")).respond_with(ResponseTemplate::new(200).set_body_bytes(content.clone())).mount(&server).await;
        let (dir, idx) = setup();
        let mut f = file_for(&server, &content);
        f.size = 99;
        let err = fetch_into_store(&f, dir.path(), &idx, &opts()).await.unwrap_err();
        assert!(matches!(err, Error::Integrity { .. }), "{err:?}");
        assert_eq!(cache_files(dir.path()), 0);
    }

    #[tokio::test]
    async fn sha1_only_is_verified() {
        let server = MockServer::start().await;
        let content = b"sha1 only".to_vec();
        Mock::given(method("GET")).and(path("/f.jar")).respond_with(ResponseTemplate::new(200).set_body_bytes(content.clone())).mount(&server).await;
        let (dir, idx) = setup();
        let mut f = file_for(&server, &content);
        f.hashes.sha512.clear();
        assert!(fetch_into_store(&f, dir.path(), &idx, &opts()).await.is_ok());
        f.hashes.sha1 = "0".repeat(40);
        let (dir2, idx2) = setup();
        assert!(matches!(fetch_into_store(&f, dir2.path(), &idx2, &opts()).await, Err(Error::Integrity { .. })));
    }

    #[tokio::test]
    async fn retry_after_500() {
        let server = MockServer::start().await;
        let content = b"apres 500".to_vec();
        Mock::given(method("GET")).and(path("/f.jar")).respond_with(ResponseTemplate::new(500)).up_to_n_times(1).expect(1).mount(&server).await;
        Mock::given(method("GET")).and(path("/f.jar")).respond_with(ResponseTemplate::new(200).set_body_bytes(content.clone())).expect(1).mount(&server).await;
        let (dir, idx) = setup();
        let o = fetch_into_store(&file_for(&server, &content), dir.path(), &idx, &opts()).await.unwrap();
        assert!(!o.from_cache);
    }

    #[tokio::test]
    async fn persistent_500_fails_after_three_attempts() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/f.jar")).respond_with(ResponseTemplate::new(500)).expect(3).mount(&server).await;
        let (dir, idx) = setup();
        let err = fetch_into_store(&file_for(&server, b"x"), dir.path(), &idx, &opts()).await.unwrap_err();
        assert!(matches!(err, Error::Network(_)));
    }

    #[tokio::test]
    async fn cancellation() {
        let server = MockServer::start().await;
        let content = b"annule".to_vec();
        Mock::given(method("GET")).and(path("/f.jar")).respond_with(ResponseTemplate::new(200).set_body_bytes(content.clone())).mount(&server).await;
        let (dir, idx) = setup();
        let f = file_for(&server, &content);
        let o = opts();
        o.cancel.store(true, Ordering::SeqCst);
        assert!(matches!(fetch_into_store(&f, dir.path(), &idx, &o).await, Err(Error::Cancelled)));
        // Annulation déclenchée depuis le callback de progression.
        let cancel = Arc::new(AtomicBool::new(false));
        let c2 = cancel.clone();
        let o = DlOptions { cancel, progress: Some(Arc::new(move |_, _| c2.store(true, Ordering::SeqCst))), ..opts() };
        assert!(matches!(fetch_into_store(&f, dir.path(), &idx, &o).await, Err(Error::Cancelled)));
        assert_eq!(cache_files(dir.path()), 0);
    }

    #[tokio::test]
    async fn install_into_instance_and_bad_filenames() {
        let server = MockServer::start().await;
        let content = b"mod installe".to_vec();
        Mock::given(method("GET")).and(path("/f.jar")).respond_with(ResponseTemplate::new(200).set_body_bytes(content.clone())).mount(&server).await;
        let (dir, idx) = setup();
        let o = fetch_into_store(&file_for(&server, &content), dir.path(), &idx, &opts()).await.unwrap();
        let game = tempfile::tempdir().unwrap();
        let kind = install_to_instance(&o, dir.path(), &idx, game.path(), "f.jar").unwrap();
        assert!(matches!(kind, LinkKind::HardLink | LinkKind::Copy));
        assert_eq!(std::fs::read(game.path().join("mods/f.jar")).unwrap(), content);
        assert_eq!(idx.get(&o.sha256).unwrap().unwrap().refs, 1);
        for bad in ["../x.jar", "a/b.jar", "a\\b.jar", "..", "", "C:evil.jar", "x\0.jar"] {
            assert!(matches!(install_to_instance(&o, dir.path(), &idx, game.path(), bad), Err(Error::Invalid(_))), "{bad:?}");
        }
        assert_eq!(idx.get(&o.sha256).unwrap().unwrap().refs, 1);
    }

    #[tokio::test]
    async fn concurrent_same_hash_is_idempotent() {
        let server = MockServer::start().await;
        let content = b"concurrent".to_vec();
        Mock::given(method("GET")).and(path("/f.jar")).respond_with(ResponseTemplate::new(200).set_body_bytes(content.clone())).mount(&server).await;
        let (dir, idx) = setup();
        let f = file_for(&server, &content);
        let o = opts();
        let (a, b) = tokio::join!(fetch_into_store(&f, dir.path(), &idx, &o), fetch_into_store(&f, dir.path(), &idx, &o));
        assert_eq!(a.unwrap().sha256, b.unwrap().sha256);
        assert_eq!(cache_files(dir.path()), 1);
    }

    #[tokio::test]
    async fn unverified_download_stores_by_sha256_and_caps_size() {
        let server = MockServer::start().await;
        let content = b"sans hash".to_vec();
        Mock::given(method("GET")).and(path("/f.jar")).respond_with(ResponseTemplate::new(200).set_body_bytes(content.clone())).mount(&server).await;
        let (dir, idx) = setup();
        let url = format!("{}/f.jar", server.uri());
        let o = fetch_into_store_unverified(&url, 1024, dir.path(), &idx, &opts()).await.unwrap();
        assert_eq!(o.sha256, hex::encode(Sha256::digest(&content)));
        assert_eq!(std::fs::read(dir.path().join(&o.rel_path)).unwrap(), content);
        assert!(idx.contains(&o.sha256).unwrap());
        let (dir2, idx2) = setup();
        let err = fetch_into_store_unverified(&url, 3, dir2.path(), &idx2, &opts()).await.unwrap_err();
        assert!(matches!(err, Error::Invalid(_)), "{err:?}");
        assert_eq!(cache_files(dir2.path()), 0);
    }
}
