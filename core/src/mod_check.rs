//! Vérification hors ligne des mods d'une instance : dépendances manquantes ou trop anciennes, incompatibilités,
//! mauvaise version de Minecraft, mauvais loader, doublons. Seuls les jars activés comptent (`.jar.disabled` ignoré).
//!
//! Règle d'or : dans le doute, ne rien signaler (versions inconnues, plages illisibles, loaders proches).

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::mod_meta::{jar_meta, DepKind, JarMeta, LoaderKind, ModMeta};
use crate::mods;
use crate::types::Result;
use crate::version_range::{self as vr, Flavor};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, specta::Type)]
pub struct ModIssue {
    /// Nom du fichier jar concerné, tel que listé par `mods::list`.
    pub file: String,
    pub mod_name: String,
    /// "missing_dep" | "incompatible" | "wrong_minecraft" | "wrong_loader" | "duplicate"
    pub kind: String,
    /// "error" | "warning"
    pub severity: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, specta::Type)]
pub struct ModCheck {
    pub issues: Vec<ModIssue>,
    /// Nombre de jars activés analysés.
    pub checked: u32,
}

/// Ids fournis par le jeu ou le loader : jamais signalés absents, versions non vérifiées.
pub(crate) const BUILTIN_IDS: &[&str] = &["minecraft", "java", "fabricloader", "fabric-loader", "quilt_loader", "quilt_base", "forge", "neoforge", "fml", "javafml", "lowcodefml"];

pub(crate) struct Prov {
    pub(crate) file: String,
    pub(crate) name: String,
    pub(crate) version: Option<String>,
}

pub(crate) type Index = HashMap<String, Vec<Prov>>;

/// Détail interne d'un `missing_dep` (clé : fichier + message du problème), utilisé par `mod_fix`.
#[derive(Debug, Clone)]
pub(crate) struct DepDetail {
    pub(crate) id: String,
    pub(crate) range: String,
    pub(crate) flavor: Flavor,
    /// Fichier du fournisseur trop ancien ; `None` si la dépendance est absente.
    pub(crate) provider: Option<String>,
}

pub(crate) type Details = HashMap<(String, String), DepDetail>;

/// Résultat de `check` avec les jars lus et les détails des dépendances en défaut.
pub(crate) struct Detailed {
    pub(crate) check: ModCheck,
    pub(crate) jars: Vec<(String, Arc<JarMeta>)>,
    pub(crate) details: Details,
}

/// Analyse les mods activés de `game_dir/mods` pour une instance `minecraft` + `loader` (fabric, quilt, forge, neoforge, vanilla).
pub fn check(game_dir: &Path, minecraft: &str, loader: &str) -> Result<ModCheck> {
    Ok(check_detailed(game_dir, minecraft, loader)?.check)
}

pub(crate) fn check_detailed(game_dir: &Path, minecraft: &str, loader: &str) -> Result<Detailed> {
    let entries = mods::list(game_dir)?;
    let dir = game_dir.join("mods");
    let mut jars = Vec::new();
    let mut checked = 0u32;
    for e in entries.iter().filter(|e| e.enabled) {
        checked += 1;
        if let Some(m) = jar_meta(&dir.join(&e.file)) {
            jars.push((e.file.clone(), m));
        }
    }
    let (issues, details) = analyze(&jars, minecraft, loader);
    Ok(Detailed { check: ModCheck { issues, checked }, jars, details })
}

fn push(out: &mut Vec<ModIssue>, file: &str, name: &str, kind: &str, severity: &str, message: String, related: Option<String>) {
    let issue = ModIssue { file: file.to_string(), mod_name: name.to_string(), kind: kind.into(), severity: severity.into(), message, related };
    if !out.contains(&issue) {
        out.push(issue);
    }
}

/// Mods du jar qui servent à la vérification pour ce loader (un jar multi-loader suit le descripteur du loader).
pub(crate) fn selected<'a>(meta: &'a JarMeta, loader: &str) -> Vec<&'a ModMeta> {
    let order: &[LoaderKind] = match loader {
        "fabric" => &[LoaderKind::Fabric],
        "quilt" => &[LoaderKind::Quilt, LoaderKind::Fabric],
        "neoforge" => &[LoaderKind::NeoForge, LoaderKind::Forge],
        "forge" => &[LoaderKind::Forge],
        _ => &[],
    };
    for k in order {
        let v: Vec<&ModMeta> = meta.mods.iter().filter(|m| m.kind == *k).collect();
        if !v.is_empty() {
            return v;
        }
    }
    Vec::new()
}

