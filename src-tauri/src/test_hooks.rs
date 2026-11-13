//! Surcharges de test (E2E) : lues UNIQUEMENT si `CUBELINK_TEST_MODE=1`.
//!
//! Hors mode test, toutes les variables `CUBELINK_*` ci-dessous sont ignorées, même si elles existent.
//! En mode test, chaque URL doit être http(s) vers `127.0.0.1`, `localhost` ou `[::1]` : le mode test ne peut
//! donc pas rediriger l'application vers un vrai serveur.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, OnceLock};

use serde::Serialize;
use tauri::State;

use crate::state::AppState;

pub const TEST_MODE_VAR: &str = "CUBELINK_TEST_MODE";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum RelayMode {
    Disabled,
    Default,
    Custom(String),
}

#[derive(Debug, Clone, Default)]
pub struct Hooks {
    pub test_mode: bool,
    pub auth_ms: Option<String>,
    pub auth_xbox: Option<String>,
    pub auth_xsts: Option<String>,
    pub auth_mc: Option<String>,
    pub mojang_meta: Option<String>,
    pub mojang_resources: Option<String>,
    pub fabric_meta: Option<String>,
    pub quilt_meta: Option<String>,
    pub neoforge_maven: Option<String>,
    pub forge_maven: Option<String>,
    pub forge_files: Option<String>,
    pub adoptium: Option<String>,
    pub modrinth_api: Option<String>,
    /// `CUBELINK_TEST_CF_API` : base de l'API CurseForge (sans `/v1`), clé factice `test-key`.
    pub cf_api: Option<String>,
    pub java_path: Option<PathBuf>,
    pub data_dir: Option<PathBuf>,
    pub relay: Option<RelayMode>,
    pub skip_java_probe: bool,
    /// `CUBELINK_TRANSFER_IDLE_SECS` : délai d'inactivité d'un transfert (secondes) avant l'échec.
    pub transfer_idle_secs: Option<u64>,
}

/// Valide une URL de test : http(s), hôte loopback exact, sans identifiants. Renvoie l'URL sans `/` final.
pub fn parse_loopback_url(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    let rest = raw
        .strip_prefix("http://")
        .or_else(|| raw.strip_prefix("https://"))
        .ok_or("URL de test : seul http(s) est accepté")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    if authority.is_empty() || authority.contains(['@', '\\', ' ']) {
        return Err("URL de test invalide".into());
    }
    let (host, port) = if let Some(h) = authority.strip_prefix('[') {
        let (h, p) = h.split_once(']').ok_or("URL de test invalide")?;
        (h, p.strip_prefix(':').or(if p.is_empty() { Some("") } else { None }))
    } else {
        match authority.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (authority, Some("")),
        }
    };
    let port = port.ok_or("URL de test invalide")?;
    if !(port.is_empty() || port.parse::<u16>().is_ok()) {
        return Err("URL de test : port invalide".into());
    }
    if !matches!(host.to_ascii_lowercase().as_str(), "127.0.0.1" | "localhost" | "::1") {
        return Err("URL de test refusée : seul 127.0.0.1 / localhost est autorisé".into());
    }
    Ok(raw.trim_end_matches('/').to_string())
}

fn allowed_java_name(path: &std::path::Path) -> bool {
    let n = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_ascii_lowercase();
    matches!(n.as_str(), "java" | "java.exe" | "javaw.exe" | "fake-java.cmd" | "fake-java.sh")
}

/// Nom de fichier Java accepté par le réglage `java_path` (les faux java seulement en mode test).
pub fn java_name_ok(name: &str, test_mode: bool) -> bool {
    let n = name.to_ascii_lowercase();
    matches!(n.as_str(), "java" | "java.exe" | "javaw.exe") || (test_mode && matches!(n.as_str(), "fake-java.cmd" | "fake-java.sh"))
}

