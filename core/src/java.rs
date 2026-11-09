//! Tâche J1 : détection de Java et version requise par version de jeu.

use std::fs;
use std::path::{Path, PathBuf};

/// Une installation Java détectée.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaInstall {
    /// Chemin de l'exécutable `java` (`bin/java` ou `bin/java.exe`).
    pub path: PathBuf,
    /// Version majeure (8, 17, 21...).
    pub major: u32,
    /// Éditeur si connu (champ IMPLEMENTOR du fichier `release`).
    pub vendor: Option<String>,
}

/// Analyse un numéro de version de Minecraft ("1.20.4", "1.21", "1.16.5-rc1").
/// Retourne (minor, patch) pour une version "1.x[.y]", sinon None (snapshot, inconnu).
fn parse_mc_version(v: &str) -> Option<(u32, u32)> {
    let v = v.trim();
    let mut parts = v.split('.');
    if parts.next()?.trim().parse::<u32>().ok()? != 1 {
        return None;
    }
    let digits = |s: &str| -> Option<u32> {
        let d: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
        d.parse().ok()
    };
    let minor = digits(parts.next()?)?;
    let patch = parts.next().and_then(digits).unwrap_or(0);
    Some((minor, patch))
}

/// Version majeure de Java requise : valeur déclarée par le JSON de version, sinon table.
pub fn required_java_major(minecraft_version: &str, declared: Option<u32>) -> u32 {
    if let Some(d) = declared.filter(|d| *d > 0) {
        return d;
    }
    match parse_mc_version(minecraft_version) {
        Some((minor, _)) if minor <= 16 => 8,
        Some((17, _)) => 16,
        Some((minor, _)) if minor < 20 => 17,
        Some((20, patch)) if patch <= 4 => 17,
        _ => 21,
    }
}

/// Extrait la version majeure d'une chaîne Java ("21.0.2", "1.8.0_392", "17").
pub fn parse_java_major(version: &str) -> Option<u32> {
    let v = version.trim();
    let mut parts = v.split(['.', '_', '-', '+']);
    let first: u32 = parts.next()?.parse().ok()?;
    if first == 1 {
        parts.next()?.parse().ok().filter(|m| *m > 0)
    } else if first > 0 {
        Some(first)
    } else {
        None
    }
}

/// Lit (major, vendor) depuis le fichier `release` d'un dossier de JRE/JDK.
fn read_release(home: &Path) -> Option<(u32, Option<String>)> {
    let text = fs::read_to_string(home.join("release")).ok()?;
    let mut version = None;
    let mut vendor = None;
    for line in text.lines() {
        let Some((k, val)) = line.split_once('=') else { continue };
        let val = val.trim().trim_matches('"').trim();
        match k.trim() {
            "JAVA_VERSION" => version = Some(val.to_string()),
            "IMPLEMENTOR" if !val.is_empty() => vendor = Some(val.to_string()),
            _ => {}
        }
    }
    Some((parse_java_major(&version?)?, vendor))
}

fn java_binary(home: &Path) -> Option<PathBuf> {
    ["bin/java.exe", "bin/java"]
        .iter()
        .map(|p| home.join(p))
        .find(|p| p.is_file())
}

/// Examine un dossier candidat (racine du JRE) ; gère aussi le layout macOS `Contents/Home`.
fn inspect_home(dir: &Path) -> Option<JavaInstall> {
    for home in [dir.to_path_buf(), dir.join("Contents").join("Home")] {
        if let (Some(path), Some((major, vendor))) = (java_binary(&home), read_release(&home)) {
            return Some(JavaInstall { path, major, vendor });
        }
    }
    None
}

/// Parcourt les dossiers : chaque dossier est soit un JRE lui-même, soit un parent de JRE.
pub fn find_installed(search_dirs: &[PathBuf]) -> Vec<JavaInstall> {
    let mut out: Vec<JavaInstall> = Vec::new();
    let mut push = |i: JavaInstall| {
        if !out.iter().any(|o| o.path == i.path) {
            out.push(i);
        }
    };
    for dir in search_dirs {
        if let Some(i) = inspect_home(dir) {
            push(i);
            continue;
        }
        let Ok(rd) = fs::read_dir(dir) else { continue };
        let mut children: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        children.sort();
        for c in children.into_iter().filter(|c| c.is_dir()) {
            if let Some(i) = inspect_home(&c) {
                push(i);
            }
        }
    }
    out
}

