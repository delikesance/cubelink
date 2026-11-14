//! « Parcourir » (recherche / installation CurseForge et Modrinth) et gestion des mods d'une instance.
//!
//! La logique est dans `core_lib::browse` et `core_lib::mods` ; ici : état de l'application, événements
//! `browse://progress`, création d'instance pour les modpacks et surcharges de test.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use core_lib::browse::{self, BrowseInstallInput, BrowseKind, BrowsePage, BrowseQuery, BrowseSource, BrowseVersion, Providers};
use core_lib::mod_check::{self, ModCheck};
use core_lib::mod_fix::{self, ContentCheck, FixPlan, FixResult, PreparedFix};
use core_lib::mods::{self, ModEntry};
use core_lib::providers::curseforge::CfResolver;
use core_lib::providers::curseforge_api::CfApi;
use core_lib::providers::modrinth_api::ModrinthClient;
use core_lib::providers::modrinth_dl::DlOptions;
use core_lib::types::{Instance, Loader};
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
        // Mode test sans faux CurseForge : pas de clé, donc jamais d'appel au vrai service.
        None if h.test_mode => CfApi::with_base("http://127.0.0.1:1", ""),
        None => CfApi::new(),
    };
    Providers { modrinth, cf, allow_loopback: h.test_mode }
}

/// Plans de correction des mods en mémoire : `plan_id` -> (instance, création, plan). Valables 10 minutes.
const FIX_PLAN_TTL: std::time::Duration = std::time::Duration::from_secs(600);
const FIX_STALE: &str = "La liste des mods a changé, relance la correction.";

type FixPlans = Mutex<HashMap<String, (String, std::time::Instant, PreparedFix)>>;

fn fix_plans() -> &'static FixPlans {
    static PLANS: OnceLock<FixPlans> = OnceLock::new();
    PLANS.get_or_init(Default::default)
}

