//! Tâche I2 : construction de la ligne de commande Java d'une instance.
//!
//! Gère le format moderne (`arguments.jvm` / `arguments.game` avec règles) et l'ancien
//! (`minecraftArguments`). Les arguments sont renvoyés en tableau : aucun shell n'est impliqué.
//! Le jeton d'accès n'apparaît jamais dans un `Debug` (arguments sensibles masqués).

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

use serde_json::Value;

use crate::loader::fabric::MergedVersion;
use crate::mojang::libraries::{classpath, resolve, Arch, Os};
use crate::mojang::versions::VersionJson;
use crate::types::{Error, Profile, Result};

/// Jeton factice transmis au jeu pour un profil hors ligne.
pub const OFFLINE_TOKEN: &str = "0";
const MASK: &str = "<masqué>";

/// JSON de version à lancer : vanilla ou profil loader déjà fusionné.
#[derive(Debug, Clone)]
pub enum LaunchVersion {
    Vanilla(VersionJson),
    Merged(MergedVersion),
}

pub struct LaunchSpec {
    pub java_path: PathBuf,
    pub game_dir: PathBuf,
    pub assets_dir: PathBuf,
    pub libs_dir: PathBuf,
    pub natives_dir: PathBuf,
    pub client_jar: PathBuf,
    pub version: LaunchVersion,
    pub profile: Profile,
    /// Jeton d'accès Minecraft (profil en ligne). Ignoré pour un profil hors ligne.
    pub access_token: Option<String>,
    /// `release`, `snapshot`… (variable `${version_type}`).
    pub version_type: String,
    pub ram_mb: u32,
    pub os: Os,
    pub arch: Arch,
    pub launcher_name: String,
    pub launcher_version: String,
    pub extra_jvm_args: Vec<String>,
}

impl fmt::Debug for LaunchSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LaunchSpec")
            .field("java_path", &self.java_path)
            .field("game_dir", &self.game_dir)
            .field("profile", &self.profile)
            .field("access_token", &self.access_token.as_ref().map(|_| MASK))
            .field("ram_mb", &self.ram_mb)
            .field("os", &self.os)
            .field("arch", &self.arch)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct LaunchCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// Indices des arguments contenant une valeur sensible.
    sensitive: Vec<usize>,
}

impl LaunchCommand {
    /// Arguments avec les valeurs sensibles masquées (sûr pour un journal).
    pub fn redacted_args(&self) -> Vec<String> {
        self.args
            .iter()
            .enumerate()
            .map(|(i, a)| if self.sensitive.contains(&i) { MASK.to_string() } else { a.clone() })
            .collect()
    }
}

impl fmt::Debug for LaunchCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LaunchCommand")
            .field("program", &self.program)
            .field("args", &self.redacted_args())
            .field("cwd", &self.cwd)
            .finish()
    }
}

/// Vue normalisée des deux formats.
struct Norm<'a> {
    id: &'a str,
    main_class: &'a str,
    asset_id: &'a str,
    libraries: &'a [Value],
    jvm: Vec<Value>,
    game: Vec<Value>,
    legacy: Option<&'a str>,
}

fn arr(v: &Option<Value>, key: &str) -> Vec<Value> {
    v.as_ref().and_then(|a| a.get(key)).and_then(Value::as_array).cloned().unwrap_or_default()
}

fn normalize(v: &LaunchVersion) -> Result<Norm<'_>> {
    match v {
        LaunchVersion::Vanilla(j) => {
            if j.arguments.is_none() && j.minecraft_arguments.is_none() {
                return Err(Error::Invalid("JSON de version sans `arguments` ni `minecraftArguments`".into()));
            }
            let modern = j.arguments.is_some();
            Ok(Norm {
                id: &j.id,
                main_class: &j.main_class,
                asset_id: &j.asset_index.id,
                libraries: &j.libraries,
                jvm: arr(&j.arguments, "jvm"),
                game: arr(&j.arguments, "game"),
                legacy: if modern { None } else { j.minecraft_arguments.as_deref() },
            })
        }
        LaunchVersion::Merged(m) => {
            let legacy = m.game_args.is_empty() && m.jvm_args.is_empty();
            if legacy && m.minecraft_arguments.is_none() {
                return Err(Error::Invalid("profil fusionné sans arguments".into()));
            }
            Ok(Norm {
                id: &m.id,
                main_class: &m.main_class,
                asset_id: &m.asset_index.id,
                libraries: &m.libraries,
                jvm: m.jvm_args.clone(),
                game: m.game_args.clone(),
                legacy: if legacy { m.minecraft_arguments.as_deref() } else { None },
            })
        }
    }
}