/// Repli : exécute `<java> -version` via un exécuteur injectable (retourne la sortie stderr/stdout).
pub fn probe_with<F>(java: &Path, run: F) -> Option<JavaInstall>
where
    F: FnOnce(&Path) -> Option<String>,
{
    let output = run(java)?;
    let start = output.find('"')? + 1;
    let end = start + output[start..].find('"')?;
    let major = parse_java_major(&output[start..end])?;
    Some(JavaInstall { path: java.to_path_buf(), major, vendor: None })
}

/// Exécuteur réel pour `probe_with` : lance `java -version`.
pub fn run_java_version(java: &Path) -> Option<String> {
    let o = std::process::Command::new(java).arg("-version").output().ok()?;
    let mut s = String::from_utf8_lossy(&o.stderr).into_owned();
    s.push_str(&String::from_utf8_lossy(&o.stdout));
    Some(s)
}

/// Dossiers de recherche usuels + dossier de runtimes du launcher.
pub fn default_search_dirs(launcher_runtimes_dir: &Path) -> Vec<PathBuf> {
    let mut v = vec![launcher_runtimes_dir.to_path_buf()];
    if let Some(h) = std::env::var_os("JAVA_HOME").filter(|h| !h.is_empty()) {
        v.push(PathBuf::from(h));
    }
    for var in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(pf) = std::env::var_os(var) {
            let pf = PathBuf::from(pf);
            for sub in ["Java", "Eclipse Adoptium", "Microsoft", "Zulu"] {
                v.push(pf.join(sub));
            }
        }
    }
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        v.push(PathBuf::from(home).join(".jdks"));
    }
    v.push(PathBuf::from("/usr/lib/jvm"));
    v.push(PathBuf::from("/Library/Java/JavaVirtualMachines"));
    v
}

