//! Tâche J2 : téléchargement et installation d'un JRE Adoptium (sha256 + taille vérifiés,
//! extraction zip sécurisée, renommage atomique vers `<java_root>/jre-<major>`).
//!
//! Archives : `.zip` (Windows) et `.tar.gz` (Linux), mêmes garde-fous (chemins, tailles, liens).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::types::{Error, Result};

pub const DEFAULT_ADOPTIUM_URL: &str = "https://api.adoptium.net";
/// Versions majeures supportées.
pub const SUPPORTED_MAJORS: [u32; 4] = [8, 17, 21, 25];
/// Version installée par défaut.
pub const DEFAULT_MAJOR: u32 = 21;

const MAX_ENTRIES: usize = 50_000;
const MAX_FILE_BYTES: u64 = 1 << 30;
const MAX_TOTAL_BYTES: u64 = 2 << 30;
const MAX_DOWNLOAD_BYTES: u64 = 512 << 20;
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JrePackage {
    pub url: String,
    pub size: u64,
    pub sha256: String,
    pub filename: String,
    /// Version majeure demandée (ajoutée pour que `install` sache où installer).
    pub major: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledJava {
    pub home: PathBuf,
    pub java_exe: PathBuf,
    pub major: u32,
}

/// Vérifie qu'un exécutable Java fonctionne et renvoie sa version majeure.
pub trait JavaProbe: Send + Sync {
    fn probe(&self, java_exe: &Path) -> Option<u32>;
}

/// Sonde réelle : lance `java -version` avec un délai maximal.
pub struct ProcessProbe;

impl JavaProbe for ProcessProbe {
    fn probe(&self, java_exe: &Path) -> Option<u32> {
        let (tx, rx) = std::sync::mpsc::channel();
        let exe = java_exe.to_path_buf();
        std::thread::spawn(move || {
            let _ = tx.send(crate::java::probe_with(&exe, crate::java::run_java_version).map(|i| i.major));
        });
        rx.recv_timeout(PROBE_TIMEOUT).ok().flatten()
    }
}

/// Nom d'OS Adoptium de la machine courante.
pub fn host_os() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "mac"
    } else {
        "linux"
    }
}

/// Architecture Adoptium de la machine courante.
pub fn host_arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x64"
    }
}

fn net(e: impl std::fmt::Display) -> Error {
    Error::Network(e.to_string())
}

fn java_exe_rel() -> &'static str {
    if cfg!(windows) {
        "bin/java.exe"
    } else {
        "bin/java"
    }
}

#[derive(Deserialize)]
struct Asset {
    binary: Binary,
}
#[derive(Deserialize)]
struct Binary {
    package: Package,
}
#[derive(Deserialize)]
struct Package {
    link: String,
    size: u64,
    checksum: String,
    #[serde(default)]
    name: Option<String>,
}

/// Interroge l'API Adoptium (URL de base par défaut).
pub async fn resolve_package(major: u32, os: &str, arch: &str) -> Result<JrePackage> {
    resolve_package_from(DEFAULT_ADOPTIUM_URL, major, os, arch).await
}

/// Variante à URL de base injectable (tests).
pub async fn resolve_package_from(base_url: &str, major: u32, os: &str, arch: &str) -> Result<JrePackage> {
    if !SUPPORTED_MAJORS.contains(&major) {
        return Err(Error::Invalid(format!("version Java {major} non supportée (8, 17, 21, 25)")));
    }
    if !matches!(os, "windows" | "linux" | "mac") || !matches!(arch, "x64" | "aarch64") {
        return Err(Error::Invalid(format!("plateforme {os}/{arch} non supportée")));
    }
    let url = format!("{}/v3/assets/latest/{major}/hotspot", base_url.trim_end_matches('/'));
    let resp = reqwest::Client::new()
        .get(&url)
        .query(&[("architecture", arch), ("image_type", "jre"), ("os", os)])
        .send()
        .await
        .map_err(net)?;
    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(Error::NotFound(format!("JRE {major} {os}/{arch}")));
    }
    if !status.is_success() {
        return Err(Error::Network(format!("Adoptium : HTTP {status}")));
    }
    let assets: Vec<Asset> = resp.json().await.map_err(net)?;
    let p = assets
        .into_iter()
        .next()
        .ok_or_else(|| Error::NotFound(format!("JRE {major} {os}/{arch}")))?
        .binary
        .package;
    let filename = p
        .name
        .filter(|n| !n.is_empty())
        .or_else(|| p.link.rsplit('/').next().map(str::to_owned))
        .unwrap_or_default();
    Ok(JrePackage { url: p.link, size: p.size, sha256: p.checksum.to_ascii_lowercase(), filename, major })
}

