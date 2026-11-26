//! Application d'une mise à jour (téléchargement, puis modification de l'instance).

use super::*;

// Point d'entrée de l'import (`mrpack.rs`) : `mod record` est privé, ce glob (`pub use apply::*`) les rend publics.
pub use super::record::{stamp_origin, RECORD_FILE};
pub(crate) use super::record::{write_install_record, PlacedFile};

/// Télécharge dans le magasin (sans toucher à l'instance) les fichiers d'index à poser. Clé : chemin du fichier.
pub(super) async fn download_all(
    pack: &MrpackIndex,
    wanted: &[&Item],
    store_root: &Path,
    index: &StoreIndex,
    opts: &ImportOptions,
    progress: &Option<FilesProgress>,
    cancel: &Arc<AtomicBool>,
) -> Result<(BTreeMap<String, StoredObject>, Vec<String>)> {
    let files: Vec<(&str, &MrpackFile)> = wanted
        .iter()
        .filter_map(|it| match it.source {
            Source::Index(i) => Some((it.path.as_str(), &pack.files[i])),
            Source::Over(_) => None,
        })
        .collect();
    let total = files.len() as u32;
    let dl = DlOptions {
        cancel: cancel.clone(),
        progress: None,
        ..opts.dl.clone()
    };
    let mut done = 0u32;
    let mut got = BTreeMap::new();
    // Fichiers « tolérants » (CurseForge) introuvables : la mise à jour continue, le joueur les pose lui-même.
    let mut missing = Vec::new();
    let mut results = stream::iter(files.iter().copied())
        .map(|(path, f)| {
            let dl = &dl;
            async move { (path, mrpack::fetch_file(f, store_root, index, dl).await) }
        })
        .buffer_unordered(opts.concurrency.max(1));
    while let Some((path, res)) = results.next().await {
        match res {
            Ok(obj) => {
                got.insert(path.to_string(), obj);
            }
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(e) if pack.files.iter().any(|f| f.path == path && f.tolerant) => {
                tracing::warn!("mise à jour du modpack : fichier non récupéré ({path}) : {e}");
                missing.push(path.to_string());
            }
            Err(e) => return Err(e),
        }
        done += 1;
        if let Some(p) = progress {
            p(done, total);
        }
    }
    drop(results);
    if cancel.load(Ordering::SeqCst) {
        return Err(Error::Cancelled);
    }
    missing.sort();
    Ok((got, missing))
}

/// Projets des fichiers du nouveau pack introuvables au téléchargement.
pub(super) fn missing_projects(new_state: &State, missing: &[String]) -> HashSet<Provenance> {
    new_state.items.values().filter(|it| missing.contains(&it.path)).filter_map(|it| it.provenance.clone()).collect()
}

/// Les deux packs lus et validés : tout ce qu'il faut pour planifier, sans avoir rien téléchargé ni modifié.
pub(super) struct Prepared {
    pub(super) old_state: State,
    pub(super) new_state: State,
    pub(super) new_idx: MrpackIndex,
    pub(super) new_zip: ZipArchive<std::fs::File>,
    pub(super) new_over: Vec<OverrideEntry>,
    /// Options propres au format du nouveau pack (CurseForge : CDN autorisés), à la place de celles de l'appelant.
    pub(super) opts: Option<ImportOptions>,
}

/// Nouveau pack lu et validé (index, archive, overrides et contenu effectif), sans rien avoir téléchargé.
pub(super) struct NewPack {
    pub(super) idx: MrpackIndex,
    pub(super) zip: ZipArchive<std::fs::File>,
    pub(super) over: Vec<OverrideEntry>,
    pub(super) state: State,
}

/// Lit un `.mrpack` : index (URL vérifiées), overrides et leurs empreintes.
pub(super) fn read_mrpack(path: &Path, opts: &ImportOptions) -> Result<NewPack> {
    let mut zip = mrpack::open_zip(path)?;
    let idx = mrpack::parse_index(path)?;
    for f in &idx.files {
        for u in &f.downloads {
            mrpack::check_url(u, opts)?;
        }
    }
    let over = mrpack::plan_overrides(&mut zip, opts, &Layout::mrpack())?;
    let hashes = hash_overrides(&mut zip, &over)?;
    let state = state_of(&idx, &over, &hashes);
    Ok(NewPack { idx, zip, over, state })
}

