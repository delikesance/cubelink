//! Code ami `mcl1-…` : chaîne partageable (aucun secret, mais elle peut contenir
//! un relais d'attache et des adresses d'indice).
//!
//! Format binaire (avant encodage) : `version(1) | epoch(u32 BE) | clé publique(32)
//! | relais(len u8 + UTF-8) | nb adresses(u8) | adresses(len u8 + UTF-8)… | CRC-32 BE(4)`.
//! Le CRC couvre tout ce qui précède. Encodage : base32 minuscule sans padding,
//! groupes de 4 caractères séparés par `-`, préfixe `mcl1-`.

use core_lib::types::{Error, Result};
use data_encoding::BASE32_NOPAD;

const PREFIX: &str = "mcl1-";
const LINK_PREFIX: &str = "launcher://add/";
const FORMAT_VERSION: u8 = 1;
/// Longueur maximale d'une chaîne (relais ou adresse), en octets.
pub const MAX_STR_LEN: usize = 64;
/// Nombre maximal d'adresses d'indice.
pub const MAX_ADDRS: usize = 8;
/// Borne maximale du code normalisé (sans tirets), en caractères.
const MAX_NORMALIZED_LEN: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FriendCodePayload {
    pub public_key: [u8; 32],
    pub epoch: u32,
    pub relay: Option<String>,
    pub addrs: Vec<String>,
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn push_str(out: &mut Vec<u8>, s: &str) {
    out.push(s.len() as u8);
    out.extend_from_slice(s.as_bytes());
}

/// Encode un payload. Les champs hors bornes (relais ou adresse > 64 octets,
/// au-delà de 8 adresses) sont ignorés pour que le code reste parsable.
pub fn encode(p: &FriendCodePayload) -> String {
    let mut buf = vec![FORMAT_VERSION];
    buf.extend_from_slice(&p.epoch.to_be_bytes());
    buf.extend_from_slice(&p.public_key);
    match p.relay.as_deref().filter(|r| r.len() <= MAX_STR_LEN) {
        Some(r) => push_str(&mut buf, r),
        None => buf.push(0),
    }
    let addrs: Vec<&String> = p.addrs.iter().filter(|a| a.len() <= MAX_STR_LEN).take(MAX_ADDRS).collect();
    buf.push(addrs.len() as u8);
    for a in addrs {
        push_str(&mut buf, a);
    }
    let crc = crc32(&buf);
    buf.extend_from_slice(&crc.to_be_bytes());

    let raw = BASE32_NOPAD.encode(&buf).to_ascii_lowercase();
    let groups: Vec<&str> = raw
        .as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).expect("base32 ascii"))
        .collect();
    format!("{PREFIX}{}", groups.join("-"))
}

/// `launcher://add/<code>`
pub fn to_link(p: &FriendCodePayload) -> String {
    format!("{LINK_PREFIX}{}", encode(p))
}

