//! Détection d'une version plus récente du modpack installé.

use super::*;

/// Version plus récente du modpack, compatible avec l'instance. `Ok(None)` : instance sans origine Modrinth, déjà à
/// jour, ou version installée introuvable (on ne propose jamais un retour en arrière). Une pré-version (beta, alpha)
/// n'est proposée que si la version installée en est déjà une.
pub async fn check(p: &Providers, inst: &Instance) -> Result<Option<ModpackUpdate>> {
    let Some(pack) = inst.pack.as_ref().filter(|o| o.source == "modrinth") else {
        return Ok(None);
    };
    let loader = Some(inst.loader.loader).filter(|l| *l != Loader::Vanilla);
    let list = browse::modrinth_versions(
        p,
        &pack.project_id,
        BrowseKind::Modpack,
        Some(&inst.minecraft),
        loader,
    )
    .await?;
    let installed = match list.iter().find(|v| v.id == pack.version_id) {
        Some(v) => v.clone(),
        // Retirée de la liste (autre Minecraft, dépubliée) : on n'a de quoi comparer que si l'API la connaît encore.
        None => match p.modrinth.version(&pack.version_id).await {
            Ok(v) => v,
            Err(Error::NotFound(_)) => return Ok(None),
            Err(e) => return Err(e),
        },
    };
    if installed.date_published.is_empty() {
        return Ok(None);
    }
    let installed_is_release =
        installed.version_type.is_empty() || installed.version_type == "release";
    // `list` est trié du plus récent au plus ancien.
    let newer = list.iter().find(|v| {
        v.id != installed.id
            && !v.files.is_empty()
            && v.date_published > installed.date_published
            && (!installed_is_release || v.version_type.is_empty() || v.version_type == "release")
    });
    Ok(newer.map(|v| ModpackUpdate {
        version_id: v.id.clone(),
        name: if v.name.trim().is_empty() {
            v.version_number.clone()
        } else {
            v.name.clone()
        },
        version_number: v.version_number.clone(),
        published: v.date_published.clone(),
    }))
}