/// Refuse une version d'un autre projet, ou qui change la version de Minecraft ou le loader.
pub(super) fn check_compatible(inst: &Instance, new_idx: &MrpackIndex, new_origin: &PackOrigin) -> Result<()> {
    let Some(current) = inst.pack.as_ref() else {
        return Err(invalid("cette instance n'a pas d'origine de modpack connue : la mise à jour n'est pas possible"));
    };
    if new_origin.source != current.source || new_origin.project_id != current.project_id {
        return Err(invalid("cette version n'appartient pas au modpack installé"));
    }
    if new_idx.minecraft != inst.minecraft || new_idx.loader.loader != inst.loader.loader {
        return Err(invalid(
            "cette version change la version de Minecraft ou le loader : installe-la comme nouvelle instance pour garder tes mondes et tes mods",
        ));
    }
    Ok(())
}

/// Assemble l'ancien état et le nouveau pack, après les contrôles de compatibilité.
pub(super) fn assemble(inst: &Instance, old_state: State, new: NewPack, new_origin: &PackOrigin) -> Result<Prepared> {
    check_compatible(inst, &new.idx, new_origin)?;
    Ok(Prepared { old_state, new_state: new.state, new_idx: new.idx, new_zip: new.zip, new_over: new.over, opts: None })
}

/// Modrinth : ancien état = `recorded` (relevé de l'instance) s'il est fourni, sinon lu dans `old_pack`.
pub(super) fn prepare_modrinth(
    inst: &Instance,
    recorded: Option<State>,
    old_pack: Option<&Path>,
    new_pack: &Path,
    new_origin: &PackOrigin,
    opts: &ImportOptions,
) -> Result<Prepared> {
    let old_state = match (recorded, old_pack) {
        (Some(s), _) => s,
        (None, Some(old)) => read_mrpack(old, opts)?.state,
        (None, None) => return Err(invalid("version installée du modpack nécessaire pour comparer")),
    };
    assemble(inst, old_state, read_mrpack(new_pack, opts)?, new_origin)
}

/// Modrinth, ancien pack lu sur le disque (forme historique, utilisée par les tests).
#[cfg(test)]
pub(super) fn prepare(inst: &Instance, old_pack: &Path, new_pack: &Path, new_origin: &PackOrigin, opts: &ImportOptions) -> Result<Prepared> {
    prepare_modrinth(inst, None, Some(old_pack), new_pack, new_origin, opts)
}

