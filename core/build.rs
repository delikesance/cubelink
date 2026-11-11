// Intègre la clé API CurseForge de développeur (celle de Cubelink) à la compilation : `tauri build`
// ne lit pas `.env`, donc l'installeur serait sinon livré sans clé. Une variable d'environnement
// déjà définie reste prioritaire. La valeur n'est jamais affichée.
use std::{env, fs, path::Path};

/// Génère `$OUT_DIR/srg_tables.rs` : les tables SRG `core/resources/lan-map-srg/<mc>.map` embarquées dans le binaire
/// (aucune ressource Tauri à déclarer). Dossier absent ou vide = liste vide (pas d'agent en Forge legacy).
fn embed_srg_tables() {
    let dir = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap()).join("resources").join("lan-map-srg");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut items = vec![];
    if let Ok(rd) = fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("map") {
                continue;
            }
            let Some(ver) = p.file_stem().and_then(|s| s.to_str()).map(str::to_string) else { continue };
            println!("cargo:rerun-if-changed={}", p.display());
            let abs = p.to_string_lossy().replace('\\', "/");
            items.push(format!("    ({ver:?}, include_str!({abs:?})),\n"));
        }
    }
    items.sort();
    let out = format!("/// (version Minecraft, table SRG) embarquées.\npub const SRG_TABLES: &[(&str, &str)] = &[\n{}];\n", items.concat());
    fs::write(Path::new(&env::var("OUT_DIR").unwrap()).join("srg_tables.rs"), out).unwrap();
}

fn main() {
    embed_srg_tables();
    println!("cargo:rerun-if-env-changed=CUBELINK_CF_API_KEY");
    let env_file = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap()).join("..").join(".env");
    println!("cargo:rerun-if-changed={}", env_file.display());
    if env::var("CUBELINK_CF_API_KEY").map(|v| !v.trim().is_empty()).unwrap_or(false) {
        return;
    }
    let Ok(text) = fs::read_to_string(&env_file) else { return };
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        if let Some(v) = line.strip_prefix("CUBELINK_CF_API_KEY=") {
            let v = v.trim().trim_matches(|c| c == '\'' || c == '"').trim();
            if !v.is_empty() {
                println!("cargo:rustc-env=CUBELINK_CF_API_KEY={v}");
            }
        }
    }
}
