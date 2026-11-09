//! Commandes d'instances : liste, création (avec installation), import .mrpack, lancement.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use core_lib::instance::{MAX_RAM_MB, MIN_RAM_MB};
use core_lib::launch_args::{build_command, LaunchSpec};
use core_lib::launch_run::{self, LaunchEvent, LaunchPhase, LaunchStateEvent};
use core_lib::mojang::libraries::{Arch, Os};
use core_lib::mojang::versions::release_ids;
use core_lib::mrpack::{self, ImportOptions};
use core_lib::types::{Instance, Loader, LoaderSpec, Profile};
use tauri::State;

use crate::error::{sanitize, user_msg};
use crate::install::{ensure_game, resolve_loader_version, safe_version, Progress};
use crate::state::{lock, AppState};

pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 80 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Validation locale (sans réseau) d'une demande de création.
pub fn validate_create(name: &str, minecraft: &str, loader: &LoaderSpec, ram_mb: u32) -> Result<(), String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("le nom de l'instance est vide".into());
    }
    if n.chars().count() > 64 {
        return Err("le nom de l'instance est trop long (64 caractères maximum)".into());
    }
    if !safe_version(minecraft) {
        return Err("version de Minecraft invalide".into());
    }
    if loader.loader != Loader::Vanilla && !loader.version.is_empty() && !safe_version(&loader.version) {
        return Err("version du loader invalide".into());
    }
    if loader.loader == Loader::Vanilla && !loader.version.is_empty() {
        return Err("une instance vanilla n'a pas de version de loader".into());
    }
    if !(MIN_RAM_MB..=MAX_RAM_MB).contains(&ram_mb) {
        return Err(format!("la mémoire doit être comprise entre {MIN_RAM_MB} et {MAX_RAM_MB} Mo"));
    }
    Ok(())
}

/// Le fichier doit exister et porter l'extension `.mrpack`.
pub fn validate_mrpack_path(path: &str) -> Result<PathBuf, String> {
    let p = Path::new(path);
    let ext_ok = p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("mrpack"));
    if !ext_ok {
        return Err("le fichier doit avoir l'extension .mrpack".into());
    }
    if !p.is_file() {
        return Err("le fichier .mrpack est introuvable".into());
    }
    p.canonicalize().map_err(|_| "le fichier .mrpack est illisible".to_string())
}

/// Traduit un événement de lancement en état du contrat.
pub fn state_for(id: &str, ev: &LaunchEvent) -> Option<LaunchStateEvent> {
    let mk = |state, message: Option<String>| Some(LaunchStateEvent { id: id.to_string(), state, message });
    match ev {
        LaunchEvent::Started { .. } => mk(LaunchPhase::Running, None),
        LaunchEvent::Output(_) => None,
        LaunchEvent::Exited { .. } => mk(LaunchPhase::Exited, None),
        LaunchEvent::Failed { message } => mk(LaunchPhase::Failed, Some(sanitize(message))),
    }
}

/// Libère `AppState.game` à la fin de la tâche de lancement.
struct GameGuard(Arc<AppState>);
impl Drop for GameGuard {
    fn drop(&mut self) {
        *lock(&self.0.game) = None;
    }
}

impl AppState {
    pub fn instances_list(&self) -> Vec<Instance> {
        self.instances.list()
    }

    pub async fn instances_create(&self, name: String, minecraft: String, loader: LoaderSpec, ram_mb: u32) -> Result<Instance, String> {
        validate_create(&name, &minecraft, &loader, ram_mb)?;
        let version = resolve_loader_version(&minecraft, &loader).await.map_err(|e| user_msg(&e))?;
        let spec = LoaderSpec { loader: loader.loader, version };
        let inst = self.instances.create(name.trim(), &minecraft, spec, ram_mb).map_err(|e| user_msg(&e))?;
        let java = self.settings_get().java_path;
        let noop: Progress = Arc::new(|_| {});
        match ensure_game(&self.paths, &inst, java, noop).await {
            Ok(_) => Ok(inst),
            Err(e) => {
                let _ = self.instances.delete(&inst.id);
                Err(user_msg(&e))
            }
        }
    }