fn invalid(msg: impl Into<String>) -> Error {
    Error::Invalid(format!("arguments : {}", msg.into()))
}

fn rule_applies(rule: &Value, os: Os, arch: Arch) -> bool {
    // Aucune fonctionnalité optionnelle (démo, résolution…) n'est active.
    if let Some(f) = rule.get("features").and_then(Value::as_object) {
        if f.values().any(|v| v.as_bool() != Some(false)) {
            return false;
        }
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
    true
}

fn rules_allow(rules: &Value, os: Os, arch: Arch) -> Result<bool> {
    let rules = rules.as_array().ok_or_else(|| invalid("`rules` n'est pas un tableau"))?;
    if rules.is_empty() {
        return Ok(true);
    }
    let mut allowed = false;
    for r in rules {
        let value = match r.get("action").and_then(Value::as_str) {
            Some("allow") => true,
            Some("deny") => false,
            _ => return Err(invalid("règle sans action valide")),
        };
        if rule_applies(r, os, arch) {
            allowed = value;
        }
    }
    Ok(allowed)
}

/// Déplie les éléments (chaîne ou `{rules, value}`) en gabarits de chaînes non substituées.
fn expand(items: &[Value], os: Os, arch: Arch) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for item in items {
        match item {
            Value::String(s) => out.push(s.clone()),
            Value::Object(o) => {
                if let Some(r) = o.get("rules") {
                    if !rules_allow(r, os, arch)? {
                        continue;
                    }
                }
                match o.get("value") {
                    Some(Value::String(s)) => out.push(s.clone()),
                    Some(Value::Array(a)) => {
                        for v in a {
                            out.push(v.as_str().ok_or_else(|| invalid("valeur non textuelle"))?.to_string());
                        }
                    }
                    _ => return Err(invalid("élément conditionnel sans `value`")),
                }
            }
            _ => return Err(invalid("élément d'argument inattendu")),
        }
    }
    Ok(out)
}

fn is_secret(name: &str) -> bool {
    matches!(name, "auth_access_token" | "auth_session")
}