/// Installe avec la sonde réelle.
pub async fn install<F>(
    package: &JrePackage,
    java_root: &Path,
    progress: F,
    cancel: Arc<AtomicBool>,
) -> Result<InstalledJava>
where
    F: Fn(u64, u64) + Send + Sync,
{
    install_with(package, java_root, progress, cancel, Arc::new(ProcessProbe)).await
}

/// Supprime un dossier à la sortie (échec ou annulation).
struct Cleanup(Vec<PathBuf>);
impl Drop for Cleanup {
    fn drop(&mut self) {
        for p in &self.0 {
            if p.is_dir() {
                let _ = std::fs::remove_dir_all(p);
            } else {
                let _ = std::fs::remove_file(p);
            }
        }
    }
}

fn check(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::SeqCst) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

/// Installation avec sonde injectable. Idempotente : une installation valide est renvoyée telle quelle.
pub async fn install_with<F>(
    package: &JrePackage,
    java_root: &Path,
    progress: F,
    cancel: Arc<AtomicBool>,
    probe: Arc<dyn JavaProbe>,
) -> Result<InstalledJava>
where
    F: Fn(u64, u64) + Send + Sync,
{
    let major = package.major;
    if !SUPPORTED_MAJORS.contains(&major) {
        return Err(Error::Invalid(format!("version Java {major} non supportée")));
    }
    let lower = package.filename.to_ascii_lowercase();
    let targz = lower.ends_with(".tar.gz") || lower.ends_with(".tgz");
    if !targz && !lower.ends_with(".zip") {
        return Err(Error::Invalid(format!("format d'archive inconnu : {}", package.filename)));
    }
    check(&cancel)?;

    let final_dir = java_root.join(format!("jre-{major}"));
    let final_exe = final_dir.join(java_exe_rel());
    if final_exe.is_file() {
        let p = probe.clone();
        let exe = final_exe.clone();
        let found = tokio::task::spawn_blocking(move || p.probe(&exe)).await.map_err(|e| Error::Other(e.to_string()))?;
        if found == Some(major) {
            return Ok(InstalledJava { home: final_dir, java_exe: final_exe, major });
        }
    }

    tokio::fs::create_dir_all(java_root).await?;
    let tag = std::process::id();
    let part = java_root.join(format!(".jre-{major}.{tag}.download"));
    let staging = java_root.join(format!(".jre-{major}.{tag}.staging"));
    let _guard = Cleanup(vec![part.clone(), staging.clone()]);
    let _ = tokio::fs::remove_dir_all(&staging).await;

    download(package, &part, &progress, &cancel).await?;

    let (zip_path, stage, c2) = (part.clone(), staging.clone(), cancel.clone());
    let home_in_stage = tokio::task::spawn_blocking(move || if targz { extract_tar_gz(&zip_path, &stage, &c2) } else { extract_zip(&zip_path, &stage, &c2) })
        .await
        .map_err(|e| Error::Other(e.to_string()))??;
    check(&cancel)?;

    let exe_in_stage = home_in_stage.join(java_exe_rel());
    if !exe_in_stage.is_file() {
        return Err(Error::Invalid(format!("{} absent de l'archive", java_exe_rel())));
    }

    // Remplace une installation invalide existante, puis renomme atomiquement.
    if final_dir.exists() {
        tokio::fs::remove_dir_all(&final_dir).await?;
    }
    tokio::fs::rename(&home_in_stage, &final_dir).await?;

    let p = probe;
    let exe = final_exe.clone();
    let found = tokio::task::spawn_blocking(move || p.probe(&exe)).await.map_err(|e| Error::Other(e.to_string()))?;
    if found != Some(major) {
        let _ = tokio::fs::remove_dir_all(&final_dir).await;
        return Err(Error::Invalid(format!("`java -version` inattendu après installation (attendu {major}, obtenu {found:?})")));
    }
    Ok(InstalledJava { home: final_dir, java_exe: final_exe, major })
}