impl Hooks {
    /// `get` lit une variable d'environnement. Sans `CUBELINK_TEST_MODE=1`, tout le reste est ignoré.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        if get(TEST_MODE_VAR).as_deref() != Some("1") {
            return Ok(Self::default());
        }
        let url = |name: &str| -> Result<Option<String>, String> {
            match get(name).filter(|v| !v.trim().is_empty()) {
                Some(v) => parse_loopback_url(&v).map(Some).map_err(|e| format!("{name} : {e}")),
                None => Ok(None),
            }
        };
        let java_path = match get("CUBELINK_JAVA_PATH").filter(|v| !v.trim().is_empty()) {
            Some(p) => {
                let p = PathBuf::from(p);
                if !allowed_java_name(&p) {
                    return Err("CUBELINK_JAVA_PATH : nom de fichier non accepté (java, java.exe, fake-java.cmd/.sh)".into());
                }
                Some(p)
            }
            None => None,
        };
        let data_dir = match get("CUBELINK_DATA_DIR").filter(|v| !v.trim().is_empty()) {
            Some(d) => {
                let d = PathBuf::from(d);
                if !d.is_absolute() {
                    return Err("CUBELINK_DATA_DIR : chemin absolu requis".into());
                }
                Some(d)
            }
            None => None,
        };
        let relay = match get("CUBELINK_RELAY_MODE").filter(|v| !v.trim().is_empty()).as_deref() {
            None => None,
            Some("disabled") => Some(RelayMode::Disabled),
            Some("default") => Some(RelayMode::Default),
            Some(s) => match s.strip_prefix("custom:") {
                Some(u) => Some(RelayMode::Custom(parse_loopback_url(u).map_err(|e| format!("CUBELINK_RELAY_MODE : {e}"))?)),
                None => return Err("CUBELINK_RELAY_MODE : disabled, default ou custom:<url>".into()),
            },
        };
        Ok(Self {
            test_mode: true,
            auth_ms: url("CUBELINK_AUTH_MS_URL")?,
            auth_xbox: url("CUBELINK_AUTH_XBOX_URL")?,
            auth_xsts: url("CUBELINK_AUTH_XSTS_URL")?,
            auth_mc: url("CUBELINK_AUTH_MC_URL")?,
            mojang_meta: url("CUBELINK_MOJANG_META_URL")?,
            mojang_resources: url("CUBELINK_MOJANG_RESOURCES_URL")?,
            fabric_meta: url("CUBELINK_FABRIC_META_URL")?,
            quilt_meta: url("CUBELINK_QUILT_META_URL")?,
            neoforge_maven: url("CUBELINK_NEOFORGE_MAVEN_URL")?,
            forge_maven: url("CUBELINK_FORGE_MAVEN_URL")?,
            forge_files: url("CUBELINK_FORGE_FILES_URL")?,
            adoptium: url("CUBELINK_ADOPTIUM_URL")?,
            // `CUBELINK_TEST_MODRINTH_API` est prioritaire sur l'ancien nom `CUBELINK_MODRINTH_API_URL`.
            modrinth_api: match url("CUBELINK_TEST_MODRINTH_API")? {
                Some(u) => Some(u),
                None => url("CUBELINK_MODRINTH_API_URL")?,
            },
            cf_api: url("CUBELINK_TEST_CF_API")?,
            java_path,
            data_dir,
            relay,
            skip_java_probe: get("CUBELINK_SKIP_JAVA_PROBE").as_deref() == Some("1"),
            transfer_idle_secs: match get("CUBELINK_TRANSFER_IDLE_SECS").filter(|v| !v.trim().is_empty()) {
                Some(v) => Some(
                    v.trim().parse::<u64>().ok().filter(|n| (1..=3600).contains(n)).ok_or("CUBELINK_TRANSFER_IDLE_SECS : entier de 1 à 3600")?,
                ),
                None => None,
            },
        })
    }

    /// URL du manifeste de versions (la base du miroir suffit).
    pub fn manifest_url(&self) -> Option<String> {
        self.mojang_meta.as_ref().map(|b| if b.ends_with(".json") { b.clone() } else { format!("{b}/mc/game/version_manifest_v2.json") })
    }

    pub fn resources_url(&self) -> Option<String> {
        self.mojang_resources.clone().or_else(|| self.mojang_meta.as_ref().map(|b| format!("{b}/resources")))
    }

    /// Base de l'API Modrinth, `/v2` ajouté au besoin.
    pub fn modrinth_base(&self) -> Option<String> {
        self.modrinth_api.as_ref().map(|b| if b.ends_with("/v2") { b.clone() } else { format!("{b}/v2") })
    }

    /// Message d'avertissement visible au démarrage (mode test seulement).
    pub fn warning(&self) -> Option<String> {
        self.test_mode.then(|| {
            format!(
                "ATTENTION : CUBELINK_TEST_MODE=1, mode test actif ({} surcharge(s) d'URL). Ne jamais utiliser en production.",
                self.active_urls().len()
            )
        })
    }

    pub fn active_urls(&self) -> BTreeMap<&'static str, String> {
        let mut m = BTreeMap::new();
        for (k, v) in [
            ("CUBELINK_AUTH_MS_URL", &self.auth_ms),
            ("CUBELINK_AUTH_XBOX_URL", &self.auth_xbox),
            ("CUBELINK_AUTH_XSTS_URL", &self.auth_xsts),
            ("CUBELINK_AUTH_MC_URL", &self.auth_mc),
            ("CUBELINK_MOJANG_META_URL", &self.mojang_meta),
            ("CUBELINK_MOJANG_RESOURCES_URL", &self.mojang_resources),
            ("CUBELINK_FABRIC_META_URL", &self.fabric_meta),
            ("CUBELINK_QUILT_META_URL", &self.quilt_meta),
            ("CUBELINK_NEOFORGE_MAVEN_URL", &self.neoforge_maven),
            ("CUBELINK_FORGE_MAVEN_URL", &self.forge_maven),
            ("CUBELINK_FORGE_FILES_URL", &self.forge_files),
            ("CUBELINK_ADOPTIUM_URL", &self.adoptium),
            ("CUBELINK_MODRINTH_API_URL", &self.modrinth_api),
            ("CUBELINK_TEST_CF_API", &self.cf_api),
        ] {
            if let Some(v) = v {
                m.insert(k, v.clone());
            }
        }
        m
    }
}

