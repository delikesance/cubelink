//! Correspondance de noms pour l'agent LAN (`core/resources/lan-agent`).
//!
//! L'agent appelle par réflexion quelques membres de Minecraft (`Minecraft.getInstance`, `IntegratedServer.publishServer`…).
//! Ces noms ne sont lisibles qu'avec NeoForge et Minecraft 26.x ; sinon il faut les traduire :
//! - jeu obfusqué (vanilla) : noms `official` (obfusqués), via `client_mappings` (ProGuard) du JSON de version ;
//! - Fabric / Quilt : noms `intermediary`, via `client_mappings` puis le tiny `mappings/mappings.tiny` du jar
//!   `net.fabricmc:intermediary` (déjà présent dans les bibliothèques du profil).
//!
//! Le fichier produit est minuscule (une dizaine de lignes) ; le téléchargement (~10 Mo) n'a lieu qu'une fois par version.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::mojang::versions::{sha1_hex, DownloadInfo};
use crate::types::{Error, Result};

/// Espace de noms du jeu à l'exécution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runtime {
    /// Noms obfusqués Mojang (vanilla).
    Official,
    /// Noms intermediary (Fabric, Quilt).
    Intermediary,
    /// Noms SRG (Forge 1.7.10 → 1.12.2) : table embarquée par version, aucun téléchargement.
    Srg,
}

impl Runtime {
    fn label(self) -> &'static str {
        match self {
            Runtime::Official => "official",
            Runtime::Intermediary => "intermediary",
            Runtime::Srg => "srg",
        }
    }
}

/// (clé, classe Mojang) des classes dont l'agent a besoin.
const CLASSES: &[(&str, &str)] = &[
    ("minecraft", "net.minecraft.client.Minecraft"),
    ("integrated", "net.minecraft.client.server.IntegratedServer"),
];

/// (clé, classe Mojang déclarante, membre Mojang, nombre de paramètres ; -1 = champ).
const MEMBERS: &[(&str, &str, &str, i32)] = &[
    ("mc.getInstance", "net.minecraft.client.Minecraft", "getInstance", 0),
    ("mc.getSingleplayerServer", "net.minecraft.client.Minecraft", "getSingleplayerServer", 0),
    ("mc.player", "net.minecraft.client.Minecraft", "player", -1),
    ("is.publishServer", "net.minecraft.client.server.IntegratedServer", "publishServer", 3),
    ("is.isPublished", "net.minecraft.client.server.IntegratedServer", "isPublished", 0),
    ("server.getDefaultGameType", "net.minecraft.server.MinecraftServer", "getDefaultGameType", 0),
    ("loop.execute", "net.minecraft.util.thread.BlockableEventLoop", "execute", 1),
];

struct PgMember {
    ret: String,
    name: String,
    params: Vec<String>,
    obf: String,
    method: bool,
}

struct PgClass {
    obf: String,
    members: Vec<PgMember>,
}

struct Proguard {
    /// Nom Mojang -> nom obfusqué, pour toutes les classes.
    obf_of: HashMap<String, String>,
    /// Membres, pour les seules classes utiles.
    classes: HashMap<String, PgClass>,
}

fn parse_member(line: &str) -> Option<PgMember> {
    let (left, obf) = line.trim().split_once(" -> ")?;
    let mut left = left;
    // Préfixe `12:15:` des numéros de ligne.
    for _ in 0..2 {
        match left.split_once(':') {
            Some((n, rest)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => left = rest,
            _ => break,
        }
    }
    let (ret, rest) = left.split_once(' ')?;
    if let Some(open) = rest.find('(') {
        let close = rest.find(')')?;
        let params = rest[open + 1..close].split(',').filter(|p| !p.is_empty()).map(str::to_string).collect();
        Some(PgMember { ret: ret.to_string(), name: rest[..open].to_string(), params, obf: obf.to_string(), method: true })
    } else {
        Some(PgMember { ret: ret.to_string(), name: rest.to_string(), params: vec![], obf: obf.to_string(), method: false })
    }
}

fn parse_proguard(text: &str) -> Proguard {
    let wanted: Vec<&str> = MEMBERS.iter().map(|m| m.1).collect();
    let mut p = Proguard { obf_of: HashMap::new(), classes: HashMap::new() };
    let mut current: Option<String> = None;
    for line in text.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if !line.starts_with(' ') {
            current = None;
            if let Some((moj, obf)) = line.trim_end_matches(':').split_once(" -> ") {
                p.obf_of.insert(moj.to_string(), obf.to_string());
                if wanted.contains(&moj) {
                    p.classes.insert(moj.to_string(), PgClass { obf: obf.to_string(), members: vec![] });
                    current = Some(moj.to_string());
                }
            }
        } else if let Some(c) = current.as_ref().and_then(|c| p.classes.get_mut(c)) {
            if let Some(m) = parse_member(line) {
                c.members.push(m);
            }
        }
    }
    p
}

