//! « Parcourir » (recherche / installation CurseForge et Modrinth) et gestion des mods d'une instance.
//!
//! La logique est dans `core_lib::browse` et `core_lib::mods` ; ici : état de l'application, événements
//! `browse://progress`, création d'instance pour les modpacks et surcharges de test.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use core_lib::browse::{self, BrowseInstallInput, BrowseKind, BrowsePage, BrowseQuery, BrowseVersion, Providers};
use core_lib::mods::{self, ModEntry};
use core_lib::providers::curseforge::CfResolver;
use core_lib::providers::curseforge_api::CfApi;
use core_lib::providers::modrinth_api::ModrinthClient;
use core_lib::providers::modrinth_dl::DlOptions;
use core_lib::types::Instance;
use serde::Serialize;
use tauri::State;

use crate::commands_instances::valid_id;
use crate::error::user_msg;
use crate::state::{lock, AppState};

/// Clé factice envoyée au faux serveur CurseForge en mode test.
pub const TEST_CF_KEY: &str = "test-key";

/// Résolveur CurseForge (import de modpack) : en mode test, tout passe par `CUBELINK_TEST_CF_API`.
pub fn cf_resolver() -> CfResolver {
    match &crate::test_hooks::hooks().cf_api {
        Some(base) => CfResolver::with_base_url(base.clone()).with_api(base.clone(), TEST_CF_KEY),
        None => CfResolver::new(),
    }
}

pub(crate) fn providers() -> Providers {
    let h = crate::test_hooks::hooks();
    let ua = concat!("Cubelink/", env!("CARGO_PKG_VERSION"));
    let modrinth = match h.modrinth_base() {
        Some(u) => ModrinthClient::with_base_url(u, ua),
        None => ModrinthClient::new(ua),
    };
    let cf = match &h.cf_api {
        Some(b) => CfApi::with_base(b.clone(), TEST_CF_KEY),
        None => CfApi::new(),
    };
    Providers { modrinth, cf, allow_loopback: h.test_mode }
}

#[derive(Debug, Clone, Serialize)]
pub struct BrowseInstallResult {
    pub instance: Instance,
    /// Noms des fichiers posés dans l'instance (modpack : vide).
    pub installed: Vec<String>,
    pub missing: Vec<String>,
}

#[derive(Serialize, Clone)]
struct ProgressEvent<'a> {
    done: u32,
    total: u32,
    label: &'a str,
}

impl AppState {
    fn emit_browse_progress(&self, done: u32, total: u32, label: &str) {
        self.emit_event("browse://progress", &ProgressEvent { done, total, label });
    }

    /// L'instance existe ; son dossier de jeu.
    fn instance_game_dir(&self, id: &str) -> Result<(Instance, PathBuf), String> {
        if !valid_id(id) {
            return Err("identifiant d'instance invalide".into());
        }
        let inst = self.instances.get(id).map_err(|e| user_msg(&e))?;
        let dir = self.instances.game_dir(id).map_err(|e| user_msg(&e))?;
        Ok((inst, dir))
    }

    /// Modifier les mods pendant que le jeu tourne (ou se prépare) corromprait l'instance.
    fn ensure_not_running(&self, id: &str) -> Result<(), String> {
        if lock(&self.game).as_deref() == Some(id) {
            return Err("le jeu est en cours d'exécution : ferme-le avant de modifier les mods de cette instance".into());
        }
        Ok(())
    }

    pub async fn browse_search(&self, query: BrowseQuery) -> Result<BrowsePage, String> {
        browse::search(&providers(), &query).await.map_err(|e| user_msg(&e))
    }

    pub async fn browse_versions(
        &self,
        source: &str,
        id: &str,
        kind: &str,
        minecraft: Option<&str>,
        loader: Option<&str>,
    ) -> Result<Vec<BrowseVersion>, String> {
        browse::versions(&providers(), source, id, kind, minecraft, loader).await.map_err(|e| user_msg(&e))
    }

