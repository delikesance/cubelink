//! Détection d'une version plus récente du modpack installé.

use super::*;

/// Version plus récente du modpack (Modrinth ou CurseForge), compatible avec l'instance. `Ok(None)` : instance sans
/// origine de modpack, déjà à jour, ou version installée introuvable (on ne propose jamais un retour en arrière).
/// Une pré-version (beta, alpha) n'est proposée que si la version installée en est déjà une. CurseForge : sans clé
/// d'API, `Ok(None)` ; les fichiers de serveur (`isServerPack`) ne sont jamais proposés.
pub async fn check(p: &Providers, inst: &Instance) -> Result<Option<ModpackUpdate>> {
    let Some(pack) = inst.pack.as_ref() else {
        return Ok(None);
    };
    match pack.source.as_str() {
        "modrinth" => check_modrinth(p, inst, pack).await,
        "curseforge" => check_curseforge(p, inst, pack).await,
        _ => Ok(None),
    }
}

async fn check_curseforge(p: &Providers, inst: &Instance, pack: &PackOrigin) -> Result<Option<ModpackUpdate>> {
    let (Ok(mod_id), Ok(file_id)) = (pack.project_id.parse::<u64>(), pack.version_id.parse::<u64>()) else {
        return Ok(None);
    };
    if !p.cf.has_key() {
        return Ok(None);
    }
    let loader = Some(inst.loader.loader).filter(|l| *l != Loader::Vanilla);
    let list = browse::cf_versions(p, &pack.project_id, BrowseKind::Modpack, Some(&inst.minecraft), loader).await?;
    let installed = match list.iter().find(|f| f.id == file_id) {
        Some(f) => f.clone(),
        // Hors de la liste filtrée : on n'a de quoi comparer que si l'API connaît encore le fichier.
        None => match p.cf.file(mod_id, file_id).await {
            Ok(f) => f,
            Err(Error::NotFound(_)) => return Ok(None),
            Err(e) => return Err(e),
        },
    };
    if installed.file_date.is_empty() {
        return Ok(None);
    }
    // `releaseType` : 1 release, 2 beta, 3 alpha (0 : inconnu, traité comme une release).
    let is_release = |release_type: u32| release_type <= 1;
    let installed_is_release = is_release(installed.release_type);
    let newer = list.iter().find(|f| {
        f.id != installed.id
            && !f.is_server_pack
            && f.url().is_some()
            && f.file_date > installed.file_date
            && (!installed_is_release || is_release(f.release_type))
    });
    Ok(newer.map(|f| {
        let name = if f.display_name.trim().is_empty() { f.file_name.clone() } else { f.display_name.clone() };
        ModpackUpdate { version_id: f.id.to_string(), version_number: name.clone(), name, published: f.file_date.clone() }
    }))
}

async fn check_modrinth(p: &Providers, inst: &Instance, pack: &PackOrigin) -> Result<Option<ModpackUpdate>> {
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
