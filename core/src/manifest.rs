//! Tâche N9 : manifeste hôte d'une instance (version, loader, fichiers partagés + SHA-256).
//!
//! Côté hôte : `build_manifest`. Côté invité : `validate_manifest` (le manifeste reçu est hostile
//! par défaut). `manifest_hash` donne l'empreinte d'une sérialisation canonique.

use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::Path;

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::java::required_java_major;
use crate::types::{Error, Instance, Manifest, ManifestFile, Result, Sha256Hex};

/// Nombre maximal de fichiers d'un manifeste.
pub const MAX_FILES: usize = 10_000;
/// Taille maximale plausible d'un fichier (16 Gio).
pub const MAX_FILE_SIZE: u64 = 16 * 1024 * 1024 * 1024;
/// Taille maximale cumulée plausible (256 Gio).
pub const MAX_TOTAL_SIZE: u64 = 256 * 1024 * 1024 * 1024;
/// Préfixes autorisés (côté invité). `kubejs` : scripts et ressources des modpacks (objets, recettes) ; sans eux le
/// serveur envoie des registres inconnus du client (« unknown keys »). `defaultconfigs` : réglages par défaut du pack.
pub const ALLOWED_PREFIXES: [&str; 6] = ["mods", "config", "resourcepacks", "shaderpacks", "kubejs", "defaultconfigs"];
/// Sous-dossiers générés localement, jamais partagés (exports et typages de KubeJS/ProbeJS, parfois énormes).
const GENERATED_DIRS: [&str; 2] = ["kubejs/exported/", "kubejs/probe/"];

const CHUNK: usize = 1024 * 1024;
/// Fragments de nom de fichier jamais partagés (jetons, identifiants).
const SECRET_HINTS: [&str; 6] = ["token", "credential", "secret", "password", "launcher_profiles", "accounts.json"];

/// Options de génération.
#[derive(Debug, Clone, Default)]
pub struct ManifestOptions {
    pub include_config: bool,
    pub include_resourcepacks: bool,
    pub include_shaderpacks: bool,
    /// Version Java déclarée par le JSON de version Mojang, si connue (sinon table de `java.rs`).
    pub declared_java_major: Option<u32>,
}

/// Fichiers ignorés : (chemin relatif ou nom, raison).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManifestReport {
    pub skipped: Vec<(String, String)>,
}

/// Progression du hachage.
#[derive(Debug, Clone)]
pub struct ManifestProgress<'a> {
    pub path: &'a str,
    /// Octets lus du fichier courant.
    pub file_bytes: u64,
    /// Octets lus au total.
    pub total_bytes: u64,
    /// Fichiers terminés (le courant exclu tant que `file_done` est faux).
    pub files_done: usize,
    pub files_total: usize,
    pub file_done: bool,
}

/// Génère le manifeste (rapport et progression ignorés).
pub fn build_manifest(instance: &Instance, game_dir: &Path, opts: &ManifestOptions) -> Result<Manifest> {
    build_manifest_with(instance, game_dir, opts, &mut |_| {}).map(|(m, _)| m)
}

