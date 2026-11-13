//! N16 : repérage des mods absents des catalogues (Modrinth par sha1, puis CurseForge par empreinte murmur2).
//!
//! « Inconnu » = ni Modrinth ni CurseForge ne reconnaissent le fichier ; « non vérifié » = on n'a pas pu savoir.
//! CurseForge n'est interrogé que pour ce que Modrinth ne reconnaît pas, et seulement avec la clé de Cubelink.
//! Aucune analyse de sécurité n'est faite ici.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;
use std::time::Duration;

use crate::providers::curseforge_api::{murmur2_fingerprint, CfApi};
use crate::providers::modrinth_api::{ModrinthClient, Version};
use crate::types::{Error, ManifestFile, Result};

const BATCH_SIZE: usize = 100;
/// Nombre maximal de lots interrogés (au-delà : `unchecked`).
const MAX_BATCHES: usize = 50;
const MAX_RETRIES_429: u32 = 2;
const MAX_PAUSE_SECS: u64 = 30;
const DEFAULT_PAUSE_SECS: u64 = 2;
/// Empreintes CurseForge par requête.
const CF_BATCH: usize = 1000;
/// Au-delà, pas d'empreinte CurseForge (le calcul lit le fichier en mémoire).
pub const MAX_CF_FINGERPRINT_BYTES: u64 = 256 * 1024 * 1024;

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
    cf: &CfApi,
) -> Result<UnknownReport> {
    let mut report = UnknownReport::default();
    let mut hashed: Vec<(String, String)> = Vec::new();
    let mut fps: Vec<CfCandidate> = Vec::new();
    for f in manifest_files.iter().filter(|f| is_mod_jar(&f.path)) {
        let full = game_dir.join(&f.path);
        match tokio::task::spawn_blocking(move || jar_hashes(&full)).await {
            Ok(Ok((h, fp))) => {
                fps.extend(fp.map(|fp| (f.path.clone(), fp, h.clone())));
                hashed.push((f.path.clone(), h));
            }
            _ => report.unchecked.push(f.path.clone()),
        }
    }
    query_hashed(&hashed, client, &mut report).await;
    query_curseforge(&fps, cf, &mut report).await;
    Ok(report)
}

/// SHA-1 et empreinte CurseForge (si la taille le permet) d'un jar sur le disque.
fn jar_hashes(path: &Path) -> std::io::Result<(String, Option<u32>)> {
    if std::fs::metadata(path)?.len() > MAX_CF_FINGERPRINT_BYTES {
        return Ok((sha1_file(path)?, None));
    }
    let bytes = std::fs::read(path)?;
    Ok((sha1_hex(&bytes), Some(murmur2_fingerprint(&bytes))))
}

/// (chemin, empreinte CurseForge, sha1 du jar).
type CfCandidate = (String, u32, String);

/// Second catalogue : ce que Modrinth ne reconnaît pas (ou n'a pas pu vérifier) est cherché sur CurseForge.
///
/// L'empreinte murmur2 (32 bits, non cryptographique, forgeable) ne sert qu'à TROUVER le fichier : il n'est
/// reconnu que si le sha1 que CurseForge publie pour ce fichier est celui du jar (sha1 calculé sur le disque, ou
/// annoncé par le manifeste et revérifié à la réception). Sans clé, ou si CurseForge ne répond pas, rien ne
/// change : un inconnu reste inconnu (jamais rétrogradé en « non vérifié », qui ne demande pas de confiance).
async fn query_curseforge(fps: &[CfCandidate], cf: &CfApi, report: &mut UnknownReport) {
    if !cf.has_key() {
        return;
    }
    let pending: HashSet<&str> = report.unknown.iter().chain(&report.unchecked).map(String::as_str).collect();
    let cand: Vec<&CfCandidate> = fps.iter().filter(|(p, _, _)| pending.contains(p.as_str())).collect();
    let mut list: Vec<u32> = cand.iter().map(|(_, fp, _)| *fp).collect();
    list.sort_unstable();
    list.dedup();
    // Empreinte -> fichiers CurseForge (projet, fichier, sha1 publié).
    let mut matched: HashMap<u32, Vec<(u64, u64, String)>> = HashMap::new();
    for chunk in list.chunks(CF_BATCH) {
        match cf.fingerprints(chunk).await {
            Ok(ms) => {
                for m in ms {
                    let sha1 = m.file.sha1();
                    matched.entry(m.file.file_fingerprint as u32).or_default().push((m.id, m.file.id, sha1));
                }
            }
            Err(e) => tracing::warn!("reconnaissance CurseForge impossible : {e}"),
        }
    }
    let mut found: HashSet<String> = HashSet::new();
    for (path, fp, sha1) in cand {
        let hit = matched
            .get(fp)
            .into_iter()
            .flatten()
            .find(|(_, _, s)| s.len() == 40 && s.eq_ignore_ascii_case(sha1));
        if let Some((project, file, _)) = hit {
            found.insert(path.clone());
            report.known.push(KnownMod {
                path: path.clone(),
                project_id: format!("curseforge:{project}"),
                version_id: file.to_string(),
            });
        }
    }
    report.unknown.retain(|p| !found.contains(p));
    report.unchecked.retain(|p| !found.contains(p));
}