fn display_name(meta: &JarMeta, sel: &[&ModMeta], file: &str) -> String {
    let m = sel.first().copied().or_else(|| meta.mods.first());
    m.map(|m| m.name.clone().unwrap_or_else(|| m.id.clone())).unwrap_or_else(|| file.to_string())
}

fn wrong_loader(meta: &JarMeta, loader: &str, has_connector: bool) -> Option<(&'static str, &'static str)> {
    let modern_forge = meta.has_forge || meta.has_neoforge;
    let fabricish = meta.has_fabric || meta.has_quilt;
    let (found, target) = match loader {
        "fabric" if modern_forge && !fabricish => (if meta.has_forge && meta.has_neoforge { "Forge/NeoForge" } else if meta.has_neoforge { "NeoForge" } else { "Forge" }, "Fabric"),
        "fabric" if meta.has_quilt && !meta.has_fabric && !modern_forge && !meta.has_legacy => ("Quilt", "Fabric"),
        "quilt" if modern_forge && !fabricish => (if meta.has_forge && meta.has_neoforge { "Forge/NeoForge" } else if meta.has_neoforge { "NeoForge" } else { "Forge" }, "Quilt"),
        "forge" | "neoforge" if fabricish && !modern_forge && !meta.has_legacy && !has_connector => {
            (if meta.has_fabric && meta.has_quilt { "Fabric/Quilt" } else if meta.has_quilt { "Quilt" } else { "Fabric" }, if loader == "forge" { "Forge" } else { "NeoForge" })
        }
        _ => return None,
    };
    Some((found, target))
}

pub(crate) fn build_index(jars: &[(String, Arc<JarMeta>)]) -> Index {
    let mut idx: Index = HashMap::new();
    for (file, meta) in jars {
        for m in &meta.mods {
            let name = m.name.clone().unwrap_or_else(|| m.id.clone());
            idx.entry(m.id.clone()).or_default().push(Prov { file: file.clone(), name: name.clone(), version: m.version.clone() });
            for (p, pv) in &m.provides {
                idx.entry(p.clone()).or_default().push(Prov { file: file.clone(), name: name.clone(), version: pv.clone().or_else(|| m.version.clone()) });
            }
        }
        for (id, v) in &meta.nested {
            idx.entry(id.clone()).or_default().push(Prov { file: file.clone(), name: id.clone(), version: v.clone() });
        }
    }
    idx
}

fn range_text(range: &str, flavor: Flavor) -> String {
    if vr::is_any(range) {
        String::new()
    } else {
        vr::describe(range, flavor)
    }
}