    pub async fn instances_import_mrpack(&self, path: &str) -> Result<Instance, String> {
        let file = validate_mrpack_path(path)?;
        let mut opts = ImportOptions { ram_mb: self.settings_get().default_ram_mb, ..ImportOptions::default() };
        if crate::test_hooks::hooks().test_mode {
            // Mode test seulement : télécharge depuis le faux Modrinth en loopback.
            opts.allowed_hosts.extend(["127.0.0.1".to_string(), "localhost".to_string()]);
            opts.allow_http = true;
        }
        // Le futur d'`import` n'est pas prouvé `Send` par rustc : on l'exécute sur un thread dédié.
        let (instances, index, store_root) = (self.instances.clone(), self.store.clone(), self.paths.store.clone());
        let handle = tokio::runtime::Handle::current();
        let inst = tokio::task::spawn_blocking(move || {
            handle.block_on(mrpack::import(&file, &instances, &store_root, &index, &opts, None, Arc::new(AtomicBool::new(false))))
        })
        .await
        .map_err(|_| "import interrompu".to_string())?
        .map_err(|e| user_msg(&e))?;
        let noop: Progress = Arc::new(|_| {});
        match ensure_game(&self.paths, &inst, self.settings_get().java_path, noop).await {
            Ok(_) => Ok(inst),
            Err(e) => {
                let _ = self.instances.delete(&inst.id);
                Err(user_msg(&e))
            }
        }
    }

    pub async fn minecraft_versions(&self) -> Result<Vec<String>, String> {
        let m = crate::install::versions_client().fetch_manifest().await.map_err(|e| user_msg(&e))?;
        Ok(release_ids(&m))
    }

    /// Vérifications synchrones puis tâche de fond ; la progression passe par `launch://state`.
    pub async fn instances_launch(self: &Arc<Self>, id: &str) -> Result<(), String> {
        if !valid_id(id) {
            return Err("identifiant d'instance invalide".into());
        }
        let inst = self.instances.get(id).map_err(|e| user_msg(&e))?;
        let profile = self.auth_status().ok_or("connectez-vous avant de lancer le jeu")?;
        let token = match &profile {
            Profile::Online { .. } => Some(self.online_token()?),
            Profile::Offline { .. } => None,
        };
        {
            let mut g = lock(&self.game);
            if g.is_some() {
                return Err("une partie est déjà en cours de préparation ou de jeu".into());
            }
            *g = Some(inst.id.clone());
        }
        let me = self.clone();
        // Runtime de Tauri (et non `tokio::spawn`) : valable même appelée hors d'un runtime tokio.
        tauri::async_runtime::spawn(async move {
            let _guard = GameGuard(me.clone());
            me.run_game(inst, profile, token).await;
        });
        Ok(())
    }

    fn emit_state(&self, id: &str, state: LaunchPhase, message: Option<String>) {
        self.emit_event("launch://state", &LaunchStateEvent { id: id.to_string(), state, message });
    }

    async fn run_game(self: &Arc<Self>, inst: Instance, profile: Profile, token: Option<String>) {
        let id = inst.id.clone();
        self.emit_state(&id, LaunchPhase::Preparing, None);
        let me = self.clone();
        let pid = id.clone();
        let progress: Progress = Arc::new(move |m| me.emit_state(&pid, LaunchPhase::Preparing, Some(m)));
        let prepared = match ensure_game(&self.paths, &inst, self.settings_get().java_path, progress).await {
            Ok(p) => p,
            Err(e) => return self.emit_state(&id, LaunchPhase::Failed, Some(user_msg(&e))),
        };
        let game_dir = match self.instances.game_dir(&id) {
            Ok(d) => d,
            Err(e) => return self.emit_state(&id, LaunchPhase::Failed, Some(user_msg(&e))),
        };
        let spec = LaunchSpec {
            java_path: prepared.java,
            game_dir,
            assets_dir: prepared.assets_dir,
            libs_dir: prepared.libs_dir,
            natives_dir: prepared.natives_dir,
            client_jar: prepared.client_jar,
            version: prepared.version,
            profile,
            access_token: token,
            version_type: "release".into(),
            ram_mb: inst.ram_mb,
            os: Os::current(),
            arch: Arch::current(),
            launcher_name: "Cubelink".into(),
            launcher_version: env!("CARGO_PKG_VERSION").into(),
            extra_jvm_args: vec![],
        };
        let cmd = match build_command(&spec) {
            Ok(c) => c,
            Err(e) => return self.emit_state(&id, LaunchPhase::Failed, Some(user_msg(&e))),
        };
        drop(spec);
        let log_dir = self.paths.logs.join(&id);
        // `game` reste vivant jusqu'à la fin de la boucle : le lâcher arrêterait le jeu.
        let mut game = match launch_run::spawn(&cmd, &log_dir).await {
            Ok(g) => g,
            Err(e) => return self.emit_state(&id, LaunchPhase::Failed, Some(user_msg(&e))),
        };
        let Some(mut rx) = game.events() else {
            return self.emit_state(&id, LaunchPhase::Failed, Some("suivi du jeu indisponible".into()));
        };
        let mut finished = false;
        while let Some(ev) = rx.recv().await {
            if let Some(s) = state_for(&id, &ev) {
                finished = matches!(s.state, LaunchPhase::Exited | LaunchPhase::Failed);
                self.emit_event("launch://state", &s);
            }
            if finished {
                break;
            }
        }
        if !finished {
            self.emit_state(&id, LaunchPhase::Exited, None);
        }
        game.wait().await;
    }
}