    pub async fn browse_install(&self, input: BrowseInstallInput) -> Result<BrowseInstallResult, String> {
        let p = providers();
        let dl = DlOptions::default();
        let cb = |d: u32, t: u32, l: &str| self.emit_browse_progress(d, t, l);
        if input.kind == BrowseKind::Modpack {
            return self.install_modpack(&p, &input, &dl).await;
        }
        let id = input.instance_id.as_deref().ok_or("choisis l'instance dans laquelle installer ce contenu")?;
        let (inst, game_dir) = self.instance_game_dir(id)?;
        self.ensure_not_running(id)?;
        let report = browse::install_content(&p, &input, &inst, &game_dir, &self.paths.store, &self.store, &dl, &cb)
            .await
            .map_err(|e| user_msg(&e))?;
        let instance = self.instances.get(id).map_err(|e| user_msg(&e))?;
        Ok(BrowseInstallResult { instance, installed: report.installed, missing: report.missing })
    }

    async fn install_modpack(&self, p: &Providers, input: &BrowseInstallInput, dl: &DlOptions) -> Result<BrowseInstallResult, String> {
        let tmp = self.paths.root.join("tmp").join(format!("browse-{}", uuid_like()));
        let cb = |d: u32, t: u32, l: &str| self.emit_browse_progress(d, t, l);
        let file = browse::fetch_modpack(p, input, &self.paths.store, &self.store, &tmp, dl, &cb).await.map_err(|e| {
            let _ = std::fs::remove_dir_all(&tmp);
            user_msg(&e)
        })?;
        let me_emit = self.emit.clone();
        let progress: core_lib::mrpack::FilesProgress = Arc::new(move |d, t| {
            me_emit(
                "browse://progress",
                serde_json::json!({ "done": d, "total": t, "label": "Installation du modpack" }),
            );
        });
        let res = self.import_pack_file(&file.display().to_string(), None, None, None, Some(progress)).await;
        let _ = std::fs::remove_dir_all(&tmp);
        let (instance, missing) = res?;
        // Illustrations : meilleur effort, un échec n'affecte jamais l'installation.
        let instance = core_lib::art::try_apply_art(p, &self.instances, instance, input.source, &input.id).await;
        Ok(BrowseInstallResult { instance, installed: Vec::new(), missing })
    }

    pub async fn mods_list(&self, instance_id: &str) -> Result<Vec<ModEntry>, String> {
        let (_, dir) = self.instance_game_dir(instance_id)?;
        mods::list(&dir).map_err(|e| user_msg(&e))
    }

    pub async fn mods_toggle(&self, instance_id: &str, file: &str, enabled: bool) -> Result<(), String> {
        let (_, dir) = self.instance_game_dir(instance_id)?;
        self.ensure_not_running(instance_id)?;
        mods::toggle(&dir, file, enabled).map_err(|e| user_msg(&e))
    }

    pub async fn mods_remove(&self, instance_id: &str, file: &str) -> Result<(), String> {
        let (_, dir) = self.instance_game_dir(instance_id)?;
        self.ensure_not_running(instance_id)?;
        mods::remove(&dir, file).map_err(|e| user_msg(&e))
    }

    pub async fn mods_add(&self, instance_id: &str, path: &str) -> Result<ModEntry, String> {
        let (_, dir) = self.instance_game_dir(instance_id)?;
        self.ensure_not_running(instance_id)?;
        let src = Path::new(path);
        if !src.is_absolute() {
            return Err("chemin de fichier invalide".into());
        }
        let src = src.to_path_buf();
        tokio::task::spawn_blocking(move || mods::add(&dir, &src))
            .await
            .map_err(|_| "ajout interrompu".to_string())?
            .map_err(|e| user_msg(&e))
    }
}

