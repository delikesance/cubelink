//! Application d'une mise à jour (téléchargement, puis modification de l'instance).

use super::*;

/// Télécharge dans le magasin (sans toucher à l'instance) les fichiers d'index à poser. Clé : chemin du fichier.
pub(super) async fn download_all(
    pack: &MrpackIndex,
    wanted: &[&Item],
    store_root: &Path,
    index: &StoreIndex,
    opts: &ImportOptions,
    progress: &Option<FilesProgress>,
    cancel: &Arc<AtomicBool>,
) -> Result<BTreeMap<String, StoredObject>> {
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
    let mut results = stream::iter(files.iter().copied())
        .map(|(path, f)| {
            let dl = &dl;
            async move { (path, mrpack::fetch_file(f, store_root, index, dl).await) }
        })
        .buffer_unordered(opts.concurrency.max(1));
    while let Some((path, res)) = results.next().await {
        got.insert(path.to_string(), res?);
        done += 1;
        if let Some(p) = progress {
            p(done, total);
        }
    }
    drop(results);
    if cancel.load(Ordering::SeqCst) {
        return Err(Error::Cancelled);
    }
    Ok(got)
}

/// Les deux packs lus et validés : tout ce qu'il faut pour planifier, sans avoir rien téléchargé ni modifié.
pub(super) struct Prepared {
    pub(super) old_state: State,
    pub(super) new_state: State,
    pub(super) new_idx: MrpackIndex,
    pub(super) new_zip: ZipArchive<std::fs::File>,
    pub(super) new_over: Vec<OverrideEntry>,
}

pub(super) fn prepare(
    inst: &Instance,
    old_pack: &Path,
    new_pack: &Path,
    new_origin: &PackOrigin,
    opts: &ImportOptions,
) -> Result<Prepared> {
    let Some(current) = inst.pack.as_ref().filter(|o| o.source == "modrinth") else {
        return Err(invalid(
            "cette instance n'a pas d'origine Modrinth connue : la mise à jour n'est pas possible",
        ));
    };
    if new_origin.source != "modrinth" || new_origin.project_id != current.project_id {
        return Err(invalid(
            "cette version n'appartient pas au modpack installé",
        ));
    }
    let mut old_zip = mrpack::open_zip(old_pack)?;
    let mut new_zip = mrpack::open_zip(new_pack)?;
    let old_idx = mrpack::parse_index(old_pack)?;
    let new_idx = mrpack::parse_index(new_pack)?;
    for f in &new_idx.files {
        for u in &f.downloads {
            mrpack::check_url(u, opts)?;
        }
    }
    if new_idx.minecraft != inst.minecraft || new_idx.loader.loader != inst.loader.loader {
        return Err(invalid(
            "cette version change la version de Minecraft ou le loader : installe-la comme nouvelle instance pour garder tes mondes et tes mods",
        ));
    }
    let layout = Layout::mrpack();
    let old_over = mrpack::plan_overrides(&mut old_zip, opts, &layout)?;
    let new_over = mrpack::plan_overrides(&mut new_zip, opts, &layout)?;
    let old_hashes = hash_overrides(&mut old_zip, &old_over)?;
    let new_hashes = hash_overrides(&mut new_zip, &new_over)?;
    Ok(Prepared {
        old_state: state_of(&old_idx, &old_over, &old_hashes),
        new_state: state_of(&new_idx, &new_over, &new_hashes),
        new_idx,
        new_zip,
        new_over,
    })
}

/// Applique la mise à jour de `old_pack` vers `new_pack` sur l'instance `instance_id`.
///
/// `new_origin` : projet et version Modrinth de `new_pack` (enregistrés dans `Instance.pack` en dernier).
/// Les futurs de ce module lisent et écrivent le disque de façon synchrone : l'appelant l'exécute hors du fil
/// principal de l'interface (`spawn_blocking`).
#[allow(clippy::too_many_arguments)]
pub async fn apply(
    instances: &InstanceStore,
    instance_id: &str,
    old_pack: &Path,
    new_pack: &Path,
    new_origin: PackOrigin,
    store_root: &Path,
    index: &StoreIndex,
    opts: &ImportOptions,
    progress: Option<FilesProgress>,
    cancel: Arc<AtomicBool>,
) -> Result<ModpackUpdateResult> {
    let inst = instances.get(instance_id)?;
    let prepared = prepare(&inst, old_pack, new_pack, &new_origin, opts)?;
    apply_prepared(
        instances, &inst, prepared, new_origin, store_root, index, opts, &progress, &cancel,
    )
    .await
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
    } = prepared;
    let (old_state, new_state, new_idx, new_zip, new_over) = (
        &old_state,
        &new_state,
        &new_idx,
        &mut new_zip,
        &new_over[..],
    );
    let game_dir = instances.game_dir(&inst.id)?;
    let disk_prov = mods::read_provenance(&game_dir);
    let plan = plan(old_state, new_state, &game_dir)?;
    let (added, removed, replaced) = plan.counts();

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
    let objects = match objects {
        Ok(o) => o,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&bdir);
            return Err(e);
        }
    };

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
                    digest: item.digest.clone(),
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
    if let Err(e) = write_manifest(&bdir, &mf) {
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
        Ok(())
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
        backup_id: id,
    })
}
