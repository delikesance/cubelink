//! Mods inconnus des catalogues déjà acceptés par l'utilisateur, mémorisés par sha256 dans `p2p/trusted_mods.json`.
//!
//! L'acceptation porte sur le CONTENU (sha256), pas sur le nom ni sur l'ami : un jar modifié, même au même chemin,
//! redemande la confiance ; un jar identique déjà accepté (même chez un autre ami) ne la redemande pas.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Mutex;

use core_lib::types::ManifestFile;

/// Plafond d'entrées conservées (un jar = 64 octets hex) : borne la taille du fichier.
const MAX_ENTRIES: usize = 20_000;

pub struct TrustedMods {
    path: PathBuf,
    set: Mutex<BTreeSet<String>>,
}

fn valid_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

impl TrustedMods {
    pub fn open(path: PathBuf) -> Self {
        let set = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Vec<String>>(&b).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|s| valid_sha256(s))
            .map(|s| s.to_ascii_lowercase())
            .collect();
        Self { path, set: Mutex::new(set) }
    }

    pub fn contains(&self, sha256: &str) -> bool {
        self.set.lock().map(|s| s.contains(&sha256.to_ascii_lowercase())).unwrap_or(false)
    }

    /// Ajoute des empreintes acceptées et réécrit le fichier (atomiquement) si quelque chose a changé.
    pub fn add_all<'a>(&self, shas: impl IntoIterator<Item = &'a str>) {
        let Ok(mut s) = self.set.lock() else { return };
        let mut changed = false;
        for h in shas.into_iter().filter(|h| valid_sha256(h)) {
            if s.len() >= MAX_ENTRIES {
                break;
            }
            changed |= s.insert(h.to_ascii_lowercase());
        }
        if !changed {
            return;
        }
        if let Ok(json) = serde_json::to_vec(&s.iter().collect::<Vec<_>>()) {
            let tmp = self.path.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }

    /// Sépare les chemins `unknown` (mods inconnus du manifeste) en (à accepter, déjà acceptés).
    /// Un chemin absent du manifeste reste « à accepter » (prudence).
    pub fn split(&self, unknown: Vec<String>, files: &[ManifestFile]) -> (Vec<String>, Vec<String>) {
        let by_path: HashMap<&str, &str> = files.iter().map(|f| (f.path.as_str(), f.sha256.as_str())).collect();
        unknown.into_iter().partition(|p| !by_path.get(p.as_str()).is_some_and(|h| self.contains(h)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, sha: &str) -> ManifestFile {
        ManifestFile { path: path.into(), sha256: sha.into(), size: 1, sha1: None }
    }

    #[test]
    fn memorise_par_contenu_et_relit() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("trusted_mods.json");
        let (a, b) = ("a".repeat(64), "b".repeat(64));
        let t = TrustedMods::open(p.clone());
        assert!(!t.contains(&a));
        t.add_all([a.as_str(), "pas-une-empreinte"]);
        assert!(t.contains(&a));
        assert!(t.contains(&a.to_uppercase()));
        let t2 = TrustedMods::open(p);
        assert!(t2.contains(&a));
        assert!(!t2.contains(&b));
        let files = vec![file("mods/x.jar", &a), file("mods/y.jar", &b)];
        let unknown = vec!["mods/x.jar".to_string(), "mods/y.jar".to_string(), "mods/hors-manifeste.jar".to_string()];
        let (todo, done) = t2.split(unknown, &files);
        assert_eq!(todo, vec!["mods/y.jar".to_string(), "mods/hors-manifeste.jar".to_string()]);
        assert_eq!(done, vec!["mods/x.jar".to_string()]);
    }

    #[test]
    fn fichier_illisible_ignore() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("trusted_mods.json");
        std::fs::write(&p, b"{pas du json").unwrap();
        assert!(!TrustedMods::open(p).contains(&"a".repeat(64)));
    }
}
