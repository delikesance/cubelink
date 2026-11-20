//! Monde solo en cours d'une instance (nom et icône), joint à l'invitation envoyée aux amis.
//!
//! Le monde ouvert est celui dont le `session.lock` a été touché le plus récemment (Minecraft le crée et le
//! verrouille à l'ouverture du monde) ; à défaut, le `level.dat` le plus récent. L'icône est le `icon.png` que le
//! jeu enregistre dans la sauvegarde (capture 64×64), réduite au besoin pour tenir dans l'invitation.

use std::io::{Cursor, Read};
use std::path::Path;
use std::time::SystemTime;

use image::{ImageEncoder, ImageReader};

/// Côté maximal d'une icône de monde reçue d'un ami.
pub const MAX_ICON_PX: u32 = 64;
/// Taille maximale (octets) de l'icône PNG envoyée.
pub const MAX_ICON_BYTES: usize = 10 * 1024;
const MAX_ICON_FILE: u64 = 512 * 1024;
const MAX_SAVES: usize = 2000;
/// `level.dat` : taille maximale du fichier compressé et du NBT décompressé (fichier non fiable : monde importé).
const MAX_LEVEL_FILE: u64 = 4 * 1024 * 1024;
const MAX_LEVEL_NBT: u64 = 8 * 1024 * 1024;
const MAX_NBT_DEPTH: u32 = 32;
const MAX_NAME_CHARS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldInfo {
    /// Nom affiché du monde (`LevelName` du `level.dat`), à défaut le nom du dossier de la sauvegarde.
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
    let folder = dir.file_name()?.to_string_lossy().trim().to_string();
    if folder.is_empty() {
        return None;
    }
    let name = level_name(&dir.join("level.dat")).unwrap_or(folder);
    let icon_png = std::fs::metadata(dir.join("icon.png"))
        .ok()
        .filter(|m| m.len() <= MAX_ICON_FILE)
        .and_then(|_| std::fs::read(dir.join("icon.png")).ok())
        .and_then(|b| small_icon(&b));
    Some(WorldInfo { name, icon_png })
}

/// Nom affiché du monde : `Data.LevelName` du `level.dat` (NBT gzip), nettoyé. `None` si le fichier est absent,
/// illisible ou sans nom exploitable (l'appelant retombe alors sur le nom du dossier).
fn level_name(level_dat: &Path) -> Option<String> {
    let file = std::fs::File::open(level_dat).ok()?;
    let mut nbt = Vec::new();
    flate2::read::GzDecoder::new(file.take(MAX_LEVEL_FILE)).take(MAX_LEVEL_NBT).read_to_end(&mut nbt).ok()?;
    let raw = Nbt { b: &nbt, pos: 0 }.find_level_name()?;
    let clean: String = raw.chars().filter(|c| !c.is_control()).collect();
    let clean: String = clean.trim().chars().take(MAX_NAME_CHARS).collect();
    let clean = clean.trim().to_string();
    (!clean.is_empty()).then_some(clean)
}

