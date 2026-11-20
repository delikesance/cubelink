//! Captures d'écran du joueur (F2 en jeu) : galerie en lecture seule de `<dossier de jeu>/screenshots/`.
//!
//! Ces fichiers sont écrits par le jeu, hors de `art/` et hors du modèle `InstanceArt` : on ne les copie pas, on
//! les lit à la demande. Le dossier est contrôlé par le jeu et par les mods : chaque nom est validé strictement,
//! les liens symboliques et les sous-dossiers sont ignorés, et le contenu réel est vérifié (octets magiques), pas
//! l'extension. Le front ne fournit jamais de chemin, seulement un nom de fichier relatif à ce dossier.
//! Pas de partage vers un ami ici : il demanderait un protocole P2P et une acceptation explicite (autre chantier).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::art::{check_file_name, data_url, make_thumbnail, set_custom_image, sniff, MAX_IMAGE_BYTES, MAX_LOCAL_IMAGE_BYTES, THUMB_MAX_PX};
use crate::instance::InstanceStore;
use crate::types::{Error, Instance, Result};

/// Sous-dossier du dossier de jeu où Minecraft range les captures.
pub const SCREENSHOTS_DIR: &str = "screenshots";
/// Nombre maximal de captures listées (les plus récentes) : au-delà, l'écran serait inutilisable.
pub const MAX_LISTED: usize = 200;

/// Une capture du joueur.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct PlayerShot {
    /// Nom du fichier dans `screenshots/` (jamais un chemin).
    pub file: String,
    /// Date de modification, en millisecondes depuis l'époque Unix (0 si inconnue).
    pub modified_ms: u64,
    /// Taille en octets.
    pub size: u64,
}

fn invalid(m: &str) -> Error {
    Error::Invalid(m.into())
}

/// Dossier des captures de l'instance (peut ne pas exister avant la première capture).
pub fn screenshots_dir(store: &InstanceStore, instance_id: &str) -> Result<PathBuf> {
    Ok(store.game_dir(instance_id)?.join(SCREENSHOTS_DIR))
}

fn has_image_extension(name: &str) -> bool {
    std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| ["png", "jpg", "jpeg", "webp"].iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// Fichier ordinaire (ni lien symbolique ni dossier) de `screenshots/`, d'un nom et d'une extension admis.
fn shot_path(store: &InstanceStore, instance_id: &str, file: &str) -> Result<(PathBuf, std::fs::Metadata)> {
    check_file_name(file)?;
    if !has_image_extension(file) {
        return Err(invalid("ce fichier n'est pas une capture (png, jpg ou webp)"));
    }
    let path = screenshots_dir(store, instance_id)?.join(file);
    let meta = std::fs::symlink_metadata(&path).map_err(|_| Error::NotFound(format!("capture {file}")))?;
    if !meta.is_file() {
        return Err(invalid("capture illisible"));
    }
    Ok((path, meta))
}

/// Captures de l'instance, les plus récentes d'abord (au plus [`MAX_LISTED`]). Dossier absent : liste vide.
pub fn list_player_shots(store: &InstanceStore, instance_id: &str) -> Result<Vec<PlayerShot>> {
    let dir = screenshots_dir(store, instance_id)?;
    let rd = match std::fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut out: Vec<PlayerShot> = Vec::new();
    for entry in rd.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else { continue };
        if check_file_name(&name).is_err() || !has_image_extension(&name) {
            continue;
        }
        // `DirEntry::metadata` ne suit pas les liens symboliques : un lien n'est jamais un fichier ordinaire.
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() || meta.len() > MAX_IMAGE_BYTES {
            continue;
        }
        let modified_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_millis() as u64);
        out.push(PlayerShot { file: name, modified_ms, size: meta.len() });
    }
    // Plus récent d'abord ; à date égale, ordre inverse du nom (les noms du jeu portent la date et l'heure).
    out.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms).then_with(|| b.file.cmp(&a.file)));
    out.truncate(MAX_LISTED);
    Ok(out)
}

