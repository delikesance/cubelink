//! Tâche S3 : placement d'un objet du cache dans une instance (hardlink, sinon copie).
//!
//! Sémantique : un hardlink partage les mêmes données que l'objet du cache. Modifier le fichier
//! de l'instance modifie donc aussi l'objet du magasin (et inversement). Les objets du magasin
//! sont adressés par contenu : l'appelant doit traiter les fichiers liés comme en lecture seule,
//! ou utiliser une copie pour les fichiers que le jeu réécrit.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::types::{Error, Result};

/// Mode de placement effectivement utilisé.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    HardLink,
    Copy,
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Indique si `a` et `b` désignent le même fichier physique (`false` si l'un n'existe pas).
///
/// Comparaison exacte via la crate `same-file` (identifiant de fichier du système). Une copie n'est jamais prise pour un hardlink.
pub fn same_file(a: &Path, b: &Path) -> bool {
    // Identité réelle du fichier (volume + index sur Windows, dev + inode sur Unix), pas une heuristique sur les dates.
    ::same_file::is_same_file(a, b).unwrap_or(false)
}

/// Valide `rel_path` (séparateurs `/` ou `\`) et le joint à `root`.
fn safe_join(root: &Path, rel_path: &str) -> Result<PathBuf> {
    let bad = |why: &str| Error::Invalid(format!("chemin relatif invalide ({why}) : {rel_path:?}"));
    if rel_path.contains('\0') {
        return Err(bad("octet nul"));
    }
    if rel_path.starts_with('/') || rel_path.starts_with('\\') {
        return Err(bad("chemin absolu"));
    }
    let mut out = root.to_path_buf();
    let mut any = false;
    for seg in rel_path.split(['/', '\\']) {
        match seg {
            "" | "." => continue,
            ".." => return Err(bad("'..'")),
            s if s.contains(':') => return Err(bad("préfixe de lecteur ou flux")),
            s => {
                out.push(s);
                any = true;
            }
        }
    }
    if !any {
        return Err(bad("vide"));
    }
    Ok(out)
}

/// Place `src` à `root/rel_path` après validation de `rel_path`.
pub fn link_into(root: &Path, rel_path: &str, src: &Path) -> Result<LinkKind> {
    let dst = safe_join(root, rel_path)?;
    link_or_copy(src, &dst)
}

/// Copie `src` dans `root/rel_path`, jamais de lien : pour les fichiers que le jeu peut réécrire en place (configs),
/// qui altéreraient sinon l'objet partagé du cache sans changer son hash.
pub fn copy_into(root: &Path, rel_path: &str, src: &Path) -> Result<LinkKind> {
    let dst = safe_join(root, rel_path)?;
    link_or_copy_with(src, &dst, &|_, _| Err(io::Error::other("copie demandée")))
}

/// Place `src` à `dst` : hardlink si possible, sinon copie.
pub fn link_or_copy(src: &Path, dst: &Path) -> Result<LinkKind> {
    link_or_copy_with(src, dst, &|s, d| fs::hard_link(s, d))
}

/// Comme [`link_or_copy`] avec une fonction de lien injectable (tests : simule un autre volume).
pub fn link_or_copy_with(
    src: &Path,
    dst: &Path,
    linker: &dyn Fn(&Path, &Path) -> io::Result<()>,
) -> Result<LinkKind> {
    if !fs::metadata(src)?.is_file() {
        return Err(Error::Invalid(format!("la source n'est pas un fichier : {}", src.display())));
    }
    if dst.exists() && same_file(src, dst) {
        return Ok(LinkKind::HardLink);
    }
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    let name = dst.file_name().and_then(|n| n.to_str()).unwrap_or("objet");
    let tmp = dst.with_file_name(format!(
        ".{name}.cubelink-tmp-{}-{}",
        std::process::id(),
        TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let kind = if linker(src, &tmp).is_ok() {
        LinkKind::HardLink
    } else {
        let _ = fs::remove_file(&tmp);
        if let Err(e) = fs::copy(src, &tmp) {
            let _ = fs::remove_file(&tmp);
            return Err(e.into());
        }
        LinkKind::Copy
    };
    if let Err(e) = fs::rename(&tmp, dst) {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("obj");
        fs::write(&src, b"contenu").unwrap();
        (d, src)
    }

    #[test]
    fn hardlink_ok_et_semantique_partagee() {
        let (d, src) = setup();
        let dst = d.path().join("inst").join("a.bin");
        assert_eq!(link_or_copy(&src, &dst).unwrap(), LinkKind::HardLink);
        assert_eq!(fs::read(&dst).unwrap(), b"contenu");
        assert!(same_file(&src, &dst));
        // Documenté : un hardlink partage les données, modifier l'un modifie l'autre.
        fs::write(&src, b"autre").unwrap();
        assert_eq!(fs::read(&dst).unwrap(), b"autre");
    }

    #[test]
    fn repli_copie_si_lien_echoue() {
        let (d, src) = setup();
        let dst = d.path().join("x/y/b.bin");
        let fail = |_: &Path, _: &Path| Err(io::Error::other("autre volume"));
        assert_eq!(link_or_copy_with(&src, &dst, &fail).unwrap(), LinkKind::Copy);
        assert_eq!(fs::read(&dst).unwrap(), b"contenu");
        assert!(!same_file(&src, &dst));
        fs::write(&src, b"autre").unwrap();
        assert_eq!(fs::read(&dst).unwrap(), b"contenu");
        // aucun fichier temporaire résiduel
        assert_eq!(fs::read_dir(dst.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn chemins_malveillants() {
        let (d, src) = setup();
        let root = d.path().join("root");
        for p in ["../x", "a/../../x", "/etc/x", "\\x", "C:\\x", "C:/x", "a\\..\\x", "", ".", "a\0b"] {
            assert!(matches!(link_into(&root, p, &src), Err(Error::Invalid(_))), "{p:?}");
        }
        assert!(!root.exists());
        assert_eq!(link_into(&root, "mods/./a.jar", &src).unwrap(), LinkKind::HardLink);
        assert!(root.join("mods").join("a.jar").is_file());
    }

    #[test]
    fn remplacement_idempotent() {
        let (d, src) = setup();
        let dst = d.path().join("i/f.bin");
        link_or_copy(&src, &dst).unwrap();
        assert_eq!(link_or_copy(&src, &dst).unwrap(), LinkKind::HardLink);
        // fichier différent existant : remplacé
        fs::remove_file(&dst).unwrap();
        fs::write(&dst, b"vieux").unwrap();
        link_or_copy(&src, &dst).unwrap();
        assert_eq!(fs::read(&dst).unwrap(), b"contenu");
        assert!(same_file(&src, &dst));
        // copie existante (non liée) : remplacée aussi
        let fail = |_: &Path, _: &Path| Err(io::Error::other("x"));
        fs::remove_file(&dst).unwrap();
        fs::write(&dst, b"vieux").unwrap();
        assert_eq!(link_or_copy_with(&src, &dst, &fail).unwrap(), LinkKind::Copy);
        assert_eq!(fs::read(&dst).unwrap(), b"contenu");
    }

    #[test]
    fn dossier_parent_cree_et_source_absente() {
        let (d, src) = setup();
        let dst = d.path().join("a/b/c/d.bin");
        link_or_copy(&src, &dst).unwrap();
        assert!(dst.is_file());
        assert!(link_or_copy(&d.path().join("nope"), &d.path().join("z")).is_err());
        assert!(!same_file(&src, &d.path().join("nope")));
    }
}
