//! Contenu effectif d'un pack et chemins protégés.

use super::*;

#[derive(Debug, Clone)]
pub(super) enum Source {
    /// Fichier de l'index (téléchargé), position dans `MrpackIndex::files`.
    Index(usize),
    /// Entrée de `overrides/` ou `client-overrides/`, position dans la liste des overrides.
    Over(usize),
}

#[derive(Debug, Clone)]
pub(super) struct Item {
    pub(super) path: String,
    pub(super) digest: Digest,
    pub(super) source: Source,
    /// Projet Modrinth du jar (déduit de l'URL), pour la provenance.
    pub(super) provenance: Option<Provenance>,
}

/// Contenu effectif d'un pack : chemin (minuscules) -> fichier. Un override remplace le fichier de l'index au même chemin
/// (comme à l'import). Les fichiers que le client ne gère pas (`env.client = unsupported`) n'y figurent pas.
pub(super) struct State {
    pub(super) items: BTreeMap<String, Item>,
}

pub(super) fn state_of(
    pack: &MrpackIndex,
    overrides: &[OverrideEntry],
    hashes: &[String],
) -> State {
    let mut items = BTreeMap::new();
    for (i, f) in pack.files.iter().enumerate() {
        if f.client_unsupported() {
            continue;
        }
        items.insert(
            f.path.to_ascii_lowercase(),
            Item {
                path: f.path.clone(),
                digest: Digest::of_index(f),
                source: Source::Index(i),
                provenance: f.provenance.clone(),
            },
        );
    }
    for (i, o) in overrides.iter().enumerate() {
        items.insert(
            o.rel.to_ascii_lowercase(),
            Item {
                path: o.rel.clone(),
                digest: Digest {
                    alg: Alg::Sha512,
                    hex: hashes[i].clone(),
                },
                source: Source::Over(i),
                provenance: None,
            },
        );
    }
    State { items }
}

pub(super) const PROTECTED_DIRS: &[&str] =
    &["saves", "screenshots", "logs", "crash-reports", "backups"];
pub(super) const PROTECTED_FILES: &[&str] = &[
    "options.txt",
    "optionsof.txt",
    "optionsshaders.txt",
    "servers.dat",
    "servers.dat_old",
    "usercache.json",
    "usernamecache.json",
];

/// Données du joueur : jamais lues ni écrites par une mise à jour, même si le pack en fournit.
pub(super) fn protected(rel: &str) -> bool {
    let l = rel.to_ascii_lowercase();
    let mut segs = l.split('/');
    let first = segs.next().unwrap_or("");
    (PROTECTED_DIRS.contains(&first) && l.contains('/'))
        || (!l.contains('/') && PROTECTED_FILES.contains(&first))
        || l.split('/').any(|s| s.starts_with(".cubelink"))
}
