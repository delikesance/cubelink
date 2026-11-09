//! Tâche M2 : résolution des libraries d'un JSON de version Mojang selon l'OS et l'architecture.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::types::{Error, Result};

const MOJANG_LIBS_URL: &str = "https://libraries.minecraft.net/";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    Linux,
    Osx,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X86_64,
    Arm64,
    X86,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(target_os = "windows") {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::Osx
        } else {
            Os::Linux
        }
    }

    /// Nom utilisé par Mojang (`os.name`, clés de `natives`).
    pub fn mojang_name(self) -> &'static str {
        match self {
            Os::Windows => "windows",
            Os::Linux => "linux",
            Os::Osx => "osx",
        }
    }
}

impl Arch {
    pub fn current() -> Arch {
        if cfg!(target_arch = "aarch64") {
            Arch::Arm64
        } else if cfg!(target_arch = "x86") {
            Arch::X86
        } else {
            Arch::X86_64
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Library {
    pub name: String,
    /// Chemin relatif sous le dossier `libraries` (séparateur `/`).
    pub path: String,
    pub url: String,
    pub sha1: Option<String>,
    pub size: Option<u64>,
    pub is_native: bool,
    pub extract_excludes: Vec<String>,
}

fn invalid(msg: impl Into<String>) -> Error {
    Error::Invalid(format!("library : {}", msg.into()))
}

/// `"org.lwjgl:lwjgl:3.3.3:natives-windows"` → `org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-windows.jar`.
pub fn maven_path(name: &str) -> Result<String> {
    let (coords, ext) = match name.split_once('@') {
        Some((c, e)) => (c, e),
        None => (name, "jar"),
    };
    let parts: Vec<&str> = coords.split(':').collect();
    if !(3..=4).contains(&parts.len()) || parts.iter().any(|p| p.is_empty()) || ext.is_empty() {
        return Err(invalid(format!("nom Maven invalide « {name} »")));
    }
    let (group, artifact, version) = (parts[0], parts[1], parts[2]);
    let file = match parts.get(3) {
        Some(c) => format!("{artifact}-{version}-{c}.{ext}"),
        None => format!("{artifact}-{version}.{ext}"),
    };
    Ok(format!("{}/{artifact}/{version}/{file}", group.replace('.', "/")))
}

fn rule_applies(rule: &Value, os: Os, arch: Arch) -> bool {
    // Les `features` dépendent du lanceur : règle non satisfaite.
    if rule.get("features").is_some() {
        return false;
    }
    let Some(o) = rule.get("os") else { return true };
    if let Some(n) = o.get("name").and_then(Value::as_str) {
        if n != os.mojang_name() {
            return false;
        }
    }
    if let Some(a) = o.get("arch").and_then(Value::as_str) {
        let ok = match a {
            "x86" | "i386" | "i686" => arch == Arch::X86,
            "x86_64" | "amd64" => arch == Arch::X86_64,
            "arm64" | "aarch64" => arch == Arch::Arm64,
            _ => false,
        };
        if !ok {
            return false;
        }
    }
    // `os.version` (regex) ignoré.
    true
}

fn rules_allow(rules: Option<&Value>, os: Os, arch: Arch) -> Result<bool> {
    let Some(rules) = rules else { return Ok(true) };
    let rules = rules.as_array().ok_or_else(|| invalid("`rules` n'est pas un tableau"))?;
    if rules.is_empty() {
        return Ok(true);
    }
    let mut allowed = false;
    for rule in rules {
        let action = rule
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("règle sans `action`"))?;
        let value = match action {
            "allow" => true,
            "deny" => false,
            other => return Err(invalid(format!("action de règle inconnue « {other} »"))),
        };
        if rule_applies(rule, os, arch) {
            allowed = value;
        }
    }
    Ok(allowed)
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn from_download(name: &str, dl: &Value, is_native: bool, excludes: &[String]) -> Result<Library> {
    let path = match str_field(dl, "path") {
        Some(p) => p,
        None => maven_path(name)?,
    };
    let url = match str_field(dl, "url") {
        Some(u) if !u.is_empty() => u,
        _ => format!("{MOJANG_LIBS_URL}{path}"),
    };
    Ok(Library {
        name: name.to_owned(),
        path,
        url,
        sha1: str_field(dl, "sha1"),
        size: dl.get("size").and_then(Value::as_u64),
        is_native,
        extract_excludes: excludes.to_vec(),
    })
}

fn resolve_one(entry: &Value, os: Os, arch: Arch) -> Result<Option<Library>> {
    if !entry.is_object() {
        return Err(invalid("entrée non objet"));
    }
    let name = entry
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("champ `name` manquant"))?;
    if !rules_allow(entry.get("rules"), os, arch)? {
        return Ok(None);
    }

    if let Some(natives) = entry.get("natives") {
        let natives = natives.as_object().ok_or_else(|| invalid("`natives` invalide"))?;
        let Some(classifier) = natives.get(os.mojang_name()).and_then(Value::as_str) else {
            return Ok(None);
        };
        let bits = if arch == Arch::X86 { "32" } else { "64" };
        let classifier = classifier.replace("${arch}", bits);
        let excludes: Vec<String> = entry
            .pointer("/extract/exclude")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect())
            .unwrap_or_default();
        let dl = entry.get("downloads").and_then(|d| d.get("classifiers")).and_then(|c| c.get(&classifier));
        let lib = match dl {
            Some(dl) => from_download(name, dl, true, &excludes)?,
            None => {
                let path = maven_path(&format!("{name}:{classifier}"))?;
                let url = format!("{MOJANG_LIBS_URL}{path}");
                Library {
                    name: name.to_owned(),
                    path,
                    url,
                    sha1: None,
                    size: None,
                    is_native: true,
                    extract_excludes: excludes,
                }
            }
        };
        return Ok(Some(lib));
    }

    let lib = match entry.pointer("/downloads/artifact") {
        Some(a) => from_download(name, a, false, &[])?,
        None => {
            // Format ancien : `name` seul, éventuellement avec `url` de dépôt Maven.
            let path = maven_path(name)?;
            let base = match str_field(entry, "url") {
                Some(u) if !u.is_empty() => {
                    if u.ends_with('/') {
                        u
                    } else {
                        format!("{u}/")
                    }
                }
                _ => MOJANG_LIBS_URL.to_owned(),
            };
            Library {
                name: name.to_owned(),
                url: format!("{base}{path}"),
                path,
                sha1: None,
                size: None,
                is_native: false,
                extract_excludes: vec![],
            }
        }
    };
    Ok(Some(lib))
}

pub fn resolve(libraries: &[Value], os: Os, arch: Arch) -> Result<Vec<Library>> {
    let mut out = Vec::new();
    for entry in libraries {
        if let Some(lib) = resolve_one(entry, os, arch)? {
            out.push(lib);
        }
    }
    Ok(out)
}

/// Chemins du classpath (natives exclues).
pub fn classpath(libs: &[Library], libs_dir: &Path) -> Vec<PathBuf> {
    libs.iter().filter(|l| !l.is_native).map(|l| libs_dir.join(&l.path)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn names(libs: &[Library]) -> Vec<&str> {
        libs.iter().map(|l| l.name.as_str()).collect()
    }

    fn osx_rules_lib() -> Value {
        json!({
            "name": "ca.weblite:java-objc-bridge:1.1",
            "downloads": {"artifact": {
                "path": "ca/weblite/java-objc-bridge/1.1/java-objc-bridge-1.1.jar",
                "sha1": "1227f9e0666314f9de41477e3ec277e542ed7f7b", "size": 91947,
                "url": "https://libraries.minecraft.net/ca/weblite/java-objc-bridge/1.1/java-objc-bridge-1.1.jar"}},
            "rules": [{"action": "allow", "os": {"name": "osx"}}]
        })
    }

    #[test]
    fn rules_osx_only_and_deny() {
        let allow_osx = osx_rules_lib();
        let deny_osx = json!({
            "name": "org.example:a:1",
            "rules": [{"action": "allow"}, {"action": "deny", "os": {"name": "osx"}}]
        });
        let allow_linux = json!({
            "name": "org.example:b:1",
            "rules": [{"action": "allow", "os": {"name": "linux"}}]
        });
        let libs = [allow_osx, deny_osx, allow_linux];
        let w = resolve(&libs, Os::Windows, Arch::X86_64).unwrap();
        assert_eq!(names(&w), ["org.example:a:1"]);
        let l = resolve(&libs, Os::Linux, Arch::X86_64).unwrap();
        assert_eq!(names(&l), ["org.example:a:1", "org.example:b:1"]);
        let m = resolve(&libs, Os::Osx, Arch::Arm64).unwrap();
        assert_eq!(names(&m), ["ca.weblite:java-objc-bridge:1.1"]);
    }

    #[test]
    fn rules_arch_features_and_version() {
        let arm = json!({"name": "a:b:1", "rules": [{"action": "allow", "os": {"name": "osx", "arch": "arm64"}}]});
        let x86 = json!({"name": "a:c:1", "rules": [{"action": "allow", "os": {"arch": "x86"}}]});
        let feat = json!({"name": "a:d:1", "rules": [{"action": "allow", "features": {"is_demo_user": true}}]});
        let ver = json!({"name": "a:e:1", "rules": [{"action": "allow", "os": {"name": "windows", "version": "^10\\."}}]});
        let libs = [arm, x86, feat, ver];
        assert_eq!(names(&resolve(&libs, Os::Osx, Arch::Arm64).unwrap()), ["a:b:1"]);
        assert_eq!(names(&resolve(&libs, Os::Windows, Arch::X86).unwrap()), ["a:c:1", "a:e:1"]);
        assert_eq!(names(&resolve(&libs, Os::Windows, Arch::X86_64).unwrap()), ["a:e:1"]);
    }

    #[test]
    fn natives_legacy_classifiers() {
        let lib = json!({
            "name": "org.lwjgl.lwjgl:lwjgl-platform:2.9.4-nightly-20150209",
            "natives": {"linux": "natives-linux", "osx": "natives-osx", "windows": "natives-windows"},
            "extract": {"exclude": ["META-INF/"]},
            "downloads": {"classifiers": {
                "natives-windows": {
                    "path": "org/lwjgl/lwjgl/lwjgl-platform/2.9.4-nightly-20150209/lwjgl-platform-2.9.4-nightly-20150209-natives-windows.jar",
                    "sha1": "b84d5102b9dbfabfeb5e43c7e2828d98a7fc80e0", "size": 578680,
                    "url": "https://libraries.minecraft.net/org/lwjgl/lwjgl/lwjgl-platform/2.9.4-nightly-20150209/lwjgl-platform-2.9.4-nightly-20150209-natives-windows.jar"}
            }}
        });
        let r = resolve(std::slice::from_ref(&lib), Os::Windows, Arch::X86_64).unwrap();
        assert_eq!(r.len(), 1);
        assert!(r[0].is_native);
        assert!(r[0].path.ends_with("natives-windows.jar"));
        assert_eq!(r[0].extract_excludes, ["META-INF/"]);
        assert_eq!(r[0].size, Some(578680));
    }

    #[test]
    fn natives_arch_placeholder_and_missing_os() {
        let lib = json!({
            "name": "tv.twitch:twitch-platform:5.16",
            "natives": {"windows": "natives-windows-${arch}"}
        });
        let w64 = resolve(std::slice::from_ref(&lib), Os::Windows, Arch::X86_64).unwrap();
        assert!(w64[0].path.ends_with("twitch-platform-5.16-natives-windows-64.jar"));
        let w32 = resolve(std::slice::from_ref(&lib), Os::Windows, Arch::X86).unwrap();
        assert!(w32[0].path.ends_with("natives-windows-32.jar"));
        assert!(w32[0].url.starts_with(MOJANG_LIBS_URL));
        assert!(resolve(&[lib], Os::Linux, Arch::X86_64).unwrap().is_empty());
    }

    #[test]
    fn modern_lwjgl_natives_are_normal_libraries() {
        let lib = json!({
            "name": "org.lwjgl:lwjgl:3.3.3:natives-windows",
            "downloads": {"artifact": {
                "path": "org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-windows.jar",
                "sha1": "9ec4ce0c2ef1e9e6ab1b2ac1d4b4ad3e1dbd62ab", "size": 141122,
                "url": "https://libraries.minecraft.net/org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-windows.jar"}},
            "rules": [{"action": "allow", "os": {"name": "windows"}}]
        });
        let r = resolve(std::slice::from_ref(&lib), Os::Windows, Arch::X86_64).unwrap();
        assert!(!r[0].is_native);
        assert_eq!(r[0].sha1.as_deref(), Some("9ec4ce0c2ef1e9e6ab1b2ac1d4b4ad3e1dbd62ab"));
        assert!(resolve(&[lib], Os::Linux, Arch::X86_64).unwrap().is_empty());
    }

    #[test]
    fn legacy_name_only() {
        let lib = json!({"name": "net.fabricmc:fabric-loader:0.16.0", "url": "https://maven.fabricmc.net/"});
        let plain = json!({"name": "com.google.guava:guava:31.1-jre"});
        let r = resolve(&[lib, plain], Os::Linux, Arch::X86_64).unwrap();
        assert_eq!(r[0].path, "net/fabricmc/fabric-loader/0.16.0/fabric-loader-0.16.0.jar");
        assert_eq!(r[0].url, "https://maven.fabricmc.net/net/fabricmc/fabric-loader/0.16.0/fabric-loader-0.16.0.jar");
        assert_eq!(
            r[1].url,
            "https://libraries.minecraft.net/com/google/guava/guava/31.1-jre/guava-31.1-jre.jar"
        );
        assert_eq!(r[1].sha1, None);
    }

    #[test]
    fn maven_path_classifier() {
        assert_eq!(
            maven_path("org.lwjgl:lwjgl:3.3.3:natives-windows").unwrap(),
            "org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-windows.jar"
        );
        assert_eq!(maven_path("a.b:c:1@zip").unwrap(), "a/b/c/1/c-1.zip");
        assert!(maven_path("a:b").is_err());
    }

    #[test]
    fn classpath_excludes_natives() {
        let natives = json!({"name": "x:nat:1", "natives": {"windows": "natives-windows"}});
        let normal = json!({"name": "x:lib:1"});
        let r = resolve(&[natives, normal], Os::Windows, Arch::X86_64).unwrap();
        assert_eq!(r.len(), 2);
        let cp = classpath(&r, Path::new("libs"));
        assert_eq!(cp, vec![Path::new("libs").join("x/lib/1/lib-1.jar")]);
    }

    #[test]
    fn malformed_is_invalid() {
        for bad in [
            json!("texte"),
            json!({"rules": []}),
            json!({"name": "pas-maven"}),
            json!({"name": "a:b:1", "rules": "oops"}),
            json!({"name": "a:b:1", "rules": [{"os": {"name": "osx"}}]}),
            json!({"name": "a:b:1", "natives": 3}),
        ] {
            assert!(matches!(resolve(&[bad], Os::Linux, Arch::X86_64), Err(Error::Invalid(_))));
        }
    }
}
