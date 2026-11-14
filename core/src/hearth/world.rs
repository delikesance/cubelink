//! Hearth : monde d'un groupe sur le disque (voir `docs/hearth.md`, § 3.2, § 6 et § 7).
//!
//! * copie d'une sauvegarde d'instance (`saves/<nom>`) vers le dossier du groupe, sans `session.lock` ;
//! * instantané d'un monde (taille, date, SHA-256) et liste des fichiers modifiés depuis cet instantané, pour les
//!   points de reprise ;
//! * contrôle qu'un dossier est bien un monde (`level.dat`).
//!
//! Tout est synchrone : l'appelant asynchrone passe par `tokio::task::spawn_blocking`. Les liens symboliques ne sont
//! jamais suivis (ni copiés, ni listés), pour qu'un lien ne fasse pas sortir la copie du monde.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::store::hash::sha256_file;
use crate::types::{Error, Result};

/// Fichier qui fait d'un dossier un monde Minecraft.
pub const LEVEL_DAT: &str = "level.dat";
/// Profondeur maximale du parcours (un monde Minecraft en a moins de 8).
const MAX_DEPTH: usize = 32;
/// Nombre maximal de fichiers parcourus (garde-fou contre un dossier absurde).
const MAX_FILES: usize = 2_000_000;
/// Un fichier modifié moins de 2 s avant l'instantané peut l'avoir été dans la même « tique » de l'horloge du système
/// de fichiers : on le relit au prochain passage plutôt que de se fier à sa date (même précaution que `git`).
const RACY_WINDOW_MS: u64 = 2_000;

/// Vrai si `dir` est un monde Minecraft : il contient un fichier `level.dat`.
pub fn is_world_dir(dir: &Path) -> bool {
    fs::metadata(dir.join(LEVEL_DAT)).is_ok_and(|m| m.is_file())
}

/// Vrai pour un fichier qui n'entre jamais dans une copie ni une version du monde : `session.lock` (verrou du jeu)
/// et les fichiers temporaires `*.tmp`. `rel` est un chemin relatif ou un simple nom.
pub fn is_excluded(rel: &str) -> bool {
    let name = rel.rsplit(['/', '\\']).next().unwrap_or(rel);
    name.eq_ignore_ascii_case("session.lock") || name.to_ascii_lowercase().ends_with(".tmp")
}

/// Nom d'une sauvegarde d'instance : un seul segment de chemin, sans `..` ni séparateur.
fn check_save_name(name: &str) -> Result<()> {
    let bad = name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\', ':', '\0'])
        || name.len() > 255
        || name.trim() != name;
    if bad {
        return Err(Error::Invalid(format!("nom de sauvegarde invalide « {name} »")));
    }
    Ok(())
}

/// Noms des sauvegardes de `<instance_dir>/saves` qui sont des mondes (`level.dat` présent), triés.
pub fn list_instance_saves(instance_dir: &Path) -> Result<Vec<String>> {
    let rd = match fs::read_dir(instance_dir.join("saves")) {
        Ok(rd) => rd,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut out = Vec::new();
    for entry in rd {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else { continue };
        if is_world_dir(&entry.path()) {
            out.push(name);
        }
    }
    out.sort();
    Ok(out)
}

/// Résultat d'une copie de monde.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyReport {
    pub files: u64,
    pub bytes: u64,
}

/// Copie la sauvegarde `saves/<save_name>` de `instance_dir` vers `dest` (voir [`copy_world`]).
pub fn copy_instance_save(instance_dir: &Path, save_name: &str, dest: &Path) -> Result<CopyReport> {
    check_save_name(save_name)?;
    let src = instance_dir.join("saves").join(save_name);
    if !src.is_dir() {
        return Err(Error::NotFound(format!("sauvegarde « {save_name} »")));
    }
    copy_world(&src, dest)
}

