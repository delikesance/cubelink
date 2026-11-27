//! « Parcourir » (recherche / installation CurseForge et Modrinth) et gestion des mods d'une instance.
//!
//! La logique est dans `core_lib::browse` et `core_lib::mods` ; ici : état de l'application, événements
//! `browse://progress`, création d'instance pour les modpacks et surcharges de test.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use core_lib::browse::{self, BrowseInstallInput, BrowseKind, BrowsePage, BrowseQuery, BrowseSource, BrowseVersion, Providers};
use core_lib::mod_check::{self, ModCheck};
use core_lib::mod_fix::{self, ContentCheck, FixPlan, FixResult, PreparedFix};
use core_lib::modpack_update::{self, ModpackUpdate, ModpackUpdateResult};
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
use crate::install::{ensure_game, Progress};
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

/// Mises à jour de modpack en cours (une seule à la fois par instance).
fn pack_updates_busy() -> &'static Mutex<HashSet<String>> {
    static BUSY: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    BUSY.get_or_init(Default::default)
}

/// Une mise à jour ou une annulation de modpack modifie cette instance : le jeu ne doit pas démarrer dessus.
pub(crate) fn pack_update_running(instance_id: &str) -> bool {
    lock(pack_updates_busy()).contains(instance_id)
}

/// Libère l'instance à la fin de l'opération, même en cas d'échec.
struct PackUpdateGuard(String);

impl PackUpdateGuard {
    fn take(instance_id: &str) -> Result<Self, String> {
        if !lock(pack_updates_busy()).insert(instance_id.to_string()) {
            return Err("une mise à jour de ce modpack est déjà en cours".into());
        }
        Ok(Self(instance_id.to_string()))
    }
}

impl Drop for PackUpdateGuard {
    fn drop(&mut self) {
        lock(pack_updates_busy()).remove(&self.0);
    }
}

/// Résultat de la dernière détection par instance : (version installée au moment du calcul, instant, résultat).
/// Évite de questionner Modrinth à chaque ouverture de l'instance.
const UPDATE_CHECK_TTL: std::time::Duration = std::time::Duration::from_secs(30 * 60);
type UpdateCache = Mutex<HashMap<String, (String, std::time::Instant, Option<ModpackUpdate>)>>;

fn update_cache() -> &'static UpdateCache {
    static CACHE: OnceLock<UpdateCache> = OnceLock::new();
    CACHE.get_or_init(Default::default)
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

/// Sources dont la mise à jour de modpack est prise en charge.
fn is_updatable_source(source: &str) -> bool {
    matches!(source, "modrinth" | "curseforge")
}

/// Identifiant de version : `[A-Za-z0-9_-]`, 1 à 64 caractères pour Modrinth ; chiffres seulement pour CurseForge
/// (identifiant de fichier).
fn valid_version_id(source: &str, id: &str) -> bool {
    let ok = |b: u8| if source == "curseforge" { b.is_ascii_digit() } else { b.is_ascii_alphanumeric() || b == b'_' || b == b'-' };
    (1..=64).contains(&id.len()) && id.bytes().all(ok)
}

