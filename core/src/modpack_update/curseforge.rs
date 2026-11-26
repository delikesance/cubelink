//! Mise à jour d'un modpack CurseForge (chantier « CurseForge ») : lecture du nouveau zip, ancien état.

use super::*;

/// CurseForge : nouveau pack résolu par `cf`, ancien état = `recorded` s'il est fourni, sinon reconstruit depuis
/// `old_pack` et les empreintes de l'API.
pub(super) async fn prepare(
    _inst: &Instance,
    _recorded: Option<State>,
    _old_pack: Option<&Path>,
    _new_pack: &Path,
    _new_origin: &PackOrigin,
    _cf: &CfResolver,
    _opts: &ImportOptions,
) -> Result<Prepared> {
    Err(invalid("la mise à jour d'un modpack CurseForge n'est pas encore prise en charge"))
}
