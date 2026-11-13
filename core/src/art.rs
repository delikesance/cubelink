//! Illustrations d'une instance de modpack : icône, captures, couleur, auteur, résumé.
//!
//! Les fichiers vivent dans `<instance>/art/` (hors `minecraft/`, donc jamais partagés comme mods).
//! La récupération en ligne est « au mieux » : rien ici ne doit jamais faire échouer une installation.
//! Les noms de fichiers sont générés (`icon.<ext>`, `shot-N.<ext>`), jamais dérivés d'une URL.

use std::path::Path;
use std::time::Duration;

use base64::Engine as _;
use futures_util::StreamExt;

use crate::browse::{BrowseSource, Providers};
use crate::instance::InstanceStore;
use crate::providers::curseforge_api::{self as cfapi};
use crate::providers::modrinth_api::SearchFilters;
use crate::types::{Error, Instance, InstanceArt, Result};

/// Hôtes d'images autorisés (HTTPS obligatoire). Le loopback n'est admis qu'avec `allow_loopback`.
pub const ART_HOSTS: &[&str] =
    &["cdn.modrinth.com", "media.forgecdn.net", "edge.forgecdn.net", "mediafilez.forgecdn.net", "cdn.forgecdn.net"];
/// Taille maximale d'une image téléchargée.
pub const MAX_IMAGE_BYTES: u64 = 12 * 1024 * 1024;
/// Taille maximale d'une image choisie par l'utilisateur.
pub const MAX_LOCAL_IMAGE_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_SCREENSHOTS: usize = 6;
/// Durée maximale de toute la récupération d'art d'une installation.
const FETCH_BUDGET: Duration = Duration::from_secs(60);

fn invalid(m: impl Into<String>) -> Error {
    Error::Invalid(m.into())
}

/// Type d'image reconnu par ses octets magiques : (MIME, extension).
pub fn sniff(bytes: &[u8]) -> Option<(&'static str, &'static str)> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some(("image/png", "png"))
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(("image/jpeg", "jpg"))
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(("image/webp", "webp"))
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(("image/gif", "gif"))
    } else {
        None
    }
}

fn check_image_url(url: &str, allow_loopback: bool) -> Result<()> {
    let u = url::Url::parse(url).map_err(|_| invalid("URL d'image invalide"))?;
    if u.username() != "" || u.password().is_some() {
        return Err(invalid("URL d'image refusée"));
    }
    let host = u.host_str().unwrap_or("").to_ascii_lowercase();
    if allow_loopback && matches!(u.scheme(), "http" | "https") && matches!(host.as_str(), "127.0.0.1" | "localhost" | "[::1]") {
        return Ok(());
    }
    if u.scheme() != "https" || !ART_HOSTS.contains(&host.as_str()) {
        return Err(invalid(format!("hôte d'image non autorisé ({host})")));
    }
    Ok(())
}

fn client() -> &'static reqwest::Client {
    static C: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(concat!("cubelink/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_default()
    })
}

/// Écriture atomique : fichier temporaire puis renommage.
fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let tmp = dir.join(format!("{name}.tmp"));
    let res = std::fs::write(&tmp, bytes).and_then(|_| std::fs::rename(&tmp, dir.join(name)));
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

/// Télécharge une image (flux plafonné, octets magiques vérifiés) dans `dir` sous `<stem>.<ext>`.
async fn download_image(url: &str, allow_loopback: bool, dir: &Path, stem: &str) -> Result<String> {
    check_image_url(url, allow_loopback)?;
    let resp = client().get(url).send().await.map_err(|e| Error::Network(e.without_url().to_string()))?;
    if !resp.status().is_success() {
        return Err(Error::Network(format!("HTTP {} pour une image", resp.status().as_u16())));
    }
    if resp.content_length().is_some_and(|l| l > MAX_IMAGE_BYTES) {
        return Err(invalid("image trop grosse"));
    }
    let mut buf: Vec<u8> = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| Error::Network(e.without_url().to_string()))?;
        if buf.len() as u64 + chunk.len() as u64 > MAX_IMAGE_BYTES {
            return Err(invalid("image trop grosse"));
        }
        buf.extend_from_slice(&chunk);
    }
    let (_, ext) = sniff(&buf).ok_or_else(|| invalid("format d'image non reconnu"))?;
    let name = format!("{stem}.{ext}");
    write_atomic(dir, &name, &buf)?;
    Ok(name)
}

/// Métadonnées distantes, avant téléchargement des images.
struct Remote {
    title: String,
    author: String,
    summary: String,
    color: Option<String>,
    icon_url: Option<String>,
    /// Par capture : l'URL à essayer d'abord (pleine résolution), puis celle de secours.
    shot_urls: Vec<(String, Option<String>)>,
    page_url: Option<String>,
}

fn hex_color(c: u64) -> String {
    format!("#{:06x}", c & 0xFF_FFFF)
}

async fn remote_modrinth(p: &Providers, id: &str) -> Result<Remote> {
    let proj = p.modrinth.project(id).await?;
    // Auteur : le propriétaire de l'équipe, sinon le premier membre ; au pire vide.
    let author = match p.modrinth.project_members(id).await {
        Ok(m) => m
            .iter()
            .find(|m| m.role.eq_ignore_ascii_case("owner"))
            .or_else(|| m.first())
            .map(|m| m.user.username.clone())
            .unwrap_or_default(),
        Err(e) => {
            tracing::warn!("art : équipe Modrinth indisponible : {e}");
            String::new()
        }
    };
    let mut gallery = proj.gallery.clone();
    // Captures « featured » d'abord, puis par `ordering` croissant (ordre d'origine conservé à égalité).
    gallery.sort_by_key(|g| (!g.featured, g.ordering.unwrap_or(i64::MAX)));
    let slug = if proj.slug.is_empty() { id.to_string() } else { proj.slug.clone() };
    Ok(Remote {
        title: proj.title,
        author,
        summary: proj.description,
        color: proj.color.map(hex_color),
        icon_url: proj.icon_url.filter(|u| !u.is_empty()),
        shot_urls: gallery
            .into_iter()
            .map(|g| match g.raw_url.filter(|r| !r.is_empty() && *r != g.url) {
                Some(raw) => (raw, Some(g.url)),
                None => (g.url, None),
            })
            .collect(),
        page_url: Some(format!("https://modrinth.com/modpack/{slug}")),
    })
}

async fn remote_curseforge(p: &Providers, id: &str) -> Result<Remote> {
    let n: u64 = id.trim().parse().map_err(|_| invalid("identifiant CurseForge invalide"))?;
    let m = p.cf.mod_info(n).await?;
    Ok(Remote {
        title: m.name,
        author: m.authors.into_iter().next().map(|a| a.name).unwrap_or_default(),
        summary: m.summary,
        color: None,
        icon_url: m.logo.and_then(|l| l.url.filter(|u| !u.is_empty()).or(l.thumbnail_url)).filter(|u| !u.is_empty()),
        shot_urls: m.screenshots.into_iter().map(|s| s.url).filter(|u| !u.is_empty()).map(|u| (u, None)).collect(),
        page_url: m.links.and_then(|l| l.website_url).filter(|u| u.starts_with("https://")),
    })
}

