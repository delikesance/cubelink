//! Tête du skin d'un compte Microsoft (8×8 px, couche du chapeau incluse), pour la carte de profil.
//!
//! Profil public : `sessionserver.mojang.com` donne la propriété `textures` (JSON en base64) qui pointe vers le
//! skin sur `textures.minecraft.net`. Aucun jeton n'est nécessaire ni envoyé.

use std::io::Cursor;
use std::time::Duration;

use base64::Engine;
use image::{ImageEncoder, ImageReader, Rgba, RgbaImage};
use serde::Deserialize;

use crate::types::{Error, Result};

const SESSION_URL: &str = "https://sessionserver.mojang.com/session/minecraft/profile/";
const PROFILE_URL: &str = "https://api.mojang.com/users/profiles/minecraft/";
const MAX_SKIN_BYTES: usize = 256 * 1024;

#[derive(Deserialize)]
struct Profile {
    #[serde(default)]
    properties: Vec<Property>,
}

#[derive(Deserialize)]
struct Property {
    name: String,
    value: String,
}

#[derive(Deserialize)]
struct Textures {
    textures: TextureSet,
}

#[derive(Deserialize)]
struct TextureSet {
    #[serde(rename = "SKIN")]
    skin: Option<Texture>,
}

#[derive(Deserialize)]
struct Texture {
    url: String,
}

fn client() -> &'static reqwest::Client {
    static C: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(concat!("cubelink/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_default()
    })
}

/// UUID sans tirets, 32 caractères hexadécimaux.
pub fn normalize_uuid(uuid: &str) -> Option<String> {
    let s: String = uuid.chars().filter(|c| *c != '-').collect::<String>().to_ascii_lowercase();
    (s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit())).then_some(s)
}

/// URL du skin tirée de la propriété `textures` (base64), forcée en https et limitée à `textures.minecraft.net`.
fn skin_url(textures_b64: &str) -> Option<String> {
    let raw = base64::engine::general_purpose::STANDARD.decode(textures_b64.trim()).ok()?;
    let t: Textures = serde_json::from_slice(&raw).ok()?;
    let url = t.textures.skin?.url;
    let rest = url.strip_prefix("http://").or_else(|| url.strip_prefix("https://"))?;
    let (host, path) = rest.split_once('/')?;
    (host == "textures.minecraft.net" && path.starts_with("texture/") && !path.contains(".."))
        .then(|| format!("https://textures.minecraft.net/{path}"))
}

/// Visage (8,8) recouvert du chapeau (40,8), en PNG 8×8. Skins 64×64 et anciens 64×32.
pub fn head_from_skin(bytes: &[u8]) -> Option<Vec<u8>> {
    let img = ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?;
    let (w, h) = img.into_dimensions().ok()?;
    if w != 64 || (h != 64 && h != 32) {
        return None;
    }
    let skin = ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?.decode().ok()?.to_rgba8();
    let mut head = RgbaImage::new(8, 8);
    for y in 0..8 {
        for x in 0..8 {
            let Rgba([r, g, b, _]) = *skin.get_pixel(8 + x, 8 + y);
            let Rgba([hr, hg, hb, ha]) = *skin.get_pixel(40 + x, 8 + y);
            let a = u16::from(ha);
            let mix = |base: u8, top: u8| ((u16::from(base) * (255 - a) + u16::from(top) * a) / 255) as u8;
            head.put_pixel(x, y, Rgba([mix(r, hr), mix(g, hg), mix(b, hb), 255]));
        }
    }
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out).write_image(head.as_raw(), 8, 8, image::ExtendedColorType::Rgba8).ok()?;
    Some(out)
}

