//! Instantané des fichiers « volatils » (config) partagés : le jeu et les mods les réécrivent pendant
//! que l'ami télécharge. On les COPIE au moment du manifeste puis on hache la COPIE : le manifeste et
//! les octets servis correspondent exactement, quoi que fasse le jeu ensuite.

use std::io::Read;
use std::path::Path;

use core_lib::types::{Error, Manifest, Result};
use sha2::{Digest, Sha256};

/// Préfixe (dossier) copié dans l'instantané ; les mods restent servis depuis le jeu (vérifiés à la demande).
pub const SNAPSHOT_PREFIX: &str = "config/";

fn hash_file(p: &Path) -> std::io::Result<(String, u64)> {
    let mut f = std::fs::File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut n_total = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        n_total += n as u64;
    }
    Ok((hex::encode(h.finalize()), n_total))
}

/// Copie les fichiers `config/…` du manifeste dans `snap_dir` (recréé à vide) et met à jour leur
/// empreinte et leur taille d'après la copie. Un fichier qui disparaît ou ne peut pas être copié
/// (verrouillé par le jeu) est retiré du manifeste : un fichier de config manquant n'est pas bloquant.
pub fn snapshot_config(manifest: &mut Manifest, game_dir: &Path, snap_dir: &Path) -> Result<()> {
    let _ = std::fs::remove_dir_all(snap_dir);
    std::fs::create_dir_all(snap_dir).map_err(Error::from)?;
    manifest.files.retain_mut(|f| {
        if !f.path.starts_with(SNAPSHOT_PREFIX) {
            return true;
        }
        let (src, dst) = (game_dir.join(&f.path), snap_dir.join(&f.path));
        let copied = dst.parent().map(std::fs::create_dir_all).transpose().and_then(|_| std::fs::copy(&src, &dst));
        match copied.and_then(|_| hash_file(&dst)) {
            Ok((sha, size)) => {
                f.sha256 = sha;
                f.size = size;
                f.sha1 = None;
                true
            }
            Err(_) => false,
        }
    });
    Ok(())
}

/// Hache un fichier (sha256 hex) : sert à vérifier un jar avant de l'importer.
pub fn sha256_of(p: &Path) -> std::io::Result<String> {
    hash_file(p).map(|(h, _)| h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_lib::types::{Loader, LoaderSpec, ManifestFile};

    fn mf(path: &str, data: &[u8]) -> ManifestFile {
        ManifestFile { path: path.into(), size: data.len() as u64, sha256: hex::encode(Sha256::digest(data)), sha1: None, cf_fingerprint: None }
    }

    #[test]
    fn config_is_copied_and_rehashed_from_the_copy() {
        let (game, snap) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::create_dir_all(game.path().join("config/sub")).unwrap();
        std::fs::create_dir_all(game.path().join("mods")).unwrap();
        std::fs::write(game.path().join("config/a.json"), b"new content").unwrap();
        std::fs::write(game.path().join("config/sub/b.json"), b"b").unwrap();
        std::fs::write(game.path().join("mods/m.jar"), b"jar").unwrap();
        // Le manifeste a été calculé AVANT que a.json change ; b.json a disparu ; m.jar n'est pas copié.
        let mut m = Manifest {
            name: "n".into(),
            minecraft: "1.21.1".into(),
            loader: LoaderSpec { loader: Loader::Vanilla, version: String::new() },
            java_major: 21,
            files: vec![mf("config/a.json", b"old"), mf("config/gone.json", b"x"), mf("config/sub/b.json", b"b"), mf("mods/m.jar", b"jar")],
        };
        snapshot_config(&mut m, game.path(), snap.path()).unwrap();
        let paths: Vec<_> = m.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["config/a.json", "config/sub/b.json", "mods/m.jar"]);
        assert_eq!(m.files[0].sha256, hex::encode(Sha256::digest(b"new content")));
        assert_eq!(m.files[0].size, 11);
        // Le jeu réécrit encore : la copie, elle, ne bouge pas.
        std::fs::write(game.path().join("config/a.json"), b"rewritten again").unwrap();
        assert_eq!(sha256_of(&snap.path().join("config/a.json")).unwrap(), m.files[0].sha256);
        assert!(!snap.path().join("mods").exists());
    }
}