/// Tiny (v1 ou v2) restreint à ce qu'il faut : classes et membres, espace `official` -> `intermediary`.
#[derive(Default)]
struct Tiny {
    classes: HashMap<String, String>,
    /// (classe officielle, nom officiel, descripteur officiel) -> nom intermediary (méthodes et champs).
    members: HashMap<(String, String, String), String>,
}

fn parse_tiny(text: &str) -> Result<Tiny> {
    let mut lines = text.lines();
    let head = lines.next().unwrap_or("");
    let v2 = head.starts_with("tiny\t2");
    if !v2 && !head.starts_with("v1\t") {
        return Err(Error::Invalid("mappings intermediary : format tiny inconnu".into()));
    }
    let mut t = Tiny::default();
    let mut owner = String::new();
    for line in lines {
        let f: Vec<&str> = line.split('\t').collect();
        if v2 {
            match f.as_slice() {
                ["c", off, inter, ..] => {
                    t.classes.insert(off.to_string(), inter.to_string());
                    owner = off.to_string();
                }
                ["", "m" | "f", desc, off, inter, ..] => {
                    t.members.insert((owner.clone(), off.to_string(), desc.to_string()), inter.to_string());
                }
                _ => {}
            }
        } else {
            match f.as_slice() {
                ["CLASS", off, inter] => {
                    t.classes.insert(off.to_string(), inter.to_string());
                }
                ["METHOD" | "FIELD", cls, desc, off, inter] => {
                    t.members.insert((cls.to_string(), off.to_string(), desc.to_string()), inter.to_string());
                }
                _ => {}
            }
        }
    }
    if t.classes.is_empty() {
        return Err(Error::Invalid("mappings intermediary vides".into()));
    }
    Ok(t)
}

fn primitive_desc(t: &str) -> Option<&'static str> {
    Some(match t {
        "void" => "V",
        "boolean" => "Z",
        "byte" => "B",
        "char" => "C",
        "short" => "S",
        "int" => "I",
        "long" => "J",
        "float" => "F",
        "double" => "D",
        _ => return None,
    })
}

/// Descripteur JVM d'un type ProGuard, en noms officiels (obfusqués).
fn type_desc(t: &str, p: &Proguard) -> String {
    if let Some(base) = t.strip_suffix("[]") {
        return format!("[{}", type_desc(base, p));
    }
    if let Some(d) = primitive_desc(t) {
        return d.to_string();
    }
    let name = p.obf_of.get(t).map(String::as_str).unwrap_or(t);
    format!("L{};", name.replace('.', "/"))
}

/// Nom d'exécution (forme `Class.getName()`) d'un type, `-` pour un tableau.
fn runtime_type(t: &str, p: &Proguard, tiny: Option<&Tiny>) -> String {
    if t.ends_with("[]") {
        return "-".into();
    }
    if primitive_desc(t).is_some() {
        return t.to_string();
    }
    let obf = p.obf_of.get(t).cloned().unwrap_or_else(|| t.to_string());
    match tiny.and_then(|x| x.classes.get(&obf.replace('.', "/"))) {
        Some(inter) => inter.replace('/', "."),
        None => obf,
    }
}