/// Curseur NBT (big-endian) sur un tampon : toute lecture hors bornes renvoie `None`, jamais de panique.
struct Nbt<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Nbt<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let s = self.b.get(self.pos..end)?;
        self.pos = end;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u16(&mut self) -> Option<usize> {
        let s = self.take(2)?;
        Some(u16::from_be_bytes([s[0], s[1]]) as usize)
    }
    fn len32(&mut self) -> Option<usize> {
        let s = self.take(4)?;
        // Une longueur négative vaut zéro élément pour Minecraft.
        Some(i32::from_be_bytes([s[0], s[1], s[2], s[3]]).max(0) as usize)
    }
    /// Chaîne NBT (longueur u16 + « modified UTF-8 » de Java).
    fn string(&mut self) -> Option<String> {
        let n = self.u16()?;
        Some(decode_mutf8(self.take(n)?))
    }
    fn skip(&mut self, tag: u8, depth: u32) -> Option<()> {
        if depth > MAX_NBT_DEPTH {
            return None;
        }
        match tag {
            1 => self.take(1).map(drop),
            2 => self.take(2).map(drop),
            3 | 5 => self.take(4).map(drop),
            4 | 6 => self.take(8).map(drop),
            7 => {
                let n = self.len32()?;
                self.take(n).map(drop)
            }
            8 => {
                let n = self.u16()?;
                self.take(n).map(drop)
            }
            9 => {
                let elem = self.u8()?;
                let n = self.len32()?;
                if n == 0 {
                    return Some(());
                }
                match elem {
                    // Liste de « fin » non vide : invalide.
                    0 => None,
                    1..=6 => {
                        let size = [1, 2, 4, 8, 4, 8][elem as usize - 1];
                        self.take(n.checked_mul(size)?).map(drop)
                    }
                    _ => (0..n).try_for_each(|_| self.skip(elem, depth + 1)),
                }
            }
            10 => loop {
                let t = self.u8()?;
                if t == 0 {
                    return Some(());
                }
                let n = self.u16()?;
                self.take(n)?;
                self.skip(t, depth + 1)?;
            },
            11 => {
                let n = self.len32()?;
                self.take(n.checked_mul(4)?).map(drop)
            }
            12 => {
                let n = self.len32()?;
                self.take(n.checked_mul(8)?).map(drop)
            }
            _ => None,
        }
    }
    /// Descend dans le compound racine puis `Data`, et renvoie la chaîne `LevelName`.
    fn find_level_name(&mut self) -> Option<String> {
        if self.u8()? != 10 {
            return None;
        }
        let n = self.u16()?;
        self.take(n)?; // nom de la racine (vide en pratique)
        loop {
            let tag = self.u8()?;
            if tag == 0 {
                return None;
            }
            let key = self.string()?;
            if tag == 10 && key == "Data" {
                return self.level_name_in_data();
            }
            self.skip(tag, 1)?;
        }
    }
    fn level_name_in_data(&mut self) -> Option<String> {
        loop {
            let tag = self.u8()?;
            if tag == 0 {
                return None;
            }
            let key = self.string()?;
            if tag == 8 && key == "LevelName" {
                return self.string();
            }
            self.skip(tag, 2)?;
        }
    }
}