/// Choisit l'installation : version exacte préférée ; sinon la plus petite version
/// supérieure compatible pour >= 17 ; pour 8, exige 8 ; pour 16, 16 ou >= 17.
pub fn select_for(required_major: u32, installs: &[JavaInstall]) -> Option<&JavaInstall> {
    if let Some(i) = installs.iter().find(|i| i.major == required_major) {
        return Some(i);
    }
    if required_major < 16 {
        return None;
    }
    installs
        .iter()
        .filter(|i| i.major > required_major && i.major >= 17)
        .min_by_key(|i| i.major)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_jre(root: &Path, name: &str, release: Option<&str>, macos: bool) -> PathBuf {
        let dir = root.join(name);
        let home = if macos { dir.join("Contents").join("Home") } else { dir.clone() };
        fs::create_dir_all(home.join("bin")).unwrap();
        fs::write(home.join("bin").join("java"), b"").unwrap();
        if let Some(r) = release {
            fs::write(home.join("release"), r).unwrap();
        }
        dir
    }

    #[test]
    fn version_table() {
        let r = |v, d| required_java_major(v, d);
        assert_eq!(r("1.8.9", None), 8);
        assert_eq!(r("1.12.2", None), 8);
        assert_eq!(r("1.16.5", None), 8);
        assert_eq!(r("1.17.1", None), 16);
        assert_eq!(r("1.18", None), 17);
        assert_eq!(r("1.20.4", None), 17);
        assert_eq!(r("1.20.5", None), 21);
        assert_eq!(r("1.20.6", None), 21);
        assert_eq!(r("1.21.1", None), 21);
        assert_eq!(r("1.21", None), 21);
        assert_eq!(r("24w14a", None), 21);
        assert_eq!(r("", None), 21);
        assert_eq!(r("1.16.5-rc1", None), 8);
        assert_eq!(r("1.12.2", Some(17)), 17);
        assert_eq!(r("1.21", Some(0)), 21);
    }

    #[test]
    fn java_major_parsing() {
        assert_eq!(parse_java_major("21.0.2"), Some(21));
        assert_eq!(parse_java_major("1.8.0_392"), Some(8));
        assert_eq!(parse_java_major("17"), Some(17));
        assert_eq!(parse_java_major("17-ea"), Some(17));
        assert_eq!(parse_java_major("garbage"), None);
        assert_eq!(parse_java_major(""), None);
    }

    #[test]
    fn finds_java_8_17_21_and_macos() {
        let t = tempfile::tempdir().unwrap();
        fake_jre(t.path(), "jre8", Some("JAVA_VERSION=\"1.8.0_392\"\n"), false);
        fake_jre(t.path(), "jdk17", Some("JAVA_VERSION=\"17.0.9\"\nIMPLEMENTOR=\"Eclipse Adoptium\"\n"), false);
        fake_jre(t.path(), "jdk21.jdk", Some("JAVA_VERSION=\"21.0.2\"\n"), true);
        let found = find_installed(&[t.path().to_path_buf()]);
        let mut majors: Vec<u32> = found.iter().map(|i| i.major).collect();
        majors.sort();
        assert_eq!(majors, vec![8, 17, 21]);
        let j17 = found.iter().find(|i| i.major == 17).unwrap();
        assert_eq!(j17.vendor.as_deref(), Some("Eclipse Adoptium"));
        assert!(found.iter().find(|i| i.major == 21).unwrap().path.ends_with("java"));
    }

    #[test]
    fn dir_itself_is_a_jre_and_empty_dir() {
        let t = tempfile::tempdir().unwrap();
        let jre = fake_jre(t.path(), "direct", Some("JAVA_VERSION=\"21.0.1\"\n"), false);
        assert_eq!(find_installed(&[jre]).len(), 1);
        let empty = tempfile::tempdir().unwrap();
        assert!(find_installed(&[empty.path().to_path_buf()]).is_empty());
        assert!(find_installed(&[empty.path().join("absent")]).is_empty());
    }

    #[test]
    fn missing_or_corrupt_release_ignored() {
        let t = tempfile::tempdir().unwrap();
        fake_jre(t.path(), "norelease", None, false);
        fake_jre(t.path(), "corrupt", Some("\u{0}\u{1}not a release"), false);
        fake_jre(t.path(), "badver", Some("JAVA_VERSION=\"abc\"\n"), false);
        fs::create_dir_all(t.path().join("nobin")).unwrap();
        fs::write(t.path().join("nobin").join("release"), "JAVA_VERSION=\"21\"\n").unwrap();
        assert!(find_installed(&[t.path().to_path_buf()]).is_empty());
    }

    fn inst(major: u32) -> JavaInstall {
        JavaInstall { path: PathBuf::from(format!("j{major}")), major, vendor: None }
    }

    #[test]
    fn selection_rules() {
        let all = vec![inst(8), inst(17), inst(21)];
        assert_eq!(select_for(17, &all).unwrap().major, 17);
        assert_eq!(select_for(8, &all).unwrap().major, 8);
        assert_eq!(select_for(17, &[inst(21), inst(25)]).unwrap().major, 21);
        assert!(select_for(8, &[inst(17), inst(21)]).is_none());
        assert!(select_for(21, &[inst(17), inst(8)]).is_none());
        assert_eq!(select_for(16, &[inst(8), inst(17)]).unwrap().major, 17);
        assert!(select_for(21, &[]).is_none());
    }

    #[test]
    fn probe_injectable() {
        let out = "openjdk version \"21.0.2\" 2024-01-16\nOpenJDK Runtime".to_string();
        let r = probe_with(Path::new("java"), |_| Some(out)).unwrap();
        assert_eq!(r.major, 21);
        assert!(probe_with(Path::new("java"), |_| None).is_none());
        assert!(probe_with(Path::new("java"), |_| Some("nope".into())).is_none());
        let old = probe_with(Path::new("java"), |_| Some("java version \"1.8.0_392\"".into()));
        assert_eq!(old.unwrap().major, 8);
    }

    #[test]
    fn default_dirs_include_launcher_dir() {
        let d = default_search_dirs(Path::new("/x/runtimes"));
        assert_eq!(d[0], PathBuf::from("/x/runtimes"));
        assert!(d.contains(&PathBuf::from("/usr/lib/jvm")));
    }
}
