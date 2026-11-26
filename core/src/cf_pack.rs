//! Lecture d'un zip de modpack CurseForge : `manifest.json`, loader, et résolution des mods (URL et SHA-1 avec la
//! clé de développeur, sinon redirection sans clé). Partagé par l'import et par la mise à jour d'un modpack.

use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use futures_util::stream::{self, StreamExt};
use serde::Deserialize;
use zip::ZipArchive;

use crate::mods::Provenance;
use crate::mrpack::{check_url, validate_file_path, validate_rel, ImportOptions, Layout, MrpackFile, MrpackIndex, MAX_INDEX_BYTES};
use crate::providers::curseforge::{CfDownload, CfResolver, CDN_HOSTS};
use crate::types::{Error, Loader, LoaderSpec, Result};

fn invalid(m: impl Into<String>) -> Error {
    Error::Invalid(m.into())
}

#[derive(Deserialize)]
pub(crate) struct CfManifest {
    minecraft: CfMinecraft,
    #[serde(default)]
    name: String,
    #[serde(default)]
    files: Vec<CfEntry>,
    #[serde(default)]
    overrides: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfMinecraft {
    version: String,
    #[serde(default)]
    mod_loaders: Vec<CfLoader>,
}

#[derive(Deserialize)]
struct CfLoader {
    id: String,
    #[serde(default)]
    primary: bool,
}

#[derive(Deserialize)]
struct CfEntry {
    #[serde(rename = "projectID")]
    project_id: u64,
    #[serde(rename = "fileID")]
    file_id: u64,
}

pub(crate) fn read_cf_manifest(archive: &mut ZipArchive<File>) -> Option<CfManifest> {
    let entry = archive.by_name("manifest.json").ok()?;
    if entry.size() > MAX_INDEX_BYTES {
        return None;
    }
    let mut buf = Vec::new();
    entry.take(MAX_INDEX_BYTES).read_to_end(&mut buf).ok()?;
    let m: CfManifest = serde_json::from_slice(&buf).ok()?;
    (!m.minecraft.version.trim().is_empty()).then_some(m)
}

fn cf_loader(m: &CfMinecraft) -> Result<LoaderSpec> {
    let Some(l) = m.mod_loaders.iter().find(|l| l.primary).or(m.mod_loaders.first()) else {
        return Ok(LoaderSpec { loader: Loader::Vanilla, version: String::new() });
    };
    let (name, version) = l.id.split_once('-').ok_or_else(|| invalid(format!("loader CurseForge invalide : {:?}", l.id)))?;
    let loader = match name.to_ascii_lowercase().as_str() {
        "forge" => Loader::Forge,
        "neoforge" => Loader::NeoForge,
        "fabric" => Loader::Fabric,
        "quilt" => Loader::Quilt,
        other => return Err(invalid(format!("loader CurseForge non supporté : {other}"))),
    };
    if version.is_empty() {
        return Err(invalid("version du loader vide"));
    }
    Ok(LoaderSpec { loader, version: version.to_string() })
}

/// Loader déclaré par le `manifest.json` d'un zip CurseForge (sans rien résoudre ni télécharger).
pub fn manifest_loader(zip_path: &std::path::Path) -> Result<LoaderSpec> {
    let mut archive = crate::mrpack::open_zip(zip_path)?;
    let m = read_cf_manifest(&mut archive).ok_or_else(|| invalid("manifest.json CurseForge invalide"))?;
    cf_loader(&m.minecraft)
}

/// Pack CurseForge résolu : index au format commun, emplacement des overrides, mods introuvables et options
/// d'import adaptées (CDN CurseForge autorisés, plus de parallélisme).
pub(crate) struct CfPack {
    pub index: MrpackIndex,
    pub layout: Layout,
    /// Descriptions lisibles des mods non résolus (`projet X / fichier Y`).
    pub missing: Vec<String>,
    pub opts: ImportOptions,
}

/// Lit le `manifest.json` de `archive` et résout ses mods avec `cf`. Un mod introuvable n'échoue pas : il est listé
/// dans `missing` ; un mod résolu sans SHA-1 (sans clé) est `unverified`, et tous sont `tolerant`.
pub(crate) async fn read(archive: &mut ZipArchive<File>, cf: &CfResolver, opts: &ImportOptions, cancel: &Arc<AtomicBool>) -> Result<CfPack> {
    let m = read_cf_manifest(archive).ok_or_else(|| invalid("manifest.json CurseForge invalide"))?;
    let loader = cf_loader(&m.minecraft)?;
    let overrides = if m.overrides.is_empty() { "overrides".to_string() } else { m.overrides.clone() };
    validate_rel(&overrides)?;

    let mut entries: Vec<(u64, u64)> = Vec::new();
    let mut seen_ids = HashSet::new();
    for f in &m.files {
        if seen_ids.insert((f.project_id, f.file_id)) {
            entries.push((f.project_id, f.file_id));
        }
    }

    let mut opts = opts.clone();
    opts.allowed_hosts.extend(CDN_HOSTS.iter().map(|h| h.to_string()));
    // Beaucoup de petits fichiers : la latence domine, on parallélise davantage (résolution et téléchargement).
    opts.concurrency = opts.concurrency.max(16);
    let mut missing: Vec<String> = Vec::new();
    let mut files = Vec::with_capacity(entries.len());
    let mut seen_paths = HashSet::new();
    // Avec une clé de développeur : une seule requête résout tous les mods (URL + SHA-1). Sinon, ou pour
    // les mods sans URL de distribution, on retombe sur la résolution par redirection (sans clé).
    let batch = if cf.has_api_key() {
        let ids: Vec<u64> = entries.iter().map(|(_, f)| *f).collect();
        match cf.resolve_batch(&ids).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("résolution groupée CurseForge indisponible, repli sans clé : {e}");
                Default::default()
            }
        }
    } else {
        Default::default()
    };
    let batch_ref = &batch;
    let opts_ref = &opts;
    let cancel_ref = cancel;
    let mut resolved = stream::iter(entries.iter().copied())
        .map(|(p, f)| async move {
            if cancel_ref.load(Ordering::SeqCst) {
                return ((p, f), Err(Error::Cancelled));
            }
            let r = async {
                let (downloads, file_name, sha1, size) = match batch_ref.get(&f) {
                    Some(b) => {
                        let direct = match &b.url {
                            Some(url) => Some(CfDownload { url: url.clone(), file_name: b.file_name.clone() }),
                            // Distribution tierce interdite : la redirection web peut encore répondre, sinon le fichier
                            // reste dans l'index sans URL (nom et SHA-1 connus) et sera « introuvable » au téléchargement.
                            None => cf.resolve(p, f).await.ok(),
                        };
                        match direct {
                            Some(d) => (vec![d.url], d.file_name, b.sha1.clone(), b.size),
                            None => (Vec::new(), b.file_name.clone(), b.sha1.clone(), b.size),
                        }
                    }
                    None => {
                        let d = cf.resolve(p, f).await?;
                        (vec![d.url], d.file_name, String::new(), 0)
                    }
                };
                for url in &downloads {
                    check_url(url, opts_ref)?;
                }
                let dir = if file_name.to_ascii_lowercase().ends_with(".zip") { "resourcepacks" } else { "mods" };
                let path = format!("{dir}/{file_name}");
                validate_file_path(&path)?;
                Ok::<_, Error>((path, downloads, sha1, size))
            }
            .await;
            ((p, f), r)
        })
        .buffer_unordered(opts.concurrency.max(1));
    while let Some(((p, f), r)) = resolved.next().await {
        match r {
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(e) => {
                tracing::warn!("mod CurseForge {p}/{f} non résolu : {e}");
                missing.push(format!("projet {p} / fichier {f}"));
            }
            Ok((path, downloads, sha1, size)) => {
                if seen_paths.insert(path.to_ascii_lowercase()) {
                    let unverified = sha1.is_empty();
                    let provenance = Some(Provenance { source: "curseforge".into(), id: p.to_string() });
                    files.push(MrpackFile { path, sha1, sha512: String::new(), downloads, file_size: size, env: None, unverified, tolerant: true, provenance });
                }
            }
        }
    }
    drop(resolved);
    if cancel.load(Ordering::SeqCst) {
        return Err(Error::Cancelled);
    }
    let pack = MrpackIndex { name: m.name, version_id: String::new(), minecraft: m.minecraft.version.trim().to_string(), loader, files };
    let layout = Layout { common: format!("{overrides}/"), client: None };
    Ok(CfPack { index: pack, layout, missing, opts })
}
