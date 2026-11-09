//! Tâche S1 : hash par blocs (SHA-256 et BLAKE3) en flux, et vérification d'intégrité.

use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::types::{Error, Result};

/// Taille des blocs de lecture : 1 Mo.
pub const CHUNK_SIZE: usize = 1024 * 1024;

/// Résultat d'un calcul simultané des deux hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hashes {
    /// SHA-256, hexadécimal minuscule (64 caractères).
    pub sha256: String,
    /// BLAKE3, hexadécimal minuscule (64 caractères).
    pub blake3: String,
}

// ---------- Synchrone ----------

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

/// BLAKE3 d'un lecteur, en flux.
pub fn blake3_reader<R: Read>(mut reader: R) -> Result<String> {
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; CHUNK_SIZE];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// SHA-256 et BLAKE3 d'un lecteur en une seule lecture.
pub fn hash_reader<R: Read>(mut reader: R) -> Result<Hashes> {
    let mut sha = Sha256::new();
    let mut b3 = blake3::Hasher::new();
    let mut buf = vec![0u8; CHUNK_SIZE];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        sha.update(&buf[..n]);
        b3.update(&buf[..n]);
    }
    Ok(Hashes {
        sha256: hex::encode(sha.finalize()),
        blake3: b3.finalize().to_hex().to_string(),
    })
}

/// SHA-256 d'un fichier.
pub fn sha256_file(path: &Path) -> Result<String> {
    sha256_reader(std::fs::File::open(path)?)
}

/// BLAKE3 d'un fichier.
pub fn blake3_file(path: &Path) -> Result<String> {
    blake3_reader(std::fs::File::open(path)?)
}

/// SHA-256 et BLAKE3 d'un fichier en une seule lecture.
pub fn hash_file(path: &Path) -> Result<Hashes> {
    hash_reader(std::fs::File::open(path)?)
}

/// Vérifie le SHA-256 d'un fichier (comparaison insensible à la casse).
pub fn verify_file(path: &Path, expected_sha256: &str) -> Result<()> {
    let expected = normalize_sha256(expected_sha256)?;
    let actual = sha256_file(path)?;
    compare(expected, actual)
}

// ---------- Asynchrone ----------

/// SHA-256 d'un lecteur asynchrone, en flux.
pub async fn sha256_reader_async<R: AsyncRead + Unpin>(mut reader: R) -> Result<String> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK_SIZE];
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// BLAKE3 d'un lecteur asynchrone, en flux.
pub async fn blake3_reader_async<R: AsyncRead + Unpin>(mut reader: R) -> Result<String> {
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; CHUNK_SIZE];
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// SHA-256 et BLAKE3 d'un lecteur asynchrone en une seule lecture.
pub async fn hash_reader_async<R: AsyncRead + Unpin>(mut reader: R) -> Result<Hashes> {
    let mut sha = Sha256::new();
    let mut b3 = blake3::Hasher::new();
    let mut buf = vec![0u8; CHUNK_SIZE];
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        sha.update(&buf[..n]);
        b3.update(&buf[..n]);
    }
    Ok(Hashes {
        sha256: hex::encode(sha.finalize()),
        blake3: b3.finalize().to_hex().to_string(),
    })
}

/// SHA-256 d'un fichier (async).
pub async fn sha256_file_async(path: &Path) -> Result<String> {
    sha256_reader_async(tokio::fs::File::open(path).await?).await
}

/// BLAKE3 d'un fichier (async).
pub async fn blake3_file_async(path: &Path) -> Result<String> {
    blake3_reader_async(tokio::fs::File::open(path).await?).await
}

/// SHA-256 et BLAKE3 d'un fichier en une seule lecture (async).
pub async fn hash_file_async(path: &Path) -> Result<Hashes> {
    hash_reader_async(tokio::fs::File::open(path).await?).await
}

/// Vérifie le SHA-256 d'un fichier (async).
pub async fn verify_file_async(path: &Path, expected_sha256: &str) -> Result<()> {
    let expected = normalize_sha256(expected_sha256)?;
    let actual = sha256_file_async(path).await?;
    compare(expected, actual)
}

// ---------- Utilitaires ----------

fn normalize_sha256(s: &str) -> Result<String> {
    if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::Invalid(format!("hash SHA-256 mal formé : {s:?}")));
    }
    Ok(s.to_ascii_lowercase())
}

fn compare(expected: String, actual: String) -> Result<()> {
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

    const EMPTY_SHA: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    const ABC_SHA: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    const EMPTY_B3: &str = "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262";
    const ABC_B3: &str = "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85";

    #[test]
    fn known_vectors() {
        assert_eq!(sha256_reader(&b""[..]).unwrap(), EMPTY_SHA);
        assert_eq!(sha256_reader(&b"abc"[..]).unwrap(), ABC_SHA);
        assert_eq!(blake3_reader(&b""[..]).unwrap(), EMPTY_B3);
        assert_eq!(blake3_reader(&b"abc"[..]).unwrap(), ABC_B3);
        let h = hash_reader(&b"abc"[..]).unwrap();
        assert_eq!(h.sha256, ABC_SHA);
        assert_eq!(h.blake3, ABC_B3);
    }

    #[tokio::test]
    async fn known_vectors_async() {
        assert_eq!(sha256_reader_async(&b"abc"[..]).await.unwrap(), ABC_SHA);
        assert_eq!(blake3_reader_async(&b""[..]).await.unwrap(), EMPTY_B3);
        let h = hash_reader_async(&b""[..]).await.unwrap();
        assert_eq!(h.sha256, EMPTY_SHA);
        assert_eq!(h.blake3, EMPTY_B3);
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
    fn big_file_consistent_and_verified() {
        let f = big_file();
        assert_eq!(f.as_file().metadata().unwrap().len(), 64 * 1024 * 1024);
        let both = hash_file(f.path()).unwrap();
        assert_eq!(both.sha256, sha256_file(f.path()).unwrap());
        assert_eq!(both.blake3, blake3_file(f.path()).unwrap());
        assert_eq!(both.sha256.len(), 64);
        verify_file(f.path(), &both.sha256).unwrap();
        verify_file(f.path(), &both.sha256.to_uppercase()).unwrap();
    }

    #[tokio::test]
    async fn big_file_async_matches_sync() {
        let f = big_file();
        let sync = hash_file(f.path()).unwrap();
        assert_eq!(hash_file_async(f.path()).await.unwrap(), sync);
        verify_file_async(f.path(), &sync.sha256).await.unwrap();
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