/// Génère le manifeste avec rapport des fichiers ignorés et callback de progression.
pub fn build_manifest_with(
    instance: &Instance,
    game_dir: &Path,
    opts: &ManifestOptions,
    on_progress: &mut dyn FnMut(&ManifestProgress<'_>),
) -> Result<(Manifest, ManifestReport)> {
    let root = game_dir.canonicalize()?;
    let mut report = ManifestReport::default();
    let mut candidates: Vec<(String, std::path::PathBuf)> = Vec::new();

    let mut dirs = vec!["mods"];
    if opts.include_config {
        dirs.extend(["config", "kubejs", "defaultconfigs"]);
    }
    if opts.include_resourcepacks {
        dirs.push("resourcepacks");
    }
    if opts.include_shaderpacks {
        dirs.push("shaderpacks");
    }
    for d in dirs {
        let p = root.join(d);
        match fs::symlink_metadata(&p) {
            Ok(m) if m.is_dir() => walk(&root, &p, d, d == "mods", &mut candidates, &mut report)?,
            Ok(_) => report.skipped.push((d.to_string(), "n'est pas un dossier".into())),
            Err(_) => {}
        }
    }

    candidates.retain(|(rel, _)| !GENERATED_DIRS.iter().any(|g| rel.starts_with(g)));
    candidates.sort_by(|a, b| a.0.cmp(&b.0));
    let files_total = candidates.len();
    if files_total > MAX_FILES {
        return Err(Error::Invalid(format!("trop de fichiers partagés : {files_total} > {MAX_FILES}")));
    }
    let mut files = Vec::with_capacity(files_total);
    let mut total_bytes = 0u64;
    for (i, (rel, abs)) in candidates.iter().enumerate() {
        let base = total_bytes;
        let want_sha1 = rel.starts_with("mods/");
        let hashed = hash_stream(abs, want_sha1, &mut |file_bytes| {
            on_progress(&ManifestProgress {
                path: rel,
                file_bytes,
                total_bytes: base + file_bytes,
                files_done: i,
                files_total,
                file_done: false,
            })
        });
        let (sha256, sha1, cf_fingerprint, size) = match hashed {
            Ok(v) => v,
            // Fichier verrouillé par le jeu (partage 32, verrou d'octets 33) ou disparu : jamais bloquant pour le partage.
            Err(Error::Io(e)) if is_locked_or_gone(&e) => {
                report.skipped.push((rel.clone(), format!("illisible pour l'instant ({e})")));
                on_progress(&ManifestProgress { path: rel, file_bytes: 0, total_bytes: base, files_done: i + 1, files_total, file_done: true });
                continue;
            }
            Err(e) => return Err(e),
        };
        total_bytes = base + size;
        on_progress(&ManifestProgress {
            path: rel,
            file_bytes: size,
            total_bytes,
            files_done: i + 1,
            files_total,
            file_done: true,
        });
        files.push(ManifestFile { path: rel.clone(), size, sha256, sha1, cf_fingerprint });
    }
    report.skipped.sort();

    let manifest = Manifest {
        name: instance.name.clone(),
        minecraft: instance.minecraft.clone(),
        loader: instance.loader.clone(),
        java_major: required_java_major(&instance.minecraft, opts.declared_java_major),
        files,
    };
    Ok((manifest, report))
}

/// SHA-256 (et, si `want_sha1` : SHA-1 et empreinte CurseForge) en une seule passe ; `progress` reçoit le
/// nombre d'octets lus.
fn hash_stream(
    path: &Path,
    want_sha1: bool,
    progress: &mut dyn FnMut(u64),
) -> Result<(Sha256Hex, Option<String>, Option<u32>, u64)> {
    let mut f = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut sha1 = want_sha1.then(crate::unknown_mods::Sha1::new);
    // Octets non blancs (seuls comptés par l'empreinte CurseForge) ; abandonné au-delà du plafond.
    let mut cf_data: Option<Vec<u8>> = want_sha1.then(Vec::new);
    let mut buf = vec![0u8; CHUNK];
    let mut size = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        if let Some(s) = sha1.as_mut() {
            s.update(&buf[..n]);
        }
        size += n as u64;
        if size > crate::unknown_mods::MAX_CF_FINGERPRINT_BYTES {
            cf_data = None;
        }
        if let Some(d) = cf_data.as_mut() {
            d.extend(buf[..n].iter().filter(|b| !matches!(b, 9 | 10 | 13 | 32)));
        }
        progress(size);
    }
    let cf = cf_data.map(|d| crate::providers::curseforge_api::murmur2_fingerprint(&d));
    Ok((hex::encode(hasher.finalize()), sha1.map(|s| s.finish()), cf, size))
}

/// Erreurs d'E/S « le fichier est utilisé par le jeu » (Windows 32 partage, 33 verrou) ou « disparu entre-temps ».
fn is_locked_or_gone(e: &std::io::Error) -> bool {
    matches!(e.raw_os_error(), Some(32) | Some(33)) || e.kind() == std::io::ErrorKind::NotFound
}

fn is_hidden(name: &str, meta: &fs::Metadata) -> bool {
    if name.starts_with('.') || name.eq_ignore_ascii_case("thumbs.db") || name.eq_ignore_ascii_case("desktop.ini") {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // HIDDEN (0x2) | SYSTEM (0x4)
        if meta.file_attributes() & 0x6 != 0 {
            return true;
        }
    }
    let _ = meta;
    false
}

