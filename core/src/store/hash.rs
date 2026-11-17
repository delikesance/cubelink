//! SHA-256 en flux et vérification d'intégrité.

use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::types::{Error, Result};

/// Taille des blocs de lecture : 1 Mo.
const CHUNK_SIZE: usize = 1024 * 1024;

/// SHA-256 d'un lecteur, en flux.
pub fn sha256_reader<R: Read>(mut reader: R) -> Result<String> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK_SIZE];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// SHA-256 d'un fichier.
pub fn sha256_file(path: &Path) -> Result<String> {
    sha256_reader(std::fs::File::open(path)?)
}

/// Vérifie le SHA-256 d'un fichier (comparaison insensible à la casse).
pub fn verify_file(path: &Path, expected_sha256: &str) -> Result<()> {
    if expected_sha256.len() != 64 || !expected_sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::Invalid(format!("hash SHA-256 mal formé : {expected_sha256:?}")));
    }
    let expected = expected_sha256.to_ascii_lowercase();
    let actual = sha256_file(path)?;
    if expected == actual {
        Ok(())
    } else {
        Err(Error::Integrity { expected, actual })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const ABC_SHA: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    const EMPTY_SHA: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn known_vectors() {
        assert_eq!(sha256_reader(&b""[..]).unwrap(), EMPTY_SHA);
        assert_eq!(sha256_reader(&b"abc"[..]).unwrap(), ABC_SHA);
    }

    fn big_file() -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        let block: Vec<u8> = (0..CHUNK_SIZE).map(|i| (i % 251) as u8).collect();
        for _ in 0..64 {
            f.write_all(&block).unwrap();
        }
        f.flush().unwrap();
        f
    }

    #[test]
    fn big_file_verified() {
        let f = big_file();
        let sha = sha256_file(f.path()).unwrap();
        assert_eq!(sha.len(), 64);
        verify_file(f.path(), &sha).unwrap();
        verify_file(f.path(), &sha.to_uppercase()).unwrap();
    }

    #[test]
    fn tampered_file_rejected() {
        let mut f = big_file();
        let good = sha256_file(f.path()).unwrap();
        f.write_all(b"x").unwrap();
        f.flush().unwrap();
        match verify_file(f.path(), &good) {
            Err(Error::Integrity { expected, actual }) => {
                assert_eq!(expected, good);
                assert_ne!(actual, good);
            }
            other => panic!("attendu Integrity, obtenu {other:?}"),
        }
    }

    #[test]
    fn malformed_hash_rejected() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"abc").unwrap();
        f.flush().unwrap();
        for bad in ["", "abc", &"g".repeat(64), &format!("{ABC_SHA}0")] {
            assert!(matches!(verify_file(f.path(), bad), Err(Error::Invalid(_))));
        }
        verify_file(f.path(), ABC_SHA).unwrap();
    }
}
