//! N16 : repérage des mods absents des catalogues (Modrinth), par sha1 des jars de `mods/`.
//!
//! Le manifeste ne contient que des sha256 : les sha1 sont calculés en flux sur le disque.
//! « Inconnu » = Modrinth ne reconnaît pas le fichier ; « non vérifié » = on n'a pas pu savoir.
//! Aucune analyse de sécurité n'est faite ici.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

use crate::providers::modrinth_api::{ModrinthClient, Version};
use crate::types::{Error, ManifestFile, Result};

const BATCH_SIZE: usize = 100;
/// Nombre maximal de lots interrogés (au-delà : `unchecked`).
const MAX_BATCHES: usize = 50;
const MAX_RETRIES_429: u32 = 2;
const MAX_PAUSE_SECS: u64 = 30;
const DEFAULT_PAUSE_SECS: u64 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownMod {
    pub path: String,
    pub project_id: String,
    pub version_id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnknownReport {
    pub known: Vec<KnownMod>,
    /// Chemins que Modrinth ne reconnaît pas.
    pub unknown: Vec<String>,
    /// Chemins non vérifiés (réseau, 429, erreur, fichier illisible).
    pub unchecked: Vec<String>,
}

/// Fonction pure : sépare connus / inconnus selon la réponse (sha1 -> version).
pub fn classify(
    files_with_hashes: &[(String, String)],
    response_map: &HashMap<String, Version>,
) -> (Vec<KnownMod>, Vec<String>) {
    let lookup: HashMap<String, &Version> =
        response_map.iter().map(|(k, v)| (k.to_ascii_lowercase(), v)).collect();
    let mut known = Vec::new();
    let mut unknown = Vec::new();
    for (path, sha1) in files_with_hashes {
        match lookup.get(&sha1.to_ascii_lowercase()) {
            Some(v) => known.push(KnownMod {
                path: path.clone(),
                project_id: v.project_id.clone(),
                version_id: v.id.clone(),
            }),
            None => unknown.push(path.clone()),
        }
    }
    (known, unknown)
}

fn is_mod_jar(path: &str) -> bool {
    path.starts_with("mods/")
        && path.to_ascii_lowercase().ends_with(".jar")
        && !path.split('/').any(|c| c == ".." || c.is_empty())
}

/// Analyse les jars `mods/*.jar` du manifeste présents dans `game_dir`.
pub async fn find_unknown(
    manifest_files: &[ManifestFile],
    game_dir: &Path,
    client: &ModrinthClient,
) -> Result<UnknownReport> {
    let mut report = UnknownReport::default();
    let mut hashed: Vec<(String, String)> = Vec::new();
    for f in manifest_files.iter().filter(|f| is_mod_jar(&f.path)) {
        let full = game_dir.join(&f.path);
        match tokio::task::spawn_blocking(move || sha1_file(&full)).await {
            Ok(Ok(h)) => hashed.push((f.path.clone(), h)),
            _ => report.unchecked.push(f.path.clone()),
        }
    }
    query_hashed(&hashed, client, &mut report).await;
    Ok(report)
}

/// Analyse AVANT téléchargement à partir des sha1 annoncés par le manifeste (aucun accès disque).
/// Les jars sans sha1 valide sont `unchecked` : jamais présumés connus ni sûrs. Le sha1 est une
/// revendication de l'hôte, vérifiée à la réception (cf. `verify_sha1_file`).
pub async fn find_unknown_from_manifest(manifest_files: &[ManifestFile], client: &ModrinthClient) -> UnknownReport {
    let mut report = UnknownReport::default();
    let mut hashed: Vec<(String, String)> = Vec::new();
    for f in manifest_files.iter().filter(|f| is_mod_jar(&f.path)) {
        match f.sha1.as_deref() {
            Some(h) if h.len() == 40 && h.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f')) => {
                hashed.push((f.path.clone(), h.to_string()));
            }
            _ => report.unchecked.push(f.path.clone()),
        }
    }
    query_hashed(&hashed, client, &mut report).await;
    report
}

async fn query_hashed(hashed: &[(String, String)], client: &ModrinthClient, report: &mut UnknownReport) {
    for (i, batch) in hashed.chunks(BATCH_SIZE).enumerate() {
        if i >= MAX_BATCHES {
            report.unchecked.extend(batch.iter().map(|(p, _)| p.clone()));
            continue;
        }
        let hashes: Vec<String> = batch.iter().map(|(_, h)| h.clone()).collect();
        match query_with_retry(client, &hashes).await {
            Ok(map) => {
                let (k, u) = classify(batch, &map);
                report.known.extend(k);
                report.unknown.extend(u);
            }
            Err(_) => report.unchecked.extend(batch.iter().map(|(p, _)| p.clone())),
        }
    }
}