#[derive(Debug, Clone, Serialize, specta::Type)]
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

    /// Compatibilité d'un mod avec une instance avant installation : déjà installé, version recommandée, blocages.
    pub async fn browse_check(&self, source: &str, id: &str, kind: &str, instance_id: &str, version_id: Option<&str>) -> Result<ContentCheck, String> {
        let source = BrowseSource::parse(source).map_err(|e| user_msg(&e))?;
        if BrowseKind::parse(kind).map_err(|e| user_msg(&e))? != BrowseKind::Mod {
            return Ok(ContentCheck::default());
        }
        let (inst, dir) = self.instance_game_dir(instance_id)?;
        let p = providers();
        mod_fix::check_content(&p, &inst.minecraft, inst.loader.loader, &dir, &self.paths.store, &self.store, &DlOptions::default(), source, id, version_id)
            .await
            .map_err(|e| user_msg(&e))
    }

    pub async fn browse_install(&self, mut input: BrowseInstallInput) -> Result<BrowseInstallResult, String> {
        let p = providers();
        let dl = DlOptions::default();
        let cb = |d: u32, t: u32, l: &str| self.emit_browse_progress(d, t, l);
        if input.kind == BrowseKind::Modpack {
            return self.install_modpack(&p, &input, &dl).await;
        }
        let id = input.instance_id.as_deref().ok_or("choisis l'instance dans laquelle installer ce contenu")?;
        let (inst, game_dir) = self.instance_game_dir(id)?;
        self.ensure_not_running(id)?;
        if input.kind == BrowseKind::Mod {
            // Jamais un mod qui casserait le pack : version choisie vérifiée, sinon la version recommandée.
            let c = mod_fix::check_content(&p, &inst.minecraft, inst.loader.loader, &game_dir, &self.paths.store, &self.store, &dl, input.source, &input.id, input.version_id.as_deref())
                .await
                .map_err(|e| user_msg(&e))?;
            match &input.version_id {
                Some(v) => {
                    if let Some(vd) = c.verdicts.iter().find(|x| x.version_id == *v).filter(|x| !x.ok) {
                        return Err(format!("Cette version n'est pas compatible avec les mods du pack : {}", vd.problems.join(" ; ")));
                    }
                }
                None => match c.recommended {
                    Some(r) => input.version_id = Some(r),
                    None if !c.blockers.is_empty() => return Err(format!("Aucune version compatible avec les mods du pack : {}", c.blockers.join(" ; "))),
                    None => {}
                },
            }
        }
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
        let res = self.import_pack_file(&file.display().to_string(), None, None, None, input.ram_mb, Some(progress)).await;
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

    pub async fn mods_icon(&self, instance_id: &str, file: &str) -> Result<Option<String>, String> {
        let (_, dir) = self.instance_game_dir(instance_id)?;
        let file = file.to_string();
        tokio::task::spawn_blocking(move || mods::icon(&dir, &file))
            .await
            .map_err(|_| "lecture interrompue".to_string())?
            .map_err(|e| user_msg(&e))
    }

    /// Vérification hors ligne de la compatibilité des mods activés de l'instance.
    pub async fn mods_check(&self, instance_id: &str) -> Result<ModCheck, String> {
        let (inst, dir) = self.instance_game_dir(instance_id)?;
        let loader = match inst.loader.loader {
            Loader::Vanilla => "vanilla",
            Loader::NeoForge => "neoforge",
            Loader::Fabric => "fabric",
            Loader::Quilt => "quilt",
            Loader::Forge => "forge",
        };
        let minecraft = inst.minecraft;
        tokio::task::spawn_blocking(move || mod_check::check(&dir, &minecraft, loader))
            .await
            .map_err(|_| "lecture interrompue".to_string())?
            .map_err(|e| user_msg(&e))
    }

    fn emit_fix_progress(&self, done: u32, total: u32, label: &str) {
        self.emit_event("mods-fix://progress", &ProgressEvent { done, total, label });
    }

    /// Calcule (sans rien modifier dans l'instance) les corrections des problèmes de mods.
    pub async fn mods_fix_plan(&self, instance_id: &str) -> Result<FixPlan, String> {
        let (inst, dir) = self.instance_game_dir(instance_id)?;
        self.ensure_not_running(instance_id)?;
        let p = providers();
        let dl = DlOptions::default();
        let cb = |d: u32, t: u32, l: &str| self.emit_fix_progress(d, t, l);
        let mut prep = mod_fix::plan(&p, &inst.minecraft, inst.loader.loader, &dir, &self.paths.store, &self.store, &dl, &cb)
            .await
            .map_err(|e| user_msg(&e))?;
        let plan_id = format!("fix-{}", uuid_like());
        prep.plan.plan_id = plan_id.clone();
        let plan = prep.plan.clone();
        let mut plans = lock(fix_plans());
        plans.retain(|_, (_, at, _)| at.elapsed() < FIX_PLAN_TTL);
        plans.insert(plan_id, (instance_id.to_string(), std::time::Instant::now(), prep));
        Ok(plan)
    }

    /// Applique les actions choisies d'un plan encore valable.
    pub async fn mods_fix_apply(&self, instance_id: &str, plan_id: &str, action_ids: Vec<String>) -> Result<FixResult, String> {
        let (inst, dir) = self.instance_game_dir(instance_id)?;
        self.ensure_not_running(instance_id)?;
        let prep = {
            let mut plans = lock(fix_plans());
            plans.retain(|_, (_, at, _)| at.elapsed() < FIX_PLAN_TTL);
            match plans.get(plan_id) {
                Some((iid, _, prep)) if iid == instance_id => prep.clone(),
                _ => return Err(FIX_STALE.into()),
            }
        };
        if mod_fix::mods_fingerprint(&dir) != prep.fingerprint {
            lock(fix_plans()).remove(plan_id);
            return Err(FIX_STALE.into());
        }
        let cb = |d: u32, t: u32, l: &str| self.emit_fix_progress(d, t, l);
        let res = mod_fix::apply(&prep, &action_ids, &dir, &self.paths.store, &self.store, &inst.minecraft, inst.loader.loader, &cb).map_err(|e| user_msg(&e))?;
        lock(fix_plans()).remove(plan_id);
        Ok(res)
    }

    /// Annule une correction : remet les jars sauvegardés et retire ceux ajoutés.
    pub async fn mods_fix_undo(&self, instance_id: &str, backup_id: &str) -> Result<ModCheck, String> {
        let (inst, dir) = self.instance_game_dir(instance_id)?;
        self.ensure_not_running(instance_id)?;
        mod_fix::undo(&dir, backup_id, &self.store, &inst.minecraft, inst.loader.loader).map_err(|e| user_msg(&e))
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

/// Recherche dans le catalogue (Modrinth ou CurseForge).
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn browse_search(state: State<'_, Arc<AppState>>, query: BrowseQuery) -> Result<BrowsePage, String> {
    state.browse_search(query).await
}

/// Versions d'un projet du catalogue, filtrées par version de Minecraft et loader.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
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

/// Installe un projet du catalogue (nouvelle instance pour un modpack). Progression par `browse://progress`.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn browse_install(state: State<'_, Arc<AppState>>, input: BrowseInstallInput) -> Result<BrowseInstallResult, String> {
    state.browse_install(input).await
}

/// Mods d'une instance (activés ou non).
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn mods_list(state: State<'_, Arc<AppState>>, instance_id: String) -> Result<Vec<ModEntry>, String> {
    state.mods_list(&instance_id).await
}

/// Icône d'un mod lue dans son jar (data URL 64 px), `None` s'il n'en a pas.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn mods_icon(state: State<'_, Arc<AppState>>, instance_id: String, file: String) -> Result<Option<String>, String> {
    state.mods_icon(&instance_id, &file).await
}

