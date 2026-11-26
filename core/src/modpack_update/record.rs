//! Relevé des fichiers installés par le pack : empreintes attendues du pack, écrites à l'installation et après
//! chaque mise à jour ou annulation. Sert d'ancien état sans retélécharger l'ancien pack.
//!
//! Fichier `<game_dir>/.cubelink-pack-record.json` : `{ format: 1, origin, files: { chemin: { digest, provenance } } }`.
//! Les empreintes sont celles ATTENDUES du pack (jamais celles du disque après coup) : sha512 de l'index sinon sha1,
//! sha512 calculé du zip pour un override, sha256 du magasin pour un fichier sans empreinte (CurseForge sans clé).
//!
//! Origine : l'import ne la connaît pas (`Instance.pack` est posé après coup). Choix : le relevé est écrit avec
//! `origin: null` à l'import, puis [`stamp_origin`] la complète ; [`old_state`] exige `origin == Some(inst.pack)`
//! (égalité stricte), sinon le relevé est périmé et ignoré. Tant que l'origine n'est pas posée, il ne sert donc à rien.
//! Un relevé invalide (JSON illisible, autre format, un seul chemin refusé par `validate_rel`) est ignoré en entier.

use super::*;

/// Nom du relevé, à la racine du dossier de jeu (protégé : le préfixe `.cubelink` n'est jamais touché par une mise à jour).
pub const RECORD_FILE: &str = ".cubelink-pack-record.json";
const RECORD_FORMAT: u32 = 1;
/// Nom de la copie du relevé dans une sauvegarde.
const BACKUP_COPY: &str = "record.json";

#[derive(Debug, Serialize, Deserialize)]
struct RecFile {
    digest: Digest,
    provenance: Option<Provenance>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Record {
    format: u32,
    origin: Option<PackOrigin>,
    files: BTreeMap<String, RecFile>,
}

/// Fichier d'index posé à l'import : `sha256` est l'objet du magasin lié (sert pour les fichiers sans empreinte).
pub(crate) struct PlacedFile<'a> {
    pub(crate) file: &'a MrpackFile,
    pub(crate) sha256: &'a str,
}