/// Remplace chaque `${nom}` ; nom inconnu -> `Error::Invalid` contenant le nom.
fn substitute(tpl: &str, vars: &HashMap<&'static str, String>, secret: &mut bool) -> Result<String> {
    let mut out = String::with_capacity(tpl.len());
    let mut rest = tpl;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after.find('}').ok_or_else(|| invalid(format!("variable non terminée dans « {tpl} »")))?;
        let name = &after[..end];
        let val = vars.get(name).ok_or_else(|| Error::Invalid(format!("variable inconnue ${{{name}}}")))?;
        if is_secret(name) {
            *secret = true;
        }
        out.push_str(val);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn s(p: &std::path::Path) -> String {
    p.to_string_lossy().into_owned()
}

pub fn build_command(spec: &LaunchSpec) -> Result<LaunchCommand> {
    if spec.ram_mb == 0 {
        return Err(Error::Invalid("mémoire vive nulle".into()));
    }
    let norm = normalize(&spec.version)?;

    let (name, uuid, user_type) = match &spec.profile {
        Profile::Online { name, uuid } => (name, uuid, "msa"),
        Profile::Offline { name, uuid } => (name, uuid, "legacy"),
    };
    let token = match &spec.profile {
        Profile::Offline { .. } => OFFLINE_TOKEN.to_string(),
        Profile::Online { .. } => spec
            .access_token
            .clone()
            .filter(|t| !t.is_empty())
            .ok_or_else(|| Error::Auth("jeton d'accès manquant pour un profil en ligne".into()))?,
    };

    let libs = resolve(norm.libraries, spec.os, spec.arch)?;
    let mut cp: Vec<String> = classpath(&libs, &spec.libs_dir).iter().map(|p| s(p)).collect();
    cp.push(s(&spec.client_jar));
    let sep = if spec.os == Os::Windows { ";" } else { ":" };

    let mut vars: HashMap<&'static str, String> = HashMap::new();
    for (k, v) in [
        ("auth_player_name", name.clone()),
        ("auth_uuid", uuid.clone()),
        ("auth_access_token", token.clone()),
        ("auth_session", token),
        ("auth_xuid", "0".into()),
        ("clientid", "0".into()),
        ("user_type", user_type.into()),
        ("user_properties", "{}".into()),
        ("version_name", norm.id.to_string()),
        ("version_type", spec.version_type.clone()),
        ("game_directory", s(&spec.game_dir)),
        ("assets_root", s(&spec.assets_dir)),
        ("game_assets", s(&spec.assets_dir)),
        ("assets_index_name", norm.asset_id.to_string()),
        ("natives_directory", s(&spec.natives_dir)),
        ("library_directory", s(&spec.libs_dir)),
        ("launcher_name", spec.launcher_name.clone()),
        ("launcher_version", spec.launcher_version.clone()),
        ("classpath", cp.join(sep)),
        ("classpath_separator", sep.into()),
    ] {
        vars.insert(k, v);
    }

    // Gabarits JVM.
    let mut jvm_tpl = if norm.legacy.is_some() { Vec::new() } else { expand(&norm.jvm, spec.os, spec.arch)? };
    if !jvm_tpl.iter().any(|a| a.starts_with("-Djava.library.path=")) {
        jvm_tpl.insert(0, "-Djava.library.path=${natives_directory}".into());
    }
    if norm.legacy.is_some() {
        jvm_tpl.push("-Dminecraft.launcher.brand=${launcher_name}".into());
        jvm_tpl.push("-Dminecraft.launcher.version=${launcher_version}".into());
    }
    if !jvm_tpl.iter().any(|a| a == "-cp" || a == "-classpath") {
        jvm_tpl.push("-cp".into());
        jvm_tpl.push("${classpath}".into());
    }

    let game_tpl: Vec<String> = match norm.legacy {
        Some(l) => l.split_whitespace().map(str::to_string).collect(),
        None => expand(&norm.game, spec.os, spec.arch)?,
    };

    let mut args: Vec<String> = vec![format!("-Xms{}M", spec.ram_mb.min(512)), format!("-Xmx{}M", spec.ram_mb)];
    args.extend(spec.extra_jvm_args.iter().cloned());
    let mut sensitive = Vec::new();
    let mut push = |args: &mut Vec<String>, tpl: &str| -> Result<()> {
        let mut secret = false;
        let v = substitute(tpl, &vars, &mut secret)?;
        if secret {
            sensitive.push(args.len());
        }
        args.push(v);
        Ok(())
    };
    for t in &jvm_tpl {
        push(&mut args, t)?;
    }
    args.push(norm.main_class.to_string());
    for t in &game_tpl {
        push(&mut args, t)?;
    }

    Ok(LaunchCommand { program: spec.java_path.clone(), args, cwd: spec.game_dir.clone(), sensitive })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::fabric::merge_with_vanilla;
    use serde_json::json;
    use std::path::Path;

    fn vanilla(extra: Value) -> VersionJson {
        let mut base = json!({
            "id": "X", "mainClass": "net.minecraft.client.main.Main",
            "assetIndex": {"id":"17","url":"u","sha1":"a","size":1},
            "downloads": {"client": {"url":"u","sha1":"b","size":2}}
        });
        base.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        serde_json::from_value(base).unwrap()
    }

    fn spec(version: LaunchVersion, profile: Profile, os: Os) -> LaunchSpec {
        LaunchSpec {
            java_path: "java".into(),
            game_dir: "gd".into(),
            assets_dir: "assets".into(),
            libs_dir: "libs".into(),
            natives_dir: "nat".into(),
            client_jar: "client.jar".into(),
            version,
            profile,
            access_token: Some("SECRET-TOKEN".into()),
            version_type: "release".into(),
            ram_mb: 4096,
            os,
            arch: Arch::X86_64,
            launcher_name: "Cubelink".into(),
            launcher_version: "0.1".into(),
            extra_jvm_args: vec!["-XX:+UseG1GC".into()],
        }
    }

    fn online() -> Profile {
        Profile::Online { name: "Steve".into(), uuid: "u-1".into() }
    }
    fn offline() -> Profile {
        Profile::Offline { name: "Alex".into(), uuid: "u-2".into() }
    }

    fn lib(rel: &str) -> String {
        Path::new("libs").join(rel).to_string_lossy().into_owned()
    }

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn v121() -> VersionJson {
        vanilla(json!({
            "id": "1.21",
            "libraries": [
                {"name": "a:one:1"},
                {"name": "org.lwjgl:lwjgl:3.3.3:natives-windows", "rules": [{"action":"allow","os":{"name":"windows"}}]},
                {"name": "b:two:2", "rules": [{"action":"allow"},{"action":"deny","os":{"name":"osx"}}]}
            ],
            "arguments": {
                "game": ["--username", "${auth_player_name}", "--version", "${version_name}",
                    "--gameDir", "${game_directory}", "--assetsDir", "${assets_root}",
                    "--assetIndex", "${assets_index_name}", "--uuid", "${auth_uuid}",
                    "--accessToken", "${auth_access_token}", "--xuid", "${auth_xuid}",
                    "--userType", "${user_type}", "--versionType", "${version_type}",
                    {"rules":[{"action":"allow","features":{"is_demo_user":true}}],"value":"--demo"},
                    {"rules":[{"action":"allow","features":{"has_custom_resolution":true}}],"value":["--width","${resolution_width}"]}],
                "jvm": [
                    {"rules":[{"action":"allow","os":{"name":"windows"}}],"value":"-XX:HeapDumpPath=dump.heapdump"},
                    {"rules":[{"action":"allow","os":{"name":"osx"}}],"value":["-XstartOnFirstThread"]},
                    "-Djava.library.path=${natives_directory}",
                    "-Dminecraft.launcher.brand=${launcher_name}",
                    "-Dminecraft.launcher.version=${launcher_version}",
                    "-cp", "${classpath}"]
            }
        }))
    }

    #[test]
    fn modern_121_windows_reference() {
        let cmd = build_command(&spec(LaunchVersion::Vanilla(v121()), online(), Os::Windows)).unwrap();
        let cp = format!("{};{};{};client.jar", lib("a/one/1/one-1.jar"), lib("org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-windows.jar"), lib("b/two/2/two-2.jar"));
        let expected = strs(&[
            "-Xms512M", "-Xmx4096M", "-XX:+UseG1GC", "-XX:HeapDumpPath=dump.heapdump",
            "-Djava.library.path=nat", "-Dminecraft.launcher.brand=Cubelink", "-Dminecraft.launcher.version=0.1",
            "-cp", &cp, "net.minecraft.client.main.Main",
            "--username", "Steve", "--version", "1.21", "--gameDir", "gd", "--assetsDir", "assets",
            "--assetIndex", "17", "--uuid", "u-1", "--accessToken", "SECRET-TOKEN", "--xuid", "0",
            "--userType", "msa", "--versionType", "release",
        ]);
        assert_eq!(cmd.args, expected);
        assert_eq!(cmd.cwd, PathBuf::from("gd"));
        assert_eq!(cmd.program, PathBuf::from("java"));
    }

    #[test]
    fn modern_osx_rules_and_separator() {
        let cmd = build_command(&spec(LaunchVersion::Vanilla(v121()), online(), Os::Osx)).unwrap();
        assert!(cmd.args.contains(&"-XstartOnFirstThread".to_string()));
        assert!(!cmd.args.iter().any(|a| a.contains("HeapDump")));
        // b:two:2 est refusée sur osx.
        let cp = cmd.args[cmd.args.iter().position(|a| a == "-cp").unwrap() + 1].clone();
        assert_eq!(cp, format!("{}:client.jar", lib("a/one/1/one-1.jar")));
        let lin = build_command(&spec(LaunchVersion::Vanilla(v121()), online(), Os::Linux)).unwrap();
        let cp = lin.args[lin.args.iter().position(|a| a == "-cp").unwrap() + 1].clone();
        assert!(cp.contains(':') && !cp.contains(';'));
    }

    #[test]
    fn legacy_1122_reference() {
        let v = vanilla(json!({
            "id": "1.12.2",
            "assetIndex": {"id":"1.12","url":"u","sha1":"a","size":1},
            "libraries": [{"name":"a:one:1"}],
            "minecraftArguments": "--username ${auth_player_name} --version ${version_name} --gameDir ${game_directory} --assetsDir ${assets_root} --assetIndex ${assets_index_name} --uuid ${auth_uuid} --accessToken ${auth_access_token} --userType ${user_type} --versionType ${version_type}"
        }));
        let mut sp = spec(LaunchVersion::Vanilla(v), online(), Os::Linux);
        sp.game_dir = "/g d".into();
        let cmd = build_command(&sp).unwrap();
        let cp = format!("{}:client.jar", lib("a/one/1/one-1.jar"));
        let expected = strs(&[
            "-Xms512M", "-Xmx4096M", "-XX:+UseG1GC", "-Djava.library.path=nat",
            "-Dminecraft.launcher.brand=Cubelink", "-Dminecraft.launcher.version=0.1",
            "-cp", &cp, "net.minecraft.client.main.Main",
            "--username", "Steve", "--version", "1.12.2", "--gameDir", "/g d", "--assetsDir", "assets",
            "--assetIndex", "1.12", "--uuid", "u-1", "--accessToken", "SECRET-TOKEN",
            "--userType", "msa", "--versionType", "release",
        ]);
        assert_eq!(cmd.args, expected);
    }

    #[test]
    fn merged_fabric_profile() {
        let profile = json!({
            "id": "fabric-loader-0.16.5-1.21", "inheritsFrom": "1.21",
            "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
            "arguments": {"game": [], "jvm": ["-DFabricMcEmu= net.minecraft.client.main.Main "]},
            "libraries": [{"name":"net.fabricmc:fabric-loader:0.16.5"}]
        });
        let m = merge_with_vanilla(&v121(), &profile);
        let cmd = build_command(&spec(LaunchVersion::Merged(m), offline(), Os::Linux)).unwrap();
        let cp = format!(
            "{}:{}:{}:client.jar",
            lib("net/fabricmc/fabric-loader/0.16.5/fabric-loader-0.16.5.jar"),
            lib("a/one/1/one-1.jar"),
            lib("b/two/2/two-2.jar")
        );
        // L'argument jvm du loader est placé après `-cp ${classpath}` (vanilla d'abord, loader ensuite).
        let expected = strs(&[
            "-Xms512M", "-Xmx4096M", "-XX:+UseG1GC",
            "-Djava.library.path=nat", "-Dminecraft.launcher.brand=Cubelink", "-Dminecraft.launcher.version=0.1",
            "-cp", &cp, "-DFabricMcEmu= net.minecraft.client.main.Main ",
            "net.fabricmc.loader.impl.launch.knot.KnotClient",
            "--username", "Alex", "--version", "fabric-loader-0.16.5-1.21", "--gameDir", "gd", "--assetsDir", "assets",
            "--assetIndex", "17", "--uuid", "u-2", "--accessToken", "0", "--xuid", "0",
            "--userType", "legacy", "--versionType", "release",
        ]);
        assert_eq!(cmd.args, expected);
    }

    #[test]
    fn offline_uses_dummy_token_even_if_one_is_given() {
        let cmd = build_command(&spec(LaunchVersion::Vanilla(v121()), offline(), Os::Linux)).unwrap();
        assert!(!cmd.args.iter().any(|a| a.contains("SECRET")));
        let i = cmd.args.iter().position(|a| a == "--accessToken").unwrap();
        assert_eq!(cmd.args[i + 1], "0");
    }

    #[test]
    fn online_without_token_is_auth_error() {
        let mut sp = spec(LaunchVersion::Vanilla(v121()), online(), Os::Linux);
        sp.access_token = None;
        assert!(matches!(build_command(&sp), Err(Error::Auth(_))));
    }

    #[test]
    fn unknown_variable_is_invalid_with_name() {
        let v = vanilla(json!({"arguments": {"game": ["${nope_var}"], "jvm": []}}));
        let err = build_command(&spec(LaunchVersion::Vanilla(v), offline(), Os::Linux)).unwrap_err();
        match err {
            Error::Invalid(m) => assert!(m.contains("nope_var"), "{m}"),
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn token_is_never_in_debug() {
        let sp = spec(LaunchVersion::Vanilla(v121()), online(), Os::Linux);
        let cmd = build_command(&sp).unwrap();
        assert!(cmd.args.iter().any(|a| a == "SECRET-TOKEN"));
        assert!(!format!("{cmd:?}").contains("SECRET-TOKEN"));
        assert!(!format!("{cmd:#?}").contains("SECRET-TOKEN"));
        assert!(!cmd.redacted_args().iter().any(|a| a.contains("SECRET")));
        assert!(!format!("{sp:?}").contains("SECRET-TOKEN"));
    }

    #[test]
    fn missing_arguments_and_zero_ram_are_invalid() {
        let v = vanilla(json!({}));
        assert!(matches!(build_command(&spec(LaunchVersion::Vanilla(v), offline(), Os::Linux)), Err(Error::Invalid(_))));
        let mut sp = spec(LaunchVersion::Vanilla(v121()), offline(), Os::Linux);
        sp.ram_mb = 0;
        assert!(matches!(build_command(&sp), Err(Error::Invalid(_))));
    }

    #[test]
    fn small_ram_caps_xms() {
        let mut sp = spec(LaunchVersion::Vanilla(v121()), offline(), Os::Linux);
        sp.ram_mb = 256;
        let c = build_command(&sp).unwrap();
        assert_eq!(&c.args[..2], ["-Xms256M", "-Xmx256M"]);
    }
}
