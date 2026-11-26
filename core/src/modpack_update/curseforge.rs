//! Mise à jour d'un modpack CurseForge (chantier « CurseForge ») : lecture du nouveau zip, ancien état.

use super::*;

/// Nouveau zip CurseForge lu (mods résolus par `cf`, overrides et contenu effectif) avec les options propres à ce
/// format (CDN CurseForge autorisés).
async fn read_pack(path: &Path, cf: &CfResolver, opts: &ImportOptions, cancel: &Arc<AtomicBool>) -> Result<(NewPack, ImportOptions)> {
    let mut zip = mrpack::open_zip(path)?;
    let cfpack = crate::cf_pack::read(&mut zip, cf, opts, cancel).await?;
    let over = mrpack::plan_overrides(&mut zip, &cfpack.opts, &cfpack.layout)?;
    let hashes = hash_overrides(&mut zip, &over)?;
    let state = state_of(&cfpack.index, &over, &hashes);
    Ok((NewPack { idx: cfpack.index, zip, over, state }, cfpack.opts))
}

/// CurseForge : nouveau pack résolu par `cf`, ancien état = `recorded` s'il est fourni, sinon reconstruit depuis
/// `old_pack` et les empreintes de l'API.
pub(super) async fn prepare(
    inst: &Instance,
    recorded: Option<State>,
    old_pack: Option<&Path>,
    new_pack: &Path,
    new_origin: &PackOrigin,
    cf: &CfResolver,
    opts: &ImportOptions,
) -> Result<Prepared> {
    if recorded.is_none() && !cf.has_api_key() {
        return Err(invalid(
            "impossible de comparer avec la version installée sans le relevé de ses fichiers : réinstalle le modpack pour pouvoir le mettre à jour",
        ));
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let (new, new_opts) = read_pack(new_pack, cf, opts, &cancel).await?;
    check_compatible(inst, &new.idx, new_origin)?;
    let old_state = match (recorded, old_pack) {
        (Some(state), _) => state,
        (None, Some(old)) => read_pack(old, cf, opts, &cancel).await?.0.state,
        (None, None) => return Err(invalid("version installée du modpack nécessaire pour comparer")),
    };
    let mut prepared = assemble(inst, old_state, new, new_origin)?;
    prepared.opts = Some(new_opts);
    Ok(prepared)
}

/// Garde anti-doublon : refuse la mise à jour si un jar de `mods/` de l'ancien pack, d'empreinte inconnue (CurseForge
/// n'a pas fourni de SHA-1), est sur le disque alors que le nouveau pack ne le contient plus ou le change. On ne peut
/// pas prouver qu'il vient du pack : il resterait à côté de la nouvelle version (deux fois le même mod). Un fichier
/// absent du disque, une config ou un jar d'empreinte connue ne bloquent rien. À appeler avant tout téléchargement.
pub(super) fn refuse_duplicates(prepared: &Prepared, game_dir: &Path) -> Result<()> {
    let mut blocking: Vec<&str> = Vec::new();
    for (key, old) in &prepared.old_state.items {
        let changed = prepared.new_state.items.get(key).is_none_or(|new| new.digest != old.digest);
        let is_unknown_jar = mrpack::mod_jar_name(&old.path).is_some() && old.digest.hex.is_empty();
        if changed && is_unknown_jar && matches!(probe(game_dir, &old.path), Probe::File { .. }) {
            blocking.push(&old.path);
        }
    }
    if blocking.is_empty() {
        return Ok(());
    }
    Err(invalid(format!(
        "mise à jour impossible : {} est sur ton disque mais Cubelink ne peut pas vérifier qu'il vient du modpack ; la nouvelle version s'installerait à côté (mod en double). Retire ce fichier à la main puis relance la mise à jour, ou réinstalle le modpack",
        blocking.join(", ")
    )))
}
