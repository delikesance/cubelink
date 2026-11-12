//! Commandes `settings_get` / `settings_set` : fichier JSON dans le dossier de données.

use std::path::Path;
use std::sync::Arc;

use core_lib::instance::{MAX_RAM_MB, MIN_RAM_MB};
use core_lib::types::{Settings, TrustLevel};
use tauri::State;

use crate::state::{default_settings, lock, AppState};

/// Charge les réglages ; fichier absent ou illisible : valeurs par défaut.
pub fn load_settings(path: &Path) -> Settings {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Settings>(&b).ok())
        .filter(|s| validate(s).is_ok())
        .unwrap_or_else(default_settings)
}

pub fn validate(s: &Settings) -> Result<(), String> {
    validate_with(s, crate::test_hooks::hooks().test_mode)
}

/// `test_mode` : accepte aussi `fake-java.cmd` / `fake-java.sh`.
pub fn validate_with(s: &Settings, test_mode: bool) -> Result<(), String> {
    let lang_ok = !s.language.is_empty()
        && s.language.len() <= 16
        && s.language.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if !lang_ok {
        return Err("langue invalide".into());
    }
    if !(MIN_RAM_MB..=MAX_RAM_MB).contains(&s.default_ram_mb) {
        return Err(format!("la mémoire doit être comprise entre {MIN_RAM_MB} et {MAX_RAM_MB} Mo"));
    }
    if s.relay_only && !s.allow_relay {
        return Err("« Toujours le relais » est incompatible avec un relais désactivé : autorisez le relais ou désactivez « Toujours le relais »".into());
    }
    crate::hotkey::validate_hotkey(&s.overlay_hotkey)?;
    if let Some(j) = &s.java_path {
        let p = Path::new(j);
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_ascii_lowercase();
        if !crate::test_hooks::java_name_ok(&name, test_mode) {
            return Err("le chemin Java doit désigner un exécutable java".into());
        }
        if !p.is_file() {
            return Err("l'exécutable Java indiqué est introuvable".into());
        }
    }
    Ok(())
}

fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}

impl AppState {
    pub fn settings_get(&self) -> Settings {
        lock(&self.settings).clone()
    }

    pub fn settings_set(&self, new: Settings) -> Result<Settings, String> {
        validate(&new)?;
        let data = serde_json::to_vec_pretty(&new).map_err(|_| "réglages non sérialisables".to_string())?;
        write_atomic(&self.paths.settings_file, &data)
            .map_err(|_| "impossible d'enregistrer les réglages".to_string())?;
        *lock(&self.settings) = new.clone();
        // Pris en compte au prochain lancement : un jeu deja ouvert dans la fenetre du launcher y reste.
        self.stage.set_wanted(new.embedded_game);
        Ok(new)
    }
}

#[tauri::command(rename_all = "snake_case")]
pub fn settings_get(state: State<'_, Arc<AppState>>) -> Result<Settings, String> {
    Ok(state.settings_get())
}

