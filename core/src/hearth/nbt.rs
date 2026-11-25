//! NBT minimal pour l'export d'un monde de groupe en monde solo (docs/hearth.md § 8).
//!
//! Un serveur dédié garde chaque joueur dans `playerdata/<uuid>.dat`, alors qu'un monde solo met le propriétaire dans
//! `level.dat`, sous `Data.Player`. L'export copie le monde du groupe puis fait passer le joueur de l'un à l'autre : sans
//! cela, il réapparaîtrait au point de départ avec un inventaire vide. Le NBT est écrit ici plutôt que tiré d'une
//! dépendance : seuls la lecture, l'insertion d'un compound et la réécriture sont nécessaires, et l'aller-retour est exact
//! (mêmes tags, mêmes types, même ordre : les tags inconnus d'un monde modé passent tels quels).

use std::fs;
use std::io::{Read, Write};
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;

use super::world::LEVEL_DAT;

/// Taille maximale d'un fichier NBT décompressé : un `level.dat` ou un fichier joueur reste très loin de cette borne, mais
/// un fichier piégé ne doit pas remplir la mémoire.
const MAX_INFLATED: u64 = 64 * 1024 * 1024;
/// Profondeur maximale d'imbrication (celle du jeu).
const MAX_DEPTH: usize = 512;