fn uuid_like() -> String {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{n:x}-{}", std::process::id())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn browse_search(state: State<'_, Arc<AppState>>, query: BrowseQuery) -> Result<BrowsePage, String> {
    state.browse_search(query).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn browse_versions(
    state: State<'_, Arc<AppState>>,
    source: String,
    id: String,
    kind: String,
    minecraft: Option<String>,
    loader: Option<String>,
) -> Result<Vec<BrowseVersion>, String> {
    state.browse_versions(&source, &id, &kind, minecraft.as_deref(), loader.as_deref()).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn browse_install(state: State<'_, Arc<AppState>>, input: BrowseInstallInput) -> Result<BrowseInstallResult, String> {
    state.browse_install(input).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn mods_list(state: State<'_, Arc<AppState>>, instance_id: String) -> Result<Vec<ModEntry>, String> {
    state.mods_list(&instance_id).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn mods_toggle(state: State<'_, Arc<AppState>>, instance_id: String, file: String, enabled: bool) -> Result<(), String> {
    state.mods_toggle(&instance_id, &file, enabled).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn mods_remove(state: State<'_, Arc<AppState>>, instance_id: String, file: String) -> Result<(), String> {
    state.mods_remove(&instance_id, &file).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn mods_add(state: State<'_, Arc<AppState>>, instance_id: String, path: String) -> Result<ModEntry, String> {
    state.mods_add(&instance_id, &path).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::test_state;
    use core_lib::types::{Loader, LoaderSpec};

    fn vanilla() -> LoaderSpec {
        LoaderSpec { loader: Loader::Vanilla, version: String::new() }
    }

    #[tokio::test]
    async fn mods_commands_roundtrip_and_validation() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let inst = st.instances.create("T", "1.21.1", vanilla(), 2048).unwrap();
        let gd = st.instances.game_dir(&inst.id).unwrap();
        std::fs::create_dir_all(gd.join("mods")).unwrap();
        std::fs::write(gd.join("mods/a.jar"), b"1").unwrap();
        let l = st.mods_list(&inst.id).await.unwrap();
        assert_eq!(l, vec![ModEntry { file: "a.jar".into(), enabled: true, size: 1, custom: false }]);
        st.mods_toggle(&inst.id, "a.jar", false).await.unwrap();
        assert!(!st.mods_list(&inst.id).await.unwrap()[0].enabled);
        assert_eq!(st.instances.get(&inst.id).unwrap().mod_count, 0);

        let src = dir.path().join("mine.jar");
        std::fs::write(&src, b"xyz").unwrap();
        let e = st.mods_add(&inst.id, src.to_str().unwrap()).await.unwrap();
        assert!(e.custom && e.enabled && e.file == "mine.jar");
        assert!(st.mods_list(&inst.id).await.unwrap().iter().any(|m| m.file == "mine.jar" && m.custom));
        assert_eq!(st.instances.get(&inst.id).unwrap().mod_count, 1);
        assert!(st.mods_add(&inst.id, "relatif.jar").await.is_err());

        st.mods_remove(&inst.id, "a.jar.disabled").await.unwrap();
        assert_eq!(st.mods_list(&inst.id).await.unwrap().len(), 1);

        for bad in ["../a.jar", "x/y.jar", "..", ""] {
            assert!(st.mods_toggle(&inst.id, bad, true).await.is_err(), "{bad:?}");
            assert!(st.mods_remove(&inst.id, bad).await.is_err(), "{bad:?}");
        }
        assert!(st.mods_list("../x").await.is_err());
        assert!(st.mods_list("inconnue").await.is_err());

        // Le jeu tourne : modifications refusées.
        *lock(&st.game) = Some(inst.id.clone());
        let e = st.mods_toggle(&inst.id, "mine.jar", false).await.unwrap_err();
        assert!(e.contains("jeu"), "{e}");
        assert!(st.mods_remove(&inst.id, "mine.jar").await.is_err());
    }

    #[tokio::test]
    async fn install_demande_une_instance_pour_un_mod() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let input = BrowseInstallInput {
            source: core_lib::browse::BrowseSource::Modrinth,
            id: "P".into(),
            kind: BrowseKind::Mod,
            version_id: None,
            instance_id: None,
        };
        let e = st.browse_install(input.clone()).await.unwrap_err();
        assert!(e.contains("instance"), "{e}");
        let e = st.browse_install(BrowseInstallInput { instance_id: Some("../x".into()), ..input }).await.unwrap_err();
        assert!(e.contains("invalide"), "{e}");
    }
}
