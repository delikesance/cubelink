//! Relevé des fichiers installés par le pack (chantier « relevé ») : empreintes attendues du pack, écrites à
//! l'installation et après chaque mise à jour ou annulation. Sert d'ancien état sans retélécharger l'ancien pack.

use super::*;

/// Ancien état tiré du relevé de l'instance, `None` s'il est absent, illisible, invalide ou périmé
/// (origine différente de `Instance.pack`).
pub(super) fn old_state(_game_dir: &Path, _inst: &Instance) -> Option<State> {
    None
}

/// `true` quand l'appelant doit fournir l'ancien pack à [`apply`] (pas de relevé valable).
pub fn needs_old_pack(instances: &InstanceStore, instance_id: &str) -> Result<bool> {
    let inst = instances.get(instance_id)?;
    let game_dir = instances.game_dir(instance_id)?;
    Ok(old_state(&game_dir, &inst).is_none())
}