/// Récupère l'art d'un projet et écrit les images dans `stage` (dossier vide, créé par l'appelant).
/// Seul l'échec des métadonnées est une erreur : une image refusée ou inaccessible est ignorée.
pub async fn fetch_art(p: &Providers, source: BrowseSource, project_id: &str, stage: &Path) -> Result<InstanceArt> {
    let r = match source {
        BrowseSource::Modrinth => remote_modrinth(p, project_id).await?,
        BrowseSource::Curseforge => remote_curseforge(p, project_id).await?,
    };
    let icon = match &r.icon_url {
        Some(u) => match download_image(u, p.allow_loopback, stage, "icon").await {
            Ok(n) => Some(n),
            Err(e) => {
                tracing::warn!("art : icône ignorée : {e}");
                None
            }
        },
        None => None,
    };
    let mut screenshots = Vec::new();
    for (url, fallback) in &r.shot_urls {
        if screenshots.len() >= MAX_SCREENSHOTS {
            break;
        }
        let stem = format!("shot-{}", screenshots.len() + 1);
        let mut got = download_image(url, p.allow_loopback, stage, &stem).await;
        if let (Err(e), Some(small)) = (&got, fallback) {
            tracing::warn!("art : capture pleine résolution ignorée ({e}), version réduite à la place");
            got = download_image(small, p.allow_loopback, stage, &stem).await;
        }
        match got {
            Ok(n) => screenshots.push(n),
            Err(e) => tracing::warn!("art : capture ignorée : {e}"),
        }
    }
    Ok(InstanceArt {
        source: source.as_str().to_string(),
        project_id: project_id.to_string(),
        title: r.title,
        author: r.author,
        summary: r.summary,
        color: r.color,
        icon,
        screenshots,
        page_url: r.page_url,
    })
}

/// Récupère l'art et remplace entièrement le dossier `art/` de l'instance, puis met à jour `instance.json`.
pub async fn apply_art(p: &Providers, store: &InstanceStore, instance_id: &str, source: BrowseSource, project_id: &str) -> Result<Instance> {
    let art_dir = store.art_dir(instance_id)?;
    store.get(instance_id)?;
    let stage = art_dir.with_file_name("art.tmp");
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage)?;
    let art = match fetch_art(p, source, project_id, &stage).await {
        Ok(a) => a,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&stage);
            return Err(e);
        }
    };
    let _ = std::fs::remove_dir_all(&art_dir);
    if let Err(e) = std::fs::rename(&stage, &art_dir) {
        let _ = std::fs::remove_dir_all(&stage);
        return Err(e.into());
    }
    store.set_art(instance_id, Some(art))
}

/// Meilleur effort (installation d'un modpack) : ne renvoie jamais d'erreur, journalise l'échec et
/// rend l'instance inchangée. Borné dans le temps.
pub async fn try_apply_art(p: &Providers, store: &InstanceStore, instance: Instance, source: BrowseSource, project_id: &str) -> Instance {
    let id = instance.id.clone();
    match tokio::time::timeout(FETCH_BUDGET, apply_art(p, store, &id, source, project_id)).await {
        Ok(Ok(updated)) => updated,
        Ok(Err(e)) => {
            tracing::warn!("art : récupération impossible pour {id} : {e}");
            instance
        }
        Err(_) => {
            tracing::warn!("art : délai dépassé pour {id}");
            instance
        }
    }
}

