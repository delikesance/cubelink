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
    /// Architecture de la JVM (champ OS_ARCH du fichier `release`, en minuscules) ;
    /// `None` si absente : l'installation est alors acceptée.
    pub arch: Option<String>,
}

/// Vrai pour une valeur `OS_ARCH` de JVM 32 bits (liste blanche : tout le reste est accepté).
fn is_32bit_arch(arch: &str) -> bool {
    let a = arch.trim().trim_matches('"').to_lowercase();
    matches!(a.as_str(), "x86" | "i386" | "i486" | "i586" | "i686" | "arm" | "aarch32")
}

impl JavaInstall {
    /// Vrai si la JVM est connue comme 32 bits.
    pub fn is_32bit(&self) -> bool {
        self.arch.as_deref().is_some_and(is_32bit_arch)
    }

    /// Une JVM 32 bits ne peut pas réserver un grand tas (`-Xmx` > ~1,5 Go) : elle est
    /// inutilisable sur un hôte 64 bits. Sur un hôte 32 bits, elle reste la seule option.
    /// Un Java x64 émulé (Windows ARM, Rosetta) annonce `amd64`/`x86_64` et reste accepté.
    pub fn usable_on_host(&self) -> bool {
        !(cfg!(target_pointer_width = "64") && self.is_32bit())
    }
}