pub(crate) fn analyze(jars: &[(String, Arc<JarMeta>)], minecraft: &str, loader: &str) -> (Vec<ModIssue>, Details) {
    let loader = loader.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut details = Details::new();
    if !matches!(loader.as_str(), "fabric" | "quilt" | "forge" | "neoforge") {
        return (out, details);
    }
    let mut jars: Vec<(String, Arc<JarMeta>)> = jars.to_vec();
    jars.sort_by_key(|(f, _)| f.to_ascii_lowercase());
    let idx = build_index(&jars);
    let has_connector = idx.contains_key("connector");
    // MixinExtras est intégré à Fabric / Quilt (Fabric Loader) et à NeoForge depuis 1.20.4.
    let mixinextras_builtin = loader != "forge" && (loader != "neoforge" || vr::compare(minecraft, "1.20.4").is_some_and(|o| o != std::cmp::Ordering::Less));

    for (file, meta) in &jars {
        let sel = selected(meta, &loader);
        let name = display_name(meta, &sel, file);
        if let Some((found, target)) = wrong_loader(meta, &loader, has_connector) {
            push(&mut out, file, &name, "wrong_loader", "error", format!("Mod pour {found}, ce pack utilise {target}."), None);
            continue;
        }
        for m in &sel {
            let flavor = m.flavor();
            for dep in &m.deps {
                if dep.side == "SERVER" {
                    continue;
                }
                let id = dep.id.as_str();
                if id == "minecraft" {
                    let bad = match dep.kind {
                        // Forge / NeoForge ne bloquent pas le lancement quand la borne est dans la même famille (`[1.21,1.21.1)` en 1.21.1).
                        DepKind::Required => vr::matches(&dep.range, minecraft, flavor) == Some(false) && (flavor == Flavor::Fabric || !vr::same_family(&dep.range, minecraft)),
                        DepKind::Breaks | DepKind::Incompatible => !vr::is_any(&dep.range) && vr::matches(&dep.range, minecraft, flavor) == Some(true),
                        DepKind::Conflicts => false,
                    };
                    if bad {
                        let msg = if dep.kind == DepKind::Required {
                            format!("Fait pour Minecraft {}, ce pack est en {minecraft}.", range_text(&dep.range, flavor))
                        } else {
                            format!("Incompatible avec Minecraft {minecraft}.")
                        };
                        push(&mut out, file, &name, "wrong_minecraft", "error", msg, None);
                    }
                    continue;
                }
                if BUILTIN_IDS.contains(&id) || (id == "mixinextras" && mixinextras_builtin) {
                    continue;
                }
                match dep.kind {
                    DepKind::Required => check_required(&mut out, &mut details, &idx, file, &name, dep, flavor, &loader),
                    kind => {
                        let (k, sev, text) = match kind {
                            DepKind::Conflicts => ("incompatible", "warning", "Déconseillé avec"),
                            _ => ("incompatible", "error", "Incompatible avec"),
                        };
                        for p in idx.get(id).into_iter().flatten().filter(|p| &p.file != file) {
                            let hit = if vr::is_any(&dep.range) { Some(true) } else { p.version.as_deref().and_then(|v| vr::matches(&dep.range, v, flavor)) };
                            if hit == Some(true) {
                                push(&mut out, file, &name, k, sev, format!("{text} {}.", p.name), Some(p.file.clone()));
                            }
                        }
                    }
                }
            }
        }
    }

    // Doublons : même id à la racine de deux jars activés. Le premier par ordre de nom est la référence.
    let mut by_id: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for (file, meta) in &jars {
        let sel = selected(meta, &loader);
        let name = display_name(meta, &sel, file);
        let ids: BTreeSet<&str> = sel.iter().map(|m| m.id.as_str()).filter(|i| !BUILTIN_IDS.contains(i)).collect();
        for id in ids {
            by_id.entry(id.to_string()).or_default().push((file.clone(), name.clone()));
        }
    }
    for files in by_id.values().filter(|f| f.len() > 1) {
        for (f, n) in &files[1..] {
            push(&mut out, f, n, "duplicate", "error", format!("Déjà présent : {}.", files[0].0), Some(files[0].0.clone()));
        }
    }

    out.sort_by(|a, b| {
        let rank = |i: &ModIssue| u8::from(i.severity != "error");
        (rank(a), a.mod_name.to_lowercase(), &a.file, &a.message).cmp(&(rank(b), b.mod_name.to_lowercase(), &b.file, &b.message))
    });
    (out, details)
}

