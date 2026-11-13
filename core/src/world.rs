//! Monde solo en cours d'une instance (nom et icône), joint à l'invitation envoyée aux amis.
//!
//! Le monde ouvert est celui dont le `session.lock` a été touché le plus récemment (Minecraft le crée et le
//! verrouille à l'ouverture du monde) ; à défaut, le `level.dat` le plus récent. L'icône est le `icon.png` que le
//! jeu enregistre dans la sauvegarde (capture 64×64), réduite au besoin pour tenir dans l'invitation.

use std::io::Cursor;
use std::path::Path;
use std::time::SystemTime;

use image::{ImageEncoder, ImageReader};

/// Côté maximal d'une icône de monde reçue d'un ami.
pub const MAX_ICON_PX: u32 = 64;
/// Taille maximale (octets) de l'icône PNG envoyée.
pub const MAX_ICON_BYTES: usize = 10 * 1024;
const MAX_ICON_FILE: u64 = 512 * 1024;
const MAX_SAVES: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldInfo {
    /// Nom du dossier de la sauvegarde.
    pub name: String,
    /// Icône PNG (au plus `MAX_ICON_PX` de côté et `MAX_ICON_BYTES`), si la sauvegarde en a une.
    pub icon_png: Option<Vec<u8>>,
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).ok()?.modified().ok()
}

/// Monde le plus récemment ouvert dans `game_dir/saves`, `None` s'il n'y en a pas.
pub fn current_world(game_dir: &Path) -> Option<WorldInfo> {
    let saves = std::fs::read_dir(game_dir.join("saves")).ok()?;
    let mut best: Option<(SystemTime, std::path::PathBuf)> = None;
    for e in saves.flatten().take(MAX_SAVES) {
        let dir = e.path();
        if !e.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let Some(t) = mtime(&dir.join("session.lock")).or_else(|| mtime(&dir.join("level.dat"))) else { continue };
        if best.as_ref().is_none_or(|(b, _)| t > *b) {
            best = Some((t, dir));
        }
    }
    let (_, dir) = best?;
    let name = dir.file_name()?.to_string_lossy().trim().to_string();
    if name.is_empty() {
        return None;
    }
    let icon_png = std::fs::metadata(dir.join("icon.png"))
        .ok()
        .filter(|m| m.len() <= MAX_ICON_FILE)
        .and_then(|_| std::fs::read(dir.join("icon.png")).ok())
        .and_then(|b| small_icon(&b));
    Some(WorldInfo { name, icon_png })
}

/// PNG de 64, 48 ou 32 px de côté (le plus grand qui tient dans `MAX_ICON_BYTES`).
pub fn small_icon(bytes: &[u8]) -> Option<Vec<u8>> {
    let reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?;
    let (w, h) = reader.into_dimensions().ok()?;
    if w == 0 || h == 0 || w > 4096 || h > 4096 {
        return None;
    }
    let img = ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?.decode().ok()?;
    for px in [64u32, 48, 32] {
        let small = if w <= px && h <= px { img.clone() } else { img.thumbnail(px, px) };
        let rgba = small.to_rgba8();
        let mut out = Vec::new();
        image::codecs::png::PngEncoder::new(&mut out)
            .write_image(rgba.as_raw(), rgba.width(), rgba.height(), image::ExtendedColorType::Rgba8)
            .ok()?;
        if out.len() <= MAX_ICON_BYTES {
            return Some(out);
        }
    }
    None
}

/// Vrai si `bytes` est un PNG d'au plus `MAX_ICON_PX` de côté (icône reçue d'un pair : aucune confiance).
pub fn is_valid_icon(bytes: &[u8]) -> bool {
    if bytes.len() > MAX_ICON_BYTES * 2 || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return false;
    }
    let Ok(reader) = ImageReader::with_format(Cursor::new(bytes), image::ImageFormat::Png).into_dimensions() else { return false };
    let (w, h) = reader;
    w > 0 && h > 0 && w <= MAX_ICON_PX && h <= MAX_ICON_PX
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x * 7) as u8, (y * 5) as u8, ((x ^ y) * 3) as u8, 255]));
        let mut out = Vec::new();
        image::codecs::png::PngEncoder::new(&mut out).write_image(img.as_raw(), w, h, image::ExtendedColorType::Rgba8).unwrap();
        out
    }

    #[test]
    fn picks_the_most_recently_opened_world_and_its_icon() {
        let tmp = tempfile::tempdir().unwrap();
        let saves = tmp.path().join("saves");
        for (name, lock) in [("Ancien", false), ("Îles flottantes", true)] {
            let d = saves.join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("level.dat"), b"x").unwrap();
            if lock {
                std::thread::sleep(Duration::from_millis(20));
                std::fs::write(d.join("session.lock"), b"x").unwrap();
                std::fs::write(d.join("icon.png"), png(64, 64)).unwrap();
            }
        }
        let w = current_world(tmp.path()).unwrap();
        assert_eq!(w.name, "Îles flottantes");
        let icon = w.icon_png.unwrap();
        assert!(is_valid_icon(&icon) && icon.len() <= MAX_ICON_BYTES);
    }

    #[test]
    fn no_saves_means_no_world() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(current_world(tmp.path()), None);
    }

    #[test]
    fn icon_check_rejects_garbage_and_large_images() {
        assert!(is_valid_icon(&png(32, 32)));
        assert!(!is_valid_icon(&png(65, 64)));
        assert!(!is_valid_icon(b"pas une image"));
        let shrunk = small_icon(&png(512, 512)).unwrap();
        assert!(is_valid_icon(&shrunk));
    }
}