async fn query_with_retry(client: &ModrinthClient, hashes: &[String]) -> Result<HashMap<String, Version>> {
    let mut attempt = 0;
    loop {
        match client.versions_from_hashes(hashes).await {
            Err(Error::Network(msg)) if msg.contains("429") && attempt < MAX_RETRIES_429 => {
                attempt += 1;
                tokio::time::sleep(Duration::from_secs(pause_secs(&msg))).await;
            }
            other => return other,
        }
    }
}

/// Extrait « réessayer dans N s » du message du client ; borné.
fn pause_secs(msg: &str) -> u64 {
    msg.split("dans ")
        .nth(1)
        .and_then(|r| r.split_whitespace().next())
        .and_then(|n| n.parse::<u64>().ok())
        .unwrap_or(DEFAULT_PAUSE_SECS)
        .min(MAX_PAUSE_SECS)
}

/// Message utilisateur prêt à l'emploi (français).
pub fn describe(report: &UnknownReport) -> String {
    let mut out = String::new();
    if !report.unknown.is_empty() {
        out.push_str(&format!(
            "Modrinth ne reconnaît pas ces fichiers ({}) : intégrité vérifiée ≠ sécurité garantie.\n",
            report.unknown.len()
        ));
        for p in &report.unknown {
            out.push_str(&format!("  - {p}\n"));
        }
        out.push_str(
            "Cela ne signifie pas qu'ils sont dangereux (mod maison, autre catalogue), ni qu'ils sont sûrs : aucune analyse de sécurité n'a été faite. N'installez que des fichiers dont vous connaissez la source.\n",
        );
    }
    if !report.unchecked.is_empty() {
        out.push_str(&format!(
            "{} fichier(s) n'ont pas pu être vérifiés (réseau, limite de débit ou fichier illisible) : ils ne sont ni reconnus ni signalés inconnus.\n",
            report.unchecked.len()
        ));
        for p in &report.unchecked {
            out.push_str(&format!("  - {p}\n"));
        }
    }
    if out.is_empty() {
        out.push_str(&format!(
            "Modrinth reconnaît tous les mods vérifiés ({}). Intégrité vérifiée ≠ sécurité garantie.\n",
            report.known.len()
        ));
    }
    out
}

// --- SHA-1 en flux (RFC 3174), uniquement pour interroger Modrinth ---

pub(crate) struct Sha1 {
    h: [u32; 5],
    buf: Vec<u8>,
    len: u64,
}

impl Sha1 {
    pub(crate) fn new() -> Self {
        Self { h: [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0], buf: Vec::with_capacity(128), len: 0 }
    }

    fn block(h: &mut [u32; 5], chunk: &[u8; 64]) {
        let mut w = [0u32; 80];
        for (i, b) in chunk.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*b);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = *h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6u32),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }

    pub(crate) fn update(&mut self, data: &[u8]) {
        self.len += data.len() as u64;
        self.buf.extend_from_slice(data);
        let full = self.buf.len() / 64 * 64;
        for chunk in self.buf[..full].as_chunks::<64>().0 {
            Self::block(&mut self.h, chunk);
        }
        self.buf.drain(..full);
    }

    pub(crate) fn finish(mut self) -> String {
        let bits = self.len * 8;
        self.buf.push(0x80);
        while self.buf.len() % 64 != 56 {
            self.buf.push(0);
        }
        self.buf.extend_from_slice(&bits.to_be_bytes());
        for chunk in self.buf.as_chunks::<64>().0 {
            Self::block(&mut self.h, chunk);
        }
        self.h.iter().map(|x| format!("{x:08x}")).collect()
    }
}

/// SHA-1 (hex minuscule) d'un tampon en mémoire.
pub fn sha1_hex(data: &[u8]) -> String {
    let mut s = Sha1::new();
    s.update(data);
    s.finish()
}

/// Vérifie en flux que le SHA-1 du fichier est `expected` (40 hex minuscules) ; sinon `Error::Integrity`.
/// Sert à contrôler la revendication `ManifestFile::sha1` d'un hôte non fiable.
pub fn verify_sha1_file(path: &Path, expected: &str) -> Result<()> {
    let actual = sha1_file(path)?;
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(Error::Integrity {
            expected: format!("sha1 {expected} annoncé par l'hôte"),
            actual: format!("sha1 {actual}"),
        })
    }
}