/// Vérifie la compatibilité des mods installés (dépendances, incompatibilités, version, loader, doublons).
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn mods_check(state: State<'_, Arc<AppState>>, instance_id: String) -> Result<ModCheck, String> {
    state.mods_check(&instance_id).await
}

/// Compatibilité d'un mod du catalogue avec une instance (mods seulement ; autres types : résultat vide).
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn browse_check(state: State<'_, Arc<AppState>>, source: String, id: String, kind: String, instance_id: String, version_id: Option<String>) -> Result<ContentCheck, String> {
    state.browse_check(&source, &id, &kind, &instance_id, version_id.as_deref()).await
}

/// Calcule les corrections possibles des problèmes de mods, sans rien modifier.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn mods_fix_plan(state: State<'_, Arc<AppState>>, instance_id: String) -> Result<FixPlan, String> {
    state.mods_fix_plan(&instance_id).await
}

/// Applique les actions choisies du plan ; les fichiers remplacés sont sauvegardés (annulable). Progression par `mods-fix://progress`.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn mods_fix_apply(state: State<'_, Arc<AppState>>, instance_id: String, plan_id: String, action_ids: Vec<String>) -> Result<FixResult, String> {
    state.mods_fix_apply(&instance_id, &plan_id, action_ids).await
}

/// Annule une correction à partir de sa sauvegarde.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn mods_fix_undo(state: State<'_, Arc<AppState>>, instance_id: String, backup_id: String) -> Result<ModCheck, String> {
    state.mods_fix_undo(&instance_id, &backup_id).await
}

/// Active ou désactive un mod (`.disabled`).
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn mods_toggle(state: State<'_, Arc<AppState>>, instance_id: String, file: String, enabled: bool) -> Result<(), String> {
    state.mods_toggle(&instance_id, &file, enabled).await
}

/// Supprime un mod de l'instance.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn mods_remove(state: State<'_, Arc<AppState>>, instance_id: String, file: String) -> Result<(), String> {
    state.mods_remove(&instance_id, &file).await
}

/// Ajoute un jar à l'instance (chemin absolu).
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
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
            ram_mb: None,
        };
        let e = st.browse_install(input.clone()).await.unwrap_err();
        assert!(e.contains("instance"), "{e}");
        let e = st.browse_install(BrowseInstallInput { instance_id: Some("../x".into()), ..input }).await.unwrap_err();
        assert!(e.contains("invalide"), "{e}");
    }

    #[tokio::test]
    async fn fix_plan_is_refused_when_mods_change_or_unknown_or_game_runs() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let inst = st.instances.create("T", "1.21.1", vanilla(), 2048).unwrap();
        let gd = st.instances.game_dir(&inst.id).unwrap();
        std::fs::create_dir_all(gd.join("mods")).unwrap();
        std::fs::write(gd.join("mods/a.jar"), b"1").unwrap();
        let plan = st.mods_fix_plan(&inst.id).await.unwrap();
        assert!(plan.plan_id.starts_with("fix-") && plan.actions.is_empty() && plan.unresolved.is_empty());
        // Plan inconnu ou d'une autre instance.
        assert_eq!(st.mods_fix_apply(&inst.id, "fix-nope", vec![]).await.unwrap_err(), FIX_STALE);
        // Le dossier mods change : plan périmé.
        std::fs::write(gd.join("mods/b.jar"), b"22").unwrap();
        assert_eq!(st.mods_fix_apply(&inst.id, &plan.plan_id, vec![]).await.unwrap_err(), FIX_STALE);
        // Plan valable : rien à modifier, pas de sauvegarde.
        let plan = st.mods_fix_plan(&inst.id).await.unwrap();
        let r = st.mods_fix_apply(&inst.id, &plan.plan_id, vec![]).await.unwrap();
        assert!(r.applied.is_empty() && r.backup_id.is_none());
        assert!(st.mods_fix_undo(&inst.id, "../x").await.is_err());
        *lock(&st.game) = Some(inst.id.clone());
        assert!(st.mods_fix_plan(&inst.id).await.unwrap_err().contains("jeu"));
        assert!(st.mods_fix_undo(&inst.id, "123").await.unwrap_err().contains("jeu"));
    }
}