fn record_path(game_dir: &Path) -> PathBuf {
    game_dir.join(RECORD_FILE)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn save(game_dir: &Path, record: &Record) -> Result<()> {
    let json = serde_json::to_vec_pretty(record).map_err(|e| Error::Other(e.to_string()))?;
    write_atomic(&record_path(game_dir), &json)
}

/// Empreinte d'un fichier d'index à enregistrer : celle de l'index, ou sha256 du magasin s'il n'en a pas.
fn index_digest(f: &MrpackFile, sha256: &str) -> Digest {
    let d = Digest::of_index(f);
    if f.unverified || d.hex.is_empty() {
        Digest { alg: Alg::Sha256, hex: sha256.to_ascii_lowercase() }
    } else {
        d
    }
}

/// Ajoute un fichier ; un chemin déjà présent à la casse près est remplacé (l'override l'emporte).
fn add(files: &mut BTreeMap<String, RecFile>, path: &str, digest: Digest, provenance: Option<Provenance>) {
    let lower = path.to_ascii_lowercase();
    files.retain(|k, _| k.to_ascii_lowercase() != lower);
    files.insert(path.to_string(), RecFile { digest, provenance });
}

/// Écrit le relevé d'une installation (origine inconnue : voir [`stamp_origin`]) à partir des fichiers d'index
/// effectivement posés et des overrides extraits (un override au même chemin l'emporte).
pub(crate) fn write_install_record(
    game_dir: &Path,
    placed: &[PlacedFile<'_>],
    archive: &mut ZipArchive<std::fs::File>,
    overrides: &[OverrideEntry],
) -> Result<()> {
    let hashes = hash_overrides(archive, overrides)?;
    let mut files = BTreeMap::new();
    for p in placed {
        add(&mut files, &p.file.path, index_digest(p.file, p.sha256), p.file.provenance.clone());
    }
    for (o, hex) in overrides.iter().zip(hashes) {
        add(&mut files, &o.rel, Digest { alg: Alg::Sha512, hex }, None);
    }
    save(game_dir, &Record { format: RECORD_FORMAT, origin: None, files })
}

/// Complète l'origine du relevé écrit à l'import. Sans relevé : rien à faire.
pub fn stamp_origin(game_dir: &Path, origin: &PackOrigin) -> Result<()> {
    let Ok(raw) = std::fs::read(record_path(game_dir)) else {
        return Ok(());
    };
    let mut record: Record =
        serde_json::from_slice(&raw).map_err(|e| invalid(format!("relevé du pack illisible : {e}")))?;
    record.origin = Some(origin.clone());
    save(game_dir, &record)
}

/// Réécrit le relevé après une mise à jour : contenu du NOUVEAU pack (`new_state`) et `origin`.
/// `objects` : objets du magasin téléchargés (sha256 des fichiers sans empreinte). `missing` : fichiers non posés, dont
/// l'ancienne version reste sur le disque (on garde alors son entrée de l'ancien état, s'il y en avait une).
pub(super) fn write_state(
    game_dir: &Path,
    origin: &PackOrigin,
    new_state: &State,
    old_state: &State,
    objects: &BTreeMap<String, StoredObject>,
    missing: &[String],
) -> Result<()> {
    let mut files = BTreeMap::new();
    for (key, item) in &new_state.items {
        let kept_old = missing.contains(&item.path);
        let Some(src) = (if kept_old { old_state.items.get(key) } else { Some(item) }) else {
            continue;
        };
        let mut digest = src.digest.clone();
        if digest.hex.is_empty() {
            match objects.get(&item.path) {
                Some(o) if !kept_old => digest = Digest { alg: Alg::Sha256, hex: o.sha256.to_ascii_lowercase() },
                _ => continue,
            }
        }
        add(&mut files, &src.path, digest, src.provenance.clone());
    }
    save(game_dir, &Record { format: RECORD_FORMAT, origin: Some(origin.clone()), files })
}

/// Copie le relevé actuel dans la sauvegarde (sans relevé : pas de copie).
pub(super) fn save_to_backup(game_dir: &Path, bdir: &Path) -> Result<()> {
    let Ok(raw) = std::fs::read(record_path(game_dir)) else {
        return Ok(());
    };
    std::fs::create_dir_all(bdir)?;
    write_atomic(&bdir.join(BACKUP_COPY), &raw)
}

/// Remet le relevé de la sauvegarde ; sans copie (sauvegarde antérieure aux relevés), supprime le relevé.
pub(super) fn restore_from_backup(game_dir: &Path, bdir: &Path) -> Result<()> {
    match std::fs::read(bdir.join(BACKUP_COPY)) {
        Ok(raw) => write_atomic(&record_path(game_dir), &raw),
        Err(_) => match std::fs::remove_file(record_path(game_dir)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        },
    }
}

fn read_record(game_dir: &Path) -> Option<Record> {
    let raw = std::fs::read(record_path(game_dir)).ok()?;
    let record: Record = serde_json::from_slice(&raw).ok()?;
    let valid = record.format == RECORD_FORMAT && record.files.keys().all(|p| mrpack::validate_rel(p).is_ok());
    valid.then_some(record)
}

/// Ancien état tiré du relevé de l'instance, `None` s'il est absent, illisible, invalide ou périmé
/// (origine différente de `Instance.pack`).
pub(super) fn old_state(game_dir: &Path, inst: &Instance) -> Option<State> {
    let record = read_record(game_dir)?;
    let current = inst.pack.as_ref()?;
    if record.origin.as_ref() != Some(current) {
        return None;
    }
    let items = record
        .files
        .into_iter()
        .map(|(path, f)| {
            let item = Item { path: path.clone(), digest: f.digest, source: Source::Over(0), provenance: f.provenance };
            (path.to_ascii_lowercase(), item)
        })
        .collect();
    Some(State { items })
}

/// `true` quand l'appelant doit fournir l'ancien pack à [`apply`] (pas de relevé valable).
pub fn needs_old_pack(instances: &InstanceStore, instance_id: &str) -> Result<bool> {
    let inst = instances.get(instance_id)?;
    let game_dir = instances.game_dir(instance_id)?;
    Ok(old_state(&game_dir, &inst).is_none())
}