/// Applique la mise à jour vers `new_pack` (`.mrpack` ou zip CurseForge) sur l'instance `instance_id`.
///
/// Ancien état : le relevé de l'instance s'il est valable ([`needs_old_pack`] vaut alors `false`), sinon `old_pack`
/// (la version installée, retéléchargée par l'appelant). `new_origin` : projet et version de `new_pack` (enregistrés
/// dans `Instance.pack` en dernier). `cf` : résolution des fichiers CurseForge.
/// Les futurs de ce module lisent et écrivent le disque de façon synchrone : l'appelant l'exécute hors du fil
/// principal de l'interface (`spawn_blocking`).
#[allow(clippy::too_many_arguments)]
pub async fn apply(
    instances: &InstanceStore,
    instance_id: &str,
    old_pack: Option<&Path>,
    new_pack: &Path,
    new_origin: PackOrigin,
    cf: &CfResolver,
    store_root: &Path,
    index: &StoreIndex,
    opts: &ImportOptions,
    progress: Option<FilesProgress>,
    cancel: Arc<AtomicBool>,
) -> Result<ModpackUpdateResult> {
    let inst = instances.get(instance_id)?;
    let game_dir = instances.game_dir(instance_id)?;
    let recorded = record::old_state(&game_dir, &inst);
    let prepared = match mrpack::detect(new_pack)? {
        mrpack::PackKind::Modrinth => prepare_modrinth(&inst, recorded, old_pack, new_pack, &new_origin, opts)?,
        mrpack::PackKind::CurseForge => {
            let prepared = curseforge::prepare(&inst, recorded, old_pack, new_pack, &new_origin, cf, opts).await?;
            curseforge::refuse_duplicates(&prepared, &game_dir)?;
            prepared
        }
        mrpack::PackKind::Plain => return Err(invalid("format de modpack inconnu")),
    };
    apply_prepared(instances, &inst, prepared, new_origin, store_root, index, opts, &progress, &cancel).await
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn apply_prepared(
    instances: &InstanceStore,
    inst: &Instance,
    prepared: Prepared,
    new_origin: PackOrigin,
    store_root: &Path,
    index: &StoreIndex,
    opts: &ImportOptions,
    progress: &Option<FilesProgress>,
    cancel: &Arc<AtomicBool>,
) -> Result<ModpackUpdateResult> {
    let Prepared {
        old_state,
        new_state,
        new_idx,
        mut new_zip,
        new_over,
        opts: own_opts,
    } = prepared;
    let opts = own_opts.as_ref().unwrap_or(opts);
    let (old_state, new_state, new_idx, new_zip, new_over) = (
        &old_state,
        &new_state,
        &new_idx,
        &mut new_zip,
        &new_over[..],
    );
    let game_dir = instances.game_dir(&inst.id)?;
    let disk_prov = mods::read_provenance(&game_dir);
    let mut plan = plan(old_state, new_state, &game_dir)?;

    let id = new_backup_id();
    let bdir = backup_root(&game_dir).join(&id);
    // Phase A : tout ce qui est nécessaire est téléchargé et extrait hors de l'instance. Un échec ici ne change rien.
    let wanted: Vec<&Item> = plan
        .actions
        .iter()
        .filter_map(|a| match a {
            Action::Add { item, .. } | Action::Replace { item, .. } => Some(item),
            Action::Remove { .. } => None,
        })
        .collect();
    let staged = (|| -> Result<()> {
        let subset: Vec<OverrideEntry> = wanted
            .iter()
            .filter_map(|it| match it.source {
                Source::Over(i) => Some(OverrideEntry {
                    index: new_over[i].index,
                    rel: new_over[i].rel.clone(),
                }),
                Source::Index(_) => None,
            })
            .collect();
        if !subset.is_empty() {
            mrpack::extract_overrides(new_zip, &subset, &bdir.join("staged"), opts, cancel)?;
        }
        Ok(())
    })();
    let objects = match staged {
        Ok(()) => download_all(new_idx, &wanted, store_root, index, opts, progress, cancel).await,
        Err(e) => Err(e),
    };
    let (objects, missing) = match objects {
        Ok(o) => o,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&bdir);
            return Err(e);
        }
    };

    // Un fichier introuvable n'est ni ajouté ni remplacé : l'ancien reste en place, le joueur pose le nouveau.
    // De même, l'ancienne version d'un mod dont la nouvelle est introuvable (autre nom de fichier, même projet) reste :
    // sinon le joueur n'aurait plus du tout ce mod.
    let missing_projects = missing_projects(new_state, &missing);
    plan.actions.retain(|a| match a {
        Action::Add { item, .. } | Action::Replace { item, .. } => !missing.contains(&item.path),
        Action::Remove { old } => !old.project.as_ref().is_some_and(|p| missing_projects.contains(p)),
    });
    let (added, removed, replaced) = plan.counts();

    // Phase B : modification de l'instance, manifeste écrit avant le premier déplacement.
    let new_loader = LoaderSpec {
        loader: new_idx.loader.loader,
        version: new_idx.loader.version.clone(),
    };
    let mut mf = Manifest {
        id: id.clone(),
        from: inst.pack.clone(),
        to: new_origin.clone(),
        loader_from: inst.loader.clone(),
        loader_to: new_loader.clone(),
        moved: Vec::new(),
        added: Vec::new(),
    };
    for a in &plan.actions {
        match a {
            Action::Remove { old } | Action::Replace { old, .. } => mf.moved.push(Moved {
                rel: old.actual.clone(),
                provenance: jar_key(&old.actual).and_then(|k| disk_prov.get(&k).cloned()),
            }),
            Action::Add { .. } => {}
        }
        match a {
            Action::Add { item, dest } | Action::Replace { item, dest, .. } => {
                mf.added.push(Added {
                    rel: dest.clone(),
                    // Sans empreinte de référence (CurseForge), celle de l'objet posé : l'annulation doit le reconnaître.
                    digest: match objects.get(&item.path) {
                        Some(o) if item.digest.hex.is_empty() => Digest { alg: Alg::Sha256, hex: o.sha256.to_ascii_lowercase() },
                        _ => item.digest.clone(),
                    },
                    sha256: objects.get(&item.path).map(|o| o.sha256.clone()),
                    prov_key: jar_key(dest).filter(|_| item.provenance.is_some()),
                })
            }
            Action::Remove { .. } => {}
        }
    }
    let fail = |e: Error, mf: &Manifest| -> Error {
        let (errors, _) = revert(&game_dir, &bdir, mf, index, true);
        if errors.is_empty() {
            let _ = std::fs::remove_dir_all(&bdir);
        } else {
            tracing::warn!("mise à jour du modpack : remise en état incomplète ({}) ; sauvegarde conservée : {}", errors.join(" ; "), bdir.display());
        }
        e
    };
    if let Err(e) = write_manifest(&bdir, &mf).and_then(|()| record::save_to_backup(&game_dir, &bdir)) {
        return Err(fail(e, &mf));
    }
    let run = || -> Result<()> {
        for m in &mf.moved {
            let src = disk_path(&game_dir, &m.rel);
            let dst = bdir
                .join("files")
                .join(m.rel.split('/').collect::<PathBuf>());
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::rename(&src, &dst)?;
        }
        let forget: Vec<String> = mf.moved.iter().filter_map(|m| jar_key(&m.rel)).collect();
        mods::forget_provenance(&game_dir, &forget)?;
        let mut record: Vec<(String, Provenance)> = Vec::new();
        for a in &plan.actions {
            let (Action::Add { item, dest } | Action::Replace { item, dest, .. }) = a else {
                continue;
            };
            match item.source {
                Source::Index(_) => {
                    let obj = objects.get(&item.path).ok_or_else(|| {
                        Error::Other(format!("{} : fichier non téléchargé", item.path))
                    })?;
                    link_into(&game_dir, dest, &store_root.join(&obj.rel_path))?;
                    index.add_ref(&obj.sha256)?;
                }
                Source::Over(_) => {
                    let from = bdir
                        .join("staged")
                        .join(item.path.split('/').collect::<PathBuf>());
                    let to = disk_path(&game_dir, dest);
                    if let Some(parent) = to.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::rename(&from, &to)?;
                }
            }
            if let (Some(k), Some(p)) = (jar_key(dest), &item.provenance) {
                record.push((k, p.clone()));
            }
        }
        mods::record_provenance(&game_dir, &record)?;
        let _ = std::fs::remove_dir_all(bdir.join("staged"));
        instances.set_pack(&inst.id, Some(new_origin.clone()))?;
        if new_loader != inst.loader {
            instances.set_loader(&inst.id, new_loader.clone())?;
        }
        record::write_state(&game_dir, &new_origin, new_state, old_state, &objects, &missing)
    };
    if let Err(e) = run() {
        // Ce qui n'a pas été posé n'existe pas encore : `revert` tolère une opération inachevée.
        let _ = instances.set_pack(&inst.id, inst.pack.clone());
        let _ = instances.set_loader(&inst.id, inst.loader.clone());
        return Err(fail(e, &mf));
    }
    prune_backups(&game_dir);
    Ok(ModpackUpdateResult {
        added,
        removed,
        replaced,
        kept: plan.kept,
        missing,
        backup_id: id,
    })
}
