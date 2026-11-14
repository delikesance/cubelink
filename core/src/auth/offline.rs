//! Profil hors ligne (tests E2E) : UUID v3 `OfflinePlayer:<pseudo>`, comme Minecraft.

use crate::types::{Error, Profile, Result};
use md5::{Digest, Md5};

/// UUID hors ligne (équivalent de `UUID.nameUUIDFromBytes`), minuscule avec tirets.
pub fn offline_uuid(name: &str) -> String {
    let mut h: [u8; 16] = Md5::digest(format!("OfflinePlayer:{name}").as_bytes()).into();
    h[6] = (h[6] & 0x0f) | 0x30;
    h[8] = (h[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(h).hyphenated().to_string()
}

/// Valide le pseudo (3 à 16 caractères `[A-Za-z0-9_]`) et construit le profil.
pub fn offline_profile(name: &str) -> Result<Profile> {
    let len = name.chars().count();
    if !(3..=16).contains(&len) {
        return Err(Error::Invalid(
            "le pseudo doit contenir entre 3 et 16 caractères".into(),
        ));
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(Error::Invalid(
            "le pseudo ne peut contenir que des lettres, chiffres et underscores".into(),
        ));
    }
    Ok(Profile::Offline {
        name: name.to_string(),
        uuid: offline_uuid(name),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_notch() {
        assert_eq!(offline_uuid("Notch"), "b50ad385-829d-3141-a216-7e7d7539ba7f");
    }

    #[test]
    fn profil_valide() {
        assert_eq!(
            offline_profile("Notch").unwrap(),
            Profile::Offline {
                name: "Notch".into(),
                uuid: "b50ad385-829d-3141-a216-7e7d7539ba7f".into()
            }
        );
        assert!(offline_profile("a_1").is_ok());
        assert!(offline_profile("abcdefghijklmnop").is_ok());
    }

    #[test]
    fn pseudos_invalides() {
        for n in ["", "ab", "abcdefghijklmnopq", "no space", "é_é_é", "a-b-c", "x.y.z"] {
            assert!(matches!(offline_profile(n), Err(Error::Invalid(_))), "{n}");
        }
    }
}