/// Analyse un numéro de version de Minecraft ("1.20.4", "1.21", "1.16.5-rc1").
/// Retourne (minor, patch) pour une version "1.x[.y]", sinon None (snapshot, inconnu).
pub(crate) fn parse_mc_version(v: &str) -> Option<(u32, u32)> {
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
fn read_release(home: &Path) -> Option<(u32, Option<String>, Option<String>)> {
    let text = fs::read_to_string(home.join("release")).ok()?;
    let mut version = None;
    let mut vendor = None;
    let mut arch = None;
    for line in text.lines() {
        let Some((k, val)) = line.split_once('=') else { continue };
        let val = val.trim().trim_matches('"').trim();
        match k.trim() {
            "JAVA_VERSION" => version = Some(val.to_string()),
            "IMPLEMENTOR" if !val.is_empty() => vendor = Some(val.to_string()),
            "OS_ARCH" if !val.is_empty() => arch = Some(val.to_lowercase()),
            _ => {}
        }
    }
    Some((parse_java_major(&version?)?, vendor, arch))
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
        if let (Some(path), Some((major, vendor, arch))) = (java_binary(&home), read_release(&home)) {
            return Some(JavaInstall { path, major, vendor, arch });
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
    Some(JavaInstall { path: java.to_path_buf(), major, vendor: None, arch: None })
}

/// Empêche Windows d'ouvrir une fenêtre de console pour un sous-processus (`CREATE_NO_WINDOW`).
/// Sans effet hors Windows. À appliquer à tout `Command` qui lance java ou un outil externe.
pub fn no_window(cmd: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

/// Exécuteur réel pour `probe_with` : lance `java -version`.
pub fn run_java_version(java: &Path) -> Option<String> {
    let mut cmd = std::process::Command::new(java);
    cmd.arg("-version");
    let o = no_window(&mut cmd).output().ok()?;
    let mut s = String::from_utf8_lossy(&o.stderr).into_owned();
    s.push_str(&String::from_utf8_lossy(&o.stdout));
    Some(s)
}

/// Verdict sur un Java choisi à la main (réglage global ou propre à l'instance) pour une version de Minecraft.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfiguredJava {
    /// La version majeure convient (mêmes règles que [`select_for`]).
    Fits,
    /// La version majeure ne convient pas : `found` est celle que la JVM annonce.
    Mismatch { found: u32 },
    /// `java -version` n'a rien d'exploitable (wrapper, sortie inhabituelle) : on ne peut pas juger.
    Unprobeable,
}

/// Sonde `java` avec l'exécuteur `run` et le compare à `required` : pure, testable sans JVM.
pub fn check_configured_with<F>(java: &Path, required: u32, run: F) -> ConfiguredJava
where
    F: FnOnce(&Path) -> Option<String>,
{
    match probe_with(java, run) {
        None => ConfiguredJava::Unprobeable,
        Some(p) if select_for(required, std::slice::from_ref(&p)).is_some() => ConfiguredJava::Fits,
        Some(p) => ConfiguredJava::Mismatch { found: p.major },
    }
}

/// Comme [`check_configured_with`] avec le vrai `java -version` (lance un processus : à appeler hors du fil asynchrone).
pub fn check_configured(java: &Path, required: u32) -> ConfiguredJava {
    check_configured_with(java, required, run_java_version)
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
    #[cfg(unix)]
    v.extend(path_java_homes(std::env::var_os("PATH").as_deref()));
    v
}

/// Racines de JDK/JRE des `java` trouvés dans le PATH (liens résolus) : couvre NixOS, où les Java vivent dans `/nix/store`
/// et ne sont dans aucun dossier usuel.
#[cfg(unix)]
fn path_java_homes(path: Option<&std::ffi::OsStr>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for dir in path.map(|p| std::env::split_paths(p).collect::<Vec<_>>()).unwrap_or_default() {
        let Ok(real) = fs::canonicalize(dir.join("java")) else { continue };
        let Some(home) = real.parent().and_then(Path::parent) else { continue };
        if !out.iter().any(|o| o == home) {
            out.push(home.to_path_buf());
        }
    }
    out
}

/// Choisit l'installation : version exacte préférée ; sinon la plus petite version
/// supérieure compatible pour >= 17 ; pour 8, exige 8 ; pour 16, 16 ou >= 17.
/// Les JVM 32 bits sont écartées sur un hôte 64 bits (voir [`JavaInstall::usable_on_host`]).
pub fn select_for(required_major: u32, installs: &[JavaInstall]) -> Option<&JavaInstall> {
    let usable = || installs.iter().filter(|i| i.usable_on_host());
    if let Some(i) = usable().find(|i| i.major == required_major) {
        return Some(i);
    }
    if required_major < 16 {
        return None;
    }
    usable()
        .filter(|i| i.major > required_major && i.major >= 17)
        .min_by_key(|i| i.major)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn says(v: &'static str) -> impl FnOnce(&Path) -> Option<String> {
        move |_| Some(format!("openjdk version \"{v}\" 2024-01-01\nOpenJDK Runtime Environment"))
    }

    #[test]
    fn java_choisi_a_la_main_compare_a_la_version_requise() {
        let j = Path::new("/x/java");
        // 17 pour 21 : refusé ; 21 pour 21 : accepté ; 21 pour 17 : accepté (>= 17) ; 16 -> 17 accepté.
        assert_eq!(check_configured_with(j, 21, says("17.0.9")), ConfiguredJava::Mismatch { found: 17 });
        assert_eq!(check_configured_with(j, 21, says("21.0.2")), ConfiguredJava::Fits);
        assert_eq!(check_configured_with(j, 17, says("21.0.2")), ConfiguredJava::Fits);
        assert_eq!(check_configured_with(j, 16, says("17.0.9")), ConfiguredJava::Fits);
        // En dessous de 16 on exige la version exacte (1.12.2 Forge : Java 8 seulement).
        assert_eq!(check_configured_with(j, 8, says("21.0.2")), ConfiguredJava::Mismatch { found: 21 });
        assert_eq!(check_configured_with(j, 8, says("1.8.0_392")), ConfiguredJava::Fits);
        assert_eq!(check_configured_with(j, 21, says("1.8.0_392")), ConfiguredJava::Mismatch { found: 8 });
        // Sortie illisible ou exécution impossible : on ne juge pas.
        assert_eq!(check_configured_with(j, 21, |_| Some("n'importe quoi".into())), ConfiguredJava::Unprobeable);
        assert_eq!(check_configured_with(j, 21, |_| None), ConfiguredJava::Unprobeable);
    }

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
        JavaInstall { path: PathBuf::from(format!("j{major}")), major, vendor: None, arch: None }
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

    fn inst_arch(major: u32, name: &str, arch: Option<&str>) -> JavaInstall {
        JavaInstall {
            path: PathBuf::from(name),
            major,
            vendor: None,
            arch: arch.map(str::to_string),
        }
    }

    #[test]
    fn arch_32_bits_reconnue() {
        for a in ["x86", "i386", "i486", "i586", "i686", "arm", "aarch32", "X86", "\"i586\""] {
            assert!(is_32bit_arch(a), "{a}");
        }
        for a in ["amd64", "x86_64", "aarch64", "arm64", "", "riscv64", "inconnu"] {
            assert!(!is_32bit_arch(a), "{a}");
        }
    }

    #[test]
    fn release_os_arch_est_lu() {
        let t = tempfile::tempdir().unwrap();
        fake_jre(t.path(), "a", Some("JAVA_VERSION=\"1.8.0_401\"\nOS_ARCH=\"I586\"\n"), false);
        fake_jre(t.path(), "b", Some("JAVA_VERSION=\"17.0.9\"\nOS_ARCH=\"amd64\"\n"), false);
        fake_jre(t.path(), "c", Some("JAVA_VERSION=\"21.0.2\"\n"), false);
        let found = find_installed(&[t.path().to_path_buf()]);
        let arch = |m| found.iter().find(|i| i.major == m).unwrap().arch.clone();
        assert_eq!(arch(8).as_deref(), Some("i586"));
        assert_eq!(arch(17).as_deref(), Some("amd64"));
        assert_eq!(arch(21), None);
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn java_32_bits_ecarte_sur_hote_64_bits() {
        let t = tempfile::tempdir().unwrap();
        fake_jre(t.path(), "jre8-x86", Some("JAVA_VERSION=\"1.8.0_401\"\nOS_ARCH=\"i586\"\n"), false);
        // Un seul Java 8, en 32 bits : rien de sélectionnable, l'appelant installe le JRE Adoptium.
        let found = find_installed(&[t.path().to_path_buf()]);
        assert_eq!(found.len(), 1);
        assert!(select_for(8, &found).is_none());

        // Avec un Java 8 64 bits à côté : lui seul est choisi, quel que soit l'ordre.
        fake_jre(t.path(), "jre8-x64", Some("JAVA_VERSION=\"1.8.0_401\"\nOS_ARCH=\"amd64\"\n"), false);
        let mut found = find_installed(&[t.path().to_path_buf()]);
        for _ in 0..2 {
            let sel = select_for(8, &found).unwrap();
            assert!(sel.path.to_string_lossy().contains("jre8-x64"));
            found.reverse();
        }
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn java_32_bits_ne_sert_pas_de_repli() {
        // 17 en 32 bits ne remplace pas le Java 16 demandé ; un 21 64 bits reste admis.
        let x86_17 = inst_arch(17, "j17-x86", Some("x86"));
        assert!(select_for(16, std::slice::from_ref(&x86_17)).is_none());
        assert!(select_for(17, std::slice::from_ref(&x86_17)).is_none());
        let all = [x86_17, inst_arch(21, "j21", Some("x86_64"))];
        assert_eq!(select_for(17, &all).unwrap().major, 21);
        assert_eq!(select_for(16, &all).unwrap().major, 21);
    }

    #[test]
    fn java_64_bits_ou_arch_inconnue_accepte() {
        for a in [Some("amd64"), Some("x86_64"), Some("aarch64"), None] {
            let i = inst_arch(8, "j8", a);
            assert!(i.usable_on_host(), "{a:?}");
            assert!(select_for(8, std::slice::from_ref(&i)).is_some());
        }
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

    #[cfg(unix)]
    #[test]
    fn path_java_resolves_links_to_home() {
        let tmp = std::env::temp_dir().join(format!("cubelink-pathjava-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("jdk/bin")).unwrap();
        fs::create_dir_all(tmp.join("sw/bin")).unwrap();
        fs::write(tmp.join("jdk/bin/java"), "").unwrap();
        std::os::unix::fs::symlink(tmp.join("jdk/bin/java"), tmp.join("sw/bin/java")).unwrap();
        let homes = path_java_homes(Some(tmp.join("sw/bin").as_os_str()));
        assert_eq!(homes, [fs::canonicalize(tmp.join("jdk")).unwrap()]);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn default_dirs_include_launcher_dir() {
        let d = default_search_dirs(Path::new("/x/runtimes"));
        assert_eq!(d[0], PathBuf::from("/x/runtimes"));
        assert!(d.contains(&PathBuf::from("/usr/lib/jvm")));
    }
}