#[tauri::command(rename_all = "snake_case")]
pub fn instances_list(state: State<'_, Arc<AppState>>) -> Result<Vec<Instance>, String> {
    Ok(state.instances_list())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn instances_create(
    state: State<'_, Arc<AppState>>,
    name: String,
    minecraft: String,
    loader: LoaderSpec,
    ram_mb: u32,
) -> Result<Instance, String> {
    state.instances_create(name, minecraft, loader, ram_mb).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn instances_import_mrpack(state: State<'_, Arc<AppState>>, path: String) -> Result<Instance, String> {
    state.instances_import_mrpack(&path).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn instances_launch(state: State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
    state.inner().instances_launch(&id).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn minecraft_versions(state: State<'_, Arc<AppState>>) -> Result<Vec<String>, String> {
    state.minecraft_versions().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::test_state;

    fn vanilla() -> LoaderSpec {
        LoaderSpec { loader: Loader::Vanilla, version: String::new() }
    }

    #[tokio::test]
    async fn creation_invalide_donne_une_erreur_claire() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let e = st.instances_create("  ".into(), "1.21.1".into(), vanilla(), 4096).await.unwrap_err();
        assert!(e.contains("nom"), "{e}");
        let e = st.instances_create("Mon monde".into(), "../x".into(), vanilla(), 4096).await.unwrap_err();
        assert!(e.contains("Minecraft"), "{e}");
        let e = st.instances_create("Mon monde".into(), "1.21.1".into(), vanilla(), 10).await.unwrap_err();
        assert!(e.contains("mémoire"), "{e}");
        let bad = LoaderSpec { loader: Loader::Fabric, version: "a b".into() };
        assert!(st.instances_create("Mon monde".into(), "1.21.1".into(), bad, 4096).await.is_err());
        assert!(st.instances_list().is_empty(), "aucune instance ne doit rester");
    }

    #[test]
    fn mrpack_valide_extension_et_existence() {
        let dir = tempfile::tempdir().unwrap();
        let txt = dir.path().join("a.txt");
        std::fs::write(&txt, b"x").unwrap();
        assert!(validate_mrpack_path(txt.to_str().unwrap()).unwrap_err().contains(".mrpack"));
        assert!(validate_mrpack_path(dir.path().join("absent.mrpack").to_str().unwrap()).unwrap_err().contains("introuvable"));
        let ok = dir.path().join("p.MRPACK");
        std::fs::write(&ok, b"x").unwrap();
        assert!(validate_mrpack_path(ok.to_str().unwrap()).is_ok());
    }

    #[test]
    fn evenements_de_lancement() {
        assert_eq!(state_for("a", &LaunchEvent::Started { pid: 1 }).unwrap().state, LaunchPhase::Running);
        assert!(state_for("a", &LaunchEvent::Output("x".into())).is_none());
        let f = state_for("a", &LaunchEvent::Failed { message: "échec dans C:\\Users\\bob\\x".into() }).unwrap();
        assert_eq!(f.state, LaunchPhase::Failed);
        assert!(!f.message.unwrap().contains("bob"));
    }

    #[tokio::test]
    async fn lancement_refuse_sans_profil_ou_id_invalide() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        assert!(st.instances_launch("../x").await.is_err());
        assert!(st.instances_launch("inconnue").await.is_err());
        let inst = st.instances.create("Test", "1.21.1", vanilla(), 2048).unwrap();
        assert!(st.instances_launch(&inst.id).await.unwrap_err().contains("connectez-vous"));
    }

    /// Régression : le lancement lançait `tokio::spawn` depuis une commande synchrone (« no reactor running »).
    /// Ici AUCUN runtime tokio n'est actif sur le thread de test : le chemin qui lance la tâche ne doit pas paniquer.
    #[test]
    fn lancement_ne_panique_pas_hors_runtime_tokio() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        assert!(tokio::runtime::Handle::try_current().is_err());
        // Version inexistante : la tâche de fond échoue vite, sans télécharger le jeu.
        let inst = st.instances.create("Test", "0.0-inexistante", vanilla(), 2048).unwrap();
        *lock(&st.profile) = Some(Profile::Offline { name: "Steve".into(), uuid: "0".repeat(32) });
        let (st2, id) = (st.clone(), inst.id.clone());
        // Comme Tauri : le futur de la commande est piloté par le runtime de Tauri.
        let r = tauri::async_runtime::block_on(async move { st2.instances_launch(&id).await });
        assert!(r.is_ok(), "{r:?}");
    }
}