/// Nom normalisé : alphanumériques en minuscules.
fn normalize(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Meilleure correspondance : nom normalisé identique, sinon premier titre (ordre de pertinence) qui COMMENCE par le nom,
/// sinon titre contenant le nom, sinon nom contenant le titre (≥ 3 caractères).
/// `exact` : seulement un nom normalisé identique (recherche automatique, sans demande de l'utilisateur).
fn pick_best(name: &str, candidates: &[(String, String)], exact: bool) -> Option<String> {
    let n = normalize(name);
    if n.chars().count() < 3 {
        return None;
    }
    let norm: Vec<(String, &String)> = candidates.iter().map(|(t, id)| (normalize(t), id)).collect();
    if let Some((_, id)) = norm.iter().find(|(t, _)| *t == n) {
        return Some((*id).clone());
    }
    if exact {
        return None;
    }
    if let Some((_, id)) = norm.iter().find(|(t, _)| t.starts_with(&n)) {
        return Some((*id).clone());
    }
    if let Some((_, id)) = norm.iter().find(|(t, _)| t.contains(&n)) {
        return Some((*id).clone());
    }
    norm.iter().find(|(t, _)| t.chars().count() >= 3 && n.contains(t.as_str())).map(|(_, id)| (*id).clone())
}

/// Cherche un modpack par nom (Modrinth, puis CurseForge) : `(source, id du projet)`.
pub async fn lookup_modpack(p: &Providers, name: &str, exact: bool) -> Result<(BrowseSource, String)> {
    let mut first_err: Option<Error> = None;
    let f = SearchFilters { project_type: Some("modpack".into()), limit: Some(20), ..Default::default() };
    match p.modrinth.search(name, &f).await {
        Ok(r) => {
            let c: Vec<_> = r.hits.into_iter().map(|h| (h.title, h.project_id)).collect();
            if let Some(id) = pick_best(name, &c, exact) {
                return Ok((BrowseSource::Modrinth, id));
            }
        }
        Err(e) => first_err = Some(e),
    }
    if p.cf.has_key() {
        match p.cf.search(cfapi::CLASS_MODPACK, name, None, None, cfapi::sort_field(None), 0, 20).await {
            Ok(r) => {
                let c: Vec<_> = r.data.into_iter().map(|m| (m.name, m.id.to_string())).collect();
                if let Some(id) = pick_best(name, &c, exact) {
                    return Ok((BrowseSource::Curseforge, id));
                }
            }
            Err(e) => {
                first_err.get_or_insert(e);
            }
        }
    }
    match first_err {
        Some(e) => Err(e),
        None => Err(Error::NotFound(format!("Aucun modpack correspondant à « {name} » trouvé. Choisis une image à la main."))),
    }
}

/// Recherche par le nom de l'instance, puis récupération de l'art. `auto` (sans demande de l'utilisateur) : jamais
/// pour une instance créée à la main, et seulement sur un nom identique (« Test » ne doit pas devenir « Testro's… »).
pub async fn lookup_and_apply(p: &Providers, store: &InstanceStore, instance_id: &str, auto: bool) -> Result<Instance> {
    let inst = store.get(instance_id)?;
    if auto && store.is_custom(instance_id) {
        return Err(Error::NotFound("instance créée à la main : pas d'illustration automatique".into()));
    }
    let (source, id) = lookup_modpack(p, &inst.name, auto).await?;
    apply_art(p, store, instance_id, source, &id).await
}

// ---------- Lecture et choix manuel ----------

fn check_file_name(file: &str) -> Result<()> {
    if file.is_empty() || file.len() > 64 || file == "." || file.contains("..") || file.contains(['/', '\\', ':', '\0']) {
        return Err(invalid("nom de fichier d'illustration invalide"));
    }
    Ok(())
}

/// Côté maximal (px) des miniatures renvoyées à l'interface.
pub const THUMB_MAX_PX: u32 = 512;
/// Au-delà, l'image source n'est pas décodée (bombe de décompression) : on renvoie l'original.
const MAX_DECODE_PIXELS: u64 = 40_000_000;
/// Sous-dossier de `art/` qui contient les miniatures en cache (supprimé avec `art/`).
const THUMB_DIR: &str = ".thumbs";
const THUMB_JPEG_QUALITY: u8 = 82;

pub fn data_url(mime: &str, bytes: &[u8]) -> String {
    format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// Miniature de `bytes` dont le plus grand côté vaut au plus `max` px, ré-encodée : PNG si l'image a de la
/// transparence, sinon JPEG. `None` si le format n'est pas décodable (p. ex. GIF), si l'image est
/// démesurée, ou si elle tient déjà dans `max` (l'original est alors renvoyé tel quel par l'appelant).
pub fn make_thumbnail(bytes: &[u8], max: u32) -> Option<(&'static str, Vec<u8>)> {
    use image::{ImageEncoder, ImageReader};
    let max = max.max(1);
    let dims = || ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?.into_dimensions().ok();
    let (w, h) = dims()?;
    if u64::from(w) * u64::from(h) > MAX_DECODE_PIXELS || (w <= max && h <= max) {
        return None;
    }
    let img = ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?.decode().ok()?;
    let small = img.thumbnail(max, max);
    let mut out = Vec::new();
    if small.color().has_alpha() {
        let rgba = small.to_rgba8();
        image::codecs::png::PngEncoder::new(&mut out)
            .write_image(rgba.as_raw(), rgba.width(), rgba.height(), image::ExtendedColorType::Rgba8)
            .ok()?;
        Some(("image/png", out))
    } else {
        let rgb = small.to_rgb8();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, THUMB_JPEG_QUALITY)
            .write_image(rgb.as_raw(), rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8)
            .ok()?;
        Some(("image/jpeg", out))
    }
}

/// Empreinte d'invalidation d'un fichier source : taille + date de modification (ns).
fn source_stamp(meta: &std::fs::Metadata) -> String {
    let ns = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    format!("{}-{ns}", meta.len())
}

/// Data URL d'une miniature (<= `THUMB_MAX_PX`) d'un fichier d'art appartenant à l'instance.
/// Voir [`image_data_url_sized`].
pub fn image_data_url(store: &InstanceStore, instance_id: &str, file: &str) -> Result<String> {
    image_data_url_sized(store, instance_id, file, THUMB_MAX_PX)
}

/// Data URL (`data:image/...;base64,...`) d'une miniature de côté maximal `max_px` (borné à 64..=2048).
///
/// La miniature est mise en cache dans `art/.thumbs/` sous un nom qui porte la taille et l'empreinte du
/// fichier source (taille + date) : si la source change, l'ancienne miniature est ignorée puis supprimée.
/// Si l'image n'est pas redimensionnable (GIF, illisible, déjà petite), l'original est renvoyé tel quel.
pub fn image_data_url_sized(store: &InstanceStore, instance_id: &str, file: &str, max_px: u32) -> Result<String> {
    check_file_name(file)?;
    let max = max_px.clamp(64, 2048);
    let inst = store.get(instance_id)?;
    let art = inst.art.ok_or_else(|| Error::NotFound("cette instance n'a pas d'illustration".into()))?;
    let owned = art.icon.as_deref() == Some(file) || art.screenshots.iter().any(|s| s == file);
    if !owned {
        return Err(invalid("fichier d'illustration inconnu pour cette instance"));
    }
    let art_dir = store.art_dir(instance_id)?;
    let path = art_dir.join(file);
    let meta = std::fs::symlink_metadata(&path).map_err(|_| Error::NotFound(format!("illustration {file}")))?;
    if !meta.is_file() || meta.len() > MAX_LOCAL_IMAGE_BYTES {
        return Err(invalid("illustration illisible"));
    }
    let thumbs = art_dir.join(THUMB_DIR);
    let prefix = format!("{file}.{max}.");
    let cached = format!("{prefix}{}", source_stamp(&meta));
    // Cache : lu seulement s'il est un fichier ordinaire, de taille raisonnable, d'un type image reconnu.
    for ext in ["jpg", "png"] {
        let p = thumbs.join(format!("{cached}.{ext}"));
        if let Ok(m) = std::fs::symlink_metadata(&p) {
            if m.is_file() && m.len() <= MAX_LOCAL_IMAGE_BYTES {
                if let Ok(b) = std::fs::read(&p) {
                    if let Some((mime, _)) = sniff(&b).filter(|(_, e)| *e == ext) {
                        return Ok(data_url(mime, &b));
                    }
                }
            }
        }
    }
    let bytes = std::fs::read(&path)?;
    let (mime, _) = sniff(&bytes).ok_or_else(|| invalid("format d'image non reconnu"))?;
    let Some((tmime, tbytes)) = make_thumbnail(&bytes, max) else {
        return Ok(data_url(mime, &bytes));
    };
    // Écriture au mieux : un échec (disque plein, lecture seule) ne doit jamais empêcher l'affichage.
    if std::fs::create_dir_all(&thumbs).is_ok() {
        let ext = if tmime == "image/png" { "png" } else { "jpg" };
        if write_atomic(&thumbs, &format!("{cached}.{ext}"), &tbytes).is_ok() {
            // Supprime les miniatures périmées de ce fichier (autre empreinte).
            if let Ok(rd) = std::fs::read_dir(&thumbs) {
                for e in rd.flatten() {
                    let n = e.file_name().to_string_lossy().into_owned();
                    if n.starts_with(&prefix) && n != format!("{cached}.jpg") && n != format!("{cached}.png") {
                        let _ = std::fs::remove_file(e.path());
                    }
                }
            }
        }
    }
    Ok(data_url(tmime, &tbytes))
}

/// Copie une image locale dans `art/` comme icône (`role = "icon"`) ou bannière (`"banner"` : première capture).
/// Crée l'art « custom » s'il n'existe pas ; supprime l'ancien fichier remplacé.
pub fn set_custom_image(store: &InstanceStore, instance_id: &str, path: &str, role: &str) -> Result<Instance> {
    if role != "icon" && role != "banner" {
        return Err(invalid("rôle d'image inconnu (icon ou banner)"));
    }
    let src = Path::new(path);
    if !src.is_absolute() {
        return Err(invalid("chemin d'image invalide"));
    }
    let ext_ok = src
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| ["png", "jpg", "jpeg", "webp"].iter().any(|x| e.eq_ignore_ascii_case(x)));
    if !ext_ok {
        return Err(invalid("l'image doit être au format png, jpg, jpeg ou webp"));
    }
    let inst = store.get(instance_id)?;
    let meta = std::fs::metadata(src).map_err(|_| Error::NotFound("image introuvable".into()))?;
    if !meta.is_file() {
        return Err(invalid("l'image doit être un fichier"));
    }
    if meta.len() > MAX_LOCAL_IMAGE_BYTES {
        return Err(invalid("image trop grosse (8 Mio maximum)"));
    }
    let bytes = std::fs::read(src)?;
    if bytes.len() as u64 > MAX_LOCAL_IMAGE_BYTES {
        return Err(invalid("image trop grosse (8 Mio maximum)"));
    }
    let ext = match sniff(&bytes) {
        Some((_, e)) if e != "gif" => e,
        _ => return Err(invalid("le contenu du fichier n'est pas une image png, jpeg ou webp")),
    };
    let dir = store.art_dir(instance_id)?;
    std::fs::create_dir_all(&dir)?;
    let stem = if role == "icon" { "icon" } else { "shot-1" };
    let name = format!("{stem}.{ext}");
    write_atomic(&dir, &name, &bytes)?;

    let mut art = inst.art.unwrap_or_else(|| InstanceArt {
        source: "custom".into(),
        project_id: String::new(),
        title: inst.name.clone(),
        author: String::new(),
        summary: String::new(),
        color: None,
        icon: None,
        screenshots: Vec::new(),
        page_url: None,
    });
    let old = if role == "icon" {
        art.icon.replace(name.clone())
    } else if art.screenshots.is_empty() {
        art.screenshots.push(name.clone());
        None
    } else {
        Some(std::mem::replace(&mut art.screenshots[0], name.clone()))
    };
    match store.set_art(instance_id, Some(art)) {
        Ok(updated) => {
            if let Some(o) = old.filter(|o| *o != name && check_file_name(o).is_ok()) {
                let _ = std::fs::remove_file(dir.join(o));
            }
            Ok(updated)
        }
        Err(e) => {
            let _ = std::fs::remove_file(dir.join(&name));
            Err(e)
        }
    }
}