pub(crate) fn sha1_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut s = Sha1::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        s.update(&buf[..n]);
    }
    Ok(s.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client(s: &MockServer) -> ModrinthClient {
        ModrinthClient::with_base_url(s.uri(), "cubelink/0.1 (test)")
    }

    fn mf(p: &str) -> ManifestFile {
        ManifestFile { path: p.into(), size: 1, sha256: "0".repeat(64), sha1: None }
    }

    #[tokio::test]
    async fn from_manifest_ninety_known_one_homemade_and_no_sha1_unchecked() {
        let s = MockServer::start().await;
        let hashes: Vec<String> = (0..91).map(|i| format!("{i:040x}")).collect();
        let mut files: Vec<ManifestFile> = hashes
            .iter()
            .enumerate()
            .map(|(i, h)| ManifestFile { sha1: Some(h.clone()), ..mf(&format!("mods/m{i}.jar")) })
            .collect();
        files.push(mf("mods/nosha1.jar"));
        files.push(mf("config/x.toml"));
        Mock::given(method("POST"))
            .and(path("/version_files"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body_for(&hashes[..90])))
            .mount(&s)
            .await;
        let r = find_unknown_from_manifest(&files, &client(&s)).await;
        assert_eq!(r.known.len(), 90);
        assert_eq!(r.unknown, vec!["mods/m90.jar".to_string()]);
        assert_eq!(r.unchecked, vec!["mods/nosha1.jar".to_string()]);
    }

    #[tokio::test]
    async fn from_manifest_network_failure_is_unchecked() {
        let s = MockServer::start().await;
        Mock::given(method("POST")).and(path("/version_files")).respond_with(ResponseTemplate::new(500)).mount(&s).await;
        let f = ManifestFile { sha1: Some("a".repeat(40)), ..mf("mods/a.jar") };
        let r = find_unknown_from_manifest(&[f], &client(&s)).await;
        assert_eq!(r.unchecked, vec!["mods/a.jar".to_string()]);
        assert!(r.unknown.is_empty());
    }

    #[test]
    fn verify_sha1_file_detects_mismatch() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a");
        std::fs::write(&p, b"abc").unwrap();
        verify_sha1_file(&p, "a9993e364706816aba3e25717850c26c9cd0d89d").unwrap();
        assert!(matches!(verify_sha1_file(&p, &"0".repeat(40)), Err(Error::Integrity { .. })));
    }

    fn version(id: &str) -> serde_json::Value {
        json!({"id": id, "project_id": format!("p-{id}"), "version_number": "1", "game_versions": [],
               "loaders": [], "files": [], "dependencies": []})
    }

    /// Écrit n jars et renvoie (manifeste, sha1 de chacun).
    fn make_jars(dir: &Path, n: usize) -> (Vec<ManifestFile>, Vec<String>) {
        std::fs::create_dir_all(dir.join("mods")).unwrap();
        let mut files = Vec::new();
        let mut hashes = Vec::new();
        for i in 0..n {
            let rel = format!("mods/m{i}.jar");
            std::fs::write(dir.join(&rel), format!("contenu {i}")).unwrap();
            hashes.push(sha1_file(&dir.join(&rel)).unwrap());
            files.push(mf(&rel));
        }
        (files, hashes)
    }

    fn body_for(hashes: &[String]) -> serde_json::Map<String, serde_json::Value> {
        hashes.iter().enumerate().map(|(i, h)| (h.clone(), version(&format!("v{i}")))).collect()
    }

    #[test]
    fn sha1_streaming_vectors() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a");
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(sha1_file(&p).unwrap(), "a9993e364706816aba3e25717850c26c9cd0d89d");
        std::fs::write(&p, vec![b'a'; 1_000_000]).unwrap();
        assert_eq!(sha1_file(&p).unwrap(), "34aa973cd4c4daa4f61eeb2bdbad27316534016f");
        std::fs::write(&p, b"").unwrap();
        assert_eq!(sha1_file(&p).unwrap(), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
    }

    #[test]
    fn classify_pure() {
        let v: Version = serde_json::from_value(version("v1")).unwrap();
        let map = HashMap::from([("AA".to_string(), v)]);
        let files = vec![("mods/a.jar".to_string(), "aa".to_string()), ("mods/b.jar".to_string(), "bb".to_string())];
        let (k, u) = classify(&files, &map);
        assert_eq!(k, vec![KnownMod { path: "mods/a.jar".into(), project_id: "p-v1".into(), version_id: "v1".into() }]);
        assert_eq!(u, vec!["mods/b.jar".to_string()]);
    }

    #[tokio::test]
    async fn ninety_known_one_homemade() {
        let s = MockServer::start().await;
        let d = tempfile::tempdir().unwrap();
        let (files, hashes) = make_jars(d.path(), 91);
        Mock::given(method("POST"))
            .and(path("/version_files"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body_for(&hashes[..90])))
            .mount(&s)
            .await;
        let r = find_unknown(&files, d.path(), &client(&s)).await.unwrap();
        assert_eq!(r.known.len(), 90);
        assert_eq!(r.unknown, vec!["mods/m90.jar".to_string()]);
        assert!(r.unchecked.is_empty());
        assert!(describe(&r).contains("intégrité vérifiée ≠ sécurité garantie"));
    }

    #[tokio::test]
    async fn partial_batch_error_is_unchecked() {
        let s = MockServer::start().await;
        let d = tempfile::tempdir().unwrap();
        let (files, hashes) = make_jars(d.path(), 150);
        Mock::given(method("POST"))
            .and(path("/version_files"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body_for(&hashes[..100])))
            .up_to_n_times(1)
            .mount(&s)
            .await;
        Mock::given(method("POST")).and(path("/version_files")).respond_with(ResponseTemplate::new(500)).mount(&s).await;
        let r = find_unknown(&files, d.path(), &client(&s)).await.unwrap();
        assert_eq!(r.known.len(), 100);
        assert_eq!(r.unchecked.len(), 50);
        assert!(r.unknown.is_empty());
    }

    #[tokio::test]
    async fn rate_limit_retries_then_succeeds() {
        let s = MockServer::start().await;
        let d = tempfile::tempdir().unwrap();
        let (files, hashes) = make_jars(d.path(), 1);
        Mock::given(method("POST"))
            .and(path("/version_files"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
            .up_to_n_times(1)
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/version_files"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body_for(&hashes)))
            .mount(&s)
            .await;
        let r = find_unknown(&files, d.path(), &client(&s)).await.unwrap();
        assert_eq!(r.known.len(), 1);
    }

    #[tokio::test]
    async fn rate_limit_persistent_is_unchecked() {
        let s = MockServer::start().await;
        let d = tempfile::tempdir().unwrap();
        let (files, _) = make_jars(d.path(), 2);
        Mock::given(method("POST"))
            .and(path("/version_files"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
            .mount(&s)
            .await;
        let r = find_unknown(&files, d.path(), &client(&s)).await.unwrap();
        assert_eq!(r.unchecked.len(), 2);
        assert!(r.unknown.is_empty() && r.known.is_empty());
    }

    #[tokio::test]
    async fn missing_jar_and_non_mods_ignored() {
        let s = MockServer::start().await;
        let d = tempfile::tempdir().unwrap();
        let files = vec![mf("mods/absent.jar"), mf("config/x.toml"), mf("resourcepacks/p.zip"), mf("mods/readme.txt")];
        let r = find_unknown(&files, d.path(), &client(&s)).await.unwrap();
        assert_eq!(r.unchecked, vec!["mods/absent.jar".to_string()]);
        assert!(r.known.is_empty() && r.unknown.is_empty());
    }

    #[tokio::test]
    async fn empty_manifest() {
        let s = MockServer::start().await;
        let d = tempfile::tempdir().unwrap();
        let r = find_unknown(&[], d.path(), &client(&s)).await.unwrap();
        assert_eq!(r, UnknownReport::default());
    }

    #[tokio::test]
    async fn multiple_batches() {
        let s = MockServer::start().await;
        let d = tempfile::tempdir().unwrap();
        let (files, hashes) = make_jars(d.path(), 205);
        Mock::given(method("POST"))
            .and(path("/version_files"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body_for(&hashes)))
            .expect(3)
            .mount(&s)
            .await;
        let r = find_unknown(&files, d.path(), &client(&s)).await.unwrap();
        assert_eq!(r.known.len(), 205);
    }

    #[test]
    fn pause_is_bounded() {
        assert_eq!(pause_secs("Modrinth limite le débit (HTTP 429) : réessayer dans 17 s"), 17);
        assert_eq!(pause_secs("réessayer dans 9999 s"), MAX_PAUSE_SECS);
        assert_eq!(pause_secs("délai d'attente inconnu"), DEFAULT_PAUSE_SECS);
    }
}
