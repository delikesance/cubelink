//! Code de vérification à comparer à l'oral entre deux amis après l'échange de clés
//! (détection d'un homme du milieu). Identique des deux côtés.
//!
//! Calcul : SHA-256 de `domaine | (clé, nonce)` pour les deux paires triées par clé
//! (les nonces suivent leur clé, pas leur rôle), chaque champ précédé de sa longueur
//! (u32 BE). On garde les 8 premiers octets.

use core_lib::types::VerifyCode;
use sha2::{Digest, Sha256};

/// Séparateur de domaine du hachage.
const DOMAIN: &[u8] = b"cubelink/verify/v1";

/// Nombre de bits affichés (16 hex = 8 octets).
///
/// Limite : 64 bits protègent contre un homme du milieu opportuniste, pas contre un
/// attaquant capable de grinder des clés hors ligne pendant des jours. Le protocole
/// d'ajout (N7) doit donc lier le code à des engagements (commit des nonces avant
/// révélation) pour empêcher ce grinding.
pub const CODE_BITS: usize = 64;

const CODE_BYTES: usize = CODE_BITS / 8;

fn absorb(h: &mut Sha256, data: &[u8]) {
    h.update((data.len() as u32).to_be_bytes());
    h.update(data);
}

/// Code de vérification de la paire (moi, ami). `nonce_a` accompagne `my_pub`,
/// `nonce_b` accompagne `their_pub` : `verify_code(a, b, na, nb)` ==
/// `verify_code(b, a, nb, na)`.
pub fn verify_code(
    my_pub: &[u8; 32],
    their_pub: &[u8; 32],
    nonce_a: &[u8],
    nonce_b: &[u8],
) -> VerifyCode {
    let (first, second) = if my_pub <= their_pub {
        ((my_pub, nonce_a), (their_pub, nonce_b))
    } else {
        ((their_pub, nonce_b), (my_pub, nonce_a))
    };
    let mut h = Sha256::new();
    absorb(&mut h, DOMAIN);
    for (key, nonce) in [first, second] {
        absorb(&mut h, key);
        absorb(&mut h, nonce);
    }
    let digest = h.finalize();
    let mut out = [0u8; CODE_BYTES];
    out.copy_from_slice(&digest[..CODE_BYTES]);
    format_code(&out)
}

/// Met en forme 8 octets : « 3F9A · C21B · 77D0 · 5E14 ».
fn format_code(bytes: &[u8; 8]) -> VerifyCode {
    let hex = hex::encode_upper(bytes);
    let groups: Vec<&str> = (0..4).map(|i| &hex[i * 4..i * 4 + 4]).collect();
    VerifyCode(groups.join(" · "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::collections::HashSet;

    #[test]
    fn format_exact() {
        let c = format_code(&[0x3F, 0x9A, 0xC2, 0x1B, 0x77, 0xD0, 0x5E, 0x14]);
        assert_eq!(c.0, "3F9A · C21B · 77D0 · 5E14");
    }

    #[test]
    fn mitm_gives_different_codes() {
        let (a, b, m) = ([1u8; 32], [2u8; 32], [3u8; 32]);
        let direct = verify_code(&a, &b, b"na", b"nb");
        let a_m = verify_code(&a, &m, b"na", b"nm1");
        let m_b = verify_code(&m, &b, b"nm2", b"nb");
        assert_ne!(a_m, direct);
        assert_ne!(m_b, direct);
        assert_ne!(a_m, m_b);
        assert_ne!(verify_code(&a, &m, b"na", b"nb"), direct);
    }

    #[test]
    fn nonces_change_code_and_are_bound_to_keys() {
        let (a, b) = ([1u8; 32], [2u8; 32]);
        let base = verify_code(&a, &b, b"x", b"y");
        assert_ne!(base, verify_code(&a, &b, b"x", b"z"));
        assert_ne!(base, verify_code(&a, &b, b"w", b"y"));
        assert_ne!(base, verify_code(&a, &b, b"y", b"x"));
        assert_ne!(verify_code(&a, &b, b"ab", b"c"), verify_code(&a, &b, b"a", b"bc"));
    }

    #[test]
    fn no_trivial_collision_10k() {
        let mut seen = HashSet::new();
        for i in 0u32..10_000 {
            let mut k = [0u8; 32];
            k[..4].copy_from_slice(&i.to_be_bytes());
            let c = verify_code(&k, &[9u8; 32], &i.to_le_bytes(), b"n");
            assert!(seen.insert(c.0), "collision à {i}");
        }
    }

    proptest! {
        #[test]
        fn symmetric(a in any::<[u8; 32]>(), b in any::<[u8; 32]>(),
                     na in prop::collection::vec(any::<u8>(), 0..64),
                     nb in prop::collection::vec(any::<u8>(), 0..64)) {
            prop_assert_eq!(verify_code(&a, &b, &na, &nb), verify_code(&b, &a, &nb, &na));
        }

        #[test]
        fn deterministic_and_well_formed(a in any::<[u8; 32]>(), b in any::<[u8; 32]>(),
                     na in prop::collection::vec(any::<u8>(), 0..64),
                     nb in prop::collection::vec(any::<u8>(), 0..64)) {
            let c1 = verify_code(&a, &b, &na, &nb);
            prop_assert_eq!(&c1, &verify_code(&a, &b, &na, &nb));
            prop_assert_eq!(c1.0.len(), "3F9A · C21B · 77D0 · 5E14".len());
        }
    }
}