/// Analyse AVANT téléchargement à partir des sha1 annoncés par le manifeste (aucun accès disque).
/// Les jars sans sha1 valide sont `unchecked` : jamais présumés connus ni sûrs. Le sha1 est une
/// revendication de l'hôte, vérifiée à la réception (cf. `verify_sha1_file`).
pub async fn find_unknown_from_manifest(
    manifest_files: &[ManifestFile],
    client: &ModrinthClient,
    cf: &CfApi,
) -> UnknownReport {
    let mut report = UnknownReport::default();
    let mut hashed: Vec<(String, String)> = Vec::new();
    // Empreinte CurseForge : simple clé de recherche (un mensonge de l'hôte ne trouve rien) ; la reconnaissance
    // repose sur le sha1 annoncé, revérifié à la réception.
    let fps: Vec<CfCandidate> = manifest_files
        .iter()
        .filter(|f| is_mod_jar(&f.path))
        .filter_map(|f| Some((f.path.clone(), f.cf_fingerprint?, f.sha1.clone()?)))
        .collect();
    for f in manifest_files.iter().filter(|f| is_mod_jar(&f.path)) {
        match f.sha1.as_deref() {
            Some(h) if h.len() == 40 && h.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f')) => {
                hashed.push((f.path.clone(), h.to_string()));
            }
            _ => report.unchecked.push(f.path.clone()),
        }
    }
    query_hashed(&hashed, client, &mut report).await;
    query_curseforge(&fps, cf, &mut report).await;
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
            "Ni Modrinth ni CurseForge ne reconnaissent ces fichiers ({}) : intégrité vérifiée ≠ sécurité garantie.\n",
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
            "Les catalogues reconnaissent tous les mods vérifiés ({}). Intégrité vérifiée ≠ sécurité garantie.\n",
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
        ManifestFile { path: p.into(), size: 1, sha256: "0".repeat(64), sha1: None, cf_fingerprint: None }
    }

    /// CurseForge sans clé : jamais interrogé.
    fn nocf() -> CfApi {
        CfApi::with_base("http://127.0.0.1:1", "")
    }

    /// Faux CurseForge : (empreinte, sha1 publié).
    fn cf_mock(known: &[(u32, String)]) -> Mock {
        let matches: Vec<_> = known
            .iter()
            .enumerate()
            .map(|(i, (fp, sha1))| {
                json!({"id": 100 + i, "file": {"id": 500 + i, "fileFingerprint": fp, "hashes": [{"value": sha1, "algo": 1}]}})
            })
            .collect();
        Mock::given(method("POST"))
            .and(path("/v1/fingerprints/432"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {"exactMatches": matches}})))
    }

    #[tokio::test]
    async fn curseforge_recognizes_what_modrinth_does_not() {
        let mr = MockServer::start().await;
        let cfs = MockServer::start().await;
        let d = tempfile::tempdir().unwrap();
        let (files, hashes) = make_jars(d.path(), 3);
        // Modrinth : m0 seulement. CurseForge : m1. m2 : maison.
        Mock::given(method("POST"))
            .and(path("/version_files"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body_for(&hashes[..1])))
            .mount(&mr)
            .await;
        let fp1 = murmur2_fingerprint(&std::fs::read(d.path().join("mods/m1.jar")).unwrap());
        // m2 : empreinte forgée identique à celle d'un fichier CurseForge, mais sha1 différent -> reste inconnu.
        let fp2 = murmur2_fingerprint(&std::fs::read(d.path().join("mods/m2.jar")).unwrap());
        cf_mock(&[(fp1, hashes[1].to_uppercase()), (fp2, "f".repeat(40))]).expect(2).mount(&cfs).await;
        let cf = CfApi::with_base(cfs.uri(), "k");
        let r = find_unknown(&files, d.path(), &client(&mr), &cf).await.unwrap();
        assert_eq!(r.unknown, vec!["mods/m2.jar".to_string()]);
        assert!(r.known.iter().any(|k| k.path == "mods/m1.jar" && k.project_id == "curseforge:100"));
        assert_eq!(r.known.len(), 2);

        // Avant téléchargement : l'empreinte vient du manifeste ; Modrinth en panne -> CurseForge rattrape m1.
        let mr2 = MockServer::start().await;
        Mock::given(method("POST")).and(path("/version_files")).respond_with(ResponseTemplate::new(500)).mount(&mr2).await;
        let mfiles: Vec<ManifestFile> = (0..3)
            .map(|i| {
                let b = std::fs::read(d.path().join(format!("mods/m{i}.jar"))).unwrap();
                ManifestFile { sha1: Some(sha1_hex(&b)), cf_fingerprint: Some(murmur2_fingerprint(&b)), ..mf(&format!("mods/m{i}.jar")) }
            })
            .collect();
        let r = find_unknown_from_manifest(&mfiles, &client(&mr2), &cf).await;
        assert_eq!(r.known.len(), 1);
        assert_eq!(r.unchecked, vec!["mods/m0.jar".to_string(), "mods/m2.jar".to_string()]);
    }

    #[tokio::test]
    async fn curseforge_down_keeps_unknown_unknown() {
        let mr = MockServer::start().await;
        let cfs = MockServer::start().await;
        let d = tempfile::tempdir().unwrap();
        let (files, _) = make_jars(d.path(), 1);
        Mock::given(method("POST"))
            .and(path("/version_files"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&mr)
            .await;
        Mock::given(method("POST")).and(path("/v1/fingerprints/432")).respond_with(ResponseTemplate::new(503)).mount(&cfs).await;
        let r = find_unknown(&files, d.path(), &client(&mr), &CfApi::with_base(cfs.uri(), "k")).await.unwrap();
        assert_eq!(r.unknown, vec!["mods/m0.jar".to_string()]);
        assert!(r.unchecked.is_empty());
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
        let r = find_unknown_from_manifest(&files, &client(&s), &nocf()).await;
        assert_eq!(r.known.len(), 90);
        assert_eq!(r.unknown, vec!["mods/m90.jar".to_string()]);
        assert_eq!(r.unchecked, vec!["mods/nosha1.jar".to_string()]);
    }

    #[tokio::test]
    async fn from_manifest_network_failure_is_unchecked() {
        let s = MockServer::start().await;
        Mock::given(method("POST")).and(path("/version_files")).respond_with(ResponseTemplate::new(500)).mount(&s).await;
        let f = ManifestFile { sha1: Some("a".repeat(40)), ..mf("mods/a.jar") };
        let r = find_unknown_from_manifest(&[f], &client(&s), &nocf()).await;
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
        let r = find_unknown(&files, d.path(), &client(&s), &nocf()).await.unwrap();
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
        let r = find_unknown(&files, d.path(), &client(&s), &nocf()).await.unwrap();
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
        let r = find_unknown(&files, d.path(), &client(&s), &nocf()).await.unwrap();
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
        let r = find_unknown(&files, d.path(), &client(&s), &nocf()).await.unwrap();
        assert_eq!(r.unchecked.len(), 2);
        assert!(r.unknown.is_empty() && r.known.is_empty());
    }

    #[tokio::test]
    async fn missing_jar_and_non_mods_ignored() {
        let s = MockServer::start().await;
        let d = tempfile::tempdir().unwrap();
        let files = vec![mf("mods/absent.jar"), mf("config/x.toml"), mf("resourcepacks/p.zip"), mf("mods/readme.txt")];
        let r = find_unknown(&files, d.path(), &client(&s), &nocf()).await.unwrap();
        assert_eq!(r.unchecked, vec!["mods/absent.jar".to_string()]);
        assert!(r.known.is_empty() && r.unknown.is_empty());
    }

    #[tokio::test]
    async fn empty_manifest() {
        let s = MockServer::start().await;
        let d = tempfile::tempdir().unwrap();
        let r = find_unknown(&[], d.path(), &client(&s), &nocf()).await.unwrap();
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
        let r = find_unknown(&files, d.path(), &client(&s), &nocf()).await.unwrap();
        assert_eq!(r.known.len(), 205);
    }

    #[test]
    fn pause_is_bounded() {
        assert_eq!(pause_secs("Modrinth limite le débit (HTTP 429) : réessayer dans 17 s"), 17);
        assert_eq!(pause_secs("réessayer dans 9999 s"), MAX_PAUSE_SECS);
        assert_eq!(pause_secs("délai d'attente inconnu"), DEFAULT_PAUSE_SECS);
    }
}
