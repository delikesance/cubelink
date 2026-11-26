//! Plan d'une mise à jour : ancien pack, nouveau pack et disque comparés.

use super::*;

/// Fichier présent sur le disque : nom réel (avec `.disabled` pour un mod désactivé).
pub(super) enum Probe {
    Absent,
    File {
        actual: String,
        disabled: bool,
    },
    /// Quelque chose d'autre qu'un fichier (dossier, lien) au chemin du pack : on n'y touche pas.
    Other,
}

pub(super) fn probe(game_dir: &Path, rel: &str) -> Probe {
    let at = |r: &str| game_dir.join(r.split('/').collect::<PathBuf>());
    match std::fs::symlink_metadata(at(rel)) {
        Ok(m) if m.is_file() => {
            return Probe::File {
                actual: rel.to_string(),
                disabled: false,
            }
        }
        Ok(_) => return Probe::Other,
        Err(_) => {}
    }
    if mrpack::mod_jar_name(rel).is_some() {
        let dis = format!("{rel}.disabled");
        match std::fs::symlink_metadata(at(&dis)) {
            Ok(m) if m.is_file() => {
                return Probe::File {
                    actual: dis,
                    disabled: true,
                }
            }
            Ok(_) => return Probe::Other,
            Err(_) => {}
        }
    }
    Probe::Absent
}

pub(super) fn disk_path(game_dir: &Path, rel: &str) -> PathBuf {
    game_dir.join(rel.split('/').collect::<PathBuf>())
}

#[derive(Debug, Clone)]
pub(super) struct Old {
    /// Nom réel sur le disque.
    pub(super) actual: String,
    pub(super) disabled: bool,
    /// Projet du jar de l'ancien pack (pour recréer un jar désactivé sous sa nouvelle version).
    pub(super) project: Option<Provenance>,
}

#[derive(Debug, Clone)]
pub(super) enum Action {
    /// Nouveau fichier du pack à poser à `dest`.
    Add { item: Item, dest: String },
    /// Fichier du pack changé : l'ancien (`old`) part dans la sauvegarde, le nouveau est posé à `dest`.
    Replace { item: Item, dest: String, old: Old },
    /// Fichier retiré du pack : il part dans la sauvegarde.
    Remove { old: Old },
}

#[derive(Debug, Default)]
pub(super) struct Plan {
    pub(super) actions: Vec<Action>,
    pub(super) kept: Vec<String>,
}

impl Plan {
    pub(super) fn counts(&self) -> (u32, u32, u32) {
        let c = |f: fn(&Action) -> bool| self.actions.iter().filter(|a| f(a)).count() as u32;
        (
            c(|a| matches!(a, Action::Add { .. })),
            c(|a| matches!(a, Action::Remove { .. })),
            c(|a| matches!(a, Action::Replace { .. })),
        )
    }
}

/// Compare l'ancien pack, le nouveau pack et le disque. Lit le disque (empreintes) ; ne modifie rien.
pub(super) fn plan(old: &State, new: &State, game_dir: &Path) -> Result<Plan> {
    let mut out = Plan::default();
    let mut kept: Vec<String> = Vec::new();
    for (key, n) in &new.items {
        if protected(&n.path) {
            continue;
        }
        let old_item = old.items.get(key);
        if old_item.is_some_and(|o| o.digest == n.digest) {
            continue; // inchangé entre les deux versions : le disque reste comme il est
        }
        match (probe(game_dir, &n.path), old_item) {
            (Probe::Absent, _) => out.actions.push(Action::Add {
                item: n.clone(),
                dest: n.path.clone(),
            }),
            (Probe::Other, _) => kept.push(n.path.clone()),
            (Probe::File { actual, disabled }, o) => {
                let on_disk = disk_path(game_dir, &actual);
                if n.digest.disk_matches(&on_disk)? {
                    continue; // déjà la bonne version
                }
                match o {
                    Some(o) if o.digest.disk_matches(&on_disk)? => {
                        out.actions.push(Action::Replace {
                            item: n.clone(),
                            dest: actual.clone(),
                            old: Old {
                                actual,
                                disabled,
                                project: o.provenance.clone(),
                            },
                        })
                    }
                    // Modifié par le joueur, ou fichier du joueur au même chemin : il garde sa version.
                    _ => kept.push(n.path.clone()),
                }
            }
        }
    }
    for (key, o) in &old.items {
        if new.items.contains_key(key) || protected(&o.path) {
            continue;
        }
        match probe(game_dir, &o.path) {
            Probe::Absent => {}
            Probe::Other => kept.push(o.path.clone()),
            Probe::File { actual, disabled } => {
                if o.digest.disk_matches(&disk_path(game_dir, &actual))? {
                    out.actions.push(Action::Remove {
                        old: Old {
                            actual,
                            disabled,
                            project: o.provenance.clone(),
                        },
                    });
                } else {
                    kept.push(o.path.clone());
                }
            }
        }
    }

    // Un mod désactivé par le joueur reste désactivé dans sa nouvelle version (même projet, autre nom de fichier).
    let disabled_projects: HashSet<Provenance> = out
        .actions
        .iter()
        .filter_map(|a| match a {
            Action::Remove { old } | Action::Replace { old, .. } if old.disabled => {
                old.project.clone()
            }
            _ => None,
        })
        .collect();
    // Noms (de base) des jars qui quittent `mods/` avec cette mise à jour.
    let leaving: HashSet<String> = out
        .actions
        .iter()
        .filter_map(|a| match a {
            Action::Remove { old } | Action::Replace { old, .. } => {
                mrpack::mod_jar_name(old.actual.trim_end_matches(".disabled")).map(str::to_string)
            }
            _ => None,
        })
        .collect();
    let mut actions = Vec::with_capacity(out.actions.len());
    for a in std::mem::take(&mut out.actions) {
        match a {
            Action::Add { item, dest } => {
                let name = mrpack::mod_jar_name(&item.path).map(str::to_string);
                if let (Some(name), Some(p)) = (&name, &item.provenance) {
                    // Le joueur a déjà une autre version de ce mod (installée depuis le catalogue, par exemple) : l'ajouter
                    // donnerait deux jars du même mod, que le jeu refuse. Il garde la sienne.
                    let other = mods::files_of_project(game_dir, &p.source, &p.id)
                        .into_iter()
                        .find(|f| {
                            f != name
                                && !leaving.contains(f)
                                && !matches!(probe(game_dir, &format!("mods/{f}")), Probe::Absent)
                        });
                    if let Some(other) = other {
                        kept.push(format!("mods/{other}"));
                        continue;
                    }
                    if disabled_projects.contains(p) {
                        actions.push(Action::Add {
                            item: item.clone(),
                            dest: format!("{dest}.disabled"),
                        });
                        continue;
                    }
                }
                actions.push(Action::Add { item, dest });
            }
            other => actions.push(other),
        }
    }
    out.actions = actions;
    kept.sort();
    kept.dedup();
    out.kept = kept;
    Ok(out)
}