/// Le front envoie les champs de `Settings` à plat.
#[tauri::command(rename_all = "snake_case")]
#[allow(clippy::too_many_arguments)]
pub fn settings_set(
    state: State<'_, Arc<AppState>>,
    language: String,
    default_ram_mb: u32,
    java_path: Option<String>,
    trust_default: TrustLevel,
    allow_relay: bool,
    close_on_launch: bool,
    relay_only: Option<bool>,
    share_mods: Option<bool>,
    discovery: Option<bool>,
    lan_auto: Option<bool>,
    overlay_enabled: Option<bool>,
    overlay_hotkey: Option<String>,
    embedded_game: Option<bool>,
    app: tauri::AppHandle,
) -> Result<Settings, String> {
    let cur = state.settings_get();
    let saved = state.settings_set(Settings {
        language,
        default_ram_mb,
        java_path: java_path.filter(|j| !j.trim().is_empty()),
        trust_default,
        allow_relay,
        close_on_launch,
        relay_only: relay_only.unwrap_or(cur.relay_only),
        share_mods: share_mods.unwrap_or(cur.share_mods),
        discovery: discovery.unwrap_or(cur.discovery),
        lan_auto: lan_auto.unwrap_or(cur.lan_auto),
        overlay_enabled: overlay_enabled.unwrap_or(cur.overlay_enabled),
        overlay_hotkey: overlay_hotkey.map(|h| h.trim().to_string()).unwrap_or(cur.overlay_hotkey),
        embedded_game: embedded_game.unwrap_or(cur.embedded_game),
    })?;
    crate::hotkey::apply_settings(&app);
    Ok(saved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::test_state;

    #[test]
    fn aller_retour_et_defauts() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        assert_eq!(st.settings_get(), default_settings());
        let mut s = default_settings();
        s.default_ram_mb = 6144;
        s.close_on_launch = true;
        s.trust_default = TrustLevel::Always;
        assert_eq!(st.settings_set(s.clone()).unwrap(), s);
        // Redémarrage : relecture depuis le fichier.
        let (st2, _) = test_state(dir.path());
        assert_eq!(st2.settings_get(), s);
    }

    #[test]
    fn refuse_valeurs_invalides() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let mut s = default_settings();
        s.default_ram_mb = 10;
        assert!(st.settings_set(s).is_err());
        let mut s = default_settings();
        s.java_path = Some("C:\\Windows\\notepad.exe".into());
        assert!(st.settings_set(s).is_err());
        assert_eq!(st.settings_get(), default_settings());
    }

    #[test]
    fn refuse_relais_seul_sans_relais() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let mut s = default_settings();
        s.relay_only = true;
        s.allow_relay = false;
        let e = st.settings_set(s.clone()).unwrap_err();
        assert!(e.contains("relais"), "{e}");
        assert_eq!(st.settings_get(), default_settings(), "rien n'est enregistré");
        s.allow_relay = true;
        assert!(st.settings_set(s.clone()).is_ok());
        s.relay_only = false;
        s.allow_relay = false;
        assert!(st.settings_set(s).is_ok());
    }

    #[test]
    fn faux_java_seulement_en_mode_test() {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("fake-java.cmd");
        std::fs::write(&fake, b"@echo off").unwrap();
        let mut s = default_settings();
        s.java_path = Some(fake.to_str().unwrap().into());
        assert!(validate_with(&s, true).is_ok());
        assert!(validate_with(&s, false).is_err());
    }

    #[test]
    fn refuse_raccourci_invalide() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let mut s = default_settings();
        s.overlay_hotkey = "Ctrl+Shift+".into();
        let e = st.settings_set(s.clone()).unwrap_err();
        assert!(e.contains("raccourci"), "{e}");
        s.overlay_hotkey = "Alt+F9".into();
        assert!(st.settings_set(s).is_ok());
    }

    #[test]
    fn ancien_settings_json_se_charge_avec_les_defauts_overlay() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("settings.json");
        std::fs::write(&f, br#"{"language":"fr","default_ram_mb":6144,"java_path":null,"trust_default":"ask","allow_relay":true,"close_on_launch":true,"relay_only":false,"share_mods":true}"#).unwrap();
        let s = load_settings(&f);
        assert_eq!(s.default_ram_mb, 6144, "l'ancien fichier est bien lu, pas remplacé par les défauts");
        assert!(s.overlay_enabled);
        assert_eq!(s.overlay_hotkey, "Ctrl+Shift+C");
        assert!(s.embedded_game, "jeu integre : active par defaut pour un ancien fichier");
    }

    #[test]
    fn jeu_integre_enregistre_et_transmis_a_la_scene() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        assert!(st.stage.wanted(), "active par defaut");
        let mut s = default_settings();
        s.embedded_game = false;
        assert!(!st.settings_set(s).unwrap().embedded_game);
        assert!(!st.stage.wanted());
        assert!(!st.stage.available(), "reglage coupe : le prochain lancement est hors de la fenetre du launcher");
        // Redemarrage : le reglage enregistre est relu et transmis.
        let (st2, _) = test_state(dir.path());
        assert!(!st2.settings_get().embedded_game);
        assert!(!st2.stage.wanted());
    }

    #[test]
    fn fichier_corrompu_donne_les_defauts() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("settings.json"), b"{pas du json").unwrap();
        assert_eq!(load_settings(&dir.path().join("settings.json")), default_settings());
    }
}