/// Copie le monde `src` dans `dest`, sans `session.lock` ni fichier `*.tmp`.
///
/// * `src` doit être un monde (`level.dat`) ; `dest` ne doit pas exister, ou être un dossier vide : une copie ne
///   remplace jamais un monde existant.
/// * La copie se fait dans un dossier voisin `<dest>.copie`, renommé en `dest` à la fin : un échec ou une coupure ne
///   laisse jamais un monde à moitié copié sous le nom définitif.
pub fn copy_world(src: &Path, dest: &Path) -> Result<CopyReport> {
    if !is_world_dir(src) {
        return Err(Error::Invalid(format!("« {} » n'est pas un monde (level.dat absent)", src.display())));
    }
    match fs::read_dir(dest) {
        Ok(mut rd) => {
            if rd.next().is_some() {
                return Err(Error::Invalid(format!("« {} » existe déjà et n'est pas vide", dest.display())));
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    if dest.starts_with(src) {
        return Err(Error::Invalid("la destination est dans le monde copié".into()));
    }
    let parent = dest.parent().ok_or_else(|| Error::Invalid("destination sans dossier parent".into()))?;
    fs::create_dir_all(parent)?;
    let mut staging_name = dest.file_name().ok_or_else(|| Error::Invalid("destination sans nom".into()))?.to_owned();
    staging_name.push(".copie");
    let staging = parent.join(staging_name);
    let _ = fs::remove_dir_all(&staging);

    let mut report = CopyReport { files: 0, bytes: 0 };
    let result = copy_tree(src, &staging, 0, &mut report).and_then(|()| {
        // Un `dest` vide existant est remplacé (le renommage échoue sinon sous Windows).
        match fs::remove_dir(dest) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        fs::rename(&staging, dest).map_err(Error::from)
    });
    if let Err(e) = result {
        let _ = fs::remove_dir_all(&staging);
        return Err(e);
    }
    Ok(report)
}

fn copy_tree(src: &Path, dst: &Path, depth: usize, report: &mut CopyReport) -> Result<()> {
    if depth > MAX_DEPTH {
        return Err(Error::Invalid("monde trop profond".into()));
    }
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            return Err(Error::Invalid(format!("nom de fichier illisible dans {}", src.display())));
        };
        let ty = entry.file_type()?;
        if ty.is_symlink() {
            continue;
        }
        if ty.is_dir() {
            copy_tree(&entry.path(), &dst.join(&name), depth + 1, report)?;
        } else if ty.is_file() {
            if is_excluded(name_str) {
                continue;
            }
            report.files += 1;
            if report.files > MAX_FILES as u64 {
                return Err(Error::Invalid("trop de fichiers dans le monde".into()));
            }
            report.bytes += fs::copy(entry.path(), dst.join(&name))?;
        }
    }
    Ok(())
}

/// Ce qu'on retient d'un fichier du monde.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStamp {
    pub size: u64,
    /// Date de modification, en millisecondes depuis l'epoch Unix.
    pub mtime_ms: u64,
    /// SHA-256 hexadécimal minuscule du contenu.
    pub sha256: String,
}

/// Instantané d'un monde : chemin relatif (séparateur `/`) vers empreinte du fichier.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Snapshot {
    pub files: BTreeMap<String, FileStamp>,
    /// Instant de la prise (ms depuis l'epoch) ; sert à repérer les fichiers « trop récents pour être sûrs ».
    pub taken_at_ms: u64,
}

impl Snapshot {
    /// Instantané complet de `dir` : tous les fichiers sont lus et hachés.
    pub fn capture(dir: &Path) -> Result<Snapshot> {
        Ok(diff_against(None, dir)?.1)
    }

    pub fn total_size(&self) -> u64 {
        self.files.values().map(|f| f.size).sum()
    }
}

/// Fichiers qui ont changé depuis un instantané.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorldDiff {
    pub added: Vec<String>,
    pub modified: Vec<String>,
    pub removed: Vec<String>,
}

impl WorldDiff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.modified.is_empty() && self.removed.is_empty()
    }

    /// Fichiers à importer : ajoutés puis modifiés (les supprimés n'ont plus de contenu).
    pub fn changed_files(&self) -> impl Iterator<Item = &String> {
        self.added.iter().chain(self.modified.iter())
    }
}

/// Compare `dir` à l'instantané `prev` : rend les fichiers ajoutés, modifiés et supprimés, et le nouvel instantané.
///
/// Un fichier dont la taille et la date n'ont pas bougé n'est pas relu (sauf s'il est « récent », voir
/// `RACY_WINDOW_MS`). Un fichier dont la date a changé mais dont le contenu est identique n'est pas « modifié ».
pub fn changed_since(prev: &Snapshot, dir: &Path) -> Result<(WorldDiff, Snapshot)> {
    diff_against(Some(prev), dir)
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn mtime_ms(meta: &fs::Metadata) -> u64 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64)
}