/// Construit le contenu du fichier de correspondance. `tiny` : requis pour `Runtime::Intermediary`.
pub fn build_lanmap(proguard: &str, tiny: Option<&str>, runtime: Runtime) -> Result<String> {
    if runtime == Runtime::Srg {
        return Err(Error::Invalid("les noms SRG viennent d'une table embarquée, pas de ProGuard".into()));
    }
    let p = parse_proguard(proguard);
    let t = match (runtime, tiny) {
        (Runtime::Intermediary, Some(text)) => Some(parse_tiny(text)?),
        (Runtime::Intermediary, None) => return Err(Error::Invalid("mappings intermediary absents".into())),
        _ => None,
    };
    let mut out = format!("# cubelink-lanmap {}\n", runtime.label());
    for (key, moj) in CLASSES {
        let c = p.classes.get(*moj).ok_or_else(|| Error::Invalid(format!("classe {moj} absente des mappings")))?;
        out.push_str(&format!("C {key} {}\n", runtime_type(moj, &p, t.as_ref())));
        let _ = c;
    }
    for (key, owner, name, nparams) in MEMBERS {
        // Intermediary nomme une méthode sur la classe qui la déclare : IntegratedServer hérite de MinecraftServer.
        let mut owners = vec![*owner];
        if owner.ends_with("IntegratedServer") {
            owners.push("net.minecraft.server.MinecraftServer");
        }
        let mut found: Option<(&PgMember, String)> = None;
        for o in owners {
            let Some(c) = p.classes.get(o) else { continue };
            let Some(m) = c.members.iter().find(|m| m.name == *name && if *nparams < 0 { !m.method } else { m.method && m.params.len() as i32 == *nparams }) else { continue };
            let hit = match &t {
                None => Some(m.obf.clone()),
                Some(tiny) => {
                    let desc = if m.method {
                        format!("({}){}", m.params.iter().map(|x| type_desc(x, &p)).collect::<String>(), type_desc(&m.ret, &p))
                    } else {
                        type_desc(&m.ret, &p)
                    };
                    tiny.members.get(&(c.obf.replace('.', "/"), m.obf.clone(), desc)).cloned()
                }
            };
            match hit {
                Some(n) => {
                    found = Some((m, n));
                    break;
                }
                None if found.is_none() => found = Some((m, m.obf.clone())),
                None => {}
            }
        }
        let (m, runtime_name) = found.ok_or_else(|| Error::Invalid(format!("membre {owner}.{name} absent des mappings")))?;
        if m.method {
            out.push_str(&format!("M {key} {runtime_name} {} {}\n", m.params.len(), runtime_type(&m.ret, &p, t.as_ref())));
        } else {
            out.push_str(&format!("F {key} {runtime_name}\n"));
        }
    }
    Ok(out)
}

/// Lit `mappings/mappings.tiny` dans le jar `net.fabricmc:intermediary`.
pub fn read_intermediary_jar(jar: &Path) -> Result<String> {
    use std::io::Read;
    let f = std::fs::File::open(jar)?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| Error::Invalid(format!("jar intermediary illisible : {e}")))?;
    let mut e = z.by_name("mappings/mappings.tiny").map_err(|e| Error::Invalid(format!("mappings.tiny absent : {e}")))?;
    let mut s = String::new();
    e.read_to_string(&mut s)?;
    Ok(s)
}

