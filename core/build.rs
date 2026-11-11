// Intègre la clé API CurseForge de développeur (celle de Cubelink) à la compilation : `tauri build`
// ne lit pas `.env`, donc l'installeur serait sinon livré sans clé. Une variable d'environnement
// déjà définie reste prioritaire. La valeur n'est jamais affichée.
use std::{env, fs, path::Path};

fn main() {
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
