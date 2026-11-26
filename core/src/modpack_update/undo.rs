//! Annulation d'une mise à jour à partir de sa sauvegarde.

use super::*;

/// Annule une mise à jour : retire les fichiers ajoutés, remet ceux qui avaient été déplacés, l'ancienne origine du pack
/// et l'ancien loader. Refuse si une mise à jour plus récente a eu lieu depuis. Un fichier que le joueur a modifié
/// depuis la mise à jour reste tel quel. Renvoie l'instance.
pub fn undo(
    instances: &InstanceStore,
    instance_id: &str,
    backup_id: &str,
    index: &StoreIndex,
) -> Result<Instance> {
    if !valid_backup_id(backup_id) {
        return Err(invalid("identifiant de sauvegarde invalide"));
    }
    let inst = instances.get(instance_id)?;
    let game_dir = instances.game_dir(instance_id)?;
    let bdir = backup_root(&game_dir).join(backup_id);
    let raw = std::fs::read(bdir.join("manifest.json")).map_err(|_| {
        Error::NotFound("sauvegarde introuvable (déjà annulée ou supprimée)".into())
    })?;
    let mf: Manifest =
        serde_json::from_slice(&raw).map_err(|e| invalid(format!("sauvegarde illisible : {e}")))?;
    if inst.pack.as_ref() != Some(&mf.to) {
        return Err(invalid(
            "une mise à jour plus récente a eu lieu depuis : annule d'abord celle-là",
        ));
    }
    let (errors, player_kept) = revert(&game_dir, &bdir, &mf, index, false);
    if !errors.is_empty() {
        return Err(Error::Other(format!("annulation incomplète ({}) ; la sauvegarde est conservée dans {BACKUP_DIR}/{backup_id}", errors.join(" ; "))));
    }
    if !player_kept.is_empty() {
        tracing::info!(
            "annulation : fichiers modifiés depuis la mise à jour, laissés tels quels : {}",
            player_kept.join(", ")
        );
    }
    record::restore_from_backup(&game_dir, &bdir)?;
    instances.set_pack(instance_id, mf.from.clone())?;
    let updated = if mf.loader_from != inst.loader {
        instances.set_loader(instance_id, mf.loader_from.clone())?
    } else {
        instances.get(instance_id)?
    };
    let _ = std::fs::remove_dir_all(&bdir);
    Ok(updated)
}