fn walk(
    root: &Path,
    dir: &Path,
    rel: &str,
    jars_only: bool,
    out: &mut Vec<(String, std::path::PathBuf)>,
    report: &mut ManifestReport,
) -> Result<()> {
    let mut entries: Vec<_> = fs::read_dir(dir)?.collect::<std::io::Result<_>>()?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let os_name = e.file_name();
        let Some(name) = os_name.to_str() else {
            report.skipped.push((format!("{rel}/{}", os_name.to_string_lossy()), "nom non UTF-8".into()));
            continue;
        };
        let rel_path = format!("{rel}/{name}");
        let path = e.path();
        let meta = fs::symlink_metadata(&path)?;
        if is_hidden(name, &meta) {
            report.skipped.push((rel_path, "fichier caché ou système".into()));
            continue;
        }
        if let Err(why) = check_segment(name) {
            report.skipped.push((rel_path, why));
            continue;
        }
        let mut is_dir = meta.is_dir();
        if meta.file_type().is_symlink() {
            match path.canonicalize() {
                Ok(t) if t.starts_with(root) => {
                    let tm = fs::metadata(&t)?;
                    if tm.is_dir() {
                        report.skipped.push((rel_path, "lien symbolique vers un dossier".into()));
                        continue;
                    }
                    is_dir = false;
                }
                Ok(_) => {
                    report.skipped.push((rel_path, "lien symbolique hors du dossier".into()));
                    continue;
                }
                Err(_) => {
                    report.skipped.push((rel_path, "lien symbolique cassé".into()));
                    continue;
                }
            }
        } else if !is_dir && !meta.is_file() {
            report.skipped.push((rel_path, "ni fichier ni dossier".into()));
            continue;
        }
        if is_dir {
            walk(root, &path, &rel_path, jars_only, out, report)?;
            continue;
        }
        if jars_only && !name.to_ascii_lowercase().ends_with(".jar") {
            report.skipped.push((rel_path, "pas un .jar".into()));
            continue;
        }
        let lower = name.to_ascii_lowercase();
        if crate::mrpack::has_dangerous_ext(name) {
            report.skipped.push((rel_path, "fichier exécutable".into()));
            continue;
        }
        if lower.ends_with(".lock") {
            report.skipped.push((rel_path, "fichier de verrou (propre au jeu qui tourne)".into()));
            continue;
        }
        if SECRET_HINTS.iter().any(|h| lower.contains(h)) {
            report.skipped.push((rel_path, "nom évoquant un jeton ou un identifiant".into()));
            continue;
        }
        out.push((rel_path, path));
    }
    Ok(())
}

// ---------- Hash canonique ----------

fn write_canonical(v: &Value, out: &mut String) {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(k.clone()).to_string());
                out.push(':');
                write_canonical(&map[k], out);
            }
            out.push('}');
        }
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(x, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// Sérialisation canonique : JSON à clés triées, sans espaces.
pub fn canonical_json(m: &Manifest) -> String {
    let v = serde_json::to_value(m).expect("Manifest est sérialisable");
    let mut s = String::new();
    write_canonical(&v, &mut s);
    s
}

/// SHA-256 (hex minuscule) de la sérialisation canonique.
///
/// NB (S1M) : le champ `sha1` (jars de `mods/`) fait partie de la sérialisation canonique quand il
/// est présent ; le hash d'un manifeste généré par cette version diffère donc de celui des versions
/// antérieures. Un manifeste sans sha1 garde exactement l'ancienne sérialisation (donc l'ancien hash).
/// Idem pour `cf_fingerprint`. Ce hash ne sert qu'à nommer le transfert côté invité (jamais comparé entre pairs).
pub fn manifest_hash(m: &Manifest) -> Sha256Hex {
    hex::encode(Sha256::digest(canonical_json(m).as_bytes()))
}

// ---------- Validation (côté invité) ----------

fn check_segment(seg: &str) -> std::result::Result<(), String> {
    if seg.is_empty() {
        return Err("segment vide".into());
    }
    if seg == "." || seg == ".." {
        return Err("segment '.' ou '..'".into());
    }
    if seg.chars().any(|c| c.is_control() || "<>:\"|?*\\".contains(c)) {
        return Err("caractère interdit".into());
    }
    if seg.ends_with('.') || seg.ends_with(' ') {
        return Err("segment finissant par '.' ou espace".into());
    }
    let stem = seg.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && matches!(stem.as_bytes()[3], b'1'..=b'9'));
    if reserved {
        return Err("nom réservé Windows".into());
    }
    Ok(())
}