/// Loader déclaré par le pack téléchargé (`.mrpack` ou zip CurseForge).
fn pack_loader(file: &Path) -> core_lib::types::Result<core_lib::types::LoaderSpec> {
    match core_lib::mrpack::detect(file)? {
        core_lib::mrpack::PackKind::CurseForge => core_lib::cf_pack::manifest_loader(file),
        _ => Ok(core_lib::mrpack::parse_index(file)?.loader),
    }
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

    /// Mod : compatibilité avec l'instance avant installation (déjà installé, version recommandée, blocages). Pack de ressources / shader : `installed` seulement.
    pub async fn browse_check(&self, source: &str, id: &str, kind: &str, instance_id: &str, version_id: Option<&str>) -> Result<ContentCheck, String> {
        let source = BrowseSource::parse(source).map_err(|e| user_msg(&e))?;
        let kind = BrowseKind::parse(kind).map_err(|e| user_msg(&e))?;
        if kind == BrowseKind::Modpack {
            return Ok(ContentCheck::default());
        }
        let (inst, dir) = self.instance_game_dir(instance_id)?;
        if kind != BrowseKind::Mod {
            // Pack de ressources / shader : seulement l'état « installé » (pas de simulation de compatibilité).
            return Ok(browse::installed_check(&dir, kind, source, id));
        }
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
        let (file, origin) = browse::fetch_modpack(p, input, &self.paths.store, &self.store, &tmp, dl, &cb).await.map_err(|e| {
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
        let res = self.import_pack_file(&file.display().to_string(), None, None, None, input.ram_mb, Some(progress), Some(origin)).await;
        let _ = std::fs::remove_dir_all(&tmp);
        let (instance, missing) = res?;
        // Illustrations : meilleur effort, un échec n'affecte jamais l'installation. L'origine est déjà posée sur
        // l'instance (import_pack_file) : elles ne sont pas cherchées par nom.
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

    /// Mise à jour disponible pour le modpack installé (Modrinth ou CurseForge), `None` si l'instance est à jour ou sans origine connue.
    /// Mise en cache 30 minutes ; une erreur réseau est renvoyée (l'interface l'ignore) et n'est pas mise en cache.
    pub async fn modpack_update_check(&self, instance_id: &str) -> Result<Option<ModpackUpdate>, String> {
        let (inst, _) = self.instance_game_dir(instance_id)?;
        let Some(pack) = inst.pack.clone().filter(|o| is_updatable_source(&o.source)) else { return Ok(None) };
        if let Some((v, at, r)) = lock(update_cache()).get(instance_id) {
            if *v == pack.version_id && at.elapsed() < UPDATE_CHECK_TTL {
                return Ok(r.clone());
            }
        }
        let r = modpack_update::check(&providers(), &inst).await.map_err(|e| user_msg(&e))?;
        lock(update_cache()).insert(instance_id.to_string(), (pack.version_id, std::time::Instant::now(), r.clone()));
        Ok(r)
    }

    /// Passe le modpack à la version `version_id` : ne touche que les fichiers du pack, garde ceux du joueur,
    /// sauvegarde l'ancien état (annulable avec `modpack_update_undo`).
    pub async fn modpack_update_apply(&self, instance_id: &str, version_id: &str) -> Result<ModpackUpdateResult, String> {
        let (inst, _) = self.instance_game_dir(instance_id)?;
        self.ensure_not_running(instance_id)?;
        let Some(pack) = inst.pack.clone().filter(|o| is_updatable_source(&o.source)) else {
            return Err("Cette instance n'a pas d'origine de modpack connue : la mise à jour n'est pas possible.".into());
        };
        if !valid_version_id(&pack.source, version_id) {
            return Err("identifiant de version invalide".into());
        }
        let _guard = PackUpdateGuard::take(instance_id)?;
        let tmp = self.paths.root.join("tmp").join(format!("modpack-update-{}", uuid_like()));
        let res = self.modpack_update_run(&inst, &pack, version_id, &tmp).await;
        let _ = std::fs::remove_dir_all(&tmp);
        lock(update_cache()).remove(instance_id);
        res
    }

    async fn modpack_update_run(&self, inst: &Instance, pack: &core_lib::types::PackOrigin, version_id: &str, tmp: &Path) -> Result<ModpackUpdateResult, String> {
        let p = providers();
        let dl = DlOptions::default();
        let cb = |d: u32, t: u32, l: &str| self.emit_browse_progress(d, t, l);
        let source = if pack.source == "curseforge" { BrowseSource::Curseforge } else { BrowseSource::Modrinth };
        let input = |v: &str| BrowseInstallInput {
            source,
            id: pack.project_id.clone(),
            kind: BrowseKind::Modpack,
            version_id: Some(v.to_string()),
            instance_id: None,
            ram_mb: None,
        };
        // L'ancien pack n'est retéléchargé que si l'instance n'a pas de relevé de ses fichiers.
        let old_file = if modpack_update::needs_old_pack(&self.instances, &inst.id).map_err(|e| user_msg(&e))? {
            let (file, _) = browse::fetch_modpack(&p, &input(&pack.version_id), &self.paths.store, &self.store, &tmp.join("old"), &dl, &cb)
                .await
                .map_err(|e| format!("Impossible de récupérer la version installée du modpack pour la comparer ({}). La mise à jour n'a pas été appliquée.", user_msg(&e)))?;
            Some(file)
        } else {
            None
        };
        let (new_file, new_origin) =
            browse::fetch_modpack(&p, &input(version_id), &self.paths.store, &self.store, &tmp.join("new"), &dl, &cb).await.map_err(|e| user_msg(&e))?;

        // Le loader change de version (même Minecraft, même loader) : il doit être installable AVANT de toucher à l'instance.
        let new_loader = {
            let f = new_file.clone();
            tokio::task::spawn_blocking(move || pack_loader(&f))
                .await
                .map_err(|_| "mise à jour interrompue".to_string())?
                .map_err(|e| user_msg(&e))?
        };
        if new_loader.loader == inst.loader.loader && new_loader != inst.loader {
            let probe = Instance { loader: new_loader, ..inst.clone() };
            let noop: Progress = Arc::new(|_| {});
            let java_setting = crate::install::JavaSetting::for_instance(probe.java_path.as_deref(), self.settings_get().java_path);
            ensure_game(&self.paths, &probe, java_setting, noop).await.map_err(|e| user_msg(&e))?;
        }

        // Les téléchargements ont pu durer : le jeu n'a pas le droit d'avoir démarré entre-temps.
        self.ensure_not_running(&inst.id)?;
        let mut opts = core_lib::mrpack::ImportOptions::default();
        if crate::test_hooks::hooks().test_mode {
            opts.allowed_hosts.extend(["127.0.0.1".to_string(), "localhost".to_string()]);
            opts.allow_http = true;
        }
        let cf = cf_resolver();
        let emit = self.emit.clone();
        let progress: core_lib::mrpack::FilesProgress =
            Arc::new(move |d, t| emit("browse://progress", serde_json::json!({ "done": d, "total": t, "label": "Mise à jour du modpack" })));
        let (instances, store, store_root, id) = (self.instances.clone(), self.store.clone(), self.paths.store.clone(), inst.id.clone());
        // Les futurs d'import ne sont pas prouvés `Send` par rustc : on les exécute sur un thread dédié.
        let handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            handle.block_on(modpack_update::apply(&instances, &id, old_file.as_deref(), &new_file, new_origin, &cf, &store_root, &store, &opts, Some(progress), Arc::new(std::sync::atomic::AtomicBool::new(false))))
        })
        .await
        .map_err(|_| "mise à jour interrompue".to_string())?
        .map_err(|e| user_msg(&e))
    }

    /// Annule une mise à jour de modpack à partir de sa sauvegarde ; renvoie l'instance telle qu'elle est ensuite.
    pub async fn modpack_update_undo(&self, instance_id: &str, backup_id: &str) -> Result<Instance, String> {
        self.instance_game_dir(instance_id)?;
        self.ensure_not_running(instance_id)?;
        let _guard = PackUpdateGuard::take(instance_id)?;
        let (instances, store, id, backup) = (self.instances.clone(), self.store.clone(), instance_id.to_string(), backup_id.to_string());
        let r = tokio::task::spawn_blocking(move || modpack_update::undo(&instances, &id, &backup, &store))
            .await
            .map_err(|_| "annulation interrompue".to_string())?
            .map_err(|e| user_msg(&e));
        lock(update_cache()).remove(instance_id);
        r
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

/// Recherche dans le catalogue (Modrinth ou CurseForge). Pour une instance Quilt, la recherche de mods inclut aussi les mods Fabric (OU de facettes sur Modrinth, pas de `modLoaderType` sur CurseForge).
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

/// Installe un projet du catalogue (nouvelle instance pour un modpack). Un mod remplace les jars du même mod déjà présents sous un autre nom (modpack, ajout manuel) : ils vont dans `mods/.cubelink-backup/<id>/` (restaurables avec `mods_fix_undo` sur cet id ; le résultat de `browse_install` ne le renvoie pas encore, donc l'interface n'offre pas d'annulation). Une dépendance déjà fournie par un jar activé sous un autre nom est ignorée. Un pack de ressources ou un shader remplace les anciennes versions du même projet (sans sauvegarde : il se retélécharge) ; sa provenance est consignée dans `.cubelink-mods.json` sous la clé `dossier/fichier` (`shaderpacks/x.zip`), les mods gardent leur clé nue. Un pack posé avant ce correctif n'a pas de provenance. Pour un modpack, l'origine installée (`Instance.pack` : `source`, `project_id`, `version_id` de la version réellement téléchargée ; pour CurseForge, l'id du fichier) est enregistrée dans `instance.json` avant la récupération de l'illustration. Progression par `browse://progress`.
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

/// Mod : compatibilité avec une instance (déjà installé, version recommandée, verdicts, blocages). Pack de ressources et shader : seulement `installed` (fichiers du projet encore présents dans `resourcepacks/` ou `shaderpacks/`, d'après la provenance ; pas de verdicts). Modpack : résultat vide.
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

/// Mise à jour disponible pour le modpack installé depuis Modrinth ou CurseForge (`None` : à jour, instance sans origine de modpack, ou CurseForge sans clé d'API). Seules les versions compatibles avec l'instance (même Minecraft, même loader) sont proposées, jamais une plus ancienne, ni un pack serveur CurseForge, et une pré-version seulement si la version installée en est une. Résultat gardé 30 minutes ; en cas d'erreur réseau la commande échoue et l'interface l'ignore.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn modpack_update_check(state: State<'_, Arc<AppState>>, instance_id: String) -> Result<Option<ModpackUpdate>, String> {
    state.modpack_update_check(&instance_id).await
}

/// Met le modpack d'une instance à la version `version_id` (Modrinth ou CurseForge, même Minecraft et même loader ; sinon refus). Compare l'ancien pack (relevé de l'instance, sinon retéléchargé par `Instance.pack.version_id`), le nouveau et le disque : un fichier du pack n'est remplacé ou retiré que s'il est encore identique à l'ancien ; un fichier modifié par le joueur est gardé (liste `kept`) ; un fichier « tolérant » du nouveau pack introuvable (mod CurseForge non redistribuable) n'arrête pas la mise à jour : il n'est ni ajouté ni remplacé et figure dans `missing`, à poser par le joueur ; ceux qu'il a ajoutés, `saves/`, `options.txt`, `servers.dat` et `logs/` ne sont jamais touchés. Tout est téléchargé avant la première modification ; l'ancien état va dans `<instance>/minecraft/.cubelink-pack-backup/<backup_id>/` (3 sauvegardes gardées) et `Instance.pack` (et la version du loader) change en dernier. Relevé : `<instance>/minecraft/.cubelink-pack-record.json` (empreintes attendues du pack, écrit à l'installation et daté par l'origine, réécrit après une mise à jour, restauré par l'annulation ; ignoré s'il est illisible, contient un chemin invalide ou ne correspond plus à `Instance.pack`). CurseForge sans relevé : ancien pack et empreintes de l'API (refus sans clé) ; refus avant tout téléchargement si un jar de l'ancien pack d'empreinte inconnue est sur le disque et que la nouvelle version le retire ou le change (il resterait en double). Refusé si le jeu tourne ou si une autre mise à jour de l'instance est en cours. Progression par `browse://progress`.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn modpack_update_apply(state: State<'_, Arc<AppState>>, instance_id: String, version_id: String) -> Result<ModpackUpdateResult, String> {
    state.modpack_update_apply(&instance_id, &version_id).await
}

/// Annule une mise à jour de modpack à partir de sa sauvegarde : retire les fichiers ajoutés, remet ceux qui avaient été déplacés, l'ancienne origine du pack et l'ancien loader. Un fichier que le joueur a modifié depuis la mise à jour reste tel quel. Refusé si une mise à jour plus récente a eu lieu depuis, si le jeu tourne ou si une autre opération est en cours.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn modpack_update_undo(state: State<'_, Arc<AppState>>, instance_id: String, backup_id: String) -> Result<Instance, String> {
    state.modpack_update_undo(&instance_id, &backup_id).await
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

    #[tokio::test]
    async fn modpack_update_commands_validate_and_refuse() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let inst = st.instances.create("T", "1.21.1", vanilla(), 2048).unwrap();
        // Instance inconnue ou identifiant invalide.
        assert!(st.modpack_update_check("../x").await.unwrap_err().contains("invalide"));
        assert!(st.modpack_update_apply("../x", "abc").await.unwrap_err().contains("invalide"));
        assert!(st.modpack_update_undo("../x", "abc").await.unwrap_err().contains("invalide"));
        // Sans origine de modpack : rien à proposer, rien à appliquer.
        assert_eq!(st.modpack_update_check(&inst.id).await.unwrap(), None);
        assert!(st.modpack_update_apply(&inst.id, "abc").await.unwrap_err().contains("origine"));
        // Identifiant de version et de sauvegarde validés.
        st.instances.set_pack(&inst.id, Some(core_lib::types::PackOrigin { source: "modrinth".into(), project_id: "P".into(), version_id: "v1".into() })).unwrap();
        assert!(st.modpack_update_apply(&inst.id, "../x").await.unwrap_err().contains("invalide"));
        assert!(st.modpack_update_apply(&inst.id, "").await.unwrap_err().contains("invalide"));
        assert!(st.modpack_update_undo(&inst.id, "../x").await.is_err());
        // CurseForge : l'identifiant de version ne contient que des chiffres.
        st.instances.set_pack(&inst.id, Some(core_lib::types::PackOrigin { source: "curseforge".into(), project_id: "100".into(), version_id: "1".into() })).unwrap();
        assert!(st.modpack_update_apply(&inst.id, "abc").await.unwrap_err().contains("invalide"));
        assert!(st.modpack_update_apply(&inst.id, "12x").await.unwrap_err().contains("invalide"));
        // Jeu en cours : refus.
        *lock(&st.game) = Some(inst.id.clone());
        assert!(st.modpack_update_apply(&inst.id, "abc").await.unwrap_err().contains("jeu"));
        assert!(st.modpack_update_undo(&inst.id, "123").await.unwrap_err().contains("jeu"));
    }

    #[test]
    fn version_ids_are_validated_per_source() {
        assert!(valid_version_id("modrinth", "modpack-7-v2"));
        assert!(valid_version_id("modrinth", "AANobbMI_x"));
        assert!(!valid_version_id("modrinth", "../x"));
        assert!(!valid_version_id("modrinth", ""));
        assert!(!valid_version_id("modrinth", &"a".repeat(65)));
        assert!(valid_version_id("curseforge", "5002"));
        assert!(!valid_version_id("curseforge", "modpack-7-v2"));
    }

    #[test]
    fn pack_update_guard_allows_one_operation_per_instance() {
        let a = PackUpdateGuard::take("inst-guard-test").unwrap();
        assert!(PackUpdateGuard::take("inst-guard-test").is_err());
        assert!(PackUpdateGuard::take("autre-inst").is_ok());
        drop(a);
        assert!(PackUpdateGuard::take("inst-guard-test").is_ok());
    }
}