/// « Modified UTF-8 » de Java : NUL en `C0 80`, caractères hors plan de base en paires de substituts UTF-16.
fn decode_mutf8(b: &[u8]) -> String {
    let mut units: Vec<u16> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let cont = |k: usize| b.get(i + k).filter(|x| *x & 0xC0 == 0x80).map(|x| (*x & 0x3F) as u16);
        let (unit, used) = match c {
            0x00..=0x7F => (c as u16, 1),
            0xC0..=0xDF => match cont(1) {
                Some(c1) => ((((c & 0x1F) as u16) << 6) | c1, 2),
                None => (0xFFFD, 1),
            },
            0xE0..=0xEF => match (cont(1), cont(2)) {
                (Some(c1), Some(c2)) => ((((c & 0x0F) as u16) << 12) | (c1 << 6) | c2, 3),
                _ => (0xFFFD, 1),
            },
            _ => (0xFFFD, 1),
        };
        units.push(unit);
        i += used;
    }
    String::from_utf16_lossy(&units)
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

    fn put_str(out: &mut Vec<u8>, s: &[u8]) {
        out.extend((s.len() as u16).to_be_bytes());
        out.extend(s);
    }
    fn tag(out: &mut Vec<u8>, t: u8, name: &str) {
        out.push(t);
        put_str(out, name.as_bytes());
    }
    fn gz(raw: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(raw).unwrap();
        e.finish().unwrap()
    }
    /// `level.dat` minimal : tags à sauter (int, liste de compounds, tableaux, compound imbriqué) avant `LevelName`.
    fn level_nbt(name: &[u8]) -> Vec<u8> {
        let mut n = vec![10, 0, 0];
        tag(&mut n, 10, "Data");
        tag(&mut n, 3, "version");
        n.extend(19133i32.to_be_bytes());
        tag(&mut n, 9, "list");
        n.push(10);
        n.extend(2i32.to_be_bytes());
        for _ in 0..2 {
            tag(&mut n, 1, "b");
            n.push(7);
            n.push(0);
        }
        tag(&mut n, 12, "longs");
        n.extend(2i32.to_be_bytes());
        n.extend([0u8; 16]);
        tag(&mut n, 11, "ints");
        n.extend(1i32.to_be_bytes());
        n.extend([0u8; 4]);
        tag(&mut n, 10, "nested");
        tag(&mut n, 8, "LevelName");
        put_str(&mut n, b"faux");
        n.push(0);
        tag(&mut n, 8, "LevelName");
        put_str(&mut n, name);
        n.extend([0, 0]);
        n
    }
    fn world_with(level_dat: &[u8]) -> (tempfile::TempDir, Option<WorldInfo>) {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("saves").join("Nouveau monde");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("session.lock"), b"x").unwrap();
        std::fs::write(d.join("level.dat"), level_dat).unwrap();
        let w = current_world(tmp.path());
        (tmp, w)
    }
    fn name_of(level_dat: &[u8]) -> String {
        world_with(level_dat).1.unwrap().name
    }

    #[test]
    fn shows_level_name_instead_of_folder_name() {
        assert_eq!(name_of(&gz(&level_nbt("Île des copains".as_bytes()))), "Île des copains");
    }

    #[test]
    fn level_name_is_cleaned_and_truncated() {
        assert_eq!(name_of(&gz(&level_nbt(b"  Mon\nmonde \x07 "))), "Monmonde");
        let long = "é".repeat(200);
        assert_eq!(name_of(&gz(&level_nbt(long.as_bytes()))).chars().count(), MAX_NAME_CHARS);
    }

    #[test]
    fn decodes_java_modified_utf8() {
        // NUL en C0 80 (retiré comme caractère de contrôle) et émoji en paire de substituts (2 x 3 octets).
        let name = gz(&level_nbt(&[b'A', 0xC0, 0x80, 0xED, 0xA0, 0xBD, 0xED, 0xB2, 0x96, b'B']));
        assert_eq!(name_of(&name), "A\u{1F496}B");
    }

    #[test]
    fn falls_back_to_folder_name_when_level_dat_is_unusable() {
        let fb = "Nouveau monde";
        let mut bad_root = vec![9, 0, 0];
        bad_root.extend([0; 8]);
        let mut no_data = vec![10, 0, 0];
        tag(&mut no_data, 3, "x");
        no_data.extend([0; 4]);
        no_data.push(0);
        let mut int_name = vec![10, 0, 0];
        tag(&mut int_name, 10, "Data");
        tag(&mut int_name, 3, "LevelName");
        int_name.extend([0; 4]);
        int_name.extend([0, 0]);
        let full = gz(&level_nbt(b"ok"));
        let cases: Vec<Vec<u8>> = vec![
            vec![],
            b"x".to_vec(),
            full[..full.len() / 2].to_vec(),
            gz(&level_nbt(b"ok"))[..10].to_vec(),
            gz(&level_nbt(b"ok")[..30]),
            gz(&bad_root),
            gz(&no_data),
            gz(&int_name),
            gz(&level_nbt(b"   ")),
            gz(&level_nbt(b"")),
        ];
        for c in cases {
            assert_eq!(name_of(&c), fb);
        }
        // level.dat absent : le dossier garde son nom (la Scène ne choisit alors que via session.lock).
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("saves").join(fb);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("session.lock"), b"x").unwrap();
        assert_eq!(current_world(tmp.path()).unwrap().name, fb);
    }

    #[test]
    fn hostile_level_dat_does_not_panic_nor_explode() {
        let fb = "Nouveau monde";
        // Longueur de chaîne déclarée supérieure aux octets restants.
        let mut n = vec![10, 0, 0];
        tag(&mut n, 10, "Data");
        tag(&mut n, 8, "LevelName");
        n.extend([0xFF, 0xFF, b'a']);
        assert_eq!(name_of(&gz(&n)), fb);
        // Listes de listes imbriquées bien au-delà de la profondeur permise.
        let mut n = vec![10, 0, 0];
        tag(&mut n, 10, "Data");
        tag(&mut n, 9, "deep");
        for _ in 0..500 {
            n.push(9);
            n.extend(1i32.to_be_bytes());
        }
        assert_eq!(name_of(&gz(&n)), fb);
        // Liste de « fin » non vide, et longueur géante de tableau.
        let mut n = vec![10, 0, 0];
        tag(&mut n, 10, "Data");
        tag(&mut n, 9, "l");
        n.push(0);
        n.extend(5i32.to_be_bytes());
        assert_eq!(name_of(&gz(&n)), fb);
        let mut n = vec![10, 0, 0];
        tag(&mut n, 10, "Data");
        tag(&mut n, 12, "l");
        n.extend(i32::MAX.to_be_bytes());
        assert_eq!(name_of(&gz(&n)), fb);
        // Bombe gzip : 50 Mo de zéros, bien au-delà de la limite décompressée.
        assert_eq!(name_of(&gz(&vec![0u8; 50 * 1024 * 1024])), fb);
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
