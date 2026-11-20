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
    pub allow_relay: bool,
    pub close_on_launch: bool,
    #[serde(default)]
    pub relay_only: Option<bool>,
    #[serde(default)]
    pub share_mods: Option<bool>,
    #[serde(default)]
    pub discovery: Option<bool>,
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
        allow_relay: i.allow_relay,
        close_on_launch: i.close_on_launch,
        relay_only: i.relay_only.unwrap_or(cur.relay_only),
        share_mods: i.share_mods.unwrap_or(cur.share_mods),
        discovery: i.discovery.unwrap_or(cur.discovery),
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

    /// Cache réel sur disque : taille, vidage, instance et monde intacts, refus pendant une partie.
    #[test]
    fn cache_taille_puis_vidage_sans_toucher_instances_ni_mondes() {
        use core_lib::store::index::object_rel_path;
        use core_lib::store::link::link_into;
        use std::time::Duration;
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let put = |n: u8, size: usize| {
            let sha = format!("{:064x}", n);
            let rel = object_rel_path(&sha).unwrap();
            let p = st.paths.store.join(&rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, vec![n; size]).unwrap();
            st.store.insert(&sha, &rel, size as u64).unwrap();
            p
        };
        let (used, free1, free2) = (put(1, 100), put(2, 1000), put(3, 50));
        let game = st.paths.instances.join("inst").join("minecraft");
        std::fs::create_dir_all(game.join("saves/monde")).unwrap();
        std::fs::write(game.join("saves/monde/level.dat"), b"monde").unwrap();
        link_into(&game, "mods/utilise.jar", &used).unwrap();

        // Les objets tout juste téléchargés sont protégés : rien à libérer pour le vrai délai.
        assert_eq!(st.cache_size().unwrap(), 0);
        assert_eq!(st.cache_clear().unwrap(), 0);
        assert!(free1.exists());

        // Avec des objets « anciens » : la taille annoncée est exactement ce qui est libéré.
        let rep = st.cache_run(true, Duration::ZERO).unwrap();
        assert_eq!(rep.bytes_freed, 1050);
        assert!(free1.exists() && free2.exists(), "le calcul ne supprime rien");
        assert_eq!(st.cache_clear_aged(Duration::ZERO).unwrap(), 1050);
        assert!(!free1.exists() && !free2.exists());
        assert!(used.exists(), "l'objet lié à une instance reste");
        assert_eq!(std::fs::read(game.join("mods/utilise.jar")).unwrap(), vec![1u8; 100]);
        assert_eq!(std::fs::read(game.join("saves/monde/level.dat")).unwrap(), b"monde");
        assert_eq!(st.cache_run(true, Duration::ZERO).unwrap().bytes_freed, 0);

        // Partie ou transfert en cours : refus lisible, rien n'est supprimé.
        *lock(&st.game) = Some("inst".into());
        let err = st.cache_clear_aged(Duration::ZERO).unwrap_err();
        assert!(err.contains("en cours"), "{err}");
        *lock(&st.game) = None;
        assert!(!st.cache_busy.load(std::sync::atomic::Ordering::SeqCst), "le drapeau est relâché");
    }
}

/// Un objet du cache plus récent que ça n'est jamais supprimé : entre son téléchargement et son placement dans une
/// instance, il n'est pas encore lié et ne doit pas disparaître sous un téléchargement en cours.
const CACHE_MIN_AGE: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// Remet `cache_busy` à faux quand le vidage se termine, quelle que soit la sortie.
struct CacheBusy<'a>(&'a std::sync::atomic::AtomicBool);

impl Drop for CacheBusy<'_> {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

impl AppState {
    fn cache_run(&self, dry_run: bool, min_age: std::time::Duration) -> Result<core_lib::store::purge::PurgeReport, String> {
        let roots = [self.paths.instances.clone()];
        let opts = core_lib::store::purge::PurgeOptions { dry_run, min_age };
        core_lib::store::purge::purge_unused(&self.paths.store, &self.store, &roots, &opts).map_err(|e| crate::error::user_msg(&e))
    }

    /// Octets que « Vider le cache » libérerait réellement (objets qu'aucune instance n'utilise). Ne supprime rien.
    pub fn cache_size(&self) -> Result<u64, String> {
        Ok(self.cache_run(true, CACHE_MIN_AGE)?.bytes_freed)
    }

    /// Vide le cache (objets qu'aucune instance n'utilise) ; renvoie les octets libérés.
    pub fn cache_clear(&self) -> Result<u64, String> {
        self.cache_clear_aged(CACHE_MIN_AGE)
    }

    fn cache_clear_aged(&self, min_age: std::time::Duration) -> Result<u64, String> {
        use std::sync::atomic::Ordering;
        if lock(&self.game).is_some() || lock(&self.join).is_some() {
            return Err("Une installation, un transfert ou une partie est en cours. Réessaie quand c'est terminé.".into());
        }
        if self.cache_busy.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            return Err("Le cache est déjà en train d'être vidé.".into());
        }
        let _busy = CacheBusy(&self.cache_busy);
        let rep = self.cache_run(false, min_age)?;
        if rep.bytes_freed == 0 && !rep.errors.is_empty() {
            tracing::warn!("vidage du cache : {}", rep.errors.join(" ; "));
            return Err("Impossible de vider le cache : certains fichiers sont utilisés. Ferme le jeu et réessaie.".into());
        }
        if !rep.errors.is_empty() {
            tracing::warn!("vidage du cache : {}", rep.errors.join(" ; "));
        }
        tracing::info!("cache vidé : {} objets, {} octets", rep.removed.len(), rep.bytes_freed);
        Ok(rep.bytes_freed)
    }
}

/// Taille du cache de mods que « Vider le cache » libérerait (octets) : fichiers du cache qu'aucune instance n'utilise, de plus de 15 min.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn cache_size(state: State<'_, Arc<AppState>>) -> Result<u64, String> {
    let st = state.inner().clone();
    tokio::task::spawn_blocking(move || st.cache_size()).await.map_err(|_| "Le calcul de la taille du cache a échoué.".to_string())?
}

/// Vide le cache de mods (fichiers qu'aucune instance n'utilise, de plus de 15 min) ; renvoie les octets libérés. Erreur lisible pendant un lancement, un transfert ou un autre vidage.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn cache_clear(state: State<'_, Arc<AppState>>) -> Result<u64, String> {
    let st = state.inner().clone();
    tokio::task::spawn_blocking(move || st.cache_clear()).await.map_err(|_| "Le vidage du cache a échoué.".to_string())?
}
