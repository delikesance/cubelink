//! Identité ed25519 de l'utilisateur : génération, stockage et signature.

use anyhow::{bail, Context, Result};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

pub use ed25519_dalek::Signature as IdentitySignature;

const KEY_FILE: &str = "identity.key";
const KEY_LEN: usize = 32;

/// Paire de clés de l'utilisateur. La clé secrète n'est jamais affichée.
pub struct Identity {
    signing: SigningKey,
}

impl fmt::Debug for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Identity")
            .field("public_key", &self.public_key_hex())
            .field("secret", &"<masquée>")
            .finish()
    }
}

impl Identity {
    /// Charge la clé de `dir`, ou la crée au premier appel.
    /// Un fichier corrompu ou de mauvaise taille est une erreur (jamais de régénération).
    pub fn load_or_create(dir: &Path) -> Result<Self> {
        let path = dir.join(KEY_FILE);
        if path.exists() {
            let bytes = std::fs::read(&path)
                .with_context(|| format!("lecture de la clé {}", path.display()))?;
            let arr: [u8; KEY_LEN] = bytes.as_slice().try_into().map_err(|_| {
                anyhow::anyhow!(
                    "fichier de clé invalide ({}) : {} octets au lieu de {KEY_LEN}",
                    path.display(),
                    bytes.len()
                )
            })?;
            return Ok(Self { signing: SigningKey::from_bytes(&arr) });
        }
        std::fs::create_dir_all(dir)
            .with_context(|| format!("création du dossier {}", dir.display()))?;
        let signing = SigningKey::generate(&mut rand::rngs::OsRng);
        write_atomic(&path, &signing.to_bytes())?;
        Ok(Self { signing })
    }

    /// Octets de la clé secrète, pour dériver la clé de l'endpoint réseau (une seule identité pour la
    /// signature et le transport). À ne jamais journaliser ni envoyer au front.
    pub fn secret_bytes(&self) -> [u8; 32] {
        self.signing.to_bytes()
    }

    pub fn public_key(&self) -> VerifyingKey {
        self.signing.verifying_key()
    }

    /// Clé publique en hexadécimal minuscule (64 caractères).
    pub fn public_key_hex(&self) -> String {
        hex::encode(self.public_key().to_bytes())
    }

    pub fn sign(&self, msg: &[u8]) -> Signature {
        self.signing.sign(msg)
    }

    /// Vérifie une signature ; `false` si la clé hex est invalide ou la signature fausse.
    pub fn verify(public_key_hex: &str, msg: &[u8], sig: &Signature) -> bool {
        let Ok(bytes) = hex::decode(public_key_hex) else { return false };
        let Ok(arr) = <[u8; 32]>::try_from(bytes.as_slice()) else { return false };
        let Ok(key) = VerifyingKey::from_bytes(&arr) else { return false };
        key.verify(msg, sig).is_ok()
    }
}

fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let mut tmp: PathBuf = path.to_path_buf();
    tmp.set_extension("tmp");
    let res = (|| -> Result<()> {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp).context("création du fichier temporaire")?;
        f.write_all(data)?;
        f.sync_all()?;
        drop(f);
        // Windows : le profil utilisateur est déjà privé par ACL héritées ; pas de permission fine ici.
        if path.exists() {
            bail!("le fichier de clé existe déjà : {}", path.display());
        }
        std::fs::rename(&tmp, path).context("renommage du fichier de clé")?;
        Ok(())
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_after_reload() {
        let d = tempfile::tempdir().unwrap();
        let a = Identity::load_or_create(d.path()).unwrap();
        let b = Identity::load_or_create(d.path()).unwrap();
        assert_eq!(a.public_key_hex(), b.public_key_hex());
        let h = a.public_key_hex();
        assert_eq!(h.len(), 64);
        assert_eq!(h, h.to_lowercase());
    }

    #[test]
    fn sign_and_verify() {
        let d = tempfile::tempdir().unwrap();
        let id = Identity::load_or_create(d.path()).unwrap();
        let sig = id.sign(b"hello");
        assert!(Identity::verify(&id.public_key_hex(), b"hello", &sig));
        assert!(!Identity::verify(&id.public_key_hex(), b"hellO", &sig));
    }

    #[test]
    fn tampered_signature_rejected() {
        let d = tempfile::tempdir().unwrap();
        let id = Identity::load_or_create(d.path()).unwrap();
        let mut bytes = id.sign(b"msg").to_bytes();
        bytes[0] ^= 0xff;
        let bad = Signature::from_bytes(&bytes);
        assert!(!Identity::verify(&id.public_key_hex(), b"msg", &bad));
    }

    #[test]
    fn corrupted_file_errors_and_is_kept() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join(KEY_FILE);
        std::fs::write(&p, b"abc").unwrap();
        assert!(Identity::load_or_create(d.path()).is_err());
        assert_eq!(std::fs::read(&p).unwrap(), b"abc");
    }

    #[test]
    fn invalid_hex_rejected() {
        let d = tempfile::tempdir().unwrap();
        let id = Identity::load_or_create(d.path()).unwrap();
        let sig = id.sign(b"m");
        assert!(!Identity::verify("zz", b"m", &sig));
        assert!(!Identity::verify("abcd", b"m", &sig));
        assert!(!Identity::verify("", b"m", &sig));
    }

    #[test]
    fn debug_hides_secret() {
        let d = tempfile::tempdir().unwrap();
        let id = Identity::load_or_create(d.path()).unwrap();
        let secret = hex::encode(id.signing.to_bytes());
        assert!(!format!("{id:?}").contains(&secret));
    }
}