/// Data URL d'une miniature (côté maximal `max_px`, borné à 64..=2048) d'une capture de l'instance.
///
/// Une image qui ne se réduit pas (déjà petite, trop grande pour être décodée) part telle quelle, sauf au-delà de
/// `MAX_LOCAL_IMAGE_BYTES` (erreur : une data URL de plus de 16 Mio par l'IPC figerait l'interface).
pub fn player_shot_data_url(store: &InstanceStore, instance_id: &str, file: &str, max_px: Option<u32>) -> Result<String> {
    let max = max_px.unwrap_or(THUMB_MAX_PX).clamp(64, 2048);
    let (path, meta) = shot_path(store, instance_id, file)?;
    if meta.len() > MAX_IMAGE_BYTES {
        return Err(invalid("capture trop lourde pour être affichée"));
    }
    let bytes = std::fs::read(&path)?;
    let mime = match sniff(&bytes) {
        Some((m, e)) if e != "gif" => m,
        _ => return Err(invalid("le contenu du fichier n'est pas une image png, jpeg ou webp")),
    };
    match make_thumbnail(&bytes, max) {
        Some((tmime, tbytes)) => Ok(data_url(tmime, &tbytes)),
        None if bytes.len() as u64 > MAX_LOCAL_IMAGE_BYTES => Err(invalid("capture trop lourde pour être affichée")),
        None => Ok(data_url(mime, &bytes)),
    }
}