/// Fichier de correspondance en cache (créé au besoin) pour cette version de Minecraft et cet espace de noms.
/// `mappings` : `downloads.client_mappings` du JSON de version (vérifié par SHA-1). `intermediary_jar` : requis
/// pour `Runtime::Intermediary`.
pub async fn ensure_lanmap(
    cache_dir: &Path,
    http: &reqwest::Client,
    mc_version: &str,
    runtime: Runtime,
    mappings: &DownloadInfo,
    intermediary_jar: Option<&Path>,
) -> Result<PathBuf> {
    let safe: String = mc_version.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')).collect();
    let dest = cache_dir.join(format!("{safe}-{}-{}.map", runtime.label(), mappings.sha1.chars().take(8).collect::<String>()));
    if dest.is_file() {
        return Ok(dest);
    }
    let resp = http.get(&mappings.url).send().await.map_err(|e| Error::Network(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(Error::Network(format!("téléchargement des mappings : HTTP {}", resp.status().as_u16())));
    }
    let bytes = resp.bytes().await.map_err(|e| Error::Network(e.to_string()))?;
    let actual = sha1_hex(&bytes);
    if !actual.eq_ignore_ascii_case(&mappings.sha1) {
        return Err(Error::Integrity { expected: mappings.sha1.clone(), actual });
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let tiny = match (runtime, intermediary_jar) {
        (Runtime::Intermediary, Some(j)) => Some(read_intermediary_jar(j)?),
        _ => None,
    };
    let map = tokio::task::spawn_blocking(move || build_lanmap(&text, tiny.as_deref(), runtime))
        .await
        .map_err(|_| Error::Other("calcul des mappings interrompu".into()))??;
    std::fs::create_dir_all(cache_dir)?;
    let tmp = dest.with_extension("map.tmp");
    std::fs::write(&tmp, map)?;
    std::fs::rename(&tmp, &dest)?;
    Ok(dest)
}

// ---- Forge legacy (SRG) : tables embarquées ----

include!(concat!(env!("OUT_DIR"), "/srg_tables.rs"));

/// Minecraft < 1.13 (« legacy » : sans ProGuard Mojang, noms SRG sous Forge). Version non reconnue = moderne.
pub fn is_legacy(mc_version: &str) -> bool {
    matches!(crate::java::parse_mc_version(mc_version), Some((minor, _)) if minor <= 12)
}

/// Table SRG embarquée pour cette version de Minecraft (`core/resources/lan-map-srg/<mc>.map`).
pub fn srg_table(mc_version: &str) -> Option<&'static str> {
    SRG_TABLES.iter().find(|(v, _)| *v == mc_version.trim()).map(|(_, t)| *t)
}

/// Espace de noms à utiliser pour traduire les membres de l'agent. `has_mappings` : le JSON de version fournit
/// `client_mappings` (ProGuard Mojang). `None` : aucune table (noms Mojang directs, ou version non gérée).
pub fn pick_runtime(loader: crate::types::Loader, mc_version: &str, has_mappings: bool) -> Option<Runtime> {
    use crate::types::Loader;
    match loader {
        Loader::Forge if is_legacy(mc_version) => srg_table(mc_version).map(|_| Runtime::Srg),
        Loader::Forge | Loader::NeoForge => None,
        Loader::Vanilla => has_mappings.then_some(Runtime::Official),
        Loader::Fabric | Loader::Quilt => has_mappings.then_some(Runtime::Intermediary),
    }
}

/// Clés que l'agent exige d'une table SRG (membres : `true` = champ `F`, sinon méthode `M`).
const SRG_CLASS_KEYS: &[&str] = &["minecraft", "integrated"];
const SRG_MEMBER_KEYS: &[(&str, bool)] = &[
    ("mc.getInstance", false),
    ("mc.getSingleplayerServer", false),
    ("mc.player", true),
    ("is.shareToLan", false),
    ("is.isPublished", false),
    ("server.getDefaultGameType", false),
    ("mc.schedule", false),
];

/// Valide une table au format lu par l'agent : entête `# ...`, lignes `C clé classe`, `M clé nom nbParams retour`,
/// `F clé nom` ; toutes les clés requises présentes, `nbParams` numérique.
pub fn validate_srg_table(text: &str) -> std::result::Result<(), String> {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    if !lines.next().is_some_and(|l| l.starts_with('#')) {
        return Err("entête `# ...` manquant".into());
    }
    let mut classes = std::collections::HashSet::new();
    let mut methods = std::collections::HashSet::new();
    let mut fields = std::collections::HashSet::new();
    for l in lines {
        if l.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = l.split_whitespace().collect();
        match f.as_slice() {
            ["C", key, _class] => {
                classes.insert(*key);
            }
            ["M", key, _name, n, _ret] if n.parse::<u32>().is_ok() => {
                methods.insert(*key);
            }
            ["F", key, _name] => {
                fields.insert(*key);
            }
            _ => return Err(format!("ligne invalide : {l}")),
        }
    }
    for k in SRG_CLASS_KEYS {
        if !classes.contains(k) {
            return Err(format!("classe `{k}` absente"));
        }
    }
    for (k, is_field) in SRG_MEMBER_KEYS {
        let ok = if *is_field { fields.contains(k) } else { methods.contains(k) };
        if !ok {
            return Err(format!("membre `{k}` absent ({})", if *is_field { "F" } else { "M" }));
        }
    }
    Ok(())
}

/// Java minimal pour charger le jar de l'agent, d'après la version du classfile de `LanAgent.class`
/// (52 = Java 8, 61 = Java 17). Jar illisible : 17 (hypothèse prudente, ancien comportement).
pub fn jar_min_java(jar: &Path) -> u32 {
    use std::io::Read;
    let read = || -> Option<u32> {
        let mut z = zip::ZipArchive::new(std::fs::File::open(jar).ok()?).ok()?;
        let mut head = [0u8; 8];
        z.by_name("app/cubelink/agent/LanAgent.class").ok()?.read_exact(&mut head).ok()?;
        (head[..4] == [0xCA, 0xFE, 0xBA, 0xBE]).then(|| u32::from(u16::from_be_bytes([head[6], head[7]])).saturating_sub(44))
    };
    read().unwrap_or(17)
}

/// Copie la table SRG de cette version dans le cache (nom lié au contenu) et renvoie son chemin ; `None` sans table.
pub fn ensure_srg_map(cache_dir: &Path, mc_version: &str) -> Result<Option<PathBuf>> {
    let Some(text) = srg_table(mc_version) else { return Ok(None) };
    let safe: String = mc_version.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')).collect();
    let dest = cache_dir.join(format!("{safe}-srg-{}.map", sha1_hex(text.as_bytes()).chars().take(8).collect::<String>()));
    if dest.is_file() {
        return Ok(Some(dest));
    }
    std::fs::create_dir_all(cache_dir)?;
    let tmp = dest.with_extension("map.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &dest)?;
    Ok(Some(dest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const PG: &str = "\
# compiler: test
net.minecraft.client.Minecraft -> fgi:
    net.minecraft.client.player.LocalPlayer player -> t
    1:1:net.minecraft.client.Minecraft getInstance() -> V
    10:12:net.minecraft.client.server.IntegratedServer getSingleplayerServer():55:57 -> W
    5:5:int unrelated() -> V
net.minecraft.client.server.IntegratedServer -> ghi:
    1:3:boolean publishServer(net.minecraft.world.level.GameType,boolean,int) -> a
    4:4:boolean isPublished() -> r
net.minecraft.server.MinecraftServer -> jkl:
    1:1:net.minecraft.world.level.GameType getDefaultGameType() -> s
net.minecraft.util.thread.BlockableEventLoop -> mno:
    1:1:void execute(java.lang.Runnable) -> g
net.minecraft.client.player.LocalPlayer -> pqr:
net.minecraft.world.level.GameType -> stu:
";

    // Intermédiaire v2 : Minecraft.getInstance renommé, `execute` non listé (méthode d'interface).
    const TINY2: &str = "tiny\t2\t0\tofficial\tintermediary\n\
c\tfgi\tnet/minecraft/class_310\n\
\tm\t()Lfgi;\tV\tmethod_1551\n\
\tm\t()Lghi;\tW\tmethod_1576\n\
\tf\tLpqr;\tt\tfield_1724\n\
c\tghi\tnet/minecraft/class_1132\n\
\tm\t(Lstu;ZI)Z\ta\tmethod_1587\n\
\tm\t()Z\tr\tmethod_1594\n\
c\tjkl\tnet/minecraft/class_1132x\n\
\tm\t()Lstu;\ts\tmethod_3790\n\
c\tmno\tnet/minecraft/class_1255\n\
c\tpqr\tnet/minecraft/class_746\n\
c\tstu\tnet/minecraft/class_1934\n";

    const TINY1: &str = "v1\tofficial\tintermediary\n\
CLASS\tfgi\tnet/minecraft/class_310\n\
METHOD\tfgi\t()Lfgi;\tV\tmethod_1551\n\
METHOD\tfgi\t()Lghi;\tW\tmethod_1576\n\
FIELD\tfgi\tLpqr;\tt\tfield_1724\n\
CLASS\tghi\tnet/minecraft/class_1132\n\
METHOD\tghi\t(Lstu;ZI)Z\ta\tmethod_1587\n\
METHOD\tghi\t()Z\tr\tmethod_1594\n\
CLASS\tjkl\tnet/minecraft/class_1132x\n\
METHOD\tjkl\t()Lstu;\ts\tmethod_3790\n\
CLASS\tmno\tnet/minecraft/class_1255\n\
CLASS\tpqr\tnet/minecraft/class_746\n\
CLASS\tstu\tnet/minecraft/class_1934\n";

    const SRG_OK: &str = "# cubelink-lanmap srg
C minecraft net.minecraft.client.Minecraft
C integrated net.minecraft.server.integrated.IntegratedServer
M mc.getInstance func_71410_x 0 net.minecraft.client.Minecraft
M mc.getSingleplayerServer func_71401_C 0 net.minecraft.server.integrated.IntegratedServer
F mc.player field_71439_g
M is.shareToLan func_71206_a 2 java.lang.String
M is.isPublished func_71344_c 0 boolean
M server.getDefaultGameType func_71265_f 0 net.minecraft.world.GameType
M mc.schedule func_152344_a 1 com.google.common.util.concurrent.ListenableFuture
";

    #[test]
    fn validation_de_table_srg() {
        assert_eq!(validate_srg_table(SRG_OK), Ok(()));
        assert!(validate_srg_table(&SRG_OK.replace("# cubelink-lanmap srg\n", "")).is_err());
        assert!(validate_srg_table(&SRG_OK.replace("func_71206_a 2", "func_71206_a deux")).is_err());
        assert!(validate_srg_table(&SRG_OK.replace("F mc.player field_71439_g\n", "")).is_err());
        assert!(validate_srg_table(&SRG_OK.replace("M mc.schedule", "M mc.autre")).is_err());
        assert!(validate_srg_table(&SRG_OK.replace("C integrated", "C autre")).is_err());
        assert!(validate_srg_table(&format!("{SRG_OK}X inconnu\n")).is_err());
        // un champ n'est pas une méthode
        assert!(validate_srg_table(&SRG_OK.replace("F mc.player field_71439_g", "M mc.player field_71439_g 0 x")).is_err());
    }

    /// Toutes les tables embarquées (fournies par T3) doivent respecter le format.
    #[test]
    fn tables_embarquees_valides() {
        for (v, t) in SRG_TABLES {
            validate_srg_table(t).unwrap_or_else(|e| panic!("table {v} : {e}"));
            assert!(is_legacy(v), "{v} n'est pas legacy");
        }
    }

    #[test]
    fn legacy_et_choix_du_runtime() {
        use crate::types::Loader;
        for v in ["1.7.10", "1.8.9", "1.12.2", "1.12"] {
            assert!(is_legacy(v), "{v}");
        }
        for v in ["1.13", "1.14.4", "1.21.1", "26.1", "24w14a", ""] {
            assert!(!is_legacy(v), "{v}");
        }
        assert_eq!(pick_runtime(Loader::Forge, "1.20.1", true), None);
        assert_eq!(pick_runtime(Loader::NeoForge, "1.21.1", true), None);
        assert_eq!(pick_runtime(Loader::Vanilla, "1.21.1", true), Some(Runtime::Official));
        assert_eq!(pick_runtime(Loader::Vanilla, "26.1", false), None);
        assert_eq!(pick_runtime(Loader::Fabric, "1.21.1", true), Some(Runtime::Intermediary));
        assert_eq!(pick_runtime(Loader::Quilt, "1.21.1", true), Some(Runtime::Intermediary));
        // legacy : Srg seulement si une table existe ; jamais Official/Intermediary
        assert_eq!(pick_runtime(Loader::Forge, "1.12.2", false), srg_table("1.12.2").map(|_| Runtime::Srg));
        assert_eq!(pick_runtime(Loader::Forge, "1.5.2", false), None);
        assert_eq!(pick_runtime(Loader::Fabric, "1.12.2", false), None);
        assert_eq!(pick_runtime(Loader::Vanilla, "1.12.2", false), None);
    }

    #[test]
    fn copie_de_la_table_srg_dans_le_cache() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(ensure_srg_map(dir.path(), "0.0.0").unwrap(), None);
        if let Some((v, t)) = SRG_TABLES.first() {
            let p = ensure_srg_map(dir.path(), v).unwrap().unwrap();
            assert_eq!(std::fs::read_to_string(&p).unwrap(), *t);
            assert_eq!(ensure_srg_map(dir.path(), v).unwrap(), Some(p));
        }
    }

    #[test]
    fn version_java_du_jar() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let mk = |name: &str, major: u16| {
            let p = dir.path().join(name);
            let mut z = zip::ZipWriter::new(std::fs::File::create(&p).unwrap());
            z.start_file("app/cubelink/agent/LanAgent.class", zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(&[0xCA, 0xFE, 0xBA, 0xBE, 0, 0, (major >> 8) as u8, major as u8]).unwrap();
            z.finish().unwrap();
            p
        };
        assert_eq!(jar_min_java(&mk("j8.jar", 52)), 8);
        assert_eq!(jar_min_java(&mk("j17.jar", 61)), 17);
        assert_eq!(jar_min_java(&dir.path().join("absent.jar")), 17);
    }

    #[test]
    fn srg_refuse_proguard() {
        assert!(build_lanmap(PG, None, Runtime::Srg).is_err());
    }

    #[test]
    fn noms_obfusques() {
        let m = build_lanmap(PG, None, Runtime::Official).unwrap();
        assert!(m.starts_with("# cubelink-lanmap official\n"));
        for l in [
            "C minecraft fgi",
            "C integrated ghi",
            "M mc.getInstance V 0 fgi",
            "M mc.getSingleplayerServer W 0 ghi",
            "F mc.player t",
            "M is.publishServer a 3 boolean",
            "M is.isPublished r 0 boolean",
            "M server.getDefaultGameType s 0 stu",
            "M loop.execute g 1 void",
        ] {
            assert!(m.lines().any(|x| x == l), "{l}\n{m}");
        }
    }

    #[test]
    fn noms_intermediary_v2_et_v1() {
        for tiny in [TINY2, TINY1] {
            let m = build_lanmap(PG, Some(tiny), Runtime::Intermediary).unwrap();
            for l in [
                "C minecraft net.minecraft.class_310",
                "C integrated net.minecraft.class_1132",
                "M mc.getInstance method_1551 0 net.minecraft.class_310",
                "M mc.getSingleplayerServer method_1576 0 net.minecraft.class_1132",
                "F mc.player field_1724",
                "M is.publishServer method_1587 3 boolean",
                "M is.isPublished method_1594 0 boolean",
                "M server.getDefaultGameType method_3790 0 net.minecraft.class_1934",
                // non listé dans le tiny : le nom officiel est conservé
                "M loop.execute g 1 void",
            ] {
                assert!(m.lines().any(|x| x == l), "{l}\n{m}");
            }
        }
    }

    /// Intermediary déclare `publishServer`/`isPublished` sur `MinecraftServer` (IntegratedServer hérite).
    #[test]
    fn methodes_declarees_sur_minecraft_server() {
        let pg = "net.minecraft.client.Minecraft -> fgi:
    net.minecraft.client.player.LocalPlayer player -> t
    1:1:net.minecraft.client.Minecraft getInstance() -> V
    1:1:net.minecraft.client.server.IntegratedServer getSingleplayerServer() -> W
net.minecraft.client.server.IntegratedServer -> ghi:
net.minecraft.server.MinecraftServer -> jkl:
    1:3:boolean publishServer(net.minecraft.world.level.GameType,boolean,int) -> a
    4:4:boolean isPublished() -> r
    1:1:net.minecraft.world.level.GameType getDefaultGameType() -> s
net.minecraft.util.thread.BlockableEventLoop -> mno:
    1:1:void execute(java.lang.Runnable) -> g
net.minecraft.client.player.LocalPlayer -> pqr:
net.minecraft.world.level.GameType -> stu:
";
        let tiny = "tiny	2	0	official	intermediary
c	fgi	net/minecraft/class_310
	m	()Lfgi;	V	method_1551
	m	()Lghi;	W	method_1576
	f	Lpqr;	t	field_1724
c	ghi	net/minecraft/class_1132
c	jkl	net/minecraft/class_1131
	m	(Lstu;ZI)Z	a	method_3763
	m	()Z	r	method_3860
	m	()Lstu;	s	method_3790
c	pqr	net/minecraft/class_746
c	stu	net/minecraft/class_1934
";
        let m = build_lanmap(pg, Some(tiny), Runtime::Intermediary).unwrap();
        assert!(m.lines().any(|l| l == "M is.publishServer method_3763 3 boolean"), "{m}");
        assert!(m.lines().any(|l| l == "M is.isPublished method_3860 0 boolean"), "{m}");
    }

    #[test]
    fn erreurs_claires() {
        assert!(build_lanmap(PG, None, Runtime::Intermediary).is_err());
        assert!(build_lanmap("net.minecraft.client.Minecraft -> a:\n", None, Runtime::Official).is_err());
        assert!(build_lanmap(PG, Some("n'importe quoi"), Runtime::Intermediary).is_err());
    }

    #[tokio::test]
    async fn telechargement_verifie_puis_cache() {
        let srv = MockServer::start().await;
        Mock::given(method("GET")).and(path("/client.txt")).respond_with(ResponseTemplate::new(200).set_body_string(PG)).expect(1).mount(&srv).await;
        let dir = tempfile::tempdir().unwrap();
        let info = DownloadInfo { url: format!("{}/client.txt", srv.uri()), sha1: sha1_hex(PG.as_bytes()), size: PG.len() as u64 };
        let http = reqwest::Client::new();
        let p = ensure_lanmap(dir.path(), &http, "1.21.1", Runtime::Official, &info, None).await.unwrap();
        assert!(std::fs::read_to_string(&p).unwrap().contains("C minecraft fgi"));
        // Second appel : cache, aucune requête (expect(1)).
        let p2 = ensure_lanmap(dir.path(), &http, "1.21.1", Runtime::Official, &info, None).await.unwrap();
        assert_eq!(p, p2);
    }

    #[tokio::test]
    async fn sha1_invalide_et_http_404() {
        let srv = MockServer::start().await;
        Mock::given(method("GET")).and(path("/client.txt")).respond_with(ResponseTemplate::new(200).set_body_string(PG)).mount(&srv).await;
        let dir = tempfile::tempdir().unwrap();
        let http = reqwest::Client::new();
        let bad = DownloadInfo { url: format!("{}/client.txt", srv.uri()), sha1: "0".repeat(40), size: 1 };
        let e = ensure_lanmap(dir.path(), &http, "1.21.1", Runtime::Official, &bad, None).await.unwrap_err();
        assert!(matches!(e, Error::Integrity { .. }), "{e:?}");
        assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none(), "rien en cache après un échec");
        let missing = DownloadInfo { url: format!("{}/absent", srv.uri()), sha1: "0".repeat(40), size: 1 };
        assert!(matches!(ensure_lanmap(dir.path(), &http, "1.21.1", Runtime::Official, &missing, None).await, Err(Error::Network(_))));
    }

    /// Outil de vérification manuelle (`cargo test -p cubelink-core lan_map::tests::reel -- --ignored --nocapture`) :
    /// CUBELINK_LANMAP_VERSION_JSON, CUBELINK_LANMAP_DIR, CUBELINK_LANMAP_RUNTIME (official|intermediary),
    /// CUBELINK_LANMAP_INTERMEDIARY (jar, pour intermediary). Affiche le chemin du fichier produit.
    #[tokio::test]
    #[ignore]
    async fn reel() {
        let v: crate::mojang::versions::VersionJson =
            serde_json::from_slice(&std::fs::read(std::env::var("CUBELINK_LANMAP_VERSION_JSON").unwrap()).unwrap()).unwrap();
        let dl = v.downloads.client_mappings.expect("client_mappings absent");
        let rt = if std::env::var("CUBELINK_LANMAP_RUNTIME").unwrap() == "intermediary" { Runtime::Intermediary } else { Runtime::Official };
        let jar = std::env::var("CUBELINK_LANMAP_INTERMEDIARY").ok().map(PathBuf::from);
        let dir = PathBuf::from(std::env::var("CUBELINK_LANMAP_DIR").unwrap());
        let p = ensure_lanmap(&dir, &reqwest::Client::new(), &v.id, rt, &dl, jar.as_deref()).await.unwrap();
        println!("LANMAP={}", p.display());
        println!("{}", std::fs::read_to_string(&p).unwrap());
    }
}