/// Une valeur NBT. Les chaînes restent en octets (UTF-8 modifié) : on ne les réinterprète pas, donc l'aller-retour est exact.
#[derive(Debug, Clone, PartialEq)]
pub enum Tag {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<i8>),
    String(Vec<u8>),
    /// Identifiant du type des éléments, puis les éléments (tous de ce type).
    List(u8, Vec<Tag>),
    /// Entrées dans l'ordre du fichier.
    Compound(Vec<(Vec<u8>, Tag)>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl Tag {
    fn id(&self) -> u8 {
        match self {
            Tag::Byte(_) => 1,
            Tag::Short(_) => 2,
            Tag::Int(_) => 3,
            Tag::Long(_) => 4,
            Tag::Float(_) => 5,
            Tag::Double(_) => 6,
            Tag::ByteArray(_) => 7,
            Tag::String(_) => 8,
            Tag::List(..) => 9,
            Tag::Compound(_) => 10,
            Tag::IntArray(_) => 11,
            Tag::LongArray(_) => 12,
        }
    }

    /// Le compound d'entrées nommées `name`, s'il existe.
    pub fn get_mut(&mut self, name: &str) -> Option<&mut Tag> {
        match self {
            Tag::Compound(v) => v.iter_mut().find(|(k, _)| k == name.as_bytes()).map(|(_, t)| t),
            _ => None,
        }
    }

    /// Remplace l'entrée `name` (à sa place, pour garder l'ordre) ou l'ajoute à la fin. Sans effet hors compound.
    pub fn set(&mut self, name: &str, value: Tag) {
        let Tag::Compound(v) = self else { return };
        match v.iter_mut().find(|(k, _)| k == name.as_bytes()) {
            Some((_, t)) => *t = value,
            None => v.push((name.as_bytes().to_vec(), value)),
        }
    }
}

/// Un fichier NBT : le compound racine et son nom (vide dans `level.dat` et `playerdata`).
#[derive(Debug, Clone, PartialEq)]
pub struct Nbt {
    pub name: Vec<u8>,
    pub root: Tag,
}

/// Encode `s` en UTF-8 modifié (celui du NBT) : le NUL sur deux octets, les caractères hors du plan de base en deux
/// moitiés de paire de substitution de trois octets chacune.
pub fn modified_utf8(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut buf = [0u16; 2];
    for ch in s.chars() {
        if ch != '\0' && (ch as u32) < 0x10000 {
            let mut b = [0u8; 4];
            out.extend_from_slice(ch.encode_utf8(&mut b).as_bytes());
            continue;
        }
        if ch == '\0' {
            out.extend_from_slice(&[0xC0, 0x80]);
            continue;
        }
        for unit in ch.encode_utf16(&mut buf) {
            let u = *unit as u32;
            out.extend_from_slice(&[0xE0 | (u >> 12) as u8, 0x80 | ((u >> 6) & 0x3F) as u8, 0x80 | (u & 0x3F) as u8]);
        }
    }
    out
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).filter(|e| *e <= self.buf.len()).ok_or_else(|| anyhow!("NBT tronqué"))?;
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.take(N)?.try_into().expect("longueur vérifiée"))
    }

    fn len(&mut self, elem_size: usize) -> Result<usize> {
        let n = i32::from_be_bytes(self.array()?);
        let n = usize::try_from(n).map_err(|_| anyhow!("longueur NBT négative"))?;
        // Une longueur annoncée qui dépasse ce qu'il reste à lire est un fichier abîmé : on ne réserve pas la mémoire avant.
        if n.checked_mul(elem_size).is_none_or(|bytes| bytes > self.buf.len() - self.pos) {
            bail!("NBT tronqué");
        }
        Ok(n)
    }

    fn name(&mut self) -> Result<Vec<u8>> {
        let n = u16::from_be_bytes(self.array()?) as usize;
        Ok(self.take(n)?.to_vec())
    }

    fn tag(&mut self, id: u8, depth: usize) -> Result<Tag> {
        if depth > MAX_DEPTH {
            bail!("NBT trop imbriqué");
        }
        Ok(match id {
            1 => Tag::Byte(i8::from_be_bytes(self.array()?)),
            2 => Tag::Short(i16::from_be_bytes(self.array()?)),
            3 => Tag::Int(i32::from_be_bytes(self.array()?)),
            4 => Tag::Long(i64::from_be_bytes(self.array()?)),
            5 => Tag::Float(f32::from_be_bytes(self.array()?)),
            6 => Tag::Double(f64::from_be_bytes(self.array()?)),
            7 => {
                let n = self.len(1)?;
                Tag::ByteArray(self.take(n)?.iter().map(|b| *b as i8).collect())
            }
            8 => Tag::String(self.name()?),
            9 => {
                let elem = self.u8()?;
                let n = self.len(1)?;
                // Une liste vide peut porter n'importe quel type (le jeu écrit 0) ; une liste non vide d'éléments « fin » n'existe pas.
                if n > 0 && (elem == 0 || elem > 12) {
                    bail!("type de liste NBT inconnu : {elem}");
                }
                let mut items = Vec::with_capacity(n);
                for _ in 0..n {
                    items.push(self.tag(elem, depth + 1)?);
                }
                Tag::List(elem, items)
            }
            10 => {
                let mut entries = Vec::new();
                loop {
                    let child = self.u8()?;
                    if child == 0 {
                        break;
                    }
                    let name = self.name()?;
                    entries.push((name, self.tag(child, depth + 1)?));
                }
                Tag::Compound(entries)
            }
            11 => {
                let n = self.len(4)?;
                Tag::IntArray((0..n).map(|_| self.array().map(i32::from_be_bytes)).collect::<Result<_>>()?)
            }
            12 => {
                let n = self.len(8)?;
                Tag::LongArray((0..n).map(|_| self.array().map(i64::from_be_bytes)).collect::<Result<_>>()?)
            }
            other => bail!("type NBT inconnu : {other}"),
        })
    }
}

/// Lit un NBT non compressé : un compound racine nommé, sans rien derrière.
pub fn parse(bytes: &[u8]) -> Result<Nbt> {
    let mut r = Reader { buf: bytes, pos: 0 };
    let id = r.u8()?;
    if id != 10 {
        bail!("la racine NBT n'est pas un compound");
    }
    let name = r.name()?;
    let root = r.tag(10, 0)?;
    if r.pos != bytes.len() {
        bail!("données en trop après le NBT");
    }
    Ok(Nbt { name, root })
}

fn put_name(out: &mut Vec<u8>, name: &[u8]) -> Result<()> {
    let n = u16::try_from(name.len()).map_err(|_| anyhow!("chaîne NBT trop longue"))?;
    out.extend_from_slice(&n.to_be_bytes());
    out.extend_from_slice(name);
    Ok(())
}