async fn download<F>(package: &JrePackage, part: &Path, progress: &F, cancel: &AtomicBool) -> Result<()>
where
    F: Fn(u64, u64) + Send + Sync,
{
    if package.size > MAX_DOWNLOAD_BYTES {
        return Err(Error::Invalid(format!("taille annoncée excessive : {}", package.size)));
    }
    let resp = reqwest::Client::new().get(&package.url).send().await.map_err(net)?;
    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(Error::NotFound(package.url.clone()));
    }
    if !status.is_success() {
        return Err(Error::Network(format!("{} : HTTP {status}", package.url)));
    }
    let mut file = tokio::fs::File::create(part).await?;
    let mut hasher = Sha256::new();
    let mut done: u64 = 0;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        check(cancel)?;
        let chunk = chunk.map_err(net)?;
        done += chunk.len() as u64;
        if done > package.size.max(1) {
            return Err(Error::Integrity { expected: format!("{} octets", package.size), actual: format!(">{} octets", package.size) });
        }
        hasher.update(&chunk);
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
        progress(done, package.size);
    }
    tokio::io::AsyncWriteExt::flush(&mut file).await?;
    drop(file);
    check(cancel)?;
    if done != package.size {
        return Err(Error::Integrity { expected: format!("{} octets", package.size), actual: format!("{done} octets") });
    }
    let actual = hex::encode(hasher.finalize());
    if !actual.eq_ignore_ascii_case(&package.sha256) {
        return Err(Error::Integrity { expected: package.sha256.to_ascii_lowercase(), actual });
    }
    Ok(())
}

/// Extrait le zip dans `dest` et renvoie le dossier « home » (racine ou unique sous-dossier contenant `bin/java`).
fn extract_zip(archive: &Path, dest: &Path, cancel: &AtomicBool) -> Result<PathBuf> {
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| Error::Invalid(format!("zip invalide : {e}")))?;
    if zip.len() > MAX_ENTRIES {
        return Err(Error::Invalid(format!("trop d'entrées dans l'archive : {}", zip.len())));
    }
    std::fs::create_dir_all(dest)?;
    let mut total: u64 = 0;
    let mut buf = vec![0u8; 64 * 1024];
    for i in 0..zip.len() {
        check(cancel)?;
        let mut entry = zip.by_index(i).map_err(|e| Error::Invalid(format!("zip invalide : {e}")))?;
        let rel = entry
            .enclosed_name()
            .ok_or_else(|| Error::Invalid(format!("chemin dangereux dans l'archive : {}", entry.name())))?;
        if rel.is_absolute() || rel.has_root() {
            return Err(Error::Invalid(format!("chemin absolu dans l'archive : {}", entry.name())));
        }
        if let Some(mode) = entry.unix_mode() {
            if mode & 0o170000 == 0o120000 {
                return Err(Error::Invalid(format!("lien symbolique refusé : {}", entry.name())));
            }
        }
        let out = dest.join(&rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut f = std::fs::File::create(&out)?;
        let mut written: u64 = 0;
        loop {
            let n = entry.read(&mut buf)?;
            if n == 0 {
                break;
            }
            written += n as u64;
            total += n as u64;
            if written > MAX_FILE_BYTES || total > MAX_TOTAL_BYTES {
                return Err(Error::Invalid("taille décompressée excessive".into()));
            }
            f.write_all(&buf[..n])?;
        }
    }
    find_home(dest)
}

/// Extrait un `.tar.gz` (JRE Linux) : droits d'exécution conservés, chemins confinés à `dest`,
/// liens ignorés (Adoptium n'en met que dans `legal/`), mêmes plafonds de taille que le zip.
fn extract_tar_gz(archive: &Path, dest: &Path, cancel: &AtomicBool) -> Result<PathBuf> {
    let file = std::fs::File::open(archive)?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(std::io::BufReader::new(file)));
    tar.set_preserve_permissions(true);
    tar.set_overwrite(true);
    std::fs::create_dir_all(dest)?;
    let bad = |e: std::io::Error| Error::Invalid(format!("tar.gz invalide : {e}"));
    let mut total: u64 = 0;
    for (n, entry) in tar.entries().map_err(bad)?.enumerate() {
        check(cancel)?;
        if n >= MAX_ENTRIES {
            return Err(Error::Invalid("trop d'entrées dans l'archive".into()));
        }
        let mut entry = entry.map_err(bad)?;
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir()) {
            continue;
        }
        let rel = entry.path().map_err(bad)?.into_owned();
        if rel.is_absolute() || rel.has_root() || rel.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            return Err(Error::Invalid(format!("chemin dangereux dans l'archive : {}", rel.display())));
        }
        let size = entry.header().size().map_err(bad)?;
        total += size;
        if size > MAX_FILE_BYTES || total > MAX_TOTAL_BYTES {
            return Err(Error::Invalid("taille décompressée excessive".into()));
        }
        // `unpack_in` refuse aussi tout chemin qui sortirait de `dest`.
        if !entry.unpack_in(dest).map_err(bad)? {
            return Err(Error::Invalid(format!("chemin dangereux dans l'archive : {}", rel.display())));
        }
    }
    find_home(dest)
}

