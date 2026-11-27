//! Anti-doublon par identifiant de mod (modid) : le même mod peut venir de CurseForge et de Modrinth sous deux
//! projets différents, donc sans provenance commune. Le modid, lui, est le même : on le lit dans le jar.

use super::*;
use crate::mod_meta;

/// Modids déclarés par un jar (minuscules). Vide si le jar est illisible ou sans métadonnées.
fn modids(path: &Path) -> HashSet<String> {
    mod_meta::jar_meta(path)
        .map(|m| m.mods.iter().map(|x| x.id.to_ascii_lowercase()).collect())
        .unwrap_or_default()
}

/// Jars de `mods/` (activés ou désactivés) absents des deux packs : ceux du joueur. Chemin relatif -> modids.
fn player_jars(game_dir: &Path, old: &State, new: &State) -> BTreeMap<String, HashSet<String>> {
    let Ok(entries) = std::fs::read_dir(game_dir.join("mods")) else {
        return BTreeMap::new();
    };
    let mut out = BTreeMap::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let base = name.strip_suffix(".disabled").unwrap_or(&name);
        let rel = format!("mods/{name}");
        let key = format!("mods/{base}").to_ascii_lowercase();
        let in_pack = old.items.contains_key(&key) || new.items.contains_key(&key);
        if mrpack::mod_jar_name(&key).is_none() || in_pack || !entry.path().is_file() {
            continue;
        }
        let ids = modids(&entry.path());
        if !ids.is_empty() {
            out.insert(rel, ids);
        }
    }
    out
}

/// Retire du plan les ajouts et remplacements d'un jar dont le modid est déjà porté par un jar du joueur : le joueur
/// garde le sien (signalé dans `kept`). À appeler une fois les fichiers du nouveau pack téléchargés ou extraits
/// (`jar_of` donne leur emplacement), avant toute modification de l'instance.
pub(super) fn drop_player_duplicates(
    plan: &mut Plan,
    game_dir: &Path,
    old: &State,
    new: &State,
    jar_of: impl Fn(&Item) -> Option<PathBuf>,
) {
    let players = player_jars(game_dir, old, new);
    if players.is_empty() {
        return;
    }
    let mut kept = Vec::new();
    plan.actions.retain(|a| {
        let (Action::Add { item, .. } | Action::Replace { item, .. }) = a else {
            return true;
        };
        let Some(ids) = mrpack::mod_jar_name(&item.path)
            .and_then(|_| jar_of(item))
            .map(|p| modids(&p))
        else {
            return true;
        };
        let twin = players.iter().find(|(_, theirs)| !theirs.is_disjoint(&ids));
        if let Some((rel, _)) = twin {
            kept.push(rel.clone());
        }
        twin.is_none()
    });
    plan.kept.extend(kept);
    plan.kept.sort();
    plan.kept.dedup();
}
