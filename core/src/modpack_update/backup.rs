//! Sauvegarde d'une mise à jour et remise en état.

use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Moved {
    /// Nom réel sur le disque, relatif à l'instance.
    pub(super) rel: String,
    /// Provenance du jar (clé = nom activé), remise à l'annulation.
    pub(super) provenance: Option<Provenance>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Added {
    pub(super) rel: String,
    /// Empreinte du fichier posé : si le joueur l'a modifié depuis, l'annulation n'y touche pas.
    pub(super) digest: Digest,
    /// Objet du magasin lié (référence relâchée à l'annulation).
    pub(super) sha256: Option<String>,
    /// Clé de provenance (nom activé du jar) enregistrée pour ce fichier.
    pub(super) prov_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Manifest {
    pub(super) id: String,
    pub(super) from: Option<PackOrigin>,
    pub(super) to: PackOrigin,
    pub(super) loader_from: LoaderSpec,
    pub(super) loader_to: LoaderSpec,
    pub(super) moved: Vec<Moved>,
    pub(super) added: Vec<Added>,
}

pub(super) fn valid_backup_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

pub(super) fn new_backup_id() -> String {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    format!("{ms:015}-{:03}", N.fetch_add(1, Ordering::Relaxed) % 1000)
}

pub(super) fn backup_root(game_dir: &Path) -> PathBuf {
    game_dir.join(BACKUP_DIR)
}

pub(super) fn write_manifest(bdir: &Path, m: &Manifest) -> Result<()> {
    std::fs::create_dir_all(bdir)?;
    let json = serde_json::to_vec_pretty(m).map_err(|e| Error::Other(e.to_string()))?;
    let tmp = bdir.join("manifest.json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, bdir.join("manifest.json"))?;
    Ok(())
}

pub(super) fn prune_backups(game_dir: &Path) {
    let Ok(rd) = std::fs::read_dir(backup_root(game_dir)) else {
        return;
    };
    let mut dirs: Vec<String> = rd
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    dirs.sort();
    while dirs.len() > KEEP_BACKUPS {
        let old = dirs.remove(0);
        let _ = std::fs::remove_dir_all(backup_root(game_dir).join(old));
    }
}

/// Clé de provenance (nom activé) d'un jar de `mods/` à partir de son nom réel sur le disque.
pub(super) fn jar_key(rel: &str) -> Option<String> {
    mrpack::mod_jar_name(rel.trim_end_matches(".disabled")).map(str::to_string)
}

/// Défait l'opération décrite par `mf` : retire les fichiers ajoutés puis remet les fichiers déplacés. Tolère une
/// opération inachevée (fichier pas encore déplacé ou pas encore posé). `force` : retire les ajouts sans comparer leur
/// empreinte (échec en cours d'application : ce sont nos propres fichiers). Renvoie (erreurs, fichiers du joueur gardés).
pub(super) fn revert(
    game_dir: &Path,
    bdir: &Path,
    mf: &Manifest,
    index: &StoreIndex,
    force: bool,
) -> (Vec<String>, Vec<String>) {
    let mut errors = Vec::new();
    let mut player_kept = Vec::new();
    let mut skip_restore: HashSet<String> = HashSet::new();
    let mut forget: Vec<String> = Vec::new();
    let backed_up = |rel: &str| {
        bdir.join("files")
            .join(rel.split('/').collect::<PathBuf>())
            .is_file()
    };
    let moved_rels: HashSet<&str> = mf.moved.iter().map(|m| m.rel.as_str()).collect();
    for a in mf.added.iter().rev() {
        // Remplacement inachevé : l'ancien fichier n'a pas encore été déplacé, il est toujours à sa place. Ce n'est pas
        // notre ajout, on n'y touche pas.
        if moved_rels.contains(a.rel.as_str()) && !backed_up(&a.rel) {
            continue;
        }
        let path = disk_path(game_dir, &a.rel);
        match std::fs::symlink_metadata(&path) {
            Err(_) => {}
            Ok(m) if !m.is_file() => {
                errors.push(format!("{} : n'est plus un fichier", a.rel));
                skip_restore.insert(a.rel.clone());
            }
            Ok(_) => {
                let untouched = force || a.digest.disk_matches(&path).unwrap_or(false);
                if !untouched {
                    // Le joueur a modifié ce fichier depuis la mise à jour : il garde sa version.
                    player_kept.push(a.rel.clone());
                    skip_restore.insert(a.rel.clone());
                    continue;
                }
                match std::fs::remove_file(&path) {
                    Ok(()) => {
                        if let Some(s) = &a.sha256 {
                            let _ = index.release_ref(s);
                        }
                    }
                    Err(e) => {
                        errors.push(format!("{} : {e}", a.rel));
                        skip_restore.insert(a.rel.clone());
                        continue;
                    }
                }
            }
        }
        if let Some(k) = &a.prov_key {
            forget.push(k.clone());
        }
    }
    if !forget.is_empty() {
        if let Err(e) = mods::forget_provenance(game_dir, &forget) {
            errors.push(format!("provenance des mods : {e}"));
        }
    }
    let mut restore_prov: Vec<(String, Provenance)> = Vec::new();
    for m in mf.moved.iter().rev() {
        if skip_restore.contains(&m.rel) {
            continue;
        }
        let src = bdir
            .join("files")
            .join(m.rel.split('/').collect::<PathBuf>());
        if !src.is_file() {
            continue; // jamais déplacé (opération inachevée) ou déjà remis
        }
        let dst = disk_path(game_dir, &m.rel);
        if dst.exists() {
            errors.push(format!("{} existe déjà", m.rel));
            continue;
        }
        let res = dst
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::rename(&src, &dst));
        match res {
            Ok(()) => {
                if let (Some(p), Some(k)) = (&m.provenance, jar_key(&m.rel)) {
                    restore_prov.push((k, p.clone()));
                }
            }
            Err(e) => errors.push(format!("{} : {e}", m.rel)),
        }
    }
    if !restore_prov.is_empty() {
        if let Err(e) = mods::record_provenance(game_dir, &restore_prov) {
            errors.push(format!("provenance des mods : {e}"));
        }
    }
    (errors, player_kept)
}