fn check_path(path: &str) -> std::result::Result<(), String> {
    if path.is_empty() {
        return Err("chemin vide".into());
    }
    if path.contains('\\') {
        return Err("contient '\\'".into());
    }
    if path.starts_with('/') {
        return Err("chemin absolu".into());
    }
    let b = path.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return Err("lettre de lecteur".into());
    }
    let segs: Vec<&str> = path.split('/').collect();
    if segs.len() < 2 {
        return Err("aucun préfixe de dossier".into());
    }
    if !ALLOWED_PREFIXES.contains(&segs[0]) {
        return Err(format!("préfixe non autorisé : {}", segs[0]));
    }
    for s in segs {
        check_segment(s)?;
    }
    Ok(())
}

/// Valide un manifeste reçu : chemins sûrs, hash bien formés, tailles et nombre bornés.
pub fn validate_manifest(m: &Manifest) -> Result<()> {
    let bad = |msg: String| Err(Error::Invalid(format!("manifeste : {msg}")));
    if m.files.len() > MAX_FILES {
        return bad(format!("trop de fichiers ({} > {MAX_FILES})", m.files.len()));
    }
    if !(1..=100).contains(&m.java_major) {
        return bad(format!("version Java implausible : {}", m.java_major));
    }
    let mut seen = HashSet::new();
    let mut total = 0u64;
    for f in &m.files {
        if let Err(why) = check_path(&f.path) {
            return bad(format!("chemin {:?} refusé : {why}", f.path));
        }
        if crate::mrpack::has_dangerous_ext(&f.path) {
            return bad(format!("fichier exécutable refusé : {:?}", f.path));
        }
        if !seen.insert(f.path.to_ascii_lowercase()) {
            return bad(format!("doublon : {:?}", f.path));
        }
        if f.sha256.len() != 64 || !f.sha256.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f')) {
            return bad(format!("sha256 mal formé pour {:?}", f.path));
        }
        if f.size > MAX_FILE_SIZE {
            return bad(format!("taille implausible pour {:?} : {}", f.path, f.size));
        }
        let is_jar = f.path.starts_with("mods/") && f.path.to_ascii_lowercase().ends_with(".jar");
        if f.cf_fingerprint.is_some() && !is_jar {
            return bad(format!("empreinte CurseForge non autorisée hors des jars de mods/ : {:?}", f.path));
        }
        if let Some(h) = &f.sha1 {
            if !is_jar {
                return bad(format!("sha1 non autorisé hors des jars de mods/ : {:?}", f.path));
            }
            if h.len() != 40 || !h.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f')) {
                return bad(format!("sha1 mal formé pour {:?}", f.path));
            }
        }
        total = total.saturating_add(f.size);
    }
    if total > MAX_TOTAL_SIZE {
        return bad(format!("taille totale implausible : {total}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Loader, LoaderSpec};
    use std::path::PathBuf;

    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    fn inst() -> Instance {
        Instance {
            id: "i1".into(),
            name: "Mon monde".into(),
            minecraft: "1.21.1".into(),
            loader: LoaderSpec { loader: Loader::NeoForge, version: "21.1.77".into() },
            ram_mb: 4096,
            mod_count: 2,
            java_path: None,
            jvm_args: String::new(),
            art: None,
        }
    }

    fn put(root: &Path, rel: &str, data: &[u8]) {
        let p: PathBuf = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, data).unwrap();
    }

    fn fake_game() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        put(r, "mods/b.jar", b"abc");
        put(r, "mods/a.jar", b"");
        put(r, "mods/readme.txt", b"x");
        put(r, "mods/.hidden.jar", b"x");
        put(r, "config/c.toml", b"k=1");
        put(r, "config/auth_token.json", b"secret");
        put(r, "resourcepacks/rp.zip", b"rp");
        put(r, "shaderpacks/s.zip", b"s");
        put(r, "kubejs/startup_scripts/items.js", b"s");
        put(r, "kubejs/assets/kubejs/textures/item/red_cell.png", b"p");
        put(r, "kubejs/exported/tags.json", b"x");
        put(r, "kubejs/probe/generated/x.d.ts", b"x");
        put(r, "defaultconfigs/ftb.snbt", b"d");
        put(r, "config/outil.exe", b"MZ");
        put(r, "saves/w/level.dat", b"x");
        put(r, "logs/latest.log", b"x");
        put(r, "crash-reports/c.txt", b"x");
        put(r, "screenshots/s.png", b"x");
        put(r, ".cubelink/state", b"x");
        put(r, "options.txt", b"x");
        put(r, "servers.dat", b"x");
        d
    }

    fn paths(m: &Manifest) -> Vec<&str> {
        m.files.iter().map(|f| f.path.as_str()).collect()
    }

    #[test]
    fn builds_manifest_of_fake_instance() {
        let d = fake_game();
        let m = build_manifest(&inst(), d.path(), &ManifestOptions::default()).unwrap();
        assert_eq!(m.name, "Mon monde");
        assert_eq!(m.minecraft, "1.21.1");
        assert_eq!(m.loader.version, "21.1.77");
        assert_eq!(m.java_major, 21);
        assert_eq!(paths(&m), ["mods/a.jar", "mods/b.jar"]);
        assert_eq!(m.files[1].sha256, ABC);
        assert_eq!(m.files[1].size, 3);
        assert_eq!(m.files[0].size, 0);
        validate_manifest(&m).unwrap();

        let opts = ManifestOptions {
            include_config: true,
            include_resourcepacks: true,
            include_shaderpacks: true,
            declared_java_major: Some(17),
        };
        let (m, rep) = build_manifest_with(&inst(), d.path(), &opts, &mut |_| {}).unwrap();
        assert_eq!(m.java_major, 17);
        assert_eq!(
            paths(&m),
            [
                "config/c.toml",
                "defaultconfigs/ftb.snbt",
                "kubejs/assets/kubejs/textures/item/red_cell.png",
                "kubejs/startup_scripts/items.js",
                "mods/a.jar",
                "mods/b.jar",
                "resourcepacks/rp.zip",
                "shaderpacks/s.zip"
            ]
        );
        assert!(rep.skipped.iter().any(|(p, _)| p == "config/auth_token.json"));
        assert!(rep.skipped.iter().any(|(p, _)| p == "mods/readme.txt"));
        assert!(rep.skipped.iter().any(|(p, why)| p == "config/outil.exe" && why.contains("exécutable")));
        validate_manifest(&m).unwrap();
    }

    #[test]
    fn deterministic_and_progress() {
        let d = fake_game();
        let opts = ManifestOptions { include_config: true, ..Default::default() };
        let m1 = build_manifest(&inst(), d.path(), &opts).unwrap();
        let mut calls = 0;
        let (m2, _) = build_manifest_with(&inst(), d.path(), &opts, &mut |p| {
            if p.file_done {
                calls += 1;
            }
        })
        .unwrap();
        assert_eq!(m1, m2);
        assert_eq!(canonical_json(&m1), canonical_json(&m2));
        assert_eq!(manifest_hash(&m1), manifest_hash(&m2));
        assert_eq!(calls, m1.files.len());
    }

    #[test]
    fn excludes_private_data_even_in_shared_dirs() {
        let d = fake_game();
        let opts = ManifestOptions {
            include_config: true,
            include_resourcepacks: true,
            include_shaderpacks: true,
            declared_java_major: None,
        };
        let m = build_manifest(&inst(), d.path(), &opts).unwrap();
        for f in &m.files {
            for bad in ["saves", "logs", "crash-reports", "screenshots", ".cubelink", "options.txt", "servers.dat", "token", ".hidden"] {
                assert!(!f.path.contains(bad), "{} ne doit pas être partagé", f.path);
            }
        }
    }

    #[test]
    fn fichiers_verrouilles_ne_bloquent_pas_le_manifeste() {
        let d = fake_game();
        put(d.path(), "config/controlify/profile-1.lock", b"");
        put(d.path(), "config/ok.toml", b"v=1");
        put(d.path(), "config/utilise.dat", b"verrouille par le jeu");
        // Verrou exclusif tenu pendant la construction (Windows : lecture refusée, erreur 33 ; ailleurs : lecture permise).
        let held = fs::File::open(d.path().join("config/utilise.dat")).unwrap();
        let locked = held.try_lock().is_ok();
        let opts = ManifestOptions { include_config: true, ..Default::default() };
        let (m, report) = build_manifest_with(&inst(), d.path(), &opts, &mut |_| {}).expect("un verrou ne doit pas faire échouer le manifeste");
        let p = paths(&m);
        assert!(p.contains(&"config/ok.toml") && p.contains(&"config/c.toml"), "{p:?}");
        assert!(!p.contains(&"config/controlify/profile-1.lock"), "{p:?}");
        assert!(report.skipped.iter().any(|(f, why)| f.ends_with("profile-1.lock") && why.contains("verrou")), "{:?}", report.skipped);
        if cfg!(windows) && locked {
            assert!(!p.contains(&"config/utilise.dat"), "{p:?}");
            assert!(report.skipped.iter().any(|(f, _)| f == "config/utilise.dat"), "{:?}", report.skipped);
        }
    }

    #[test]
    fn outward_symlink_is_skipped() {
        let d = fake_game();
        let outside = tempfile::tempdir().unwrap();
        put(outside.path(), "evil.jar", b"evil");
        let link = d.path().join("mods/evil.jar");
        #[cfg(unix)]
        let res = std::os::unix::fs::symlink(outside.path().join("evil.jar"), &link);
        #[cfg(windows)]
        let res = std::os::windows::fs::symlink_file(outside.path().join("evil.jar"), &link);
        if res.is_err() {
            eprintln!("création de lien symbolique impossible : test ignoré");
            return;
        }
        let (m, rep) = build_manifest_with(&inst(), d.path(), &ManifestOptions::default(), &mut |_| {}).unwrap();
        assert_eq!(paths(&m), ["mods/a.jar", "mods/b.jar"]);
        assert!(rep.skipped.iter().any(|(p, why)| p == "mods/evil.jar" && why.contains("hors")));
    }

    fn mf(files: Vec<ManifestFile>) -> Manifest {
        Manifest { name: "n".into(), minecraft: "1.21.1".into(), loader: inst().loader, java_major: 21, files }
    }

    fn file(path: &str) -> ManifestFile {
        ManifestFile { path: path.into(), size: 1, sha256: ABC.into(), sha1: None, cf_fingerprint: None }
    }

    const ABC_SHA1: &str = "a9993e364706816aba3e25717850c26c9cd0d89d";

    #[test]
    fn sha1_computed_for_mod_jars_only_and_deterministic() {
        let d = fake_game();
        let opts = ManifestOptions { include_config: true, ..Default::default() };
        let m1 = build_manifest(&inst(), d.path(), &opts).unwrap();
        let m2 = build_manifest(&inst(), d.path(), &opts).unwrap();
        assert_eq!(m1, m2);
        let f = |p: &str| m1.files.iter().find(|x| x.path == p).unwrap();
        assert_eq!(f("mods/b.jar").sha1.as_deref(), Some(ABC_SHA1));
        assert_eq!(f("mods/a.jar").sha1.as_deref(), Some("da39a3ee5e6b4b0d3255bfef95601890afd80709"));
        assert!(m1.files.iter().filter(|x| !x.path.starts_with("mods/")).all(|x| x.sha1.is_none()));
        assert!(m1.files.iter().filter(|f| f.path.starts_with("config/")).all(|f| f.sha1.is_none()));
        // Empreinte CurseForge : même règle que le sha1 (jars de mods/ seulement), égale au calcul de référence.
        let cf = crate::providers::curseforge_api::murmur2_fingerprint;
        assert_eq!(f("mods/b.jar").cf_fingerprint, Some(cf(&std::fs::read(d.path().join("mods/b.jar")).unwrap())));
        assert_eq!(f("mods/a.jar").cf_fingerprint, Some(cf(b"")));
        assert!(m1.files.iter().filter(|x| !x.path.starts_with("mods/")).all(|x| x.cf_fingerprint.is_none()));
        validate_manifest(&m1).unwrap();
        let mut bad = file("config/c.toml");
        bad.cf_fingerprint = Some(1);
        assert!(validate_manifest(&mf(vec![bad])).is_err());
    }

    #[test]
    fn sha1_validation_and_backward_compat() {
        // ancien manifeste JSON sans sha1 : valide
        let old = format!(
            "{{\"name\":\"n\",\"minecraft\":\"1.21.1\",\"loader\":{{\"loader\":\"neoforge\",\"version\":\"1\"}},\"java_major\":21,\"files\":[{{\"path\":\"mods/a.jar\",\"size\":1,\"sha256\":\"{ABC}\"}}]}}"
        );
        let m: Manifest = serde_json::from_str(&old).unwrap();
        assert!(m.files[0].sha1.is_none());
        validate_manifest(&m).unwrap();
        let with = |p: &str, s: &str| {
            let mut f = file(p);
            f.sha1 = Some(s.into());
            validate_manifest(&mf(vec![f]))
        };
        with("mods/a.jar", ABC_SHA1).unwrap();
        assert!(with("mods/a.jar", &ABC_SHA1.to_uppercase()).is_err());
        assert!(with("mods/a.jar", "abc").is_err());
        assert!(with("config/c.toml", ABC_SHA1).is_err());
    }

    #[test]
    fn rejects_hostile_manifests() {
        validate_manifest(&mf(vec![file("mods/a.jar"), file("config/x/y.toml")])).unwrap();
        for p in [
            "../x", "C:\\x", "C:/x", "mods/../../x", "/etc/passwd", "mods\\a.jar", "mods/a\0.jar", "mods//a.jar",
            "saves/w/level.dat", "options.txt", "a.jar", "mods/CON", "mods/nul.txt", "mods/COM1.jar", "mods/aux",
            "mods/a.jar.", "mods/./a.jar", "", "mods/a:b.jar", "config/x.exe", "kubejs/run.BAT", "defaultconfigs/a.ps1",
            "config/sub/lib.dll", "resourcepacks/s.sh", "mods/a.exe",
        ] {
            assert!(validate_manifest(&mf(vec![file(p)])).is_err(), "{p:?} aurait dû être refusé");
        }
        assert!(validate_manifest(&mf(vec![file("mods/a.jar"), file("mods/a.jar")])).is_err());
        assert!(validate_manifest(&mf(vec![file("mods/a.jar"), file("mods/A.JAR")])).is_err());
        let mut f = file("mods/a.jar");
        f.sha256 = ABC.to_uppercase();
        assert!(validate_manifest(&mf(vec![f])).is_err());
        let mut f = file("mods/a.jar");
        f.sha256 = "abc".into();
        assert!(validate_manifest(&mf(vec![f])).is_err());
        let mut f = file("mods/a.jar");
        f.size = u64::MAX;
        assert!(validate_manifest(&mf(vec![f])).is_err());
        let many: Vec<_> = (0..=MAX_FILES).map(|i| file(&format!("mods/{i}.jar"))).collect();
        assert!(validate_manifest(&mf(many)).is_err());
        let mut m = mf(vec![]);
        m.java_major = 0;
        assert!(validate_manifest(&m).is_err());
    }

    #[test]
    fn manifest_hash_fixed_vector() {
        let m = mf(vec![file("mods/a.jar")]);
        let expected_json = format!(
            "{{\"files\":[{{\"path\":\"mods/a.jar\",\"sha256\":\"{ABC}\",\"size\":1}}],\"java_major\":21,\"loader\":{{\"loader\":\"neoforge\",\"version\":\"21.1.77\"}},\"minecraft\":\"1.21.1\",\"name\":\"n\"}}"
        );
        assert_eq!(canonical_json(&m), expected_json);
        assert_eq!(manifest_hash(&m), "d495886a0d78b65ae0ecff6db3627e359a20b108a330b02c6800b445db51ccb6");
    }

    #[test]
    fn manifest_hash_fixed_vector_with_sha1() {
        let mut f = file("mods/a.jar");
        f.sha1 = Some(ABC_SHA1.into());
        let m = mf(vec![f]);
        let expected_json = format!(
            "{{\"files\":[{{\"path\":\"mods/a.jar\",\"sha1\":\"{ABC_SHA1}\",\"sha256\":\"{ABC}\",\"size\":1}}],\"java_major\":21,\"loader\":{{\"loader\":\"neoforge\",\"version\":\"21.1.77\"}},\"minecraft\":\"1.21.1\",\"name\":\"n\"}}"
        );
        assert_eq!(canonical_json(&m), expected_json);
        assert_eq!(manifest_hash(&m), "fcd7784d3c4fd02591e4d30fffa6241a3492e7eccd1bad7c52e8abd81d78e121");
    }
}