static HOOKS: OnceLock<Hooks> = OnceLock::new();

fn from_env() -> Result<Hooks, String> {
    Hooks::from_lookup(|k| std::env::var(k).ok())
}

/// À appeler au démarrage : valide l'environnement et affiche l'avertissement du mode test.
pub fn init() -> Result<&'static Hooks, String> {
    let h = from_env()?;
    if let Some(w) = h.warning() {
        eprintln!("[cubelink] {w}");
    }
    Ok(HOOKS.get_or_init(|| h))
}

/// Surcharges actives (inertes hors mode test ou si l'environnement est invalide).
pub fn hooks() -> &'static Hooks {
    HOOKS.get_or_init(|| from_env().unwrap_or_default())
}

/// `http://` n'est toléré que vers le loopback, et seulement en mode test.
pub fn http_allowed(url: &str) -> bool {
    hooks().test_mode && url.starts_with("http://") && parse_loopback_url(url).is_ok()
}

static TUNNEL_PORT: AtomicU16 = AtomicU16::new(0);
static GUEST_TUNNEL_PORT: AtomicU16 = AtomicU16::new(0);

/// Port local du tunnel ouvert côté invité (0 : aucun).
pub fn set_guest_tunnel_port(p: u16) {
    GUEST_TUNNEL_PORT.store(p, Ordering::SeqCst);
}