#[allow(clippy::too_many_arguments)]
fn check_required(out: &mut Vec<ModIssue>, details: &mut Details, idx: &Index, file: &str, name: &str, dep: &crate::mod_meta::Dep, flavor: Flavor, loader: &str) {
    let id = dep.id.as_str();
    let provs = idx.get(id);
    let Some(provs) = provs.filter(|p| !p.is_empty()) else {
        // Quilt fait tourner les mods Fabric via Quilted Fabric API : ses modules `fabric-*` n'ont pas toujours un id identique.
        if loader == "quilt" && id.starts_with("fabric") && idx.keys().any(|k| k.starts_with("quilted_fabric_api")) {
            return;
        }
        let rt = range_text(&dep.range, flavor);
        let msg = if rt.is_empty() { format!("Requiert {id} (absent).") } else { format!("Requiert {id} {rt} (absent).") };
        details.insert((file.to_string(), msg.clone()), DepDetail { id: id.to_string(), range: dep.range.clone(), flavor, provider: None });
        push(out, file, name, "missing_dep", "error", msg, None);
        return;
    };
    if vr::is_any(&dep.range) {
        return;
    }
    let results: Vec<Option<bool>> = provs.iter().map(|p| p.version.as_deref().and_then(|v| vr::matches(&dep.range, v, flavor))).collect();
    if results.iter().any(|r| *r != Some(false)) {
        return; // au moins un fournisseur convient ou est indécidable
    }
    let p = &provs[0];
    let v = p.version.as_deref().unwrap_or("?");
    let related = (p.file != file).then(|| p.file.clone());
    let msg = format!("Requiert {} {}, version installée : {v}.", p.name, range_text(&dep.range, flavor));
    details.insert((file.to_string(), msg.clone()), DepDetail { id: id.to_string(), range: dep.range.clone(), flavor, provider: Some(p.file.clone()) });
    push(out, file, name, "missing_dep", "error", msg, related);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mod_meta::tests::{jar, zip_bytes};

    fn run(files: &[(&str, Files)], mc: &str, loader: &str) -> ModCheck {
        let d = tempfile::tempdir().unwrap();
        let mods = d.path().join("minecraft").join("mods");
        std::fs::create_dir_all(&mods).unwrap();
        for (name, content) in files {
            jar(&mods, name, content);
        }
        check(&d.path().join("minecraft"), mc, loader).unwrap()
    }

    type Files<'a> = Vec<(&'a str, &'a [u8])>;

    fn fab(id: &str, extra: &str) -> Vec<u8> {
        format!(r#"{{"id":"{id}","version":"1.0","name":"{}"{extra}}}"#, id.to_uppercase()).into_bytes()
    }

    #[test]
    fn dependance_absente_et_trop_ancienne() {
        let a = fab("a", r#","depends":{"flywheel":">=1.0","gone":"*"}"#);
        let fly = br#"{"id":"flywheel","version":"0.9","name":"Flywheel"}"#.to_vec();
        let r = run(&[("a.jar", vec![("fabric.mod.json", &a)]), ("fly.jar", vec![("fabric.mod.json", &fly)])], "1.21.1", "fabric");
        assert_eq!(r.checked, 2);
        assert_eq!(r.issues.len(), 2);
        let gone = r.issues.iter().find(|i| i.message.contains("gone")).unwrap();
        assert_eq!((gone.kind.as_str(), gone.severity.as_str(), gone.message.as_str()), ("missing_dep", "error", "Requiert gone (absent)."));
        let old = r.issues.iter().find(|i| i.message.contains("Flywheel")).unwrap();
        assert_eq!(old.message, "Requiert Flywheel 1.0 ou plus récent, version installée : 0.9.");
        assert_eq!(old.related.as_deref(), Some("fly.jar"));
        assert_eq!(old.mod_name, "A");
    }

    #[test]
    fn absente_avec_plage() {
        let a = fab("a", r#","depends":{"flywheel":">=1.0"}"#);
        let r = run(&[("a.jar", vec![("fabric.mod.json", &a)])], "1.21.1", "fabric");
        assert_eq!(r.issues[0].message, "Requiert flywheel 1.0 ou plus récent (absent).");
    }

    #[test]
    fn satisfaite_par_provides_et_jar_imbrique_fabric() {
        let a = fab("a", r#","depends":{"old-name":"*","fabric-api-base":">=0.4","fabricloader":">=99","java":">=99"}"#);
        let b = fab("b", r#","provides":["old-name"]"#);
        let nested = fab("fabric-api-base", "");
        let nested_jar = zip_bytes(&[("fabric.mod.json", &nested)]);
        let api = fab("fabric-api", r#","jars":[{"file":"META-INF/jars/base.jar"}]"#);
        let r = run(
            &[("a.jar", vec![("fabric.mod.json", &a)]), ("b.jar", vec![("fabric.mod.json", &b)]), ("api.jar", vec![("fabric.mod.json", &api), ("META-INF/jars/base.jar", &nested_jar)])],
            "1.21.1",
            "fabric",
        );
        // fabric-api-base nested en 1.0 : >=0.4 satisfait.
        assert_eq!(r.issues, vec![]);
    }

    #[test]
    fn satisfaite_par_jarjar_neoforge() {
        let inner = zip_bytes(&[("META-INF/neoforge.mods.toml", b"[[mods]]\nmodId=\"lib\"\nversion=\"2.0\"\n")]);
        let meta = br#"{"jars":[{"identifier":{"group":"g","artifact":"lib"},"version":{"artifactVersion":"2.0"},"path":"META-INF/jarjar/lib.jar"}]}"#;
        let toml = b"[[mods]]\nmodId=\"a\"\nversion=\"1\"\n[[dependencies.a]]\nmodId=\"lib\"\ntype=\"required\"\nversionRange=\"[1.0,)\"\n[[dependencies.a]]\nmodId=\"neoforge\"\nversionRange=\"[99,)\"\n[[dependencies.a]]\nmodId=\"mixinextras\"\nversionRange=\"[0.4,)\"\n";
        let r = run(&[("a.jar", vec![("META-INF/neoforge.mods.toml", toml), ("META-INF/jarjar/metadata.json", meta), ("META-INF/jarjar/lib.jar", &inner)])], "1.21.1", "neoforge");
        assert_eq!(r.issues, vec![]);
        // MixinExtras n'est pas intégré à NeoForge avant 1.20.4.
        let r = run(&[("a.jar", vec![("META-INF/neoforge.mods.toml", toml)])], "1.20.1", "neoforge");
        assert!(r.issues.iter().any(|i| i.message.contains("mixinextras")));
    }

    #[test]
    fn breaks_et_conflicts() {
        let a = fab("a", r#","breaks":{"b":"*"},"conflicts":{"c":"<2"}"#);
        let b = fab("b", "");
        let c = fab("c", "");
        let r = run(&[("a.jar", vec![("fabric.mod.json", &a)]), ("b.jar", vec![("fabric.mod.json", &b)]), ("c.jar", vec![("fabric.mod.json", &c)])], "1.21.1", "fabric");
        assert_eq!(r.issues.len(), 2);
        assert_eq!((r.issues[0].kind.as_str(), r.issues[0].severity.as_str(), r.issues[0].message.as_str(), r.issues[0].related.as_deref()), ("incompatible", "error", "Incompatible avec B.", Some("b.jar")));
        assert_eq!((r.issues[1].severity.as_str(), r.issues[1].message.as_str()), ("warning", "Déconseillé avec C."));
    }

    #[test]
    fn forge_incompatible_et_discouraged() {
        let toml = b"[[mods]]\nmodId=\"a\"\n[[dependencies.a]]\nmodId=\"b\"\ntype=\"incompatible\"\n[[dependencies.a]]\nmodId=\"c\"\ntype=\"discouraged\"\n";
        let b = b"[[mods]]\nmodId=\"b\"\ndisplayName=\"Bee\"\nversion=\"1\"\n";
        let c = b"[[mods]]\nmodId=\"c\"\nversion=\"1\"\n";
        let r = run(&[("a.jar", vec![("META-INF/mods.toml", toml)]), ("b.jar", vec![("META-INF/mods.toml", b)]), ("c.jar", vec![("META-INF/mods.toml", c)])], "1.20.1", "forge");
        assert_eq!(r.issues.len(), 2);
        assert_eq!(r.issues[0].message, "Incompatible avec Bee.");
        assert_eq!(r.issues[1].severity, "warning");
    }

    #[test]
    fn mauvais_minecraft() {
        let a = fab("a", r#","depends":{"minecraft":"~1.20.1"}"#);
        let r = run(&[("a.jar", vec![("fabric.mod.json", &a)])], "1.21.1", "fabric");
        assert_eq!(r.issues.len(), 1);
        assert_eq!((r.issues[0].kind.as_str(), r.issues[0].message.as_str()), ("wrong_minecraft", "Fait pour Minecraft ~1.20.1, ce pack est en 1.21.1."));
        let toml = b"[[mods]]\nmodId=\"n\"\n[[dependencies.n]]\nmodId=\"minecraft\"\nversionRange=\"[1.20.1,1.21)\"\n";
        let r = run(&[("n.jar", vec![("META-INF/neoforge.mods.toml", toml)])], "1.19.2", "neoforge");
        assert_eq!(r.issues[0].message, "Fait pour Minecraft entre 1.20.1 et 1.21 (exclu), ce pack est en 1.19.2.");
        // Borne de la même famille (1.21 / 1.21.1) : NeoForge démarre, rien à signaler.
        let r = run(&[("n.jar", vec![("META-INF/neoforge.mods.toml", toml)])], "1.21.1", "neoforge");
        assert_eq!(r.issues.len(), 0);
        let r = run(&[("n.jar", vec![("META-INF/neoforge.mods.toml", toml)])], "1.20.4", "neoforge");
        assert_eq!(r.issues.len(), 0);
        // snapshot : indécidable, rien.
        let r = run(&[("n.jar", vec![("META-INF/neoforge.mods.toml", toml)])], "24w14a", "neoforge");
        assert_eq!(r.issues.len(), 0);
    }

    #[test]
    fn mauvais_loader_et_multi_loader() {
        let a = fab("a", "");
        let n = b"[[mods]]\nmodId=\"n\"\n";
        let r = run(&[("a.jar", vec![("fabric.mod.json", &a)]), ("n.jar", vec![("META-INF/neoforge.mods.toml", n)])], "1.21.1", "neoforge");
        assert_eq!(r.issues.len(), 1);
        assert_eq!((r.issues[0].file.as_str(), r.issues[0].message.as_str()), ("a.jar", "Mod pour Fabric, ce pack utilise NeoForge."));
        let r = run(&[("a.jar", vec![("fabric.mod.json", &a)]), ("n.jar", vec![("META-INF/neoforge.mods.toml", n)])], "1.21.1", "fabric");
        assert_eq!(r.issues[0].message, "Mod pour NeoForge, ce pack utilise Fabric.");
        // Quilt accepte Fabric ; Forge dans NeoForge : jamais signalé.
        let r = run(&[("a.jar", vec![("fabric.mod.json", &a)])], "1.21.1", "quilt");
        assert!(r.issues.is_empty());
        let f = b"[[mods]]\nmodId=\"f\"\n";
        let r = run(&[("f.jar", vec![("META-INF/mods.toml", f)])], "1.20.1", "neoforge");
        assert!(r.issues.is_empty());
        // multi-loader
        let r = run(&[("m.jar", vec![("fabric.mod.json", &a), ("META-INF/neoforge.mods.toml", n)])], "1.21.1", "neoforge");
        assert!(r.issues.is_empty());
        let r = run(&[("m.jar", vec![("fabric.mod.json", &a), ("META-INF/neoforge.mods.toml", n)])], "1.21.1", "fabric");
        assert!(r.issues.is_empty());
    }

    #[test]
    fn multi_loader_suit_le_descripteur_du_loader() {
        // Le descripteur Fabric exige une dépendance absente ; en NeoForge elle ne doit pas être regardée.
        let a = fab("m", r#","depends":{"absent":"*"}"#);
        let n = b"[[mods]]\nmodId=\"m\"\n";
        let r = run(&[("m.jar", vec![("fabric.mod.json", &a), ("META-INF/neoforge.mods.toml", n)])], "1.21.1", "neoforge");
        assert!(r.issues.is_empty());
        let r = run(&[("m.jar", vec![("fabric.mod.json", &a), ("META-INF/neoforge.mods.toml", n)])], "1.21.1", "fabric");
        assert_eq!(r.issues.len(), 1);
    }

    #[test]
    fn doublon() {
        let a1 = fab("same", "");
        let r = run(&[("b-new.jar", vec![("fabric.mod.json", &a1)]), ("a-old.jar", vec![("fabric.mod.json", &a1)])], "1.21.1", "fabric");
        assert_eq!(r.issues.len(), 1);
        assert_eq!((r.issues[0].file.as_str(), r.issues[0].kind.as_str(), r.issues[0].message.as_str(), r.issues[0].related.as_deref()), ("b-new.jar", "duplicate", "Déjà présent : a-old.jar.", Some("a-old.jar")));
    }

    #[test]
    fn jar_desactive_ignore() {
        let a = fab("a", r#","depends":{"b":"*"}"#);
        let b = fab("b", "");
        // b est désactivé : il ne fournit rien.
        let r = run(&[("a.jar", vec![("fabric.mod.json", &a)]), ("b.jar.disabled", vec![("fabric.mod.json", &b)])], "1.21.1", "fabric");
        assert_eq!(r.checked, 1);
        assert_eq!(r.issues.len(), 1);
        // a désactivé : pas analysé.
        let r = run(&[("a.jar.disabled", vec![("fabric.mod.json", &a)])], "1.21.1", "fabric");
        assert_eq!((r.checked, r.issues.len()), (0, 0));
        // doublon avec un désactivé : rien.
        let r = run(&[("a.jar", vec![("fabric.mod.json", &b)]), ("c.jar.disabled", vec![("fabric.mod.json", &b)])], "1.21.1", "fabric");
        assert!(r.issues.is_empty());
    }

    #[test]
    fn version_du_manifest_et_version_inconnue() {
        let toml = b"[[mods]]\nmodId=\"lib\"\nversion=\"${file.jarVersion}\"\n";
        let user = b"[[mods]]\nmodId=\"a\"\n[[dependencies.a]]\nmodId=\"lib\"\nmandatory=true
versionRange=\"[2.0,)\"\n";
        let mf_new: &[u8] = b"Manifest-Version: 1.0\nImplementation-Version: 2.5\n";
        let r = run(&[("a.jar", vec![("META-INF/mods.toml", user)]), ("lib.jar", vec![("META-INF/mods.toml", toml), ("META-INF/MANIFEST.MF", mf_new)])], "1.20.1", "forge");
        assert!(r.issues.is_empty());
        let mf_old: &[u8] = b"Manifest-Version: 1.0\nImplementation-Version: 1.5\n";
        let r = run(&[("a.jar", vec![("META-INF/mods.toml", user)]), ("lib.jar", vec![("META-INF/mods.toml", toml), ("META-INF/MANIFEST.MF", mf_old)])], "1.20.1", "forge");
        assert_eq!(r.issues.len(), 1);
        // Version inconnue (pas de manifeste) : jamais « trop vieux ».
        let r = run(&[("a.jar", vec![("META-INF/mods.toml", user)]), ("lib.jar", vec![("META-INF/mods.toml", toml)])], "1.20.1", "forge");
        assert!(r.issues.is_empty());
    }

    #[test]
    fn plage_indecidable_ne_dit_rien() {
        let a = fab("a", r#","depends":{"b":">=truc"}"#);
        let b = fab("b", "");
        let r = run(&[("a.jar", vec![("fabric.mod.json", &a)]), ("b.jar", vec![("fabric.mod.json", &b)])], "1.21.1", "fabric");
        assert!(r.issues.is_empty());
    }

    #[test]
    fn tri_erreurs_avant_avertissements_et_jars_sans_metadonnees() {
        let a = fab("zzz", r#","conflicts":{"b":"*"}"#);
        let b = fab("b", "");
        let c = fab("c", r#","depends":{"gone":"*"}"#);
        let r = run(
            &[("a.jar", vec![("fabric.mod.json", &a)]), ("b.jar", vec![("fabric.mod.json", &b)]), ("c.jar", vec![("fabric.mod.json", &c)]), ("lib.jar", vec![("x.txt", b"x")])],
            "1.21.1",
            "fabric",
        );
        assert_eq!(r.checked, 4);
        assert_eq!(r.issues.iter().map(|i| i.severity.as_str()).collect::<Vec<_>>(), vec!["error", "warning"]);
        let r = run(&[("a.jar", vec![("fabric.mod.json", &a)])], "1.21.1", "vanilla");
        assert!(r.issues.is_empty());
    }

    /// Lecture seule des vraies instances : `cargo test -p cubelink-core --lib -- --ignored --nocapture instances_reelles`.
    #[test]
    #[ignore]
    fn instances_reelles() {
        let Some(appdata) = std::env::var_os("APPDATA") else { return };
        let root = std::path::PathBuf::from(appdata).join("app.cubelink.launcher").join("instances");
        for e in std::fs::read_dir(&root).unwrap().flatten() {
            let Ok(txt) = std::fs::read_to_string(e.path().join("instance.json")) else { continue };
            let Ok(inst) = serde_json::from_str::<crate::types::Instance>(&txt) else { continue };
            let loader = serde_json::to_value(inst.loader.loader).unwrap();
            let loader = loader.as_str().unwrap();
            let game_dir = e.path().join("minecraft");
            let t = std::time::Instant::now();
            let r = check(&game_dir, &inst.minecraft, loader).unwrap();
            println!("== {} ({} {}) : {} jars, {} issues, {:?}", inst.name, inst.minecraft, loader, r.checked, r.issues.len(), t.elapsed());
            for i in &r.issues {
                println!("   [{}/{}] {} ({}) : {} {:?}", i.severity, i.kind, i.mod_name, i.file, i.message, i.related);
            }
        }
    }
}
