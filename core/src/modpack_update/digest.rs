//! Empreintes des fichiers d'un pack et du disque.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Alg {
    Sha1,
    Sha512,
}

/// Empreinte d'un fichier du pack. Deux empreintes ne se comparent que si elles ont le même algorithme.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Digest {
    pub(super) alg: Alg,
    pub(super) hex: String,
}

impl Digest {
    pub(super) fn of_index(f: &MrpackFile) -> Digest {
        if f.sha512.is_empty() {
            Digest {
                alg: Alg::Sha1,
                hex: f.sha1.to_ascii_lowercase(),
            }
        } else {
            Digest {
                alg: Alg::Sha512,
                hex: f.sha512.to_ascii_lowercase(),
            }
        }
    }

    /// Empreinte du fichier du disque avec le même algorithme que `self` (flux, pas de lecture complète en mémoire).
    pub(super) fn disk_matches(&self, path: &Path) -> Result<bool> {
        Ok(hash_file(path, self.alg)? == self.hex)
    }
}

pub(super) fn hash_file(path: &Path, alg: Alg) -> Result<String> {
    match alg {
        Alg::Sha1 => Ok(crate::unknown_mods::sha1_file(path)?),
        Alg::Sha512 => {
            let mut f = std::fs::File::open(path)?;
            let mut h = Sha512::new();
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = f.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                h.update(&buf[..n]);
            }
            Ok(hex::encode(h.finalize()))
        }
    }
}

/// SHA-512 de chaque override (même ordre que `entries`), lu en flux, taille bornée par celle déclarée.
pub(super) fn hash_overrides(
    archive: &mut ZipArchive<std::fs::File>,
    entries: &[OverrideEntry],
) -> Result<Vec<String>> {
    let mut out = Vec::with_capacity(entries.len());
    for e in entries {
        let entry = archive
            .by_index(e.index)
            .map_err(|x| invalid(format!("{} : {x}", e.rel)))?;
        let declared = entry.size();
        let mut reader = entry.take(declared + 1);
        let mut h = Sha512::new();
        let mut buf = vec![0u8; 64 * 1024];
        let mut total = 0u64;
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            total += n as u64;
            h.update(&buf[..n]);
        }
        if total > declared {
            return Err(invalid(format!(
                "{} : taille réelle supérieure à celle déclarée (zip bomb)",
                e.rel
            )));
        }
        out.push(hex::encode(h.finalize()));
    }
    Ok(out)
}