pub fn guest_tunnel_port() -> u16 {
    GUEST_TUNNEL_PORT.load(Ordering::SeqCst)
}

pub fn set_tunnel_port(p: u16) {
    TUNNEL_PORT.store(p, Ordering::SeqCst);
}

/// 1024 à 65535.
pub fn validate_world_port(p: u32) -> Result<u16, String> {
    u16::try_from(p).ok().filter(|p| *p >= 1024).ok_or_else(|| "le port du monde doit être compris entre 1024 et 65535".to_string())
}

#[derive(Debug, Clone, Serialize)]
pub struct TestStateInfo {
    pub test_mode: bool,
    pub data_dir: String,
    pub data_profile: String,
    pub paths: BTreeMap<&'static str, String>,
    pub url_overrides: BTreeMap<&'static str, String>,
    pub java_path: Option<String>,
    pub relay_mode: Option<RelayMode>,
    pub skip_java_probe: bool,
    /// Port du monde LAN de l'hôte (côté hôte, 0 sinon). Ne pas confondre avec `guest_tunnel_port`.
    pub tunnel_port: u16,
    /// Port local du tunnel invité (`join_open_tunnel`), 0 si fermé.
    pub guest_tunnel_port: u16,
}

impl AppState {
    pub fn test_state_info(&self) -> TestStateInfo {
        let h = hooks();
        let p = &self.paths;
        let s = |x: &std::path::Path| x.display().to_string();
        let paths = BTreeMap::from([
            ("root", s(&p.root)),
            ("instances", s(&p.instances)),
            ("store", s(&p.store)),
            ("versions", s(&p.versions)),
            ("java", s(&p.java)),
            ("logs", s(&p.logs)),
            ("p2p", s(&p.p2p)),
            ("settings_file", s(&p.settings_file)),
        ]);
        TestStateInfo {
            test_mode: h.test_mode,
            data_dir: s(&p.root),
            data_profile: p.root.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string(),
            paths,
            url_overrides: h.active_urls(),
            java_path: h.java_path.as_ref().map(|j| s(j)),
            relay_mode: h.relay.clone(),
            skip_java_probe: h.skip_java_probe,
            tunnel_port: TUNNEL_PORT.load(Ordering::SeqCst),
            guest_tunnel_port: guest_tunnel_port(),
        }
    }
}

/// Enregistrée dans le gestionnaire d'invocation uniquement si `CUBELINK_TEST_MODE=1`.
#[tauri::command(rename_all = "snake_case")]
pub fn test_state(state: State<'_, Arc<AppState>>) -> Result<TestStateInfo, String> {
    if !hooks().test_mode {
        return Err("commande indisponible hors mode test".into());
    }
    Ok(state.test_state_info())
}

/// Coupe de force la connexion iroh du tunnel invité (test de reconnexion) ; mode test uniquement,
/// jamais enregistrée ni exposée en production. Renvoie faux s'il n'y a pas de tunnel ouvert.
#[tauri::command]
pub async fn test_cut_guest_connection(state: State<'_, Arc<AppState>>) -> Result<bool, String> {
    if !hooks().test_mode {
        return Err("commande indisponible hors mode test".into());
    }
    Ok(state.backend().test_cut_guest_connection().await)
}

/// Sonde Java factice : prétend que l'exécutable est la version attendue (faux JRE non exécutable).
pub struct FakeProbe(pub u32);