/// Cache de la description longue, dans `art/` (supprimé avec les illustrations).
/// Le suffixe change quand le nettoyage change, pour ne pas resservir un ancien cache.
const DESCRIPTION_FILE: &str = "description-v3.html";
/// Au-delà, la description en cache est redemandée (en cas d'échec réseau, l'ancienne reste servie).
const DESCRIPTION_TTL: Duration = Duration::from_secs(24 * 3600);
const MAX_DESCRIPTION_BYTES: usize = 512 * 1024;

/// Nettoie du HTML venu d'un hébergeur : balises de mise en forme, liens et images en http(s) seulement ;
/// pas de script, de style, d'iframe ni d'attribut d'événement. Les liens s'ouvrent dans le navigateur.
pub fn sanitize_description(html: &str) -> String {
    let clean = ammonia::Builder::default()
        .url_schemes(["http", "https"].into_iter().collect())
        .link_rel(Some("noopener noreferrer"))
        .set_tag_attribute_value("a", "target", "_blank")
        // Centrage d'origine (boutons, bannières) : seul `text-align` survit dans `style`.
        .add_generic_attributes(["style", "align"])
        .filter_style_properties(["text-align"].into_iter().collect())
        .clean(html)
        .to_string();
    drop_empty_blocks(&clean)
}

/// Retire les blocs vides servant d'espacement (`<p>&nbsp;</p>`, `<h2><br></h2>`…), qui creusent des trous.
fn drop_empty_blocks(html: &str) -> String {
    let mut out = html.replace(r#" style="""#, "");
    // Plusieurs passes : vider un `<span>` peut laisser son `<h2>` parent vide.
    loop {
        let before = out.len();
        for tag in ["span", "strong", "em", "b", "i", "p", "h1", "h2", "h3", "h4", "h5", "h6", "div"] {
            for inner in ["", "&nbsp;", "<br>", " ", "&nbsp;<br>", "<br>&nbsp;"] {
                out = out.replace(&format!("<{tag}>{inner}</{tag}>"), "");
            }
        }
        if out.len() == before { return out; }
    }
}

/// Markdown (Modrinth, HTML intégré accepté) vers HTML, avant nettoyage.
fn markdown_to_html(md: &str) -> String {
    use pulldown_cmark::{Options, Parser};
    let mut out = String::new();
    pulldown_cmark::html::push_html(&mut out, Parser::new_ext(md, Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH));
    out
}

/// Description longue d'un projet, en HTML nettoyé.
pub async fn fetch_description(p: &Providers, source: BrowseSource, project_id: &str) -> Result<String> {
    let raw = match source {
        BrowseSource::Modrinth => markdown_to_html(&p.modrinth.project(project_id).await?.body),
        BrowseSource::Curseforge => {
            let n: u64 = project_id.trim().parse().map_err(|_| invalid("identifiant CurseForge invalide"))?;
            p.cf.mod_description(n).await?
        }
    };
    let mut html = sanitize_description(&raw);
    if html.len() > MAX_DESCRIPTION_BYTES {
        let mut cut = MAX_DESCRIPTION_BYTES;
        while !html.is_char_boundary(cut) {
            cut -= 1;
        }
        html = sanitize_description(&html[..cut]);
    }
    Ok(html)
}

/// Description longue de l'instance (HTML nettoyé), depuis le cache de `art/` ou l'hébergeur d'origine.
/// `None` pour une instance sans projet d'origine (illustration « custom » ou absente).
pub async fn instance_description(p: &Providers, store: &InstanceStore, instance_id: &str) -> Result<Option<String>> {
    let inst = store.get(instance_id)?;
    let Some(art) = inst.art else { return Ok(None) };
    let source = match art.source.as_str() {
        "modrinth" => BrowseSource::Modrinth,
        "curseforge" => BrowseSource::Curseforge,
        _ => return Ok(None),
    };
    let dir = store.art_dir(instance_id)?;
    let path = dir.join(DESCRIPTION_FILE);
    let cached = std::fs::read_to_string(&path).ok();
    let fresh = std::fs::metadata(&path).ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).is_some_and(|age| age < DESCRIPTION_TTL);
    if let (Some(c), true) = (&cached, fresh) {
        return Ok(Some(c.clone()));
    }
    match fetch_description(p, source, &art.project_id).await {
        Ok(html) => {
            if std::fs::create_dir_all(&dir).is_ok() {
                let _ = write_atomic(&dir, DESCRIPTION_FILE, html.as_bytes());
            }
            Ok(Some(html))
        }
        Err(e) => match cached {
            Some(c) => {
                tracing::warn!("description : hébergeur indisponible, cache servi : {e}");
                Ok(Some(c))
            }
            None => Err(e),
        },
    }
}

