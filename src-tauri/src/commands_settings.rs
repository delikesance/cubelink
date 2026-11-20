//! Commandes `settings_get` / `settings_set` : fichier JSON dans le dossier de données.

use std::path::Path;
use std::sync::Arc;

use core_lib::instance::{MAX_RAM_MB, MIN_RAM_MB};
use core_lib::types::{Settings, TrustLevel};
use tauri::State;

use crate::state::{default_settings, lock, AppState};

/// Charge les réglages ; fichier absent ou qui n'est pas un objet JSON : valeurs par défaut.
///
/// Sinon on répare champ par champ : un champ illisible ou invalide revient à sa valeur par défaut, tous les autres
/// réglages sont gardés (découverte, démarrage avec la session, relais...). L'état du disque (JDK disparu, disque
/// externe débranché) ne compte pas comme une erreur de réglage : le chemin Java est simplement ignoré. Rien n'est
/// réécrit au chargement : le fichier d'origine reste intact jusqu'au prochain `settings_set`.
pub fn load_settings(path: &Path) -> Settings {
    let Some(raw) = std::fs::read(path).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()) else {
        return default_settings();
    };
    let Some(obj) = raw.as_object() else { return default_settings() };
    let (s, warnings) = sanitize(overlay_on_defaults(obj), crate::test_hooks::hooks().test_mode);
    for w in warnings {
        eprintln!("[Cubelink] réglages : {w}");
    }
    s
}

/// Part des valeurs par défaut et superpose les clés du fichier une à une ; une clé de mauvais type est ignorée.
fn overlay_on_defaults(obj: &serde_json::Map<String, serde_json::Value>) -> Settings {
    let mut cur = match serde_json::to_value(default_settings()) {
        Ok(serde_json::Value::Object(m)) => m,
        _ => return default_settings(),
    };
    for (k, v) in obj {
        if !cur.contains_key(k) {
            continue;
        }
        let old = cur.insert(k.clone(), v.clone());
        if serde_json::from_value::<Settings>(serde_json::Value::Object(cur.clone())).is_err() {
            eprintln!("[Cubelink] réglages : valeur illisible pour « {k} », défaut utilisé");
            if let Some(o) = old {
                cur.insert(k.clone(), o);
            }
        }
    }
    serde_json::from_value(serde_json::Value::Object(cur)).unwrap_or_else(|_| default_settings())
}

/// Corrige les champs structurellement invalides en gardant le reste. Ne regarde pas le disque.
fn sanitize(mut s: Settings, test_mode: bool) -> (Settings, Vec<String>) {
    let d = default_settings();
    let mut w = Vec::new();
    if !lang_ok(&s.language) {
        w.push("langue invalide, défaut utilisé".to_string());
        s.language = d.language;
    }
    if !(MIN_RAM_MB..=MAX_RAM_MB).contains(&s.default_ram_mb) {
        w.push(format!("mémoire hors bornes ({} Mo), défaut utilisé", s.default_ram_mb));
        s.default_ram_mb = d.default_ram_mb;
    }
    if crate::hotkey::validate_hotkey(&s.overlay_hotkey).is_err() {
        w.push("raccourci invalide, défaut utilisé".to_string());
        s.overlay_hotkey = d.overlay_hotkey;
    }
    if let Some(j) = &s.java_path {
        let p = Path::new(j);
        if java_path_name_err(p, test_mode).is_some() {
            w.push(format!("java_path invalide, ignoré ({j})"));
            s.java_path = None;
        } else if !p.is_file() {
            w.push(format!("java_path introuvable, ignoré ({j})"));
            s.java_path = None;
        }
    }
    (s, w)
}

fn lang_ok(l: &str) -> bool {
    !l.is_empty() && l.len() <= 16 && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn java_path_name_err(p: &Path, test_mode: bool) -> Option<&'static str> {
    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_ascii_lowercase();
    (!crate::test_hooks::java_name_ok(&name, test_mode)).then_some("le chemin Java doit désigner un exécutable java")
}

pub fn validate(s: &Settings) -> Result<(), String> {
    validate_with(s, crate::test_hooks::hooks().test_mode)
}