impl core_lib::java_install::JavaProbe for FakeProbe {
    fn probe(&self, _java_exe: &std::path::Path) -> Option<u32> {
        Some(self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn surcharge_ignoree_sans_le_drapeau() {
        for flag in [&[][..], &[("CUBELINK_TEST_MODE", "0")], &[("CUBELINK_TEST_MODE", "true")]] {
            let mut pairs: Vec<(&str, &str)> = flag.to_vec();
            pairs.extend([("CUBELINK_AUTH_MS_URL", "http://127.0.0.1:1"), ("CUBELINK_DATA_DIR", "C:\\x"), ("CUBELINK_SKIP_JAVA_PROBE", "1")]);
            let h = Hooks::from_lookup(env(&pairs)).unwrap();
            assert!(!h.test_mode && h.auth_ms.is_none() && h.data_dir.is_none() && !h.skip_java_probe);
            assert!(h.warning().is_none() && h.active_urls().is_empty());
        }
    }

    #[test]
    fn url_externe_refusee_meme_avec_le_drapeau() {
        for bad in [
            "https://login.microsoftonline.com",
            "http://127.0.0.1.evil.com",
            "http://evil.com/127.0.0.1",
            "http://user@127.0.0.1",
            "http://127.0.0.1@evil.com",
            "ftp://127.0.0.1",
            "127.0.0.1:80",
            "http://127.0.0.1:99999",
        ] {
            let pairs = [("CUBELINK_TEST_MODE", "1"), ("CUBELINK_MOJANG_META_URL", bad)];
            assert!(Hooks::from_lookup(env(&pairs)).is_err(), "{bad}");
        }
        let relay: &[(&str, &str)] = &[("CUBELINK_TEST_MODE", "1"), ("CUBELINK_RELAY_MODE", "custom:https://relay.n0.example")];
        assert!(Hooks::from_lookup(env(relay)).is_err());
    }

    #[test]
    fn surcharges_valides_en_mode_test() {
        let p: &[(&str, &str)] = &[
            ("CUBELINK_TEST_MODE", "1"),
            ("CUBELINK_AUTH_MS_URL", "http://127.0.0.1:5000/"),
            ("CUBELINK_MOJANG_META_URL", "http://localhost:6000"),
            ("CUBELINK_MODRINTH_API_URL", "http://127.0.0.1:7000"),
            ("CUBELINK_QUILT_META_URL", "http://127.0.0.1:7001"),
            ("CUBELINK_NEOFORGE_MAVEN_URL", "http://127.0.0.1:7002"),
            ("CUBELINK_FORGE_MAVEN_URL", "http://127.0.0.1:7003"),
            ("CUBELINK_JAVA_PATH", if cfg!(windows) { "C:\\fx\\fake-java.cmd" } else { "/fx/fake-java.cmd" }),
            ("CUBELINK_RELAY_MODE", "custom:http://127.0.0.1:8000"),
            ("CUBELINK_SKIP_JAVA_PROBE", "1"),
        ];
        let h = Hooks::from_lookup(env(p)).unwrap();
        assert_eq!(h.auth_ms.as_deref(), Some("http://127.0.0.1:5000"));
        assert_eq!(h.manifest_url().unwrap(), "http://localhost:6000/mc/game/version_manifest_v2.json");
        assert_eq!(h.resources_url().unwrap(), "http://localhost:6000/resources");
        assert_eq!(h.modrinth_base().unwrap(), "http://127.0.0.1:7000/v2");
        assert_eq!(h.relay, Some(RelayMode::Custom("http://127.0.0.1:8000".into())));
        assert!(h.skip_java_probe && h.java_path.is_some());
        assert_eq!(h.quilt_meta.as_deref(), Some("http://127.0.0.1:7001"));
        assert_eq!(h.active_urls().len(), 6);
    }

    #[test]
    fn surcharges_parcourir() {
        let p: &[(&str, &str)] = &[
            ("CUBELINK_TEST_MODE", "1"),
            ("CUBELINK_TEST_CF_API", "http://127.0.0.1:7100/"),
            ("CUBELINK_TEST_MODRINTH_API", "http://127.0.0.1:7101"),
            ("CUBELINK_MODRINTH_API_URL", "http://127.0.0.1:7999"),
        ];
        let h = Hooks::from_lookup(env(p)).unwrap();
        assert_eq!(h.cf_api.as_deref(), Some("http://127.0.0.1:7100"));
        assert_eq!(h.modrinth_base().unwrap(), "http://127.0.0.1:7101/v2");
        // Ignorées hors mode test, refusées si non loopback.
        let off: &[(&str, &str)] = &[("CUBELINK_TEST_CF_API", "http://127.0.0.1:7100")];
        assert!(Hooks::from_lookup(env(off)).unwrap().cf_api.is_none());
        let bad: &[(&str, &str)] = &[("CUBELINK_TEST_MODE", "1"), ("CUBELINK_TEST_CF_API", "https://api.curseforge.com")];
        assert!(Hooks::from_lookup(env(bad)).is_err());
        let bad: &[(&str, &str)] = &[("CUBELINK_TEST_MODE", "1"), ("CUBELINK_TEST_MODRINTH_API", "https://api.modrinth.com")];
        assert!(Hooks::from_lookup(env(bad)).is_err());
    }

    #[test]
    fn java_et_relais_invalides() {
        let p: &[(&str, &str)] = &[("CUBELINK_TEST_MODE", "1"), ("CUBELINK_JAVA_PATH", "C:\\Windows\\notepad.exe")];
        assert!(Hooks::from_lookup(env(p)).is_err());
        let p: &[(&str, &str)] = &[("CUBELINK_TEST_MODE", "1"), ("CUBELINK_RELAY_MODE", "bizarre")];
        assert!(Hooks::from_lookup(env(p)).is_err());
        assert!(java_name_ok("fake-java.cmd", true) && !java_name_ok("fake-java.cmd", false) && java_name_ok("java.exe", false));
    }

    #[test]
    fn delai_d_inactivite_de_test() {
        let ok: &[(&str, &str)] = &[("CUBELINK_TEST_MODE", "1"), ("CUBELINK_TRANSFER_IDLE_SECS", "5")];
        assert_eq!(Hooks::from_lookup(env(ok)).unwrap().transfer_idle_secs, Some(5));
        for bad in ["0", "abc", "99999"] {
            let p = [("CUBELINK_TEST_MODE", "1"), ("CUBELINK_TRANSFER_IDLE_SECS", bad)];
            assert!(Hooks::from_lookup(env(&p)).is_err(), "{bad}");
        }
        let off: &[(&str, &str)] = &[("CUBELINK_TRANSFER_IDLE_SECS", "5")];
        assert_eq!(Hooks::from_lookup(env(off)).unwrap().transfer_idle_secs, None);
    }

    #[test]
    fn ports_invalides() {
        for bad in [0, 80, 1023, 65536, 70000] {
            assert!(validate_world_port(bad).is_err(), "{bad}");
        }
        assert_eq!(validate_world_port(1024), Ok(1024));
        assert_eq!(validate_world_port(65535), Ok(65535));
    }

    #[test]
    fn dossier_de_donnees_isole() {
        let p: &[(&str, &str)] = &[("CUBELINK_TEST_MODE", "1"), ("CUBELINK_DATA_DIR", "relatif/profil-a")];
        assert!(Hooks::from_lookup(env(p)).is_err(), "chemin relatif refusé");
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("A"), dir.path().join("B"));
        let (sa, _) = crate::state::test_support::test_state(&a);
        let (sb, _) = crate::state::test_support::test_state(&b);
        assert!(sa.paths.store.starts_with(&a) && sb.paths.store.starts_with(&b));
        assert_ne!(sa.paths.settings_file, sb.paths.settings_file);
        assert_eq!(sa.test_state_info().data_profile, "A");
    }

    #[test]
    fn mode_test_affiche_l_avertissement() {
        let p: &[(&str, &str)] = &[("CUBELINK_TEST_MODE", "1"), ("CUBELINK_AUTH_MS_URL", "http://127.0.0.1:1")];
        let w = Hooks::from_lookup(env(p)).unwrap().warning().unwrap();
        assert!(w.contains("ATTENTION") && w.contains("mode test") && w.contains('1'));
    }
}