/// Utilise une capture du joueur comme bannière du modpack (copie dans `art/`, comme un fichier choisi à la main).
/// Le chemin est résolu ici : le front ne fournit qu'un nom de fichier.
pub fn set_screenshot_as_banner(store: &InstanceStore, instance_id: &str, file: &str) -> Result<Instance> {
    let (path, _) = shot_path(store, instance_id, file)?;
    set_custom_image(store, instance_id, &path.to_string_lossy(), "banner")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Loader, LoaderSpec};
    use base64::Engine as _;
    use std::time::{Duration, SystemTime};

    fn store() -> (tempfile::TempDir, InstanceStore, Instance) {
        let t = tempfile::tempdir().unwrap();
        let s = InstanceStore::new(t.path().join("instances"));
        let i = s.create("Mon Pack", "1.21.1", LoaderSpec { loader: Loader::Vanilla, version: String::new() }, 2048).unwrap();
        (t, s, i)
    }

    fn img(w: u32, h: u32, fmt: image::ImageFormat) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        let rgb = image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 9]));
        image::DynamicImage::ImageRgb8(rgb).write_to(&mut out, fmt).unwrap();
        out.into_inner()
    }

    /// Écrit une capture dans `screenshots/` avec une date de modification donnée (secondes avant maintenant).
    fn put(s: &InstanceStore, id: &str, name: &str, bytes: &[u8], age_s: u64) {
        let dir = screenshots_dir(s, id).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let f = std::fs::File::create(dir.join(name)).unwrap();
        std::io::Write::write_all(&mut &f, bytes).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(age_s)).unwrap();
    }

    fn files(v: &[PlayerShot]) -> Vec<&str> {
        v.iter().map(|p| p.file.as_str()).collect()
    }

    #[test]
    fn dossier_absent_donne_une_liste_vide() {
        let (_t, s, i) = store();
        assert!(list_player_shots(&s, &i.id).unwrap().is_empty());
        assert!(list_player_shots(&s, "../x").is_err());
    }

    #[test]
    fn liste_triee_par_date_et_filtree() {
        let (_t, s, i) = store();
        let png = img(40, 30, image::ImageFormat::Png);
        put(&s, &i.id, "2026-11-20_10.00.00.png", &png, 300);
        put(&s, &i.id, "2026-11-20_11.00.00.png", &png, 100);
        put(&s, &i.id, "vieille.JPG", &img(40, 30, image::ImageFormat::Jpeg), 900);
        put(&s, &i.id, "notes.txt", b"pas une image", 10);
        put(&s, &i.id, "sans-extension", &png, 10);
        let dir = screenshots_dir(&s, &i.id).unwrap();
        std::fs::create_dir_all(dir.join("dossier.png")).unwrap();
        let l = list_player_shots(&s, &i.id).unwrap();
        assert_eq!(files(&l), ["2026-11-20_11.00.00.png", "2026-11-20_10.00.00.png", "vieille.JPG"]);
        assert_eq!(l[0].size, png.len() as u64);
        assert!(l[0].modified_ms > l[1].modified_ms);
    }

    #[test]
    fn liste_plafonnee_et_ignore_les_fichiers_geants() {
        let (_t, s, i) = store();
        let png = img(8, 8, image::ImageFormat::Png);
        for n in 0..MAX_LISTED + 5 {
            put(&s, &i.id, &format!("c{n:04}.png"), &png, 1000 - n as u64);
        }
        let dir = screenshots_dir(&s, &i.id).unwrap();
        let big = std::fs::File::create(dir.join("enorme.png")).unwrap();
        big.set_len(MAX_IMAGE_BYTES + 1).unwrap();
        let l = list_player_shots(&s, &i.id).unwrap();
        assert_eq!(l.len(), MAX_LISTED);
        assert!(!files(&l).contains(&"enorme.png"));
        assert_eq!(l[0].file, format!("c{:04}.png", MAX_LISTED + 4), "les plus récentes sont gardées");
        assert!(!files(&l).contains(&"c0000.png"), "les plus anciennes sont écartées");
    }

    #[cfg(unix)]
    #[test]
    fn liens_symboliques_ignores_et_refuses() {
        let (t, s, i) = store();
        let outside = t.path().join("secret.png");
        std::fs::write(&outside, img(20, 20, image::ImageFormat::Png)).unwrap();
        let dir = screenshots_dir(&s, &i.id).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("lien.png")).unwrap();
        put(&s, &i.id, "vraie.png", &img(20, 20, image::ImageFormat::Png), 5);
        assert_eq!(files(&list_player_shots(&s, &i.id).unwrap()), ["vraie.png"]);
        assert!(player_shot_data_url(&s, &i.id, "lien.png", None).is_err());
        assert!(set_screenshot_as_banner(&s, &i.id, "lien.png").is_err());
        assert!(s.get(&i.id).unwrap().art.is_none());
    }

    #[test]
    fn noms_dangereux_refuses() {
        let (t, s, i) = store();
        std::fs::write(t.path().join("x.png"), img(20, 20, image::ImageFormat::Png)).unwrap();
        put(&s, &i.id, "ok.png", &img(20, 20, image::ImageFormat::Png), 5);
        let abs = t.path().join("x.png").display().to_string();
        for bad in ["../x.png", "..\\x.png", "a/b.png", &abs, "", ".", "c:x.png", "nul\0.png", "ok.png/", "instance.json", "ok.gif"] {
            assert!(player_shot_data_url(&s, &i.id, bad, None).is_err(), "{bad:?} doit être refusé");
            assert!(set_screenshot_as_banner(&s, &i.id, bad).is_err(), "{bad:?} doit être refusé");
        }
        assert!(player_shot_data_url(&s, "../x", "ok.png", None).is_err());
        assert!(matches!(player_shot_data_url(&s, &i.id, "absente.png", None), Err(Error::NotFound(_))));
    }

    #[test]
    fn miniature_data_url_png_jpeg_et_faux_png() {
        let (_t, s, i) = store();
        put(&s, &i.id, "grande.png", &img(1920, 1080, image::ImageFormat::Png), 5);
        put(&s, &i.id, "photo.jpeg", &img(1600, 900, image::ImageFormat::Jpeg), 5);
        put(&s, &i.id, "petite.png", &img(32, 32, image::ImageFormat::Png), 5);
        put(&s, &i.id, "faux.png", b"<html>pas une image du tout</html>", 5);
        put(&s, &i.id, "anime.png", b"GIF89a\x01\x00\x01\x00", 5);
        let dec = |u: &str| {
            let (head, b64) = u.strip_prefix("data:").unwrap().split_once(";base64,").unwrap();
            let bytes = base64::engine::general_purpose::STANDARD.decode(b64).unwrap();
            (head.to_string(), image::load_from_memory(&bytes).unwrap())
        };
        let (mime, im) = dec(&player_shot_data_url(&s, &i.id, "grande.png", None).unwrap());
        assert_eq!(mime, "image/jpeg");
        assert_eq!(im.width().max(im.height()), THUMB_MAX_PX);
        let (_, im) = dec(&player_shot_data_url(&s, &i.id, "grande.png", Some(100_000)).unwrap());
        assert_eq!((im.width(), im.height()), (1920, 1080), "borné à 2048 : l'original tient déjà");
        let (_, im) = dec(&player_shot_data_url(&s, &i.id, "photo.jpeg", Some(128)).unwrap());
        assert_eq!(im.width().max(im.height()), 128);
        let (mime, im) = dec(&player_shot_data_url(&s, &i.id, "petite.png", None).unwrap());
        assert_eq!((mime.as_str(), im.width()), ("image/png", 32), "déjà petite : renvoyée telle quelle");
        assert!(player_shot_data_url(&s, &i.id, "faux.png", None).is_err());
        assert!(player_shot_data_url(&s, &i.id, "anime.png", None).is_err());
    }

    #[test]
    fn capture_trop_lourde_refusee_sans_decoder() {
        let (_t, s, i) = store();
        put(&s, &i.id, "lourde.png", &img(16, 16, image::ImageFormat::Png), 5);
        let p = screenshots_dir(&s, &i.id).unwrap().join("lourde.png");
        // Un PNG valide suivi de remplissage : déjà petit en pixels, donc renvoyé tel quel s'il tient dans 8 Mio.
        let mut bytes = std::fs::read(&p).unwrap();
        bytes.resize(MAX_LOCAL_IMAGE_BYTES as usize + 1, 0);
        std::fs::write(&p, &bytes).unwrap();
        assert!(player_shot_data_url(&s, &i.id, "lourde.png", None).is_err());
        assert!(set_screenshot_as_banner(&s, &i.id, "lourde.png").is_err());
    }

    #[test]
    fn banniere_copiee_dans_art_et_remplacee() {
        let (_t, s, i) = store();
        assert!(s.get(&i.id).unwrap().art.is_none());
        let png = img(64, 36, image::ImageFormat::Png);
        put(&s, &i.id, "a.png", &png, 20);
        put(&s, &i.id, "b.jpg", &img(64, 36, image::ImageFormat::Jpeg), 10);
        let u = set_screenshot_as_banner(&s, &i.id, "a.png").unwrap();
        let art = u.art.unwrap();
        assert_eq!((art.source.as_str(), art.screenshots.as_slice()), ("custom", ["shot-1.png".to_string()].as_slice()));
        let art_dir = s.art_dir(&i.id).unwrap();
        assert_eq!(std::fs::read(art_dir.join("shot-1.png")).unwrap(), png);
        // La capture d'origine reste intacte.
        assert!(screenshots_dir(&s, &i.id).unwrap().join("a.png").is_file());
        let u = set_screenshot_as_banner(&s, &i.id, "b.jpg").unwrap();
        assert_eq!(u.art.unwrap().screenshots, ["shot-1.jpg"]);
        assert!(art_dir.join("shot-1.jpg").is_file());
        assert!(!art_dir.join("shot-1.png").exists(), "l'ancienne bannière est supprimée");
        assert!(screenshots_dir(&s, &i.id).unwrap().join("b.jpg").is_file());
    }
}