fn diff_against(prev: Option<&Snapshot>, dir: &Path) -> Result<(WorldDiff, Snapshot)> {
    // La prise est datée AVANT la lecture : un fichier modifié pendant le parcours sera jugé « récent » au prochain.
    let taken_at_ms = now_ms();
    let mut found: Vec<(String, PathBuf)> = Vec::new();
    walk(dir, "", 0, &mut found)?;
    let mut next = Snapshot { files: BTreeMap::new(), taken_at_ms };
    let mut diff = WorldDiff::default();
    for (rel, path) in found {
        let meta = match fs::metadata(&path) {
            Ok(m) => m,
            // Fichier supprimé entre le parcours et la lecture : il sera vu comme supprimé.
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        let (size, mt) = (meta.len(), mtime_ms(&meta));
        let old = prev.and_then(|p| p.files.get(&rel));
        let trusted = old.filter(|o| {
            o.size == size
                && o.mtime_ms == mt
                && prev.is_some_and(|p| o.mtime_ms.saturating_add(RACY_WINDOW_MS) < p.taken_at_ms)
        });
        if let Some(o) = trusted {
            next.files.insert(rel, o.clone());
            continue;
        }
        let sha256 = match sha256_file(&path) {
            Ok(h) => h,
            Err(Error::Io(e)) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        match old {
            None => {
                if prev.is_some() {
                    diff.added.push(rel.clone());
                }
            }
            Some(o) if o.sha256 != sha256 => diff.modified.push(rel.clone()),
            Some(_) => {}
        }
        next.files.insert(rel, FileStamp { size, mtime_ms: mt, sha256 });
    }
    if let Some(p) = prev {
        diff.removed = p.files.keys().filter(|k| !next.files.contains_key(*k)).cloned().collect();
    }
    Ok((diff, next))
}

fn walk(dir: &Path, rel: &str, depth: usize, out: &mut Vec<(String, PathBuf)>) -> Result<()> {
    if depth > MAX_DEPTH {
        return Err(Error::Invalid("monde trop profond".into()));
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_symlink() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(Error::Invalid(format!("nom de fichier illisible dans {}", dir.display())));
        };
        let child_rel = if rel.is_empty() { name.to_owned() } else { format!("{rel}/{name}") };
        if ty.is_dir() {
            walk(&entry.path(), &child_rel, depth + 1, out)?;
        } else if ty.is_file() && !is_excluded(name) {
            if out.len() >= MAX_FILES {
                return Err(Error::Invalid("trop de fichiers dans le monde".into()));
            }
            out.push((child_rel, entry.path()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn write(root: &Path, rel: &str, data: &[u8]) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, data).unwrap();
    }

    fn sample_world(root: &Path) {
        write(root, "level.dat", b"level");
        write(root, "session.lock", b"lock");
        write(root, "region/r.0.0.mca", b"region-data");
        write(root, "playerdata/abc.dat", b"player");
        write(root, "data/raids.dat.tmp", b"tmp");
    }

    #[test]
    fn world_detection() {
        let t = tempfile::tempdir().unwrap();
        assert!(!is_world_dir(t.path()));
        write(t.path(), "level.dat", b"x");
        assert!(is_world_dir(t.path()));
        assert!(!is_world_dir(&t.path().join("level.dat")));
    }

    #[test]
    fn exclusions() {
        assert!(is_excluded("session.lock"));
        assert!(is_excluded("dim/session.lock"));
        assert!(is_excluded("SESSION.LOCK"));
        assert!(is_excluded("data/x.tmp"));
        assert!(!is_excluded("region/r.0.0.mca"));
        assert!(!is_excluded("level.dat_old"));
    }

    #[test]
    fn copy_skips_lock_and_tmp() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("saves").join("Monde");
        sample_world(&src);
        let dst = t.path().join("hearth").join("world");
        let r = copy_world(&src, &dst).unwrap();
        assert_eq!(r.files, 3);
        assert_eq!(r.bytes, (5 + 11 + 6) as u64);
        assert!(dst.join("level.dat").is_file());
        assert!(dst.join("region/r.0.0.mca").is_file());
        assert!(!dst.join("session.lock").exists());
        assert!(!dst.join("data/raids.dat.tmp").exists());
        assert!(!t.path().join("hearth").join("world.copie").exists());
        // La source n'est pas touchée.
        assert!(src.join("session.lock").is_file());
    }

    #[test]
    fn copy_refuses_existing_world_and_non_world() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("src");
        sample_world(&src);
        let dst = t.path().join("dst");
        write(&dst, "autre.txt", b"x");
        assert!(matches!(copy_world(&src, &dst), Err(Error::Invalid(_))));
        assert!(dst.join("autre.txt").is_file());
        let not_world = t.path().join("vide");
        fs::create_dir_all(&not_world).unwrap();
        assert!(matches!(copy_world(&not_world, &t.path().join("d2")), Err(Error::Invalid(_))));
        // Destination vide existante : acceptée.
        let empty = t.path().join("empty");
        fs::create_dir_all(&empty).unwrap();
        copy_world(&src, &empty).unwrap();
        assert!(empty.join("level.dat").is_file());
        // Destination dans la source : refusée.
        assert!(copy_world(&src, &src.join("sous")).is_err());
    }

    #[test]
    fn instance_saves_listing_and_names() {
        let t = tempfile::tempdir().unwrap();
        write(t.path(), "saves/Alpha/level.dat", b"a");
        write(t.path(), "saves/Beta/level.dat", b"b");
        write(t.path(), "saves/Pas un monde/readme.txt", b"c");
        write(t.path(), "saves/fichier.txt", b"d");
        assert_eq!(list_instance_saves(t.path()).unwrap(), vec!["Alpha".to_string(), "Beta".to_string()]);
        assert!(list_instance_saves(&t.path().join("absent")).unwrap().is_empty());
        let dst = t.path().join("g").join("world");
        copy_instance_save(t.path(), "Alpha", &dst).unwrap();
        assert!(dst.join("level.dat").is_file());
        for bad in ["", "..", ".", "a/b", "a\\b", "../x", " Alpha", "C:x"] {
            assert!(matches!(copy_instance_save(t.path(), bad, &t.path().join("z")), Err(Error::Invalid(_))), "{bad:?}");
        }
        assert!(matches!(copy_instance_save(t.path(), "Absent", &t.path().join("z")), Err(Error::NotFound(_))));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_not_followed() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("src");
        sample_world(&src);
        let outside = t.path().join("secret.txt");
        fs::write(&outside, b"secret").unwrap();
        std::os::unix::fs::symlink(&outside, src.join("lien.txt")).unwrap();
        std::os::unix::fs::symlink(t.path(), src.join("boucle")).unwrap();
        let dst = t.path().join("dst");
        copy_world(&src, &dst).unwrap();
        assert!(!dst.join("lien.txt").exists());
        assert!(!dst.join("boucle").exists());
        let snap = Snapshot::capture(&src).unwrap();
        assert!(!snap.files.contains_key("lien.txt"));
    }

    /// Recule la date d'un fichier pour sortir de la fenêtre « récent » sans attendre.
    fn age(path: &Path, secs: u64) {
        let f = fs::OpenOptions::new().write(true).open(path).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(secs)).unwrap();
    }

    #[test]
    fn snapshot_and_diff() {
        let t = tempfile::tempdir().unwrap();
        let w = t.path();
        sample_world(w);
        for f in ["level.dat", "region/r.0.0.mca", "playerdata/abc.dat"] {
            age(&w.join(f), 3600);
        }
        let snap = Snapshot::capture(w).unwrap();
        assert_eq!(snap.files.len(), 3);
        assert!(!snap.files.contains_key("session.lock"));
        assert_eq!(snap.total_size(), 22);
        assert_eq!(snap.files["level.dat"].sha256.len(), 64);

        // Rien n'a changé.
        let (d, snap2) = changed_since(&snap, w).unwrap();
        assert!(d.is_empty(), "{d:?}");
        assert_eq!(snap2.files, snap.files);

        // Modification de contenu à taille égale, ajout, suppression.
        write(w, "region/r.0.0.mca", b"REGION-DATA");
        write(w, "region/r.0.1.mca", b"new");
        fs::remove_file(w.join("playerdata/abc.dat")).unwrap();
        let (d, snap3) = changed_since(&snap, w).unwrap();
        assert_eq!(d.added, vec!["region/r.0.1.mca".to_string()]);
        assert_eq!(d.modified, vec!["region/r.0.0.mca".to_string()]);
        assert_eq!(d.removed, vec!["playerdata/abc.dat".to_string()]);
        assert_eq!(d.changed_files().count(), 2);
        assert!(!d.is_empty());
        assert_eq!(snap3.files.len(), 3);
    }

    #[test]
    fn touched_but_identical_is_not_modified() {
        let t = tempfile::tempdir().unwrap();
        let w = t.path();
        sample_world(w);
        let f = w.join("region/r.0.0.mca");
        age(&f, 3600);
        let snap = Snapshot::capture(w).unwrap();
        age(&f, 1800); // même contenu, autre date
        let (d, next) = changed_since(&snap, w).unwrap();
        assert!(d.is_empty(), "{d:?}");
        assert_ne!(next.files["region/r.0.0.mca"].mtime_ms, snap.files["region/r.0.0.mca"].mtime_ms);
    }

    #[test]
    fn racy_file_is_rehashed_even_if_stamp_matches() {
        let t = tempfile::tempdir().unwrap();
        let w = t.path();
        write(w, "level.dat", b"aaaa");
        let f = w.join("level.dat");
        // Date récente : à la prise, le fichier est dans la fenêtre « récent ».
        let snap = Snapshot::capture(w).unwrap();
        let stamp = fs::metadata(&f).unwrap().modified().unwrap();
        // Même taille, même date, contenu différent.
        fs::write(&f, b"bbbb").unwrap();
        fs::OpenOptions::new().write(true).open(&f).unwrap().set_modified(stamp).unwrap();
        let (d, _) = changed_since(&snap, w).unwrap();
        assert_eq!(d.modified, vec!["level.dat".to_string()]);
    }

    #[test]
    fn first_capture_reports_no_added() {
        let t = tempfile::tempdir().unwrap();
        sample_world(t.path());
        let (d, snap) = diff_against(None, t.path()).unwrap();
        assert!(d.is_empty());
        assert_eq!(snap.files.len(), 3);
    }
}