#[cfg(test)]
mod description_tests {
    use super::*;

    #[test]
    fn nettoyage_garde_le_centrage_et_rien_d_autre_dans_style() {
        let h = sanitize_description(r#"<p style="text-align: center; background: url(https://e.vil/x)">a</p><div align="center">b</div>"#);
        assert!(h.contains("text-align"), "{h}");
        assert!(!h.contains("background") && !h.contains("e.vil"), "{h}");
        assert!(h.contains(r#"align="center""#), "{h}");
    }

    #[test]
    fn nettoyage_retire_les_blocs_vides() {
        let h = sanitize_description(r#"<p>a</p><p>&nbsp;</p><h2><span style="color:red">&nbsp;</span></h2><p><br></p><p>b</p>"#);
        assert_eq!(h, "<p>a</p><p>b</p>");
    }

    #[test]
    fn nettoyage_retire_le_dangereux_et_garde_la_mise_en_forme() {
        let h = sanitize_description(r#"<h2>Titre</h2><p onclick="x()">Texte <b>gras</b></p><script>alert(1)</script><iframe src="https://e.vil"></iframe><a href="javascript:alert(1)">j</a><a href="https://modrinth.com">ok</a><img src="https://cdn.modrinth.com/a.png" onerror="x()">"#);
        assert!(h.contains("<h2>Titre</h2>") && h.contains("<b>gras</b>"));
        assert!(!h.contains("script") && !h.contains("iframe") && !h.contains("onclick") && !h.contains("onerror") && !h.contains("javascript"));
        assert!(h.contains(r#"href="https://modrinth.com""#) && h.contains(r#"target="_blank""#));
        assert!(h.contains(r#"src="https://cdn.modrinth.com/a.png""#));
    }

    #[test]
    fn markdown_devient_html() {
        let h = sanitize_description(&markdown_to_html("# Pack\n\n**Fort** et [lien](https://x.org)\n\n<center>ok</center>"));
        assert!(h.contains("<h1>Pack</h1>") && h.contains("<strong>Fort</strong>") && h.contains("https://x.org"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::curseforge_api::CfApi;
    use crate::providers::modrinth_api::ModrinthClient;
    use crate::types::{Loader, LoaderSpec};
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3, 4];
    const JPG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3];

    fn providers(mr: &MockServer, cf: &MockServer, allow_loopback: bool) -> Providers {
        Providers {
            modrinth: ModrinthClient::with_base_url(format!("{}/v2", mr.uri()), "test"),
            cf: CfApi::with_base(cf.uri(), "k"),
            allow_loopback,
        }
    }

    fn store() -> (tempfile::TempDir, InstanceStore, Instance) {
        let t = tempfile::tempdir().unwrap();
        let s = InstanceStore::new(t.path().join("instances"));
        let i = s
            .create("Mon Pack", "1.21.1", LoaderSpec { loader: Loader::Fabric, version: "0.1".into() }, 2048)
            .unwrap();
        (t, s, i)
    }

    async fn serve(s: &MockServer, p: &str, body: &[u8]) {
        Mock::given(method("GET")).and(path(p.to_string())).respond_with(ResponseTemplate::new(200).set_body_bytes(body.to_vec())).mount(s).await;
    }

    async fn modrinth_project(mr: &MockServer, id: &str, body: serde_json::Value) {
        Mock::given(method("GET")).and(path(format!("/v2/project/{id}"))).respond_with(ResponseTemplate::new(200).set_body_json(body)).mount(mr).await;
    }

    #[test]
    fn sniff_and_color() {
        assert_eq!(sniff(PNG), Some(("image/png", "png")));
        assert_eq!(sniff(JPG), Some(("image/jpeg", "jpg")));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 ").unwrap().1, "webp");
        assert_eq!(sniff(b"GIF89a..").unwrap().1, "gif");
        assert_eq!(sniff(b"<svg/>"), None);
        assert_eq!(hex_color(0x0A1B2C), "#0a1b2c");
        assert_eq!(hex_color(0xFF), "#0000ff");
    }

    #[test]
    fn url_rules() {
        assert!(check_image_url("https://cdn.modrinth.com/a.png", false).is_ok());
        assert!(check_image_url("https://media.forgecdn.net/a.png", false).is_ok());
        assert!(check_image_url("http://cdn.modrinth.com/a.png", false).is_err());
        assert!(check_image_url("https://evil.example.com/a.png", false).is_err());
        assert!(check_image_url("https://u:p@cdn.modrinth.com/a.png", false).is_err());
        assert!(check_image_url("http://127.0.0.1:1/a.png", false).is_err());
        assert!(check_image_url("http://127.0.0.1:1/a.png", true).is_ok());
        assert!(check_image_url("http://evil.example.com/a.png", true).is_err());
    }

    #[tokio::test]
    async fn modrinth_art_with_gallery_color_and_author() {
        let (mr, cf) = (MockServer::start().await, MockServer::start().await);
        modrinth_project(
            &mr,
            "P1",
            json!({"id": "P1", "slug": "mon-pack", "title": "Mon Pack", "description": "résumé", "icon_url": format!("{}/img/icon", mr.uri()),
                   "color": 0x00AABB, "gallery": [
                {"url": format!("{}/img/b", mr.uri()), "featured": false, "ordering": 1},
                {"url": format!("{}/img/a", mr.uri()), "featured": true, "ordering": 5},
                {"url": format!("{}/img/c", mr.uri()), "featured": false, "ordering": 0}]}),
        )
        .await;
        Mock::given(method("GET"))
            .and(path("/v2/project/P1/members"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"role": "Member", "user": {"username": "bob"}}, {"role": "Owner", "user": {"username": "alice"}}])))
            .mount(&mr)
            .await;
        serve(&mr, "/img/icon", PNG).await;
        serve(&mr, "/img/a", JPG).await;
        serve(&mr, "/img/b", PNG).await;
        serve(&mr, "/img/c", PNG).await;
        let (_t, s, i) = store();
        let p = providers(&mr, &cf, true);
        let u = apply_art(&p, &s, &i.id, BrowseSource::Modrinth, "P1").await.unwrap();
        let a = u.art.unwrap();
        assert_eq!((a.source.as_str(), a.title.as_str(), a.author.as_str(), a.summary.as_str()), ("modrinth", "Mon Pack", "alice", "résumé"));
        assert_eq!(a.color.as_deref(), Some("#00aabb"));
        assert_eq!(a.icon.as_deref(), Some("icon.png"));
        // featured d'abord, puis ordering croissant.
        assert_eq!(a.screenshots, vec!["shot-1.jpg", "shot-2.png", "shot-3.png"]);
        assert_eq!(a.page_url.as_deref(), Some("https://modrinth.com/modpack/mon-pack"));
        let dir = s.art_dir(&i.id).unwrap();
        assert!(dir.join("icon.png").is_file() && dir.join("shot-1.jpg").is_file());
        assert!(!dir.with_file_name("art.tmp").exists());
        assert_eq!(s.get(&i.id).unwrap().art.unwrap().title, "Mon Pack");
        // Le data URL est lisible et le type vient des octets.
        assert!(image_data_url(&s, &i.id, "shot-1.jpg").unwrap().starts_with("data:image/jpeg;base64,"));
        assert!(image_data_url(&s, &i.id, "icon.png").unwrap().starts_with("data:image/png;base64,"));
        // Une nouvelle récupération remplace tout le dossier.
        std::fs::write(dir.join("vieux.txt"), b"x").unwrap();
        apply_art(&p, &s, &i.id, BrowseSource::Modrinth, "P1").await.unwrap();
        assert!(!dir.join("vieux.txt").exists());
    }

    #[tokio::test]
    async fn bad_images_are_skipped_but_art_is_kept() {
        let (mr, cf) = (MockServer::start().await, MockServer::start().await);
        let big = vec![0x89u8; 1]
            .into_iter()
            .chain(b"PNG\r\n\x1a\n".iter().copied())
            .chain(std::iter::repeat_n(0u8, (MAX_IMAGE_BYTES as usize) + 10))
            .collect::<Vec<u8>>();
        modrinth_project(
            &mr,
            "P2",
            json!({"id": "P2", "slug": "x", "title": "X", "description": "d", "icon_url": format!("{}/img/svg", mr.uri()), "color": null,
                   "gallery": [
                {"url": format!("{}/img/big", mr.uri()), "featured": true, "ordering": 0},
                {"url": "https://evil.example.com/x.png", "featured": false, "ordering": 1},
                {"url": "http://cdn.modrinth.com/x.png", "featured": false, "ordering": 2},
                {"url": format!("{}/img/ok", mr.uri()), "featured": false, "ordering": 3}]}),
        )
        .await;
        serve(&mr, "/img/svg", b"<svg xmlns='http://www.w3.org/2000/svg'/>").await;
        serve(&mr, "/img/big", &big).await;
        serve(&mr, "/img/ok", PNG).await;
        let (_t, s, i) = store();
        let a = apply_art(&providers(&mr, &cf, true), &s, &i.id, BrowseSource::Modrinth, "P2").await.unwrap().art.unwrap();
        assert_eq!(a.icon, None);
        assert_eq!(a.color, None);
        assert_eq!(a.author, "");
        assert_eq!(a.screenshots, vec!["shot-1.png"]);
        let names: Vec<_> = std::fs::read_dir(s.art_dir(&i.id).unwrap()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["shot-1.png"], "aucun résidu (tmp, image refusée)");
    }

    #[tokio::test]
    async fn loopback_refused_without_flag() {
        let (mr, cf) = (MockServer::start().await, MockServer::start().await);
        modrinth_project(&mr, "P3", json!({"id": "P3", "title": "T", "icon_url": format!("{}/img/i", mr.uri())})).await;
        serve(&mr, "/img/i", PNG).await;
        let (_t, s, i) = store();
        let a = apply_art(&providers(&mr, &cf, false), &s, &i.id, BrowseSource::Modrinth, "P3").await.unwrap().art.unwrap();
        assert_eq!(a.icon, None);
    }

    #[tokio::test]
    async fn curseforge_art() {
        let (mr, cf) = (MockServer::start().await, MockServer::start().await);
        Mock::given(method("GET"))
            .and(path("/v1/mods/42"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {
                "id": 42, "name": "CF Pack", "summary": "sum", "slug": "cf-pack",
                "links": {"websiteUrl": "https://www.curseforge.com/minecraft/modpacks/cf-pack"},
                "authors": [{"name": "zed"}, {"name": "other"}],
                "logo": {"url": format!("{}/cdn/logo", cf.uri()), "thumbnailUrl": "https://media.forgecdn.net/t.png"},
                "screenshots": [{"url": format!("{}/cdn/s1", cf.uri())}, {"url": format!("{}/cdn/s2", cf.uri())}]}})))
            .mount(&cf)
            .await;
        serve(&cf, "/cdn/logo", PNG).await;
        serve(&cf, "/cdn/s1", JPG).await;
        serve(&cf, "/cdn/s2", PNG).await;
        let (_t, s, i) = store();
        let a = apply_art(&providers(&mr, &cf, true), &s, &i.id, BrowseSource::Curseforge, "42").await.unwrap().art.unwrap();
        assert_eq!((a.source.as_str(), a.title.as_str(), a.author.as_str(), a.summary.as_str()), ("curseforge", "CF Pack", "zed", "sum"));
        assert_eq!(a.color, None);
        assert_eq!(a.icon.as_deref(), Some("icon.png"));
        assert_eq!(a.screenshots, vec!["shot-1.jpg", "shot-2.png"]);
        assert_eq!(a.page_url.as_deref(), Some("https://www.curseforge.com/minecraft/modpacks/cf-pack"));
    }

    #[tokio::test]
    async fn at_most_six_screenshots() {
        let (mr, cf) = (MockServer::start().await, MockServer::start().await);
        let gallery: Vec<_> = (0..9).map(|n| json!({"url": format!("{}/img/{n}", mr.uri()), "featured": false, "ordering": n})).collect();
        modrinth_project(&mr, "P4", json!({"id": "P4", "title": "T", "gallery": gallery})).await;
        for n in 0..9 {
            serve(&mr, &format!("/img/{n}"), PNG).await;
        }
        let (_t, s, i) = store();
        let a = apply_art(&providers(&mr, &cf, true), &s, &i.id, BrowseSource::Modrinth, "P4").await.unwrap().art.unwrap();
        assert_eq!(a.screenshots.len(), 6);
    }

    #[tokio::test]
    async fn modrinth_prefers_full_resolution_and_falls_back_to_reduced() {
        let (mr, cf) = (MockServer::start().await, MockServer::start().await);
        let u = mr.uri();
        modrinth_project(
            &mr,
            "P5",
            json!({"id": "P5", "title": "T", "gallery": [
                {"url": format!("{u}/img/a_350"), "raw_url": format!("{u}/img/a"), "ordering": 0},
                {"url": format!("{u}/img/b_350"), "raw_url": format!("{u}/img/b-absente"), "ordering": 1}]}),
        )
        .await;
        serve(&mr, "/img/a", JPG).await; // original de la 1re : jpeg
        serve(&mr, "/img/a_350", PNG).await;
        serve(&mr, "/img/b_350", PNG).await; // original de la 2e introuvable : version réduite
        let (_t, s, i) = store();
        let a = apply_art(&providers(&mr, &cf, true), &s, &i.id, BrowseSource::Modrinth, "P5").await.unwrap().art.unwrap();
        assert_eq!(a.screenshots, vec!["shot-1.jpg", "shot-2.png"]);
    }

    #[tokio::test]
    async fn network_failure_never_fails_install() {
        let (mr, cf) = (MockServer::start().await, MockServer::start().await);
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(500)).mount(&mr).await;
        let (_t, s, i) = store();
        let p = providers(&mr, &cf, true);
        assert!(apply_art(&p, &s, &i.id, BrowseSource::Modrinth, "P").await.is_err());
        let out = try_apply_art(&p, &s, i.clone(), BrowseSource::Modrinth, "P").await;
        assert_eq!(out, i);
        assert!(!s.art_dir(&i.id).unwrap().exists() && !s.art_dir(&i.id).unwrap().with_file_name("art.tmp").exists());
        assert_eq!(s.get(&i.id).unwrap().art, None);
        // Projet invalide côté CurseForge : même chose.
        let out = try_apply_art(&p, &s, i.clone(), BrowseSource::Curseforge, "pas-un-nombre").await;
        assert_eq!(out, i);
    }

    #[tokio::test]
    async fn try_apply_updates_on_success() {
        let (mr, cf) = (MockServer::start().await, MockServer::start().await);
        modrinth_project(&mr, "P5", json!({"id": "P5", "title": "Ok"})).await;
        let (_t, s, i) = store();
        let out = try_apply_art(&providers(&mr, &cf, true), &s, i, BrowseSource::Modrinth, "P5").await;
        assert_eq!(out.art.unwrap().title, "Ok");
    }

    #[tokio::test]
    async fn lookup_exact_match_then_none() {
        let (mr, cf) = (MockServer::start().await, MockServer::start().await);
        Mock::given(method("GET"))
            .and(path("/v2/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"hits": [
                {"project_id": "A", "slug": "a", "title": "Mon Pack Deluxe Edition", "author": "x"},
                {"project_id": "B", "slug": "b", "title": "mon-pack", "author": "x"}], "total_hits": 2})))
            .mount(&mr)
            .await;
        Mock::given(method("GET")).and(path("/v1/mods/search")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": []}))).mount(&cf).await;
        let p = providers(&mr, &cf, true);
        assert_eq!(lookup_modpack(&p, "Mon Pack", false).await.unwrap(), (BrowseSource::Modrinth, "B".to_string()));
        // Contenant : « Mon Pack Deluxe Edition » contient « Mon Pack Deluxe ».
        assert_eq!(lookup_modpack(&p, "Mon Pack Deluxe", false).await.unwrap(), (BrowseSource::Modrinth, "A".to_string()));
        match lookup_modpack(&p, "Totalement Autre", false).await.unwrap_err() {
            Error::NotFound(m) => assert_eq!(m, "Aucun modpack correspondant à « Totalement Autre » trouvé. Choisis une image à la main."),
            e => panic!("{e:?}"),
        }
    }

    #[tokio::test]
    async fn lookup_falls_back_to_curseforge_and_applies() {
        let (mr, cf) = (MockServer::start().await, MockServer::start().await);
        Mock::given(method("GET")).and(path("/v2/search")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"hits": [], "total_hits": 0}))).mount(&mr).await;
        Mock::given(method("GET"))
            .and(path("/v1/mods/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": [{"id": 7, "name": "Mon Pack"}]})))
            .mount(&cf)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/mods/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {"id": 7, "name": "Mon Pack", "summary": "s"}})))
            .mount(&cf)
            .await;
        let (_t, s, i) = store();
        let u = lookup_and_apply(&providers(&mr, &cf, true), &s, &i.id, false).await.unwrap();
        let a = u.art.unwrap();
        assert_eq!((a.source.as_str(), a.project_id.as_str()), ("curseforge", "7"));
    }

    #[test]
    fn data_url_refuses_foreign_files() {
        let (_t, s, i) = store();
        let dir = s.art_dir(&i.id).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("secret.png"), PNG).unwrap();
        std::fs::write(dir.join("icon.png"), PNG).unwrap();
        // Pas d'art : refusé.
        assert!(image_data_url(&s, &i.id, "icon.png").is_err());
        let art = InstanceArt {
            source: "custom".into(),
            project_id: String::new(),
            title: "t".into(),
            author: String::new(),
            summary: String::new(),
            color: None,
            icon: Some("icon.png".into()),
            screenshots: vec![],
            page_url: None,
        };
        s.set_art(&i.id, Some(art)).unwrap();
        assert!(image_data_url(&s, &i.id, "icon.png").is_ok());
        for bad in ["secret.png", "../instance.json", "..", "a/b.png", "a\\b.png", "", "..\\x", "C:\\x.png", "/etc/passwd"] {
            assert!(image_data_url(&s, &i.id, bad).is_err(), "{bad:?}");
        }
        assert!(image_data_url(&s, "../x", "icon.png").is_err());
        // Contenu qui n'est pas une image : refusé.
        std::fs::write(dir.join("icon.png"), b"<svg/>").unwrap();
        assert!(image_data_url(&s, &i.id, "icon.png").is_err());
    }

    #[test]
    fn set_custom_image_rules_and_replacement() {
        let (t, s, i) = store();
        let f = |n: &str, b: &[u8]| {
            let p = t.path().join(n);
            std::fs::write(&p, b).unwrap();
            p.display().to_string()
        };
        let png = f("a.png", PNG);
        // Rôle, chemin relatif, extension, magic, taille.
        assert!(set_custom_image(&s, &i.id, &png, "logo").is_err());
        assert!(set_custom_image(&s, &i.id, "a.png", "icon").is_err());
        assert!(set_custom_image(&s, &i.id, &f("a.gif", b"GIF89a.."), "icon").is_err());
        assert!(set_custom_image(&s, &i.id, &f("b.png", b"pas une image"), "icon").is_err());
        assert!(set_custom_image(&s, &i.id, &f("c.jpg", PNG), "icon").is_ok(), "le type vient des octets, pas de l'extension");
        let big = vec![0xFFu8, 0xD8, 0xFF].into_iter().chain(std::iter::repeat_n(0u8, MAX_LOCAL_IMAGE_BYTES as usize)).collect::<Vec<_>>();
        assert!(set_custom_image(&s, &i.id, &f("big.jpg", &big), "banner").is_err());
        assert!(set_custom_image(&s, &i.id, &t.path().join("absent.png").display().to_string(), "icon").is_err());
        assert!(set_custom_image(&s, "../x", &png, "icon").is_err());

        let a = s.get(&i.id).unwrap().art.unwrap();
        assert_eq!((a.source.as_str(), a.title.as_str(), a.icon.as_deref()), ("custom", "Mon Pack", Some("icon.png")));
        // Remplacement d'icône par un jpeg : l'ancien fichier disparaît.
        let jpg = f("d.jpeg", JPG);
        let u = set_custom_image(&s, &i.id, &jpg, "icon").unwrap();
        assert_eq!(u.art.as_ref().unwrap().icon.as_deref(), Some("icon.jpg"));
        let dir = s.art_dir(&i.id).unwrap();
        assert!(!dir.join("icon.png").exists() && dir.join("icon.jpg").is_file());
        // Bannière : première capture, remplacée sans doublon.
        let u = set_custom_image(&s, &i.id, &png, "banner").unwrap();
        assert_eq!(u.art.as_ref().unwrap().screenshots, vec!["shot-1.png"]);
        let u = set_custom_image(&s, &i.id, &jpg, "banner").unwrap();
        assert_eq!(u.art.unwrap().screenshots, vec!["shot-1.jpg"]);
        assert!(!dir.join("shot-1.png").exists() && dir.join("shot-1.jpg").is_file());
    }

    // ---------- miniatures ----------

    fn encode_img(w: u32, h: u32, alpha: bool, fmt: image::ImageFormat) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        if alpha {
            let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x % 256) as u8, (y % 256) as u8, 7, if x % 2 == 0 { 255 } else { 90 }]));
            image::DynamicImage::ImageRgba8(img).write_to(&mut out, fmt).unwrap();
        } else {
            let img = image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 7]));
            image::DynamicImage::ImageRgb8(img).write_to(&mut out, fmt).unwrap();
        }
        out.into_inner()
    }

    fn decode_data_url(u: &str) -> (String, image::DynamicImage, Vec<u8>) {
        let (head, b64) = u.strip_prefix("data:").unwrap().split_once(";base64,").unwrap();
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64).unwrap();
        (head.to_string(), image::load_from_memory(&bytes).unwrap(), bytes)
    }

    fn custom(s: &InstanceStore, id: &str, dir: &Path, name: &str, bytes: &[u8], role: &str) {
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        set_custom_image(s, id, &p.display().to_string(), role).unwrap();
    }

    #[test]
    fn thumbnail_downscales_caches_and_invalidates() {
        let (t, s, i) = store();
        let big = encode_img(1600, 900, false, image::ImageFormat::Jpeg);
        custom(&s, &i.id, t.path(), "big.jpg", &big, "icon");
        let url = image_data_url(&s, &i.id, "icon.jpg").unwrap();
        let (mime, img, bytes) = decode_data_url(&url);
        assert_eq!(mime, "image/jpeg");
        assert_eq!((img.width(), img.height()), (512, 288), "ratio conservé, plus grand côté 512");
        assert!(bytes.len() < big.len(), "plus léger que l'original");
        // Cache sur disque, une seule miniature ; un second appel renvoie la même chose.
        let thumbs = s.art_dir(&i.id).unwrap().join(".thumbs");
        let count = || std::fs::read_dir(&thumbs).unwrap().flatten().count();
        assert_eq!(count(), 1);
        assert_eq!(image_data_url(&s, &i.id, "icon.jpg").unwrap(), url);
        // Le cache est réellement lu : on y met une autre image valide, elle est renvoyée telle quelle.
        let cached = std::fs::read_dir(&thumbs).unwrap().flatten().next().unwrap().path();
        let marker = encode_img(10, 10, false, image::ImageFormat::Jpeg);
        std::fs::write(&cached, &marker).unwrap();
        let (_, m, _) = decode_data_url(&image_data_url(&s, &i.id, "icon.jpg").unwrap());
        assert_eq!((m.width(), m.height()), (10, 10));
        // Cache corrompu : regénéré.
        std::fs::write(&cached, b"pourri").unwrap();
        let (_, r, _) = decode_data_url(&image_data_url(&s, &i.id, "icon.jpg").unwrap());
        assert_eq!(r.width(), 512);
        // La source change (même nom, autre contenu) : nouvelle miniature, l'ancienne est supprimée.
        let big2 = encode_img(900, 1800, false, image::ImageFormat::Jpeg);
        custom(&s, &i.id, t.path(), "big2.jpg", &big2, "icon");
        let (_, n, _) = decode_data_url(&image_data_url(&s, &i.id, "icon.jpg").unwrap());
        assert_eq!((n.width(), n.height()), (256, 512));
        assert_eq!(count(), 1, "miniature périmée supprimée");
        // Taille demandée : bornée et mise en cache à part.
        let (_, small, _) = decode_data_url(&image_data_url_sized(&s, &i.id, "icon.jpg", 128).unwrap());
        assert_eq!(small.height(), 128);
        assert_eq!(count(), 2);
        let (_, huge, _) = decode_data_url(&image_data_url_sized(&s, &i.id, "icon.jpg", 1).unwrap());
        assert_eq!(huge.height(), 64, "plancher à 64 px");
    }

    #[test]
    fn thumbnail_keeps_alpha_and_passes_small_or_undecodable_images_through() {
        let (t, s, i) = store();
        // Transparence : PNG conservé.
        let rgba = encode_img(700, 700, true, image::ImageFormat::Png);
        custom(&s, &i.id, t.path(), "a.png", &rgba, "icon");
        let (mime, img, _) = decode_data_url(&image_data_url(&s, &i.id, "icon.png").unwrap());
        assert_eq!((mime.as_str(), img.width(), img.color().has_alpha()), ("image/png", 512, true));
        // Déjà petite : octets d'origine, aucun cache.
        let small = encode_img(100, 60, false, image::ImageFormat::Png);
        custom(&s, &i.id, t.path(), "s.png", &small, "banner");
        let u = image_data_url(&s, &i.id, "shot-1.png").unwrap();
        assert_eq!(decode_data_url(&u).2, small);
        // Faux PNG (octets magiques seuls) : original renvoyé, pas d'erreur.
        let dir = s.art_dir(&i.id).unwrap();
        std::fs::write(dir.join("shot-1.png"), PNG).unwrap();
        assert!(image_data_url(&s, &i.id, "shot-1.png").unwrap().starts_with("data:image/png;base64,"));
        // Les noms hors de l'instance restent refusés, même avec le cache.
        assert!(image_data_url(&s, &i.id, ".thumbs").is_err());
        assert!(image_data_url(&s, &i.id, "../icon.png").is_err());
    }

    #[test]
    fn pick_best_prefere_le_bon_numero_de_version() {
        let c: Vec<(String, String)> = [
            ("Cobblemon SMP - The #1 Legendary Pokemon Adventure", "1"),
            ("All the Mods 10: To the Sky   ATM10SKY", "2"),
            ("All the Mods 10 - ATM10", "3"),
            ("All the Mods 11 - ATM11", "4"),
            ("All the Mods 11 Lite", "5"),
        ]
        .iter()
        .map(|(t, i)| (t.to_string(), i.to_string()))
        .collect();
        assert_eq!(pick_best("All the Mods 11", &c, false).as_deref(), Some("4"));
        assert_eq!(pick_best("All the Mods 12", &c, false), None);
        assert_eq!(pick_best("ab", &c, false), None);
        // Recherche automatique : nom identique seulement (« Test » ≠ « Testro's Minecraft Overhaul »).
        let t = vec![("Testro's Minecraft Overhaul".to_string(), "T".to_string())];
        assert_eq!(pick_best("Test", &t, false).as_deref(), Some("T"));
        assert_eq!(pick_best("Test", &t, true), None);
        assert_eq!(pick_best("testro's minecraft overhaul", &t, true).as_deref(), Some("T"));
    }
}