/// Dossier « home » du JRE : `dest` ou son unique sous-dossier contenant `bin/java`.
fn find_home(dest: &Path) -> Result<PathBuf> {
    if dest.join(java_exe_rel()).is_file() {
        return Ok(dest.to_path_buf());
    }
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(dest)?.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.retain(|d| d.join(java_exe_rel()).is_file());
    match dirs.len() {
        1 => Ok(dirs.remove(0)),
        _ => Err(Error::Invalid(format!("{} introuvable dans l'archive", java_exe_rel()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    struct FakeProbe(Option<u32>);
    impl JavaProbe for FakeProbe {
        fn probe(&self, _: &Path) -> Option<u32> {
            self.0
        }
    }

    fn fake_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        for (name, data) in entries {
            w.start_file(*name, opts).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    fn jre_zip() -> Vec<u8> {
        let exe = format!("jdk-21.0.5+11-jre/{}", java_exe_rel());
        fake_zip(&[(&exe, b"fake java"), ("jdk-21.0.5+11-jre/release", b"JAVA_VERSION=\"21\"")])
    }

    fn pkg(server: &MockServer, data: &[u8]) -> JrePackage {
        JrePackage {
            url: format!("{}/dl/jre.zip", server.uri()),
            size: data.len() as u64,
            sha256: hex::encode(Sha256::digest(data)),
            filename: "jre.zip".into(),
            major: 21,
        }
    }

    async fn serve(server: &MockServer, data: Vec<u8>) {
        Mock::given(method("GET")).and(path("/dl/jre.zip")).respond_with(ResponseTemplate::new(200).set_body_bytes(data)).mount(server).await;
    }

    fn leftovers(root: &Path) -> Vec<String> {
        std::fs::read_dir(root).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect()
    }

    #[tokio::test]
    async fn resolve_nominal_et_erreurs() {
        let server = MockServer::start().await;
        let body = serde_json::json!([{ "binary": { "package": {
            "link": "https://x/jre.zip", "size": 42, "checksum": "ABCD", "name": "jre.zip" } } }]);
        Mock::given(method("GET"))
            .and(path("/v3/assets/latest/21/hotspot"))
            .and(query_param("image_type", "jre"))
            .and(query_param("os", "windows"))
            .and(query_param("architecture", "x64"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
        let p = resolve_package_from(&server.uri(), 21, "windows", "x64").await.unwrap();
        assert_eq!((p.url.as_str(), p.size, p.sha256.as_str(), p.filename.as_str()), ("https://x/jre.zip", 42, "abcd", "jre.zip"));
        // 17 non mocké : 404 de wiremock.
        assert!(matches!(resolve_package_from(&server.uri(), 17, "windows", "x64").await, Err(Error::NotFound(_))));
        assert!(matches!(resolve_package_from(&server.uri(), 11, "windows", "x64").await, Err(Error::Invalid(_))));
    }

    #[tokio::test]
    async fn installation_nominale_puis_idempotente() {
        let server = MockServer::start().await;
        let data = jre_zip();
        Mock::given(method("GET")).and(path("/dl/jre.zip")).respond_with(ResponseTemplate::new(200).set_body_bytes(data.clone())).expect(1).mount(&server).await;
        let root = tempfile::tempdir().unwrap();
        let p = pkg(&server, &data);
        let calls = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let c = calls.clone();
        let inst = install_with(&p, root.path(), move |_, _| { c.fetch_add(1, Ordering::SeqCst); }, Arc::new(AtomicBool::new(false)), Arc::new(FakeProbe(Some(21))))
            .await
            .unwrap();
        assert!(calls.load(Ordering::SeqCst) > 0);
        assert_eq!(inst.major, 21);
        assert_eq!(inst.home, root.path().join("jre-21"));
        assert!(inst.java_exe.is_file());
        assert_eq!(leftovers(root.path()), vec!["jre-21".to_string()]);
        // Deuxième appel : le mock n'admet qu'une requête (expect(1) vérifié à la fermeture).
        let again = install_with(&p, root.path(), |_, _| {}, Arc::new(AtomicBool::new(false)), Arc::new(FakeProbe(Some(21)))).await.unwrap();
        assert_eq!(again, inst);
    }

    #[tokio::test]
    async fn deja_installe_aucune_requete() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(200)).expect(0).mount(&server).await;
        let root = tempfile::tempdir().unwrap();
        let exe = root.path().join("jre-21").join(java_exe_rel());
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"x").unwrap();
        let p = pkg(&server, b"abc");
        let inst = install_with(&p, root.path(), |_, _| {}, Arc::new(AtomicBool::new(false)), Arc::new(FakeProbe(Some(21)))).await.unwrap();
        assert_eq!(inst.java_exe, exe);
    }

    #[tokio::test]
    async fn sha256_faux_rien_installe() {
        let server = MockServer::start().await;
        let data = jre_zip();
        serve(&server, data.clone()).await;
        let root = tempfile::tempdir().unwrap();
        let mut p = pkg(&server, &data);
        p.sha256 = "0".repeat(64);
        let r = install_with(&p, root.path(), |_, _| {}, Arc::new(AtomicBool::new(false)), Arc::new(FakeProbe(Some(21)))).await;
        assert!(matches!(r, Err(Error::Integrity { .. })));
        assert!(leftovers(root.path()).is_empty());
    }

    #[tokio::test]
    async fn zip_slip_refuse() {
        let server = MockServer::start().await;
        let data = fake_zip(&[("../evil.txt", b"pwn"), ("jre/bin/java.exe", b"x")]);
        serve(&server, data.clone()).await;
        let root = tempfile::tempdir().unwrap();
        let jroot = root.path().join("java");
        let p = pkg(&server, &data);
        let r = install_with(&p, &jroot, |_, _| {}, Arc::new(AtomicBool::new(false)), Arc::new(FakeProbe(Some(21)))).await;
        assert!(matches!(r, Err(Error::Invalid(_))), "{r:?}");
        assert!(!root.path().join("evil.txt").exists());
        assert!(leftovers(&jroot).is_empty());
    }

    #[tokio::test]
    async fn annulation_nettoie() {
        let server = MockServer::start().await;
        let data = jre_zip();
        serve(&server, data.clone()).await;
        let root = tempfile::tempdir().unwrap();
        let p = pkg(&server, &data);
        let r = install_with(&p, root.path(), |_, _| {}, Arc::new(AtomicBool::new(true)), Arc::new(FakeProbe(Some(21)))).await;
        assert!(matches!(r, Err(Error::Cancelled)));
        assert!(leftovers(root.path()).is_empty());
    }

    #[tokio::test]
    async fn annulation_pendant_telechargement() {
        let server = MockServer::start().await;
        let data = jre_zip();
        serve(&server, data.clone()).await;
        let root = tempfile::tempdir().unwrap();
        let p = pkg(&server, &data);
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        let r = install_with(&p, root.path(), move |_, _| c.store(true, Ordering::SeqCst), cancel, Arc::new(FakeProbe(Some(21)))).await;
        assert!(matches!(r, Err(Error::Cancelled)));
        assert!(leftovers(root.path()).is_empty());
    }

    #[tokio::test]
    async fn telechargement_404_notfound() {
        let server = MockServer::start().await;
        let root = tempfile::tempdir().unwrap();
        let p = pkg(&server, b"abc");
        let r = install_with(&p, root.path(), |_, _| {}, Arc::new(AtomicBool::new(false)), Arc::new(FakeProbe(Some(21)))).await;
        assert!(matches!(r, Err(Error::NotFound(_))));
        assert!(leftovers(root.path()).is_empty());
    }

    #[tokio::test]
    async fn version_non_supportee_et_format_inconnu() {
        let root = tempfile::tempdir().unwrap();
        let mut p = JrePackage { url: "http://127.0.0.1:1/x".into(), size: 1, sha256: "0".repeat(64), filename: "jre.zip".into(), major: 11 };
        let r = install_with(&p, root.path(), |_, _| {}, Arc::new(AtomicBool::new(false)), Arc::new(FakeProbe(Some(11)))).await;
        assert!(matches!(r, Err(Error::Invalid(_))));
        p.major = 21;
        p.filename = "jre.7z".into();
        let r = install_with(&p, root.path(), |_, _| {}, Arc::new(AtomicBool::new(false)), Arc::new(FakeProbe(Some(21)))).await;
        assert!(matches!(r, Err(Error::Invalid(_))));
    }

    /// `.tar.gz` à la Adoptium Linux : un dossier racine, `bin/java` exécutable, un lien dans `legal/`.
    fn jre_tar_gz(extra: Option<(&str, &[u8])>) -> Vec<u8> {
        let mut b = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        let mut add = |name: &str, data: &[u8], mode: u32| {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(mode);
            h.set_cksum();
            b.append_data(&mut h, name, data).unwrap();
        };
        add(&format!("jdk-21.0.5+11-jre/{}", java_exe_rel()), b"fake java", 0o755);
        add("jdk-21.0.5+11-jre/release", b"JAVA_VERSION=\"21\"", 0o644);
        if let Some((n, d)) = extra {
            add(n, d, 0o644);
        }
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Symlink);
        h.set_size(0);
        h.set_cksum();
        b.append_link(&mut h, "jdk-21.0.5+11-jre/legal/LICENSE", "../release").unwrap();
        b.into_inner().unwrap().finish().unwrap()
    }

    #[tokio::test]
    async fn installe_un_tar_gz_linux() {
        let server = MockServer::start().await;
        let data = jre_tar_gz(None);
        Mock::given(method("GET")).and(path("/dl/jre.tar.gz")).respond_with(ResponseTemplate::new(200).set_body_bytes(data.clone())).mount(&server).await;
        let root = tempfile::tempdir().unwrap();
        let p = JrePackage {
            url: format!("{}/dl/jre.tar.gz", server.uri()),
            size: data.len() as u64,
            sha256: hex::encode(Sha256::digest(&data)),
            filename: "OpenJDK21U-jre_x64_linux_hotspot_21.0.5_11.tar.gz".into(),
            major: 21,
        };
        let j = install_with(&p, root.path(), |_, _| {}, Arc::new(AtomicBool::new(false)), Arc::new(FakeProbe(Some(21)))).await.unwrap();
        assert_eq!(j.home, root.path().join("jre-21"));
        assert_eq!(std::fs::read(&j.java_exe).unwrap(), b"fake java");
        // Lien ignoré, pas suivi.
        assert!(!j.home.join("legal/LICENSE").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&j.java_exe).unwrap().permissions().mode() & 0o111, 0o111);
        }
        assert_eq!(leftovers(root.path()), vec!["jre-21".to_string()]);
    }

    #[test]
    fn tar_gz_chemin_dangereux_refuse() {
        let dir = tempfile::tempdir().unwrap();
        // `tar::Builder` refuse d'écrire « .. » : on forge l'en-tête à la main.
        let mut raw = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        let mut h = tar::Header::new_old();
        h.as_old_mut().name[..9].copy_from_slice(b"../evil.t");
        h.set_size(1);
        h.set_cksum();
        raw.append(&h, &b"x"[..]).unwrap();
        let data = raw.into_inner().unwrap().finish().unwrap();
        let a = dir.path().join("a.tar.gz");
        std::fs::write(&a, data).unwrap();
        let r = extract_tar_gz(&a, &dir.path().join("out"), &AtomicBool::new(false));
        assert!(matches!(r, Err(Error::Invalid(_))), "{r:?}");
        assert!(!dir.path().join("evil.t").exists());
        let _ = jre_tar_gz(Some(("x", b"")));
    }

    #[tokio::test]
    async fn sonde_negative_retire_l_installation() {
        let server = MockServer::start().await;
        let data = jre_zip();
        serve(&server, data.clone()).await;
        let root = tempfile::tempdir().unwrap();
        let p = pkg(&server, &data);
        let r = install_with(&p, root.path(), |_, _| {}, Arc::new(AtomicBool::new(false)), Arc::new(FakeProbe(None))).await;
        assert!(matches!(r, Err(Error::Invalid(_))));
        assert!(leftovers(root.path()).is_empty());
    }
}