fn invalid(msg: &str) -> Error {
    Error::Invalid(format!("code ami : {msg}"))
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.0.len() < n {
            return Err(invalid("données tronquées"));
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn string(&mut self) -> Result<String> {
        let n = usize::from(self.take(1)?[0]);
        if n > MAX_STR_LEN {
            return Err(invalid("chaîne trop longue"));
        }
        String::from_utf8(self.take(n)?.to_vec()).map_err(|_| invalid("UTF-8 invalide"))
    }
}

/// Parse tolérant : majuscules, espaces, retours à la ligne, tirets en trop,
/// préfixe `launcher://add/`.
pub fn parse(input: &str) -> Result<FriendCodePayload> {
    if input.len() > 4 * MAX_NORMALIZED_LEN {
        return Err(invalid("trop long"));
    }
    let mut s: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    s.make_ascii_lowercase();
    let s = s.strip_prefix(LINK_PREFIX).unwrap_or(&s);
    let body = s.strip_prefix(PREFIX).ok_or_else(|| invalid("préfixe mcl1- attendu"))?;
    let body: String = body.chars().filter(|&c| c != '-').collect();
    if body.len() > MAX_NORMALIZED_LEN {
        return Err(invalid("trop long"));
    }
    if !body.bytes().all(|b| matches!(b, b'a'..=b'z' | b'2'..=b'7')) {
        return Err(invalid("caractère hors alphabet"));
    }
    let bytes = BASE32_NOPAD
        .decode(body.to_ascii_uppercase().as_bytes())
        .map_err(|_| invalid("base32 invalide"))?;
    // version + epoch + clé + relais(1) + nb adresses(1) + crc
    if bytes.len() < 1 + 4 + 32 + 1 + 1 + 4 {
        return Err(invalid("trop court"));
    }
    let (data, crc_bytes) = bytes.split_at(bytes.len() - 4);
    if crc32(data).to_be_bytes() != crc_bytes {
        return Err(invalid("somme de contrôle fausse"));
    }
    let mut r = Reader(data);
    let version = r.take(1)?[0];
    if version != FORMAT_VERSION {
        return Err(invalid("version inconnue"));
    }
    let epoch = u32::from_be_bytes(r.take(4)?.try_into().expect("4 octets"));
    let public_key: [u8; 32] = r.take(32)?.try_into().expect("32 octets");
    let relay = Some(r.string()?).filter(|s| !s.is_empty());
    let n = usize::from(r.take(1)?[0]);
    if n > MAX_ADDRS {
        return Err(invalid("trop d'adresses"));
    }
    let addrs = (0..n).map(|_| r.string()).collect::<Result<Vec<_>>>()?;
    if !r.0.is_empty() {
        return Err(invalid("octets en trop"));
    }
    Ok(FriendCodePayload { public_key, epoch, relay, addrs })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn sample() -> FriendCodePayload {
        FriendCodePayload {
            public_key: [7; 32],
            epoch: 3,
            relay: Some("relay.example.org:4433".into()),
            addrs: vec!["203.0.113.5:4433".into(), "[2001:db8::1]:4433".into()],
        }
    }

    #[test]
    fn shape_and_ui_regex() {
        let c = encode(&sample());
        assert!(c.starts_with("mcl1-"));
        assert!(c.len() >= 16 + 5);
        assert!(c[5..].bytes().all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b) || b == b'-'));
        assert!(c[5..].split('-').all(|g| g.len() <= 4));
        assert!(to_link(&sample()).starts_with("launcher://add/mcl1-"));
    }

    #[test]
    fn tolerant_parsing() {
        let p = sample();
        let c = encode(&p);
        assert_eq!(parse(&c.to_uppercase()).unwrap(), p);
        assert_eq!(parse(&format!("  {}\r\n", c.replace('-', "- \n"))).unwrap(), p);
        assert_eq!(parse(&c.replace('-', "--")).unwrap(), p);
        assert_eq!(parse(&to_link(&p)).unwrap(), p);
    }

    #[test]
    fn rejections() {
        let c = encode(&sample());
        assert!(parse("").is_err());
        assert!(parse(&c.replacen("mcl1", "mcl2", 1)).is_err());
        assert!(parse(&format!("{c}1")).is_err());
        assert!(parse(&format!("{c}!")).is_err());
        assert!(parse(&format!("mcl1-{}", "a".repeat(5000))).is_err());
        assert!(parse("mcl1-aaaa").is_err());
    }

    #[test]
    fn unknown_version_with_valid_crc_rejected() {
        let mut buf = vec![2u8];
        buf.extend_from_slice(&[0; 4 + 32]);
        buf.extend_from_slice(&[0, 0]);
        let crc = crc32(&buf);
        buf.extend_from_slice(&crc.to_be_bytes());
        let code = format!("mcl1-{}", BASE32_NOPAD.encode(&buf).to_lowercase());
        assert!(parse(&code).is_err());
    }

    #[test]
    fn crc_known_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    fn payload_strategy() -> impl Strategy<Value = FriendCodePayload> {
        (
            any::<[u8; 32]>(),
            any::<u32>(),
            proptest::option::of("[a-z0-9.:-]{1,64}"),
            proptest::collection::vec("[a-zA-Z0-9.:]{1,64}", 0..=MAX_ADDRS),
        )
            .prop_map(|(public_key, epoch, relay, addrs)| FriendCodePayload { public_key, epoch, relay, addrs })
    }

    proptest! {
        #[test]
        fn roundtrip(p in payload_strategy()) {
            prop_assert_eq!(parse(&encode(&p)).unwrap(), p.clone());
            prop_assert_eq!(parse(&to_link(&p)).unwrap(), p);
        }

        #[test]
        fn modified_char_rejected(p in payload_strategy(), idx in any::<prop::sample::Index>(), repl in 0usize..32) {
            let code = encode(&p);
            let body: Vec<char> = code[5..].chars().collect();
            let positions: Vec<usize> = (0..body.len()).filter(|&i| body[i] != '-').collect();
            let i = positions[idx.index(positions.len())];
            let alphabet: Vec<char> = "abcdefghijklmnopqrstuvwxyz234567".chars().collect();
            let mut new = alphabet[repl];
            if new == body[i] { new = alphabet[(repl + 1) % 32]; }
            let mut b = body.clone();
            b[i] = new;
            let modified = format!("mcl1-{}", b.into_iter().collect::<String>());
            prop_assert!(parse(&modified).is_err());
        }

        #[test]
        fn no_panic_on_random_input(s in "\\PC{0,300}") {
            let _ = parse(&s);
        }

        #[test]
        fn no_panic_on_prefixed_noise(s in "[a-z2-7-]{0,300}") {
            let _ = parse(&format!("mcl1-{s}"));
        }
    }
}