fn put_len(out: &mut Vec<u8>, n: usize) -> Result<()> {
    out.extend_from_slice(&i32::try_from(n).map_err(|_| anyhow!("tableau NBT trop long"))?.to_be_bytes());
    Ok(())
}

fn write_tag(out: &mut Vec<u8>, t: &Tag) -> Result<()> {
    match t {
        Tag::Byte(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Short(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Int(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Long(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Float(v) => out.extend_from_slice(&v.to_bits().to_be_bytes()),
        Tag::Double(v) => out.extend_from_slice(&v.to_bits().to_be_bytes()),
        Tag::ByteArray(v) => {
            put_len(out, v.len())?;
            out.extend(v.iter().map(|b| *b as u8));
        }
        Tag::String(s) => put_name(out, s)?,
        Tag::List(elem, items) => {
            out.push(*elem);
            put_len(out, items.len())?;
            for i in items {
                write_tag(out, i)?;
            }
        }
        Tag::Compound(entries) => {
            for (name, v) in entries {
                out.push(v.id());
                put_name(out, name)?;
                write_tag(out, v)?;
            }
            out.push(0);
        }
        Tag::IntArray(v) => {
            put_len(out, v.len())?;
            for x in v {
                out.extend_from_slice(&x.to_be_bytes());
            }
        }
        Tag::LongArray(v) => {
            put_len(out, v.len())?;
            for x in v {
                out.extend_from_slice(&x.to_be_bytes());
            }
        }
    }
    Ok(())
}

/// Écrit un NBT non compressé.
pub fn to_bytes(nbt: &Nbt) -> Result<Vec<u8>> {
    let mut out = vec![10];
    put_name(&mut out, &nbt.name)?;
    write_tag(&mut out, &nbt.root)?;
    Ok(out)
}

/// Lit un fichier NBT compressé en gzip (`level.dat`, `playerdata/*.dat`).
pub fn read_gz(path: &Path) -> Result<Nbt> {
    let file = fs::File::open(path).with_context(|| format!("ouverture de {}", path.display()))?;
    let mut raw = Vec::new();
    // Un octet de plus que la borne : si on le lit, le fichier la dépasse.
    GzDecoder::new(file).take(MAX_INFLATED + 1).read_to_end(&mut raw).with_context(|| format!("décompression de {}", path.display()))?;
    if raw.len() as u64 > MAX_INFLATED {
        bail!("{} est trop gros", path.display());
    }
    parse(&raw).with_context(|| format!("lecture de {}", path.display()))
}

/// Écrit `nbt` en gzip dans `path`, via un fichier voisin puis un renommage : une coupure ne laisse jamais un fichier à moitié écrit.
pub fn write_gz(path: &Path, nbt: &Nbt) -> Result<()> {
    let bytes = to_bytes(nbt)?;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".cubelink-tmp");
    let tmp = std::path::PathBuf::from(tmp);
    let written = (|| -> Result<()> {
        let mut enc = GzEncoder::new(fs::File::create(&tmp)?, Compression::default());
        enc.write_all(&bytes)?;
        enc.finish()?.sync_all()?;
        fs::rename(&tmp, path)?;
        Ok(())
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written.with_context(|| format!("écriture de {}", path.display()))
}

/// Ce que `prepare_solo_world` a fait du monde exporté.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SoloPrepared {
    /// Le joueur a été remis dans `Data.Player` de `level.dat`.
    pub has_player: bool,
}

/// Prépare le monde `dir` (déjà copié depuis le groupe) pour être ouvert en solo : donne `level_name` au monde et remet le
/// joueur `player_uuid` (UUID avec tirets) de `playerdata/<uuid>.dat` dans `Data.Player`. Le joueur absent n'est pas une
/// erreur : le monde s'ouvre et il apparaît au point de départ. `level.dat` n'est réécrit qu'en entier (fichier voisin puis
/// renommage). Le fichier du joueur reste en place. Les tags inconnus de `level.dat` sont conservés tels quels.
pub fn prepare_solo_world(dir: &Path, level_name: &str, player_uuid: Option<&str>) -> Result<SoloPrepared> {
    let level_path = dir.join(LEVEL_DAT);
    let mut level = read_gz(&level_path)?;
    let data = level.root.get_mut("Data").ok_or_else(|| anyhow!("level.dat sans tag Data"))?;
    if !matches!(data, Tag::Compound(_)) {
        bail!("le tag Data de level.dat n'est pas un compound");
    }
    data.set("LevelName", Tag::String(modified_utf8(level_name)));
    let mut has_player = false;
    if let Some(uuid) = player_uuid.filter(|u| valid_uuid(u)) {
        let file = dir.join("playerdata").join(format!("{uuid}.dat"));
        if file.is_file() {
            let player = read_gz(&file)?;
            data.set("Player", player.root);
            has_player = true;
        }
    }
    write_gz(&level_path, &level)?;
    Ok(SoloPrepared { has_player })
}

/// UUID au format `8-4-4-4-12` en hexadécimal : il sert de nom de fichier, rien d'autre n'y passe.
fn valid_uuid(s: &str) -> bool {
    let groups: Vec<&str> = s.split('-').collect();
    groups.len() == 5
        && groups.iter().zip([8, 4, 4, 4, 12]).all(|(g, n)| g.len() == n && g.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Met un UUID Minecraft (avec ou sans tirets) au format avec tirets, en minuscules.
pub fn dashed_uuid(uuid: &str) -> Option<String> {
    let s: String = uuid.chars().filter(|c| *c != '-').collect::<String>().to_ascii_lowercase();
    if s.len() != 32 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!("{}-{}-{}-{}-{}", &s[..8], &s[8..12], &s[12..16], &s[16..20], &s[20..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> Vec<u8> {
        v.as_bytes().to_vec()
    }

    fn entry(k: &str, v: Tag) -> (Vec<u8>, Tag) {
        (s(k), v)
    }

    /// Un compound avec un tag de chaque type, dans un ordre qui n'est pas alphabétique.
    fn sample() -> Tag {
        Tag::Compound(vec![
            entry("zByte", Tag::Byte(-3)),
            entry("Short", Tag::Short(-300)),
            entry("Int", Tag::Int(i32::MIN)),
            entry("Long", Tag::Long(i64::MAX)),
            entry("Float", Tag::Float(1.5)),
            entry("Double", Tag::Double(-2.25)),
            entry("Bytes", Tag::ByteArray(vec![1, -1, 127])),
            entry("Str", Tag::String(modified_utf8("Café \u{1F30D} \0"))),
            entry("Ints", Tag::IntArray(vec![1, 2, -3])),
            entry("Longs", Tag::LongArray(vec![i64::MIN, 0])),
            entry("Empty", Tag::List(0, vec![])),
            entry("EmptyTyped", Tag::List(10, vec![])),
            entry("Items", Tag::List(10, vec![Tag::Compound(vec![entry("id", Tag::String(s("minecraft:stone")))]), Tag::Compound(vec![])])),
            entry("Nested", Tag::List(9, vec![Tag::List(3, vec![Tag::Int(1)]), Tag::List(1, vec![])])),
        ])
    }

    fn gz(dir: &Path, rel: &str, root: Tag) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        write_gz(&p, &Nbt { name: vec![], root }).unwrap();
    }

    const UUID: &str = "11111111-2222-3333-4444-555555555555";

    #[test]
    fn aller_retour_exact_sur_tous_les_types() {
        let nbt = Nbt { name: s("racine"), root: sample() };
        let bytes = to_bytes(&nbt).unwrap();
        let back = parse(&bytes).unwrap();
        assert_eq!(back, nbt);
        assert_eq!(to_bytes(&back).unwrap(), bytes, "mêmes octets : ordre et types conservés");
    }

    #[test]
    fn utf8_modifie_nul_et_hors_plan_de_base() {
        assert_eq!(modified_utf8("a\0b"), vec![b'a', 0xC0, 0x80, b'b']);
        // U+1F30D : paire de substitution D83C DF0D, trois octets par moitié.
        assert_eq!(modified_utf8("\u{1F30D}"), vec![0xED, 0xA0, 0xBC, 0xED, 0xBC, 0x8D]);
        assert_eq!(modified_utf8("é"), "é".as_bytes());
    }

    #[test]
    fn fichier_abime_ou_piege_refuse_sans_panique() {
        let good = to_bytes(&Nbt { name: vec![], root: sample() }).unwrap();
        for cut in 0..good.len() {
            assert!(parse(&good[..cut]).is_err(), "tronqué à {cut}");
        }
        let mut extra = good.clone();
        extra.push(0);
        assert!(parse(&extra).is_err(), "octets en trop");
        // Tableau d'entiers annonçant 2 milliards d'éléments : refus sans réserver la mémoire.
        let mut huge = vec![10, 0, 0, 11, 0, 1, b'a', 0x7F, 0xFF, 0xFF, 0xFF, 0];
        assert!(parse(&huge).is_err());
        huge[0] = 3;
        assert!(parse(&huge).is_err(), "racine qui n'est pas un compound");
        // Imbrication sans fin.
        let mut deep = vec![10, 0, 0];
        for _ in 0..(MAX_DEPTH + 10) {
            deep.extend_from_slice(&[10, 0, 1, b'a']);
        }
        assert!(parse(&deep).is_err());
    }

    fn world(dir: &Path, with_player: bool) -> Tag {
        let player = Tag::Compound(vec![entry("Pos", Tag::List(6, vec![Tag::Double(1.0), Tag::Double(64.0), Tag::Double(-3.0)])), entry("XpLevel", Tag::Int(7))]);
        let level = Tag::Compound(vec![entry(
            "Data",
            Tag::Compound(vec![
                entry("LevelName", Tag::String(s("Ancien nom"))),
                entry("modded_thing", Tag::Compound(vec![entry("x", Tag::Long(42))])),
                entry("DataVersion", Tag::Int(4189)),
            ]),
        )]);
        gz(dir, LEVEL_DAT, level);
        if with_player {
            gz(dir, &format!("playerdata/{UUID}.dat"), player.clone());
        }
        player
    }

    #[test]
    fn le_joueur_passe_dans_data_player_et_le_reste_est_intact() {
        let tmp = tempfile::tempdir().unwrap();
        let player = world(tmp.path(), true);
        let r = prepare_solo_world(tmp.path(), "Mon monde (export du groupe)", Some(UUID)).unwrap();
        assert!(r.has_player);
        let mut level = read_gz(&tmp.path().join(LEVEL_DAT)).unwrap();
        let Tag::Compound(data) = level.root.get_mut("Data").unwrap().clone() else { panic!("Data") };
        let names: Vec<&[u8]> = data.iter().map(|(k, _)| k.as_slice()).collect();
        // Ordre conservé : LevelName reste à sa place, Player est ajouté à la fin.
        assert_eq!(names, [b"LevelName".as_slice(), b"modded_thing", b"DataVersion", b"Player"]);
        assert_eq!(level.root.get_mut("Data").unwrap().get_mut("LevelName"), Some(&mut Tag::String(s("Mon monde (export du groupe)"))));
        assert_eq!(level.root.get_mut("Data").unwrap().get_mut("Player"), Some(&mut player.clone()));
        assert_eq!(level.root.get_mut("Data").unwrap().get_mut("modded_thing"), Some(&mut Tag::Compound(vec![entry("x", Tag::Long(42))])));
        // Le fichier du joueur reste en place, et aucun fichier temporaire ne traîne.
        assert!(tmp.path().join(format!("playerdata/{UUID}.dat")).is_file());
        assert!(!tmp.path().join("level.dat.cubelink-tmp").exists());
    }

    #[test]
    fn joueur_deja_dans_level_dat_remplace() {
        let tmp = tempfile::tempdir().unwrap();
        let player = world(tmp.path(), true);
        // Un premier passage, puis un second avec un autre joueur dans le fichier : Player est remplacé, pas doublé.
        prepare_solo_world(tmp.path(), "A", Some(UUID)).unwrap();
        let newer = Tag::Compound(vec![entry("XpLevel", Tag::Int(99))]);
        gz(tmp.path(), &format!("playerdata/{UUID}.dat"), newer.clone());
        prepare_solo_world(tmp.path(), "B", Some(UUID)).unwrap();
        let mut level = read_gz(&tmp.path().join(LEVEL_DAT)).unwrap();
        let Tag::Compound(data) = level.root.get_mut("Data").unwrap().clone() else { panic!("Data") };
        assert_eq!(data.iter().filter(|(k, _)| k == b"Player").count(), 1);
        assert_eq!(level.root.get_mut("Data").unwrap().get_mut("Player"), Some(&mut newer.clone()));
        assert_ne!(newer, player);
    }

    #[test]
    fn sans_fichier_joueur_le_monde_est_renomme_sans_player() {
        let tmp = tempfile::tempdir().unwrap();
        world(tmp.path(), false);
        let r = prepare_solo_world(tmp.path(), "Sans joueur", Some(UUID)).unwrap();
        assert!(!r.has_player);
        let mut level = read_gz(&tmp.path().join(LEVEL_DAT)).unwrap();
        assert!(level.root.get_mut("Data").unwrap().get_mut("Player").is_none());
        assert_eq!(level.root.get_mut("Data").unwrap().get_mut("LevelName"), Some(&mut Tag::String(s("Sans joueur"))));
        // Pas d'UUID du tout (compte absent) : même résultat.
        assert!(!prepare_solo_world(tmp.path(), "Sans joueur", None).unwrap().has_player);
    }

    #[test]
    fn uuid_qui_n_en_est_pas_un_n_ouvre_aucun_fichier() {
        let tmp = tempfile::tempdir().unwrap();
        world(tmp.path(), true);
        for bad in ["../level", "", "1111", "zzzzzzzz-2222-3333-4444-555555555555", "11111111222233334444555555555555"] {
            assert!(!prepare_solo_world(tmp.path(), "N", Some(bad)).unwrap().has_player, "{bad}");
        }
    }

    #[test]
    fn fichier_joueur_corrompu_ou_level_dat_illisible_laisse_level_dat_intact() {
        let tmp = tempfile::tempdir().unwrap();
        world(tmp.path(), false);
        fs::create_dir_all(tmp.path().join("playerdata")).unwrap();
        fs::write(tmp.path().join(format!("playerdata/{UUID}.dat")), b"pas du gzip").unwrap();
        let before = fs::read(tmp.path().join(LEVEL_DAT)).unwrap();
        assert!(prepare_solo_world(tmp.path(), "N", Some(UUID)).is_err());
        assert_eq!(fs::read(tmp.path().join(LEVEL_DAT)).unwrap(), before, "level.dat n'a pas bougé");
        assert!(!tmp.path().join("level.dat.cubelink-tmp").exists());
        fs::write(tmp.path().join(LEVEL_DAT), b"abime").unwrap();
        assert!(prepare_solo_world(tmp.path(), "N", None).is_err());
        assert_eq!(fs::read(tmp.path().join(LEVEL_DAT)).unwrap(), b"abime");
    }

    #[test]
    fn uuid_avec_tirets() {
        assert_eq!(dashed_uuid("11111111222233334444555555555555").as_deref(), Some(UUID));
        assert_eq!(dashed_uuid("11111111-2222-3333-4444-555555555555").as_deref(), Some(UUID));
        assert_eq!(dashed_uuid("ABCDEFAB-2222-3333-4444-555555555555").as_deref(), Some("abcdefab-2222-3333-4444-555555555555"));
        assert_eq!(dashed_uuid("pas un uuid"), None);
    }
}