/// Validation à l'enregistrement : règles structurelles puis existence du fichier Java sur le disque.
/// `test_mode` : accepte aussi `fake-java.cmd` / `fake-java.sh`.
pub fn validate_with(s: &Settings, test_mode: bool) -> Result<(), String> {
    if !lang_ok(&s.language) {
        return Err("langue invalide".into());
    }
    if !(MIN_RAM_MB..=MAX_RAM_MB).contains(&s.default_ram_mb) {
        return Err(format!("la mémoire doit être comprise entre {MIN_RAM_MB} et {MAX_RAM_MB} Mo"));
    }
    crate::hotkey::validate_hotkey(&s.overlay_hotkey)?;
    if let Some(j) = &s.java_path {
        let p = Path::new(j);
        if let Some(e) = java_path_name_err(p, test_mode) {
            return Err(e.into());
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

/// Réglages.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub fn settings_get(state: State<'_, Arc<AppState>>) -> Result<Settings, String> {
    Ok(state.settings_get())
}

/// Entrée de `settings_set` : les champs de `Settings` ; les plus récents sont facultatifs (absents : valeur actuelle gardée).
#[derive(Debug, Clone, serde::Deserialize, specta::Type)]
pub struct SettingsInput {
    pub language: String,
    pub default_ram_mb: u32,
    #[serde(default)]
    pub java_path: Option<String>,
    pub trust_default: TrustLevel,
    pub close_on_launch: bool,
    #[serde(default)]
    pub share_mods: Option<bool>,
    #[serde(default)]
    pub lan_auto: Option<bool>,
    #[serde(default)]
    pub overlay_enabled: Option<bool>,
    #[serde(default)]
    pub overlay_hotkey: Option<String>,
    #[serde(default)]
    pub embedded_game: Option<bool>,
    #[serde(default)]
    pub start_with_os: Option<bool>,
    #[serde(default)]
    pub open_to_all_friends: Option<bool>,
}

/// Le front envoie les réglages dans `{ settings }`.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub fn settings_set(state: State<'_, Arc<AppState>>, settings: SettingsInput, app: tauri::AppHandle) -> Result<Settings, String> {
    let i = settings;
    let cur = state.settings_get();
    let saved = state.settings_set(Settings {
        language: i.language,
        default_ram_mb: i.default_ram_mb,
        java_path: i.java_path.filter(|j| !j.trim().is_empty()),
        trust_default: i.trust_default,
        close_on_launch: i.close_on_launch,
        share_mods: i.share_mods.unwrap_or(cur.share_mods),
        lan_auto: i.lan_auto.unwrap_or(cur.lan_auto),
        overlay_enabled: i.overlay_enabled.unwrap_or(cur.overlay_enabled),
        overlay_hotkey: i.overlay_hotkey.map(|h| h.trim().to_string()).unwrap_or(cur.overlay_hotkey),
        embedded_game: i.embedded_game.unwrap_or(cur.embedded_game),
        start_with_os: i.start_with_os.unwrap_or(cur.start_with_os),
        open_to_all_friends: i.open_to_all_friends.unwrap_or(cur.open_to_all_friends),
    })?;
    crate::hotkey::apply_settings(&app);
    crate::background::apply_autostart(&app);
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

    /// #55 : plus aucun relais n0 ; `allow_relay` et `relay_only` n'existent plus. Un ancien fichier qui les contient
    /// (même contradictoires) se charge sans erreur, ses autres champs sont gardés, et ils disparaissent à la
    /// réécriture.
    #[test]
    fn anciens_reglages_relais_ignores_puis_supprimes() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let f = &st.paths.settings_file;
        std::fs::write(f, br#"{"language":"fr","default_ram_mb":6144,"java_path":null,"trust_default":"ask","allow_relay":false,"close_on_launch":true,"relay_only":true,"share_mods":true,"discovery":true}"#).unwrap();
        let s = load_settings(f);
        assert_eq!(s.default_ram_mb, 6144, "l'ancien fichier est bien lu, pas remplacé par les défauts");
        assert!(s.close_on_launch);
        st.settings_set(s).unwrap();
        let rewritten = std::fs::read_to_string(f).unwrap();
        for k in ["allow_relay", "relay_only", "discovery"] {
            assert!(!rewritten.contains(k), "{k} doit disparaître à la réécriture : {rewritten}");
        }
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

    fn ecrire(dir: &Path, json: &str) -> std::path::PathBuf {
        let f = dir.join("settings.json");
        std::fs::write(&f, json).unwrap();
        f
    }

    const PRIVES: &str = r#""trust_default":"always","allow_relay":false,"close_on_launch":true,"relay_only":false,"share_mods":false,"discovery":false,"lan_auto":false,"overlay_enabled":false,"embedded_game":false,"open_to_all_friends":false,"start_with_os":false"#;

    fn verifie_prives(s: &Settings) {
        assert_eq!(s.trust_default, TrustLevel::Always);
        assert!(!s.share_mods && !s.lan_auto);
        assert!(!s.overlay_enabled && !s.embedded_game && !s.start_with_os && s.close_on_launch);
    }

    #[test]
    fn java_path_disparu_conserve_les_autres_reglages() {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("fake-java.sh");
        let json = format!(
            r#"{{"language":"en","default_ram_mb":6144,"java_path":{:?},"overlay_hotkey":"Alt+F9",{PRIVES}}}"#,
            fake.to_str().unwrap()
        );
        let f = ecrire(dir.path(), &json);
        let (s, w) = sanitize(overlay_on_defaults(serde_json::from_str::<serde_json::Value>(&json).unwrap().as_object().unwrap()), true);
        assert_eq!(w.len(), 1, "{w:?}");
        assert_eq!(s.java_path, None);
        verifie_prives(&s);
        assert_eq!((s.language.as_str(), s.default_ram_mb, s.overlay_hotkey.as_str()), ("en", 6144, "Alt+F9"));
        // Chemin réel (mode test désactivé : le nom fake-java.sh est refusé) : le reste est conservé aussi.
        let l = load_settings(&f);
        verifie_prives(&l);
        assert_eq!(l.default_ram_mb, 6144);
        assert_eq!(l.java_path, None);
        assert_eq!(std::fs::read_to_string(&f).unwrap(), json, "le fichier n'est pas réécrit au chargement");
        // Le disque revient : le chemin est retrouvé.
        std::fs::write(&fake, b"#!/bin/sh").unwrap();
        let (s2, w2) = sanitize(overlay_on_defaults(serde_json::from_str::<serde_json::Value>(&json).unwrap().as_object().unwrap()), true);
        assert!(w2.is_empty(), "{w2:?}");
        assert_eq!(s2.java_path.as_deref(), fake.to_str());
    }

    #[test]
    fn java_path_au_nom_invalide_ignore_le_reste_garde() {
        let dir = tempfile::tempdir().unwrap();
        let json = format!(r#"{{"language":"fr","default_ram_mb":6144,"java_path":"C:\\Windows\\notepad.exe",{PRIVES}}}"#);
        let s = load_settings(&ecrire(dir.path(), &json));
        assert_eq!(s.java_path, None);
        verifie_prives(&s);
        assert_eq!(s.default_ram_mb, 6144);
    }

    #[test]
    fn raccourci_invalide_ne_reinitialise_que_le_raccourci() {
        let dir = tempfile::tempdir().unwrap();
        let json = format!(r#"{{"language":"en","default_ram_mb":6144,"java_path":null,"overlay_hotkey":"Ctrl+Shift+",{PRIVES}}}"#);
        let s = load_settings(&ecrire(dir.path(), &json));
        assert_eq!(s.overlay_hotkey, "Ctrl+Shift+C");
        verifie_prives(&s);
        assert_eq!((s.language.as_str(), s.default_ram_mb), ("en", 6144));
    }

    #[test]
    fn ram_hors_bornes_ne_reinitialise_que_la_ram() {
        let dir = tempfile::tempdir().unwrap();
        let json = format!(r#"{{"language":"en","default_ram_mb":10,"java_path":null,"overlay_hotkey":"Alt+F9",{PRIVES}}}"#);
        let s = load_settings(&ecrire(dir.path(), &json));
        assert_eq!(s.default_ram_mb, 4096);
        verifie_prives(&s);
        assert_eq!((s.language.as_str(), s.overlay_hotkey.as_str()), ("en", "Alt+F9"));
    }

    #[test]
    fn langue_invalide_ne_reinitialise_que_la_langue() {
        let dir = tempfile::tempdir().unwrap();
        let json = format!(r#"{{"language":"fr FR!","default_ram_mb":6144,"java_path":null,{PRIVES}}}"#);
        let s = load_settings(&ecrire(dir.path(), &json));
        assert_eq!(s.language, "fr");
        assert_eq!(s.default_ram_mb, 6144);
        verifie_prives(&s);
    }

    #[test]
    fn champ_de_mauvais_type_ne_reinitialise_que_ce_champ() {
        let dir = tempfile::tempdir().unwrap();
        let json = r#"{"language":"fr","default_ram_mb":"abc","java_path":null,"trust_default":"inconnu","allow_relay":false,"close_on_launch":false,"relay_only":false,"share_mods":true,"discovery":false,"start_with_os":false}"#;
        let s = load_settings(&ecrire(dir.path(), json));
        assert_eq!(s.default_ram_mb, 4096);
        assert_eq!(s.trust_default, TrustLevel::Ask);
        assert!(!s.start_with_os);
    }

    #[test]
    fn json_qui_n_est_pas_un_objet_donne_les_defauts() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_settings(&ecrire(dir.path(), "[1,2]")), default_settings());
        assert_eq!(load_settings(&dir.path().join("absent.json")), default_settings());
    }

    #[test]
    fn redemarrage_avec_java_disparu_garde_les_reglages_et_permet_de_sauvegarder() {
        let dir = tempfile::tempdir().unwrap();
        let json = format!(r#"{{"language":"fr","default_ram_mb":6144,"java_path":"/disque/absent/java",{PRIVES}}}"#);
        ecrire(dir.path(), &json);
        let (st, _) = test_state(dir.path());
        let s = st.settings_get();
        assert!(!s.start_with_os);
        assert_eq!(s.java_path, None);
        // Ce que renvoie le front après ouverture des Réglages s'enregistre sans erreur.
        assert_eq!(st.settings_set(s.clone()).unwrap(), s);
        // Un chemin disparu saisi à la main reste refusé à l'enregistrement.
        let mut bad = s;
        bad.java_path = Some("/disque/absent/java".into());
        assert!(st.settings_set(bad).unwrap_err().contains("introuvable"));
    }
}