/// Pseudo Minecraft valide : 3 à 16 caractères `[A-Za-z0-9_]` (évite d'injecter autre chose dans l'URL).
pub fn valid_name(name: &str) -> bool {
    (3..=16).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Tête du skin du joueur au pseudo `name` (les amis n'ont pas d'UUID : on le résout par l'API publique de profils).
/// Pseudo inconnu (compte hors ligne, inexistant) : `Err(NotFound)`.
pub async fn head_png_by_name(name: &str) -> Result<Vec<u8>> {
    if !valid_name(name) {
        return Err(Error::Invalid("pseudo invalide".into()));
    }
    let net = |e: reqwest::Error| Error::Network(e.to_string());
    let resp = client().get(format!("{PROFILE_URL}{name}")).send().await.map_err(net)?;
    if !resp.status().is_success() {
        return Err(Error::NotFound("profil Minecraft".into()));
    }
    #[derive(Deserialize)]
    struct Id {
        id: String,
    }
    let id: Id = resp.json().await.map_err(net)?;
    head_png(&id.id).await
}

/// Tête du skin du compte `uuid` (PNG 8×8). Erreur réseau ou profil sans skin : `Err`.
pub async fn head_png(uuid: &str) -> Result<Vec<u8>> {
    let id = normalize_uuid(uuid).ok_or_else(|| Error::Invalid("identifiant de compte invalide".into()))?;
    let net = |e: reqwest::Error| Error::Network(e.to_string());
    let resp = client().get(format!("{SESSION_URL}{id}")).send().await.map_err(net)?;
    if !resp.status().is_success() {
        return Err(Error::NotFound("profil Minecraft".into()));
    }
    let profile: Profile = resp.json().await.map_err(net)?;
    let url = profile
        .properties
        .iter()
        .find(|p| p.name == "textures")
        .and_then(|p| skin_url(&p.value))
        .ok_or_else(|| Error::NotFound("skin".into()))?;
    let resp = client().get(url).send().await.map_err(net)?;
    if !resp.status().is_success() {
        return Err(Error::NotFound("skin".into()));
    }
    let bytes = resp.bytes().await.map_err(net)?;
    if bytes.len() > MAX_SKIN_BYTES {
        return Err(Error::Invalid("skin trop gros".into()));
    }
    head_from_skin(&bytes).ok_or_else(|| Error::Invalid("skin illisible".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_is_normalized() {
        assert_eq!(normalize_uuid("069A79F4-44E9-4726-A5BE-FCA90E38AAF5").as_deref(), Some("069a79f444e94726a5befca90e38aaf5"));
        assert_eq!(normalize_uuid("../etc"), None);
    }

    #[test]
    fn names_are_validated() {
        assert!(valid_name("Moiragh") && valid_name("garder_500"));
        assert!(!valid_name("ab") && !valid_name("../x") && !valid_name("a b c") && !valid_name("12345678901234567"));
    }

    #[test]
    fn skin_url_is_limited_to_mojang_textures() {
        let enc = |url: &str| base64::engine::general_purpose::STANDARD.encode(format!(r#"{{"textures":{{"SKIN":{{"url":"{url}"}}}}}}"#));
        assert_eq!(skin_url(&enc("http://textures.minecraft.net/texture/abc")).as_deref(), Some("https://textures.minecraft.net/texture/abc"));
        assert_eq!(skin_url(&enc("https://evil.example/texture/abc")), None);
        assert_eq!(skin_url(&enc("https://textures.minecraft.net/../x")), None);
    }

    #[test]
    fn head_blends_the_hat_layer_over_the_face() {
        let mut skin = RgbaImage::from_pixel(64, 64, Rgba([0, 0, 0, 0]));
        for y in 8..16 {
            for x in 8..16 {
                skin.put_pixel(x, y, Rgba([200, 150, 100, 255]));
            }
        }
        skin.put_pixel(40, 8, Rgba([255, 0, 0, 255]));
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png).write_image(skin.as_raw(), 64, 64, image::ExtendedColorType::Rgba8).unwrap();
        let head = image::load_from_memory(&head_from_skin(&png).unwrap()).unwrap().to_rgba8();
        assert_eq!((head.width(), head.height()), (8, 8));
        assert_eq!(*head.get_pixel(0, 0), Rgba([255, 0, 0, 255]));
        assert_eq!(*head.get_pixel(1, 0), Rgba([200, 150, 100, 255]));
        assert_eq!(head_from_skin(b"x"), None);
    }
}
