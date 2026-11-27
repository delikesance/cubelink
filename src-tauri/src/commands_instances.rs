//! Commandes d'instances : liste, création (avec installation), import .mrpack, lancement.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use core_lib::import_launcher::{self, DetectedInstance, ImportParams, Launcher, Roots};
use core_lib::instance::{MAX_RAM_MB, MIN_RAM_MB};
use core_lib::launch_args::{build_command, LaunchSpec, LaunchVersion};
use core_lib::launch_run::{self, LaunchEvent, LaunchPhase, LaunchStateEvent};
use core_lib::mojang::libraries::{Arch, Os};
use core_lib::mojang::versions::release_ids;
use core_lib::mrpack::{self, ImportOptions, PackKind, PlainSpec};
use core_lib::types::{Instance, Loader, LoaderSpec, PackOrigin, Profile};
use tauri::State;

use crate::error::{sanitize, user_msg};
use crate::install::{ensure_game, ensure_game_with, resolve_loader_version, safe_version, JavaSetting, Progress};
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
    if ram_mb != core_lib::instance::RAM_DEFAULT && !(MIN_RAM_MB..=MAX_RAM_MB).contains(&ram_mb) {
        return Err(format!("la mémoire doit être comprise entre {MIN_RAM_MB} et {MAX_RAM_MB} Mo"));
    }
    Ok(())
}

/// Le fichier doit exister et porter l'extension `.mrpack` ou `.zip`.
pub fn validate_mrpack_path(path: &str) -> Result<PathBuf, String> {
    let p = Path::new(path);
    let ext_ok = p
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("mrpack") || e.eq_ignore_ascii_case("zip"));
    if !ext_ok {
        return Err("le fichier doit avoir l'extension .mrpack ou .zip".into());
    }
    if !p.is_file() {
        return Err("le fichier du modpack est introuvable".into());
    }
    p.canonicalize().map_err(|_| "le fichier du modpack est illisible".to_string())
}

/// Dossiers où chercher les autres lanceurs : le profil de l'utilisateur, ou le dossier de test en mode test.
fn launcher_roots() -> Roots {
    let h = crate::test_hooks::hooks();
    match (&h.launcher_home, h.test_mode) {
        (Some(home), true) => Roots { appdata: Some(home.join("AppData")), home: Some(home.clone()), data_home: Some(home.join(".local/share")) },
        _ => Roots::from_env(),
    }
}

/// Imports d'instances d'un autre lanceur en cours (une instance source n'est importée qu'une fois à la fois).
static LAUNCHER_IMPORTS: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

struct LauncherImportGuard(PathBuf);

impl LauncherImportGuard {
    fn begin(path: &Path) -> Option<Self> {
        let mut running = LAUNCHER_IMPORTS.lock().unwrap_or_else(|e| e.into_inner());
        if running.iter().any(|p| p == path) {
            return None;
        }
        running.push(path.to_path_buf());
        Some(Self(path.to_path_buf()))
    }
}

impl Drop for LauncherImportGuard {
    fn drop(&mut self) {
        LAUNCHER_IMPORTS.lock().unwrap_or_else(|e| e.into_inner()).retain(|p| *p != self.0);
    }
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
/// Agent qui ouvre le monde solo au réseau local (voir `core/resources/lan-agent`) : actif pour les instances
/// `Own` si `Settings.lan_auto`. Jar : `CUBELINK_LAN_AGENT` sinon la ressource Tauri. En mode test, seulement si
/// `CUBELINK_LAN_AGENT` est défini (les e2e ne chargent pas l'agent).
///
/// Invité (`join` = hôte:port du tunnel) : le même jar est chargé en mode `join=` (aucune ouverture de monde) ; il ne
/// fait que relancer la connexion si le `--quickPlayMultiplayer` du jeu n'est pas parti (voir `LanAgent.joinLoop`).
fn lan_agent_jar(st: &AppState, own: bool, join: Option<&(String, u16)>) -> Option<std::path::PathBuf> {
    let env = std::env::var_os("CUBELINK_LAN_AGENT").map(std::path::PathBuf::from);
    if crate::test_hooks::hooks().test_mode && env.is_none() {
        return None;
    }
    if st.lan_agent_off.load(std::sync::atomic::Ordering::SeqCst) || (join.is_none() && (!own || !st.settings_get().lan_auto)) {
        return None;
    }
    let p = env.or_else(|| lock(&st.lan_agent).clone())?;
    // La JVM refuse un `-javaagent` au format Windows étendu (préfixe verbatim, renvoyé par le résolveur de
    // ressources Tauri) : « FATAL ERROR … appending to system class path failed ». On passe un chemin classique.
    let p = plain_windows_path(&p);
    p.is_file().then_some(p)
}

/// Argument `-javaagent` complet pour ce jar (options d'invité ou de test comprises).
fn lan_agent_arg(jar: &std::path::Path, join: Option<&(String, u16)>) -> String {
    let a = javaagent_arg(jar, None);
    if let Some((host, port)) = join {
        return with_join(a, host, *port);
    }
    let flag = std::env::var("CUBELINK_LAN_NOAUTH").ok();
    with_test_noauth(a, noauth_enabled(crate::test_hooks::hooks().test_mode, flag.as_deref()))
}

/// Faut-il injecter l'agent LAN ? `Err(raison)` sinon (journalisée). Pure, testable.
/// - `java_major` : Java qui lance le jeu (`None` si inconnu) ; `jar_min_java` : Java minimal du jar (`lan_map::jar_min_java`) ;
/// - legacy (MC < 1.13) : hôte Forge avec table SRG uniquement (l'invité n'a pas besoin de l'agent : `--server/--port`) ;
/// - moderne (≥ 1.13, version non reconnue comprise) : inchangé.
pub(crate) fn agent_supported(
    java_major: Option<u32>,
    jar_min_java: u32,
    mc_version: &str,
    loader: Loader,
    join: bool,
    has_srg_table: bool,
) -> Result<(), &'static str> {
    let legacy = core_lib::lan_map::is_legacy(mc_version);
    match java_major {
        Some(j) if j < 8 => return Err("Java < 8"),
        Some(j) if j < jar_min_java => return Err("le jar de l'agent exige une Java plus récente"),
        None if legacy => return Err("version de Java inconnue (Minecraft legacy)"),
        _ => {}
    }
    if legacy {
        if join {
            return Err("invité legacy : --server/--port suffit");
        }
        if loader != Loader::Forge {
            return Err("Minecraft < 1.13 : seul Forge est géré");
        }
        if !has_srg_table {
            return Err("pas de table SRG pour cette version");
        }
    }
    Ok(())
}

/// TEST UNIQUEMENT : l'option `noauth` de l'agent (serveur LAN sans verification de session) n'est possible
/// qu'en mode test (`CUBELINK_TEST_MODE=1`) ET avec `CUBELINK_LAN_NOAUTH=1`. Jamais un réglage utilisateur.
pub(crate) fn noauth_enabled(test_mode: bool, env: Option<&str>) -> bool {
    test_mode && env == Some("1")
}

/// Ajoute la table de noms `map=` à l'argument (`=` si c'est la première option, `,` sinon).
pub(crate) fn with_map(arg: String, map: &std::path::Path) -> String {
    let sep = if arg.contains('=') { "," } else { "=" };
    format!("{arg}{sep}map={}", plain_windows_path(map).display())
}

/// Option `join=<hôte>:<port>` de l'agent (mode invité). L'hôte est le tunnel local : seuls `[A-Za-z0-9.-]` passent.
pub(crate) fn with_join(arg: String, host: &str, port: u16) -> String {
    let host: String = host.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-').collect();
    format!("{arg}=join={host}:{port}")
}


pub(crate) fn with_test_noauth(arg: String, enabled: bool) -> String {
    if !enabled {
        return arg;
    }
    match arg.find('=') {
        Some(i) => format!("{}noauth,{}", &arg[..=i], &arg[i + 1..]),
        None => format!("{arg}=noauth"),
    }
}

/// Argument JVM exact produit en production : `-javaagent:<jar>[=map=<table>]`, chemins sans préfixe verbatim.
pub(crate) fn javaagent_arg(jar: &std::path::Path, map: Option<&std::path::Path>) -> String {
    let jar = ascii_safe_path(&plain_windows_path(jar));
    let mut a = format!("-javaagent:{}", jar.display());
    if let Some(m) = map {
        a.push_str(&format!("=map={}", ascii_safe_path(&plain_windows_path(m)).display()));
    }
    a
}

/// La JVM Windows lit l'option `-javaagent` dans la page de code ANSI : un chemin avec des caractères hors de cette
/// page (CJK, cyrillique sur une Windows occidentale…) donne « Error opening zip file or JAR manifest missing ».
/// Un chemin non ASCII est donc copié sous un dossier ASCII (temp, `%PUBLIC%`, `%ProgramData%`) ; sinon renvoyé tel quel.
pub(crate) fn ascii_safe_path(p: &std::path::Path) -> std::path::PathBuf {
    if p.to_string_lossy().is_ascii() {
        return p.to_path_buf();
    }
    let Some(name) = p.file_name().map(|n| n.to_string_lossy().into_owned()).filter(|n| n.is_ascii()) else { return p.to_path_buf() };
    let mut bases = vec![std::env::temp_dir()];
    bases.extend(["PUBLIC", "ProgramData"].iter().filter_map(|k| std::env::var_os(k).map(std::path::PathBuf::from)));
    for b in bases {
        let dir = b.join("cubelink-lan");
        if !dir.to_string_lossy().is_ascii() {
            continue;
        }
        let dest = dir.join(&name);
        let same = match (std::fs::metadata(p), std::fs::metadata(&dest)) {
            (Ok(a), Ok(b)) => a.len() == b.len(),
            _ => false,
        };
        if same || (std::fs::create_dir_all(&dir).is_ok() && std::fs::copy(p, &dest).is_ok()) {
            return dest;
        }
    }
    p.to_path_buf()
}

/// Retire le préfixe Windows étendu (`\\?\` ou `\\?\UNC\`) d'un chemin ; sans effet sur les autres chemins.
pub(crate) fn plain_windows_path(p: &std::path::Path) -> std::path::PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        std::path::PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        std::path::PathBuf::from(rest)
    } else {
        p.to_path_buf()
    }
}

/// Fichier de correspondance de noms pour l'agent LAN (`core_lib::lan_map`), ou `None` quand les noms Mojang
/// suffisent (NeoForge, Minecraft 26.x) ou ne sont pas gérés (Forge : non vérifié). Toute erreur est journalisée
/// et rend `None` : l'agent signale alors lui-même dans `latest.log` qu'il ne trouve pas les noms.
async fn lan_map_path(st: &AppState, inst: &Instance, version: &LaunchVersion, libs_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    use core_lib::lan_map::{ensure_lanmap, ensure_srg_map, pick_runtime, Runtime};
    let (downloads, libs) = match version {
        LaunchVersion::Vanilla(v) => (&v.downloads, &v.libraries),
        LaunchVersion::Merged(m) => (&m.downloads, &m.libraries),
    };
    let dir = st.paths.root.join("lanmap");
    // Forge 1.7.10 → 1.12.2 : table SRG embarquée (aucun téléchargement).
    if pick_runtime(inst.loader.loader, &inst.minecraft, downloads.client_mappings.is_some()) == Some(Runtime::Srg) {
        return match ensure_srg_map(&dir, &inst.minecraft) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("LAN: table SRG illisible ({e})");
                None
            }
        };
    }
    // Jeu non obfusqué (26.x) : pas de mappings Mojang, les noms Mojang directs suffisent, aucune traduction.
    downloads.client_mappings.as_ref()?;
    let (runtime, inter) = match inst.loader.loader {
        Loader::Vanilla => (Runtime::Official, None),
        Loader::Fabric | Loader::Quilt => {
            let jar = libs.iter().filter_map(|l| l.get("name")?.as_str()).find(|n| n.starts_with("net.fabricmc:intermediary:"))
                .and_then(|n| core_lib::mojang::libraries::maven_path(n).ok())
                .map(|rel| libs_dir.join(rel));
            match jar {
                Some(j) if j.is_file() => (Runtime::Intermediary, Some(j)),
                _ => {
                    tracing::warn!("LAN: jar intermediary introuvable, noms Fabric non traduits");
                    return None;
                }
            }
        }
        Loader::NeoForge | Loader::Forge => return None,
    };
    let mappings = downloads.client_mappings.as_ref()?; // absent : jeu non obfusqué (26.x)
    match ensure_lanmap(&dir, &core_lib::http::client(), &inst.minecraft, runtime, mappings, inter.as_deref()).await {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::warn!("LAN: correspondance de noms indisponible ({e}), l'ouverture auto peut échouer");
            None
        }
    }
}

/// Une JVM qui s'arrête en moins de ce délai avec un message d'agent est attribuée à l'agent.
const AGENT_CRASH_WINDOW: std::time::Duration = std::time::Duration::from_secs(15);

/// Ligne de sortie typique d'un échec de chargement de `-javaagent` (erreur native de la JVM).
pub(crate) fn agent_crash_marker(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    l.contains("processing of -javaagent failed")
        || l.contains("agent library failed agent_onload")
        || l.contains("error opening zip file or jar manifest missing")
        || (l.contains("unable to add") && l.contains("system class path"))
        || (l.contains("-javaagent") && (l.contains("error") || l.contains("failed")))
}

fn is_failure(ev: &LaunchEvent) -> bool {
    matches!(ev, LaunchEvent::Failed { .. }) || matches!(ev, LaunchEvent::Exited { code } if *code != 0)
}

struct GameGuard(Arc<AppState>);
impl Drop for GameGuard {
    fn drop(&mut self) {
        // Filet : quoi qu'il arrive a la tache de jeu (y compris une panique), le rideau de la zone integree ne reste pas ferme.
        // Sans effet si la fin du jeu a deja ete annoncee.
        self.0.stage.run_finished();
        *lock(&self.0.game_handle) = None;
        *lock(&self.0.launch_cancel) = None;
        *lock(&self.0.game) = None;
    }
}

impl AppState {
    /// Mémoire donnée au jeu (Mo) : réglage de l'instance, sinon réglage global, sinon « Automatique ». Les mods sont
    /// comptés sur le disque à l'instant du lancement : ceux d'un monde d'ami viennent d'être synchronisés.
    pub(crate) fn launch_ram_mb(&self, inst: &Instance) -> u32 {
        let mods = self.instances.mod_count(&inst.id).unwrap_or(inst.mod_count);
        core_lib::instance::effective_ram_mb(inst.ram_mb, self.settings_get().default_ram_mb, mods, core_lib::sysmem::physical_ram_mb())
    }

    pub fn instances_list(&self) -> Vec<Instance> {
        self.instances.list()
    }

    pub async fn instances_create(&self, name: String, minecraft: String, loader: LoaderSpec, ram_mb: u32) -> Result<Instance, String> {
        validate_create(&name, &minecraft, &loader, ram_mb)?;
        let version = resolve_loader_version(&minecraft, &loader).await.map_err(|e| user_msg(&e))?;
        let spec = LoaderSpec { loader: loader.loader, version };
        let inst = self.instances.create(name.trim(), &minecraft, spec, ram_mb).map_err(|e| user_msg(&e))?;
        let _ = self.instances.mark_custom(&inst.id);
        self.prepare_game_best_effort(&inst).await;
        Ok(inst)
    }

    /// Prépare le jeu (loader, Java, assets) juste après une création ou un import, « au mieux » : un échec (réseau coupé, Mojang
    /// ou Adoptium indisponible…) ne supprime jamais l'instance, qui est valide ; `ensure_game` est idempotent et reprendra au
    /// premier lancement. La progression passe par `import://progress`, l'échec par `import://deferred`. Renvoie `true` si le jeu est prêt.
    async fn prepare_game_best_effort(&self, inst: &Instance) -> bool {
        let emit = self.emit.clone();
        let progress: Progress = Arc::new(move |message| emit("import://progress", serde_json::json!({ "message": message })));
        let java = JavaSetting::global(self.settings_get().java_path);
        let r = ensure_game(&self.paths, inst, java, progress).await;
        self.note_prepare_result(inst, r.map(|_| ()))
    }

    /// Suite d'une préparation : sur échec, journalise et annonce `import://deferred` (l'instance reste).
    fn note_prepare_result(&self, inst: &Instance, r: core_lib::types::Result<()>) -> bool {
        match r {
            Ok(()) => true,
            Err(e) => {
                let message = user_msg(&e);
                eprintln!("[Cubelink] préparation du jeu différée pour l'instance {} : {message}", inst.id);
                self.emit_event("import://deferred", &serde_json::json!({ "instance_id": inst.id, "message": message }));
                false
            }
        }
    }

    /// Importe un modpack : `.mrpack` (Modrinth), zip CurseForge (`manifest.json`, sans clé API ; les mods introuvables sont signalés par `import://warning`) ou
    /// zip générique (contenu d'instance brut, avec nom / version de Minecraft / loader choisis par l'utilisateur).
    pub async fn instances_import_mrpack(
        &self,
        path: &str,
        name: Option<String>,
        minecraft: Option<String>,
        loader: Option<LoaderSpec>,
        ram_mb: Option<u32>,
    ) -> Result<Instance, String> {
        let (inst, missing) = self.import_pack_file(path, name, minecraft, loader, ram_mb, None, None).await?;
        if !missing.is_empty() {
            self.emit_event("import://warning", &serde_json::json!({ "missing": missing }));
        }
        Ok(inst)
    }

    /// Cœur de l'import : renvoie l'instance et les mods CurseForge introuvables (sans émettre d'événement).
    ///
    /// `origin` : projet et version du catalogue quand l'appelant les connaît (installation depuis Parcourir).
    /// Sans `origin`, un `.mrpack` est reconnu par son empreinte SHA-1 auprès de Modrinth (meilleur effort :
    /// hors ligne, pack modifié ou non publié, l'import réussit quand même, sans origine).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn import_pack_file(
        &self,
        path: &str,
        name: Option<String>,
        minecraft: Option<String>,
        loader: Option<LoaderSpec>,
        ram_mb: Option<u32>,
        progress: Option<core_lib::mrpack::FilesProgress>,
        origin: Option<PackOrigin>,
    ) -> Result<(Instance, Vec<String>), String> {
        let file = validate_mrpack_path(path)?;
        // Sans choix : « Par défaut » (la valeur globale, lue à chaque lancement).
        let ram_mb = ram_mb.unwrap_or(core_lib::instance::RAM_DEFAULT);
        if ram_mb != core_lib::instance::RAM_DEFAULT && !(MIN_RAM_MB..=MAX_RAM_MB).contains(&ram_mb) {
            return Err(format!("la mémoire doit être comprise entre {MIN_RAM_MB} et {MAX_RAM_MB} Mo"));
        }
        let mut opts = ImportOptions { ram_mb, ..ImportOptions::default() };
        if crate::test_hooks::hooks().test_mode {
            // Mode test seulement : télécharge depuis le faux Modrinth en loopback.
            opts.allowed_hosts.extend(["127.0.0.1".to_string(), "localhost".to_string()]);
            opts.allow_http = true;
        }
        let kind = {
            let f = file.clone();
            tokio::task::spawn_blocking(move || mrpack::detect(&f))
                .await
                .map_err(|_| "import interrompu".to_string())?
                .map_err(|e| user_msg(&e))?
        };
        let hash_target = (origin.is_none() && kind == PackKind::Modrinth).then(|| file.clone());
        let (instances, index, store_root) = (self.instances.clone(), self.store.clone(), self.paths.store.clone());
        let cancel = Arc::new(AtomicBool::new(false));
        // Les futurs d'import ne sont pas prouvés `Send` par rustc : on les exécute sur un thread dédié.
        let handle = tokio::runtime::Handle::current();
        let mut missing: Vec<String> = Vec::new();
        let result = match kind {
            PackKind::Modrinth => tokio::task::spawn_blocking(move || {
                handle.block_on(mrpack::import(&file, &instances, &store_root, &index, &opts, progress, cancel))
            })
            .await,
            PackKind::CurseForge => {
                let r = tokio::task::spawn_blocking(move || {
                    let cf = crate::commands_browse::cf_resolver();
                    handle.block_on(mrpack::import_curseforge(&file, &cf, &instances, &store_root, &index, &opts, progress, cancel))
                })
                .await;
                r.map(|res| {
                    res.map(|c| {
                        missing = c.missing;
                        c.instance
                    })
                })
            }
            PackKind::Plain => {
                let (Some(minecraft), Some(loader)) = (minecraft, loader) else {
                    return Err("ce zip ne contient pas de métadonnées : choisis la version de Minecraft et le loader".into());
                };
                let default_name = file.file_stem().and_then(|s| s.to_str()).unwrap_or("Modpack").to_string();
                let name = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).unwrap_or(default_name);
                validate_create(&name, &minecraft, &loader, opts.ram_mb)?;
                let version = resolve_loader_version(&minecraft, &loader).await.map_err(|e| user_msg(&e))?;
                let spec = PlainSpec { name, minecraft, loader: LoaderSpec { loader: loader.loader, version } };
                tokio::task::spawn_blocking(move || {
                    handle.block_on(mrpack::import_plain(&file, &spec, &instances, &store_root, &index, &opts, cancel))
                })
                .await
            }
        };
        let inst = result.map_err(|_| "import interrompu".to_string())?.map_err(|e| user_msg(&e))?;
        // Les mods sont déjà installés : un échec de préparation du jeu ne doit pas les effacer.
        self.prepare_game_best_effort(&inst).await;
        let inst = self.attach_pack_origin(&crate::commands_browse::providers(), inst, origin, hash_target).await;
        Ok((inst, missing))
    }

    /// Enregistre l'origine du modpack sur l'instance importée : celle de l'appelant si elle est connue, sinon
    /// (`.mrpack` local) celle que Modrinth reconnaît à l'empreinte du fichier. Meilleur effort : sans origine ou en cas
    /// d'échec, l'instance est rendue telle quelle.
    async fn attach_pack_origin(&self, p: &core_lib::browse::Providers, inst: Instance, origin: Option<PackOrigin>, hash_target: Option<PathBuf>) -> Instance {
        let origin = match (origin, hash_target) {
            (Some(o), _) => Some(o),
            (None, Some(f)) => core_lib::browse::resolve_pack_origin(p, &f).await,
            (None, None) => None,
        };
        let Some(o) = origin else { return inst };
        match self.instances.set_pack(&inst.id, Some(o.clone())) {
            Ok(updated) => {
                // Le relevé écrit à l'import n'a pas d'origine : sans elle, la mise à jour retéléchargerait l'ancien pack.
                if let Err(e) = self.instances.game_dir(&inst.id).and_then(|g| core_lib::modpack_update::stamp_origin(&g, &o)) {
                    eprintln!("[Cubelink] relevé du modpack non daté pour {} : {e}", inst.id);
                }
                updated
            }
            Err(e) => {
                eprintln!("[Cubelink] origine du modpack non enregistrée pour {} : {e}", inst.id);
                inst
            }
        }
    }

    /// Instances trouvées chez Prism, la Modrinth App et la CurseForge App (dossiers habituels), ou dans le dossier `path`
    /// choisi à la main. Lecture seule.
    pub async fn launcher_import_scan(&self, path: Option<String>) -> Result<Vec<DetectedInstance>, String> {
        let folder = match path.map(|p| p.trim().to_string()).filter(|p| !p.is_empty()) {
            Some(p) => {
                let f = PathBuf::from(p);
                if !f.is_dir() {
                    return Err("ce dossier est introuvable".into());
                }
                Some(f)
            }
            None => None,
        };
        let roots = launcher_roots();
        tokio::task::spawn_blocking(move || match folder {
            Some(f) => import_launcher::scan_folder(&f),
            None => import_launcher::scan_known(&roots),
        })
        .await
        .map_err(|_| "recherche interrompue".to_string())
    }

    /// Importe l'instance du dossier `path`. Elle est relue sur le disque (aucune confiance dans ce que l'interface a reçu
    /// d'une recherche), copiée sans jamais modifier la source, puis le jeu est préparé comme pour une instance neuve.
    pub async fn launcher_import_run(&self, path: &str, launcher: Launcher, name: Option<String>, ram_mb: Option<u32>) -> Result<Instance, String> {
        let ram_mb = ram_mb.unwrap_or(core_lib::instance::RAM_DEFAULT);
        if ram_mb != core_lib::instance::RAM_DEFAULT && !(MIN_RAM_MB..=MAX_RAM_MB).contains(&ram_mb) {
            return Err(format!("la mémoire doit être comprise entre {MIN_RAM_MB} et {MAX_RAM_MB} Mo"));
        }
        let folder = PathBuf::from(path.trim());
        let key = folder.canonicalize().map_err(|_| "le dossier de l'instance est introuvable".to_string())?;
        let _running = LauncherImportGuard::begin(&key).ok_or("cette instance est déjà en cours d'import")?;
        let mut det = {
            let f = key.clone();
            tokio::task::spawn_blocking(move || import_launcher::load_instance(&f, launcher))
                .await
                .map_err(|_| "import interrompu".to_string())?
                .map_err(|e| user_msg(&e))?
        };
        if det.unsupported.is_none() {
            // Version du loader absente chez l'autre lanceur : la recommandée, cherchée AVANT de copier des Go de mondes.
            det.loader.version = resolve_loader_version(&det.minecraft, &det.loader).await.map_err(|e| user_msg(&e))?;
        }
        let name = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
        let (instances, index, store_root) = (self.instances.clone(), self.store.clone(), self.paths.store.clone());
        let emit = self.emit.clone();
        let progress: core_lib::mrpack::FilesProgress =
            Arc::new(move |d, t| emit("browse://progress", serde_json::json!({ "done": d, "total": t, "label": "Import de l'instance" })));
        let params = ImportParams { name, ram_mb, ..ImportParams::default() };
        let inst = tokio::task::spawn_blocking(move || {
            import_launcher::import_instance(&det, &params, &instances, &store_root, &index, Some(progress), Arc::new(AtomicBool::new(false)))
        })
        .await
        .map_err(|_| "import interrompu".to_string())?
        .map_err(|e| user_msg(&e))?;
        let noop: Progress = Arc::new(|_| {});
        let java_setting = JavaSetting::for_instance(inst.java_path.as_deref(), self.settings_get().java_path);
        if let Err(e) = ensure_game(&self.paths, &inst, java_setting, noop).await {
            let _ = self.instances.delete(&inst.id);
            return Err(user_msg(&e));
        }
        // Illustration : recherche silencieuse par nom, meilleur effort (hors ligne ou sans résultat, l'instance est rendue telle quelle).
        let art = tokio::time::timeout(std::time::Duration::from_secs(20), self.instances_art_lookup(&inst.id, true)).await;
        Ok(art.ok().and_then(Result::ok).unwrap_or(inst))
    }

    pub async fn minecraft_versions(&self) -> Result<Vec<String>, String> {
        let m = crate::install::versions_client().fetch_manifest().await.map_err(|e| user_msg(&e))?;
        Ok(release_ids(&m))
    }

    /// Vérifications synchrones puis tâche de fond ; la progression passe par `launch://state`.
    pub async fn instances_launch(self: &Arc<Self>, id: &str) -> Result<(), String> {
        // Un jeu tourne dans la zone integree : lancer une autre instance = changement de modpack (fermeture propre puis lancement).
        // Un jeu dans sa propre fenetre (reglage coupe, repli) garde le comportement habituel : refus, une partie est en cours.
        if self.stage.staged() && lock(&self.game_handle).as_ref().is_some_and(|(g, _)| g != id) {
            return self.game_switch(id).await;
        }
        self.launch_with(id, None).await
    }

    /// Lance l'instance ; `join` : (hôte, port) à rejoindre automatiquement (tunnel invité, refermé à la fin du jeu).
    pub(crate) async fn launch_with(self: &Arc<Self>, id: &str, join: Option<(String, u16)>) -> Result<(), String> {
        let r = self.launch_checked(id, join).await;
        if let Err(e) = &r {
            // Apres un changement de modpack le rideau attend ce lancement, ferme a mi-barre : s'il est refuse, le rideau se
            // rouvre sur le launcher, ou l'interface affiche l'erreur rendue ici.
            self.stage.abort_switch(&format!("lancement refuse : {e}"));
        }
        r
    }

    async fn launch_checked(self: &Arc<Self>, id: &str, join: Option<(String, u16)>) -> Result<(), String> {
        if self.quit.quitting() {
            return Err("Cubelink est en train de se fermer".into());
        }
        if !valid_id(id) {
            return Err("identifiant d'instance invalide".into());
        }
        if crate::commands_browse::pack_update_running(id) {
            return Err("une mise à jour du modpack est en cours : attends qu'elle se termine avant de jouer".into());
        }
        let inst = self.instances.get(id).map_err(|e| user_msg(&e))?;
        let profile = self.auth_status_refresh().await.ok_or("connectez-vous avant de lancer le jeu")?;
        let token = match &profile {
            Profile::Online { .. } => Some(self.online_token().await?),
            Profile::Offline { .. } => None,
        };
        {
            let mut g = lock(&self.game);
            if g.is_some() {
                return Err("une partie est déjà en cours de préparation ou de jeu".into());
            }
            *g = Some(inst.id.clone());
            // Sous le verrou `game` : un seul lancement à la fois, donc un seul drapeau.
            *lock(&self.launch_cancel) = Some((inst.id.clone(), Arc::new(AtomicBool::new(false))));
        }
        // Zone de jeu integree : les panneaux se ferment des le clic (sur le launcher, ou sur le jeu qui vient de se fermer).
        let what = format!("{} ({}, {:?})", inst.id, inst.minecraft, inst.loader.loader);
        if let Some(why) = self.stage.unavailable_reason() {
            self.stage.note(&format!("lancement de {what} hors de la fenetre du launcher : {why}"));
        } else {
            self.stage.note(&format!("lancement de {what} dans la fenetre du launcher"));
            if let Ok(dir) = self.instances.game_dir(&inst.id) {
                // NeoForge (FML 4) : l'ecran precoce cree la fenetre cachee, la fait repondre toutes les 50 ms pendant le
                // chargement des mods puis la rend au jeu (meme fenetre). Le couper (`false`) gele la fenetre ~20 s : on le garde.
                // Forge : non verifie, on garde l'ancien choix (ecran precoce coupe).
                let early = match inst.loader.loader {
                    core_lib::types::Loader::NeoForge => Some(true),
                    core_lib::types::Loader::Forge => Some(false),
                    _ => None,
                };
                if let Some(on) = early {
                    if let Err(e) = set_early_window(&dir.join("config").join("fml.toml"), on) {
                        tracing::warn!("fml.toml : {e}");
                    }
                }
                self.stage.begin(&inst.name, &dir.join("mods"), &self.paths.logs.join(&inst.id).join("launch-profile.json"));
            }
        }
        // Ce lancement n'est pas entre dans la zone integree (reglage coupe, session coupee, dossier illisible) : un rideau
        // laisse ferme par un changement de modpack n'aurait plus personne pour le rouvrir.
        if !self.stage.staged() {
            self.stage.abort_switch("le jeu suivant s'ouvre hors de la fenetre du launcher");
        }
        let me = self.clone();
        let cancel = lock(&self.launch_cancel).as_ref().map(|(_, f)| f.clone()).unwrap_or_default();
        // Runtime de Tauri (et non `tokio::spawn`) : valable même appelée hors d'un runtime tokio.
        tauri::async_runtime::spawn(async move {
            let id = inst.id.clone();
            let cancelled = {
                let _guard = GameGuard(me.clone());
                let joined = join.is_some();
                let cancelled = me.run_game(inst, profile, token, join, cancel).await;
                me.stage.run_finished();
                if joined {
                    me.join_close_tunnel().await;
                }
                cancelled
            };
            // Annulé pendant la préparation : `exited` part une fois `game` libéré, pour qu'un nouveau clic sur « Jouer » passe.
            // Le verrou `game` reste tenu pendant l'émission : un lancement concurrent de la même instance (ami, lien profond)
            // ne peut pas poser son `preparing` avant, donc cet `exited` ne périme jamais le suivant.
            if cancelled {
                let game = lock(&me.game);
                if game.is_none() {
                    me.emit_state(&id, LaunchPhase::Exited, None);
                }
            }
        });
        Ok(())
    }

    pub(crate) fn emit_state(&self, id: &str, state: LaunchPhase, message: Option<String>) {
        self.emit_event("launch://state", &LaunchStateEvent { id: id.to_string(), state, message });
    }

    /// Lance le jeu ; si l'agent LAN fait planter la JVM au chargement (erreur native impossible à attraper côté
    /// Java), relance UNE fois la même instance sans agent et désactive l'agent pour la session.
    /// Renvoie `true` si le lancement a été annulé avant le démarrage du jeu (l'événement `exited` reste à émettre).
    async fn run_game(
        self: &Arc<Self>,
        inst: Instance,
        profile: Profile,
        token: Option<String>,
        join_server: Option<(String, u16)>,
        cancel: Arc<AtomicBool>,
    ) -> bool {
        let mut crashed = false;
        if self.run_game_once(inst.clone(), profile.clone(), token.clone(), join_server.clone(), &mut crashed, &cancel).await {
            return true;
        }
        if crashed {
            self.lan_agent_off.store(true, std::sync::atomic::Ordering::SeqCst);
            tracing::warn!("LAN: l'agent a fait planter la JVM, relance sans agent");
            return self.run_game_once(inst, profile, token, join_server, &mut false, &cancel).await;
        }
        false
    }

    /// `true` : annulé pendant la préparation, avant le démarrage du jeu (rien n'a été émis : voir `run_game`).
    async fn run_game_once(
        self: &Arc<Self>,
        inst: Instance,
        profile: Profile,
        token: Option<String>,
        join_server: Option<(String, u16)>,
        crashed: &mut bool,
        cancel: &Arc<AtomicBool>,
    ) -> bool {
        let id = inst.id.clone();
        self.emit_state(&id, LaunchPhase::Preparing, None);
        let me = self.clone();
        let pid = id.clone();
        let progress: Progress = Arc::new(move |m| me.emit_state(&pid, LaunchPhase::Preparing, Some(m)));
        let java_setting = JavaSetting::for_instance(inst.java_path.as_deref(), self.settings_get().java_path);
        let prepared = match ensure_game_with(&self.paths, &inst, java_setting, progress, cancel.clone()).await {
            Ok(p) => p,
            Err(core_lib::types::Error::Cancelled) => return true,
            Err(e) => return self.fail(&id, user_msg(&e)),
        };
        let game_dir = match self.instances.game_dir(&id) {
            Ok(d) => d,
            Err(e) => return self.fail(&id, user_msg(&e)),
        };
        // Rejoindre : l'écran d'accueil « accessibilité » d'un dossier neuf bloquerait la connexion directe.
        if join_server.is_some() {
            if let Err(e) = core_lib::instance::skip_accessibility_onboarding(&game_dir) {
                tracing::warn!("options.txt : accueil accessibilité non désactivé ({e})");
            }
        }
        // Partage / agent LAN : seulement pour nos propres instances, jamais en rejoignant un ami.
        let own = join_server.is_none() && self.instances.kind(&id).map(|k| k == core_lib::instance::InstanceKind::Own).unwrap_or(false);
        let agent_arg = lan_agent_jar(self, own, join_server.as_ref()).and_then(|jar| {
            let java = core_lib::java::probe_with(&prepared.java, core_lib::java::run_java_version).map(|i| i.major);
            let has_srg = core_lib::lan_map::srg_table(&inst.minecraft).is_some();
            match agent_supported(java, core_lib::lan_map::jar_min_java(&jar), &inst.minecraft, inst.loader.loader, join_server.is_some(), has_srg) {
                Ok(()) => Some(lan_agent_arg(&jar, join_server.as_ref())),
                Err(why) => {
                    tracing::warn!("LAN: agent non injecté ({why}) : {} {:?}, Java {java:?}", inst.minecraft, inst.loader.loader);
                    None
                }
            }
        });
        let agent_on = agent_arg.is_some();
        let lan_map = if agent_on && join_server.is_none() { lan_map_path(self, &inst, &prepared.version, &prepared.libs_dir).await } else { None };
        let ram_mb_eff = self.launch_ram_mb(&inst);
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
            ram_mb: ram_mb_eff,
            os: Os::current(),
            arch: Arch::current(),
            launcher_name: "Cubelink".into(),
            launcher_version: env!("CARGO_PKG_VERSION").into(),
            extra_jvm_args: {
                let user = core_lib::instance::split_jvm_args(&inst.jvm_args);
                let mut a = core_lib::instance::default_jvm_args(cfg!(windows), ram_mb_eff, &user);
                a.extend(user);
                a.extend(agent_arg.map(|arg| match &lan_map {
                    Some(m) => with_map(arg, m),
                    None => arg,
                }));
                a
            },
            join_server: join_server.clone(),
            // Outil de test réel (e2e/14) : jamais hors mode test.
            quick_play_world: if crate::test_hooks::hooks().test_mode { std::env::var("CUBELINK_QUICKPLAY_WORLD").ok().filter(|w| !w.trim().is_empty()) } else { None },
        };
        let mut cmd = match build_command(&spec) {
            Ok(c) => c,
            Err(e) => return self.fail(&id, user_msg(&e)),
        };
        drop(spec);
        // Essai : fenetre du jeu cachee des sa creation (jamais hors de la zone integree, voir `Stage::hide_start`).
        cmd.hide_first_window = self.stage.hide_start(crate::stage::hide_start_switch());
        let log_dir = self.paths.logs.join(&id);
        // « Annuler le lancement » arrivé pendant la dernière étape : on ne démarre pas le jeu.
        if cancel.load(std::sync::atomic::Ordering::SeqCst) {
            return true;
        }
        // Le launcher se ferme (fenetre fermee pendant la preparation) : on ne demarre pas un jeu que plus rien ne suivrait.
        // `pass` est garde jusqu'a ce que le jeu soit suivi : une fermeture qui arrive entre-temps attend (voir `QuitGate`).
        let Some(pass) = self.quit.enter_spawn() else {
            self.stage.note("fermeture du launcher pendant la preparation : le jeu n'est pas demarre");
            return self.fail(&id, "Cubelink est en train de se fermer".into());
        };
        // Le jeu reste vivant (via `game_handle`) jusqu'à la fin de la boucle : le lâcher l'arrêterait.
        // Linux : un jeu de la Scene est force en X11/XWayland (Wayland natif non adoptable), voir `RunOptions::force_x11`.
        // Commande d'enrobage de l'instance (Linux : gamemoderun, mangohud...), placee devant Java.
        let run_opts = launch_run::RunOptions {
            force_x11: self.stage.force_x11(),
            wrapper: core_lib::instance::split_jvm_args(&inst.wrapper),
            ..Default::default()
        };
        let mut game = match launch_run::spawn_with(&cmd, &log_dir, run_opts).await {
            Ok(g) => g,
            Err(e) => return self.fail(&id, user_msg(&e)),
        };
        if self.quit.quitting() {
            // La fermeture est arrivee pendant le demarrage du processus : il est arrete tout de suite, avant toute fenetre.
            self.stage.note("fermeture du launcher pendant le demarrage du jeu : processus arrete");
            let _ = game.kill().await;
            return self.fail(&id, "Cubelink est en train de se fermer".into());
        }
        let Some(mut rx) = game.events() else {
            return self.fail(&id, "suivi du jeu indisponible".into());
        };
        let game = Arc::new(game);
        // Invité : Dynamic FPS (et autres) bride le rendu hors premier plan et retarde le quickPlay (voir `foreground`).
        #[cfg(any(windows, target_os = "linux"))]
        if join_server.is_some() {
            cubelink_host::foreground::bring_game_to_front(game.pid);
        }
        *lock(&self.game_handle) = Some((id.clone(), game.clone()));
        // Clic sur « Annuler le lancement » entre le démarrage du processus et la pose de `game_handle` : on arrête le jeu
        // tout juste lancé (la boucle ci-dessous émet `exited`).
        if cancel.load(std::sync::atomic::Ordering::SeqCst) {
            let _ = game.kill().await;
        }
        // `stage.staged()` est relu a chaque usage : un repli en cours de lancement rend sa fenetre au jeu, et tout le pilotage
        // du rideau devient alors sans objet (les appels eux-memes sont inertes hors de la zone integree).
        let mut revealed = false;
        let profile_path = self.paths.logs.join(&id).join("launch-profile.json");
        self.stage.watch(game.pid);
        // A partir d'ici la fermeture du launcher sait quoi faire du jeu : rattache, elle le ferme proprement.
        drop(pass);
        let mut batch = crate::commands_game::LogBatcher::default();
        let mut tick = tokio::time::interval(crate::commands_game::LOG_FLUSH_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut finished = false;
        let mut host = own.then(|| crate::commands_host::HostAuto::start(self, &id));
        let started = std::time::Instant::now();
        let mut agent_marker = false;
        loop {
            tokio::select! {
                ev = rx.recv() => {
                    let Some(ev) = ev else { break };
                    if let LaunchEvent::Output(line) = &ev {
                        agent_marker |= agent_crash_marker(line);
                        self.stage.feed_line(line);
                        if let Some(h) = host.as_mut() {
                            h.on_line(line);
                        }
                        batch.push(line);
                        continue;
                    }
                    self.emit_logs(&id, &mut batch);
                    if let LaunchEvent::Exited { code } = &ev {
                        self.stage.game_exited(*code);
                    }
                    if agent_on && agent_marker && started.elapsed() < AGENT_CRASH_WINDOW && is_failure(&ev) {
                        *crashed = true; // pas d'état d'échec visible : on relance sans agent
                        break;
                    }
                    if let Some(s) = state_for(&id, &ev) {
                        finished = matches!(s.state, LaunchPhase::Exited | LaunchPhase::Failed);
                        self.emit_event("launch://state", &s);
                    }
                    if finished {
                        break;
                    }
                }
                _ = tick.tick() => {
                    self.emit_logs(&id, &mut batch);
                    if !revealed && self.stage.is_ready() {
                        // le jeu est pret : la barre atteint 100 % puis les panneaux s'ecartent sur lui
                        revealed = true;
                        self.stage.mark_ready(&profile_path);
                        tokio::time::sleep(std::time::Duration::from_millis(450)).await;
                        self.stage.reveal();
                    }
                }
            }
        }
        self.emit_logs(&id, &mut batch);
        if let Some(h) = host {
            h.finish().await;
        }
        if !finished && !*crashed {
            self.emit_state(&id, LaunchPhase::Exited, None);
        }
        game.wait().await;
        false
    }

    fn fail(&self, id: &str, message: String) -> bool {
        self.emit_state(id, LaunchPhase::Failed, Some(message));
        false
    }
}

/// Instances installées.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub fn instances_list(state: State<'_, Arc<AppState>>) -> Result<Vec<Instance>, String> {
    // Les modpacks reçus d'un ami (instances d'invité) restent hors de la liste : ils ne sont lancés que via « Rejoindre ».
    Ok(state.instances_list().into_iter().filter(|i| !core_lib::guest::is_guest_instance(&state.instances, &i.id)).collect())
}

/// Crée une instance vide, puis prépare le jeu (progression `import://progress`). Si cette préparation échoue (réseau, Java), l'instance est conservée et `import://deferred` l'annonce : le jeu sera téléchargé au premier lancement.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn instances_create(
    state: State<'_, Arc<AppState>>,
    name: String,
    minecraft: String,
    loader: LoaderSpec,
    ram_mb: u32,
) -> Result<Instance, String> {
    state.instances_create(name, minecraft, loader, ram_mb).await
}

/// Importe un .mrpack, un zip CurseForge ou un zip brut (les champs facultatifs ne servent qu'au zip brut). Avertissements par `import://warning`. La préparation du jeu qui suit l'import est « au mieux » : progression `import://progress` ; si elle échoue, l'instance et ses mods sont conservés et `import://deferred` l'annonce (le jeu sera téléchargé au premier lancement). Seule une erreur d'import elle-même renvoie une erreur. Un `.mrpack` est reconnu auprès de Modrinth par son SHA-1 (meilleur effort, borné à 20 s, silencieux hors ligne) : s'il est connu, `Instance.pack` reçoit son projet et sa version, sinon `pack` reste absent et l'import réussit quand même.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn instances_import_mrpack(
    state: State<'_, Arc<AppState>>,
    path: String,
    name: Option<String>,
    minecraft: Option<String>,
    loader: Option<LoaderSpec>,
    ram_mb: Option<u32>,
) -> Result<Instance, String> {
    state.instances_import_mrpack(&path, name, minecraft, loader, ram_mb).await
}

/// Cherche les instances de Prism / MultiMC, de la Modrinth App et de la CurseForge App (dossiers habituels Windows et Linux, Flatpak compris) ou, avec `path`, dans le dossier choisi (une instance, un dossier d'instances ou la racine d'un lanceur). Lecture seule : rien n'est écrit chez l'autre lanceur. Une instance au loader inconnu de Cubelink est listée avec `unsupported` (raison) et n'est pas importable.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn launcher_import_scan(state: State<'_, Arc<AppState>>, path: Option<String>) -> Result<Vec<DetectedInstance>, String> {
    state.launcher_import_scan(path).await
}

/// Importe l'instance du dossier `path` (relue sur le disque, jamais l'objet reçu d'une recherche) : version de Minecraft et loader repris (version du loader recommandée si le lanceur ne la donne pas), `mods/`, `config/`, `saves/`, `resourcepacks/`, `shaderpacks/`, `options.txt` et `servers.dat` placés dans une nouvelle instance. Les jars et zips de packs vont dans le magasin par hash puis sont liés (lien physique, repli copie), le reste est copié ; la source n'est jamais modifiée ni liée, les liens symboliques ne sont pas suivis. Échec : l'instance n'est pas créée. Progression par `browse://progress`, puis recherche silencieuse de l'illustration par nom (20 s au plus).
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn launcher_import_run(state: State<'_, Arc<AppState>>, path: String, launcher: Launcher, name: Option<String>, ram_mb: Option<u32>) -> Result<Instance, String> {
    state.launcher_import_run(&path, launcher, name, ram_mb).await
}

/// Lance l'instance. Progression par `launch://state`, sortie du jeu par `launch://log`.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn instances_launch(state: State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
    state.inner().instances_launch(&id).await
}

/// Versions de Minecraft disponibles.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn minecraft_versions(state: State<'_, Arc<AppState>>) -> Result<Vec<String>, String> {
    state.minecraft_versions().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::test_state;

    #[test]
    fn un_import_d_instance_ne_demarre_pas_deux_fois_sur_le_meme_dossier() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = (d.path().join("a"), d.path().join("b"));
        let g = LauncherImportGuard::begin(&a).expect("premier import");
        assert!(LauncherImportGuard::begin(&a).is_none(), "même dossier : refusé");
        assert!(LauncherImportGuard::begin(&b).is_some(), "autre dossier : accepté");
        drop(g);
        assert!(LauncherImportGuard::begin(&a).is_some(), "libéré à la fin");
    }

    #[tokio::test]
    async fn import_d_un_autre_lanceur_valide_ses_entrees_avant_de_toucher_au_disque() {
        let d = tempfile::tempdir().unwrap();
        let (st, _) = test_state(d.path());
        let nothing = d.path().join("rien");
        assert!(st.launcher_import_scan(Some(nothing.display().to_string())).await.is_err());
        let e = st.launcher_import_run(&nothing.display().to_string(), Launcher::Prism, None, None).await.unwrap_err();
        assert!(e.contains("introuvable"), "{e}");
        let e = st.launcher_import_run(&d.path().display().to_string(), Launcher::Prism, None, Some(12)).await.unwrap_err();
        assert!(e.contains("mémoire"), "{e}");
        // Un dossier qui n'est pas une instance de ce lanceur : refusé, aucune instance créée.
        let e = st.launcher_import_run(&d.path().display().to_string(), Launcher::Curseforge, None, None).await.unwrap_err();
        assert!(e.contains("instance"), "{e}");
        assert!(st.instances_list().is_empty());
    }

    #[tokio::test]
    async fn attach_pack_origin_known_hash_unknown_and_explicit() {
        use core_lib::browse::Providers;
        use core_lib::providers::curseforge_api::CfApi;
        use core_lib::providers::modrinth_api::ModrinthClient;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let d = tempfile::tempdir().unwrap();
        let (state, _ev) = test_state(d.path());
        let mr = MockServer::start().await;
        let cf = MockServer::start().await;
        let p = Providers { modrinth: ModrinthClient::with_base_url(format!("{}/v2", mr.uri()), "test"), cf: CfApi::with_base(cf.uri(), "k"), allow_loopback: true };
        let spec = LoaderSpec { loader: Loader::Fabric, version: "0.1".into() };
        let file = d.path().join("pack.mrpack");
        std::fs::write(&file, b"un mrpack").unwrap();
        let sha1 = core_lib::unknown_mods::sha1_hex(b"un mrpack");
        Mock::given(method("GET"))
            .and(path(format!("/v2/version_file/{sha1}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "VER1", "project_id": "PROJ1", "version_number": "1.2", "files": [], "dependencies": []})))
            .mount(&mr)
            .await;
        // 1. .mrpack local reconnu par son empreinte : origine écrite dans l'instance et dans instance.json.
        let a = state.instances.create("A", "1.21.1", spec.clone(), 2048).unwrap();
        let a = state.attach_pack_origin(&p, a, None, Some(file.clone())).await;
        let want = PackOrigin { source: "modrinth".into(), project_id: "PROJ1".into(), version_id: "VER1".into() };
        assert_eq!(a.pack, Some(want.clone()));
        let raw = std::fs::read_to_string(d.path().join("instances").join(&a.id).join("instance.json")).unwrap();
        assert!(raw.contains("PROJ1") && raw.contains("VER1"), "{raw}");
        assert_eq!(state.instances.get(&a.id).unwrap().pack, Some(want));
        // 2. Hash inconnu (404 par défaut du faux serveur) : pas d'origine, pas d'erreur.
        std::fs::write(&file, b"pack modifie").unwrap();
        let b = state.instances.create("B", "1.21.1", spec.clone(), 2048).unwrap();
        let b = state.attach_pack_origin(&p, b, None, Some(file.clone())).await;
        assert_eq!(b.pack, None);
        assert_eq!(state.instances.get(&b.id).unwrap().pack, None);
        // 3. Origine fournie (Parcourir) : posée telle quelle, sans interroger Modrinth (aucune cible de hash).
        let c = state.instances.create("C", "1.21.1", spec, 2048).unwrap();
        let given = PackOrigin { source: "curseforge".into(), project_id: "50".into(), version_id: "7".into() };
        // Relevé écrit à l'import, sans origine : l'origine posée le date, il devient utilisable pour la mise à jour.
        let g = state.instances.game_dir(&c.id).unwrap();
        std::fs::write(g.join(core_lib::modpack_update::RECORD_FILE), br#"{"format":1,"origin":null,"files":{}}"#).unwrap();
        assert!(core_lib::modpack_update::needs_old_pack(&state.instances, &c.id).unwrap());
        let c = state.attach_pack_origin(&p, c, Some(given.clone()), None).await;
        assert_eq!(c.pack, Some(given));
        assert!(!core_lib::modpack_update::needs_old_pack(&state.instances, &c.id).unwrap());
    }

    #[test]
    fn agent_supporte_selon_version_java_et_loader() {
        use Loader::*;
        let ok = |j, jm, v, l, join, t| agent_supported(j, jm, v, l, join, t);
        // moderne : inchangé (Java >= jar), table SRG sans effet
        assert_eq!(ok(Some(21), 8, "1.21.1", Fabric, false, false), Ok(()));
        assert_eq!(ok(Some(21), 17, "26.1", NeoForge, true, false), Ok(()));
        assert_eq!(ok(Some(17), 17, "1.20.1", Forge, false, false), Ok(()));
        assert_eq!(ok(None, 17, "1.21.1", Vanilla, false, false), Ok(()));
        assert_eq!(ok(Some(21), 17, "snapshot-inconnu", Vanilla, false, false), Ok(()));
        // 1.13 → 1.16 sous Java 8 : seulement si le jar est en classfile 52
        assert_eq!(ok(Some(8), 8, "1.16.5", Fabric, false, false), Ok(()));
        assert!(ok(Some(8), 17, "1.16.5", Fabric, false, false).is_err());
        assert!(ok(Some(8), 17, "1.14.4", Forge, true, false).is_err());
        // Java < 8 : jamais
        assert!(ok(Some(7), 8, "1.7.10", Forge, false, true).is_err());
        assert!(ok(Some(6), 8, "1.21.1", Fabric, false, false).is_err());
        // legacy : Forge + hôte + table + Java 8 avec jar 52
        for v in ["1.7.10", "1.8.9", "1.12.2"] {
            assert_eq!(ok(Some(8), 8, v, Forge, false, true), Ok(()), "{v}");
            assert!(ok(Some(8), 8, v, Forge, false, false).is_err(), "{v} sans table");
            assert!(ok(Some(8), 8, v, Forge, true, true).is_err(), "{v} invité");
            assert!(ok(Some(8), 8, v, Vanilla, false, true).is_err(), "{v} vanilla");
            assert!(ok(Some(8), 8, v, Fabric, false, true).is_err(), "{v} fabric");
            assert!(ok(Some(8), 17, v, Forge, false, true).is_err(), "{v} jar Java 17 sous Java 8");
            assert!(ok(None, 8, v, Forge, false, true).is_err(), "{v} Java inconnue");
        }
        // Java 17 sur legacy avec jar 52 : accepté (Java plus récente)
        assert_eq!(ok(Some(17), 8, "1.12.2", Forge, false, true), Ok(()));
    }

    #[test]
    fn marqueurs_d_echec_d_agent() {
        for l in [
            "FATAL ERROR in native method: processing of -javaagent failed, appending to system class path failed",
            r"Unable to add \\?\C:\x\a.jar to system class path",
            r"Error opening zip file or JAR manifest missing : \?\C:\a.jar",
            "agent library failed Agent_OnLoad: instrument",
        ] {
            assert!(agent_crash_marker(l), "{l}");
        }
        assert!(!agent_crash_marker("[Cubelink] LAN: agent charge (java 25)"));
        assert!(!agent_crash_marker("Exception in thread main"));
        assert!(is_failure(&LaunchEvent::Exited { code: 1 }) && !is_failure(&LaunchEvent::Exited { code: 0 }));
    }

    /// Premier java trouvé : CUBELINK_TEST_JAVA, JAVA_HOME, JDK usuels, PATH.
    fn find_java() -> Option<std::path::PathBuf> {
        let exe = if cfg!(windows) { "java.exe" } else { "java" };
        let mut c: Vec<std::path::PathBuf> = vec![];
        c.extend(std::env::var_os("CUBELINK_TEST_JAVA").map(Into::into));
        c.extend(std::env::var_os("JAVA_HOME").map(|h| std::path::PathBuf::from(h).join("bin").join(exe)));
        for root in [r"C:\Program Files\Microsoft", r"C:\Program Files\Java", r"C:\Program Files\Eclipse Adoptium"] {
            if let Ok(rd) = std::fs::read_dir(root) {
                c.extend(rd.flatten().map(|e| e.path().join("bin").join(exe)));
            }
        }
        c.push(std::path::PathBuf::from(exe));
        c.into_iter().find(|p| std::process::Command::new(p).arg("-version").output().map(|o| o.status.success()).unwrap_or(false))
    }

    fn run_with_agent(java: &std::path::Path, arg: &str, jar: &std::path::Path) -> (Option<i32>, String) {
        let mut cmd = std::process::Command::new(java);
        core_lib::java::no_window(&mut cmd);
        let o = cmd.arg(arg).arg("-cp").arg(jar).arg("app.cubelink.agent.LanAgent").arg("wait").output().unwrap();
        (o.status.code(), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)))
    }

    /// VRAIE JVM, arguments exacts de la production, chemin avec espaces et caractères non ASCII.
    #[test]
    fn vraie_jvm_charge_l_agent_et_detecte_les_echecs() {
        let Some(java) = find_java() else {
            eprintln!("SKIP vraie_jvm : aucun java trouvé (CUBELINK_TEST_JAVA)");
            return;
        };
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/resources/cubelink-lan-agent.jar");
        assert!(src.is_file(), "jar de l'agent absent : lance core/resources/lan-agent/build.sh");
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("Mon dossier é");
        std::fs::create_dir_all(&sub).unwrap();
        let jar = sub.join("cubelink-lan-agent.jar");
        std::fs::copy(&src, &jar).unwrap();
        let map = sub.join("table é.map");
        std::fs::write(&map, "# cubelink-lanmap official\nC minecraft fgo\n").unwrap();

        // 1. chemin normal, avec table
        let (code, out) = run_with_agent(&java, &javaagent_arg(&jar, Some(&map)), &jar);
        assert_eq!(code, Some(0), "{out}");
        assert!(out.contains("[Cubelink] LAN:"), "{out}");
        assert!(!agent_crash_marker(&out), "{out}");
        // 2. variante verbatim (bug réel) : normalisée par `javaagent_arg`, et le mal d'origine est reproduit sans elle
        #[cfg(windows)]
        {
            let verb = std::path::PathBuf::from(format!(r"\\?\{}", jar.display()));
            let arg = javaagent_arg(&verb, None);
            assert!(!arg.contains(r"\\?\"), "{arg}");
            let (code, out) = run_with_agent(&java, &arg, &jar);
            assert_eq!(code, Some(0), "{out}");
            let (code, out) = run_with_agent(&java, &format!("-javaagent:{}", verb.display()), &jar);
            assert_ne!(code, Some(0), "{out}");
            assert!(agent_crash_marker(&out), "{out}");
        }
        // 2b. dossier CJK : copié sous un chemin ASCII par `javaagent_arg` (la JVM ne sait pas lire le chemin brut)
        let cjk = dir.path().join("日本語 dossier");
        std::fs::create_dir_all(&cjk).unwrap();
        let jar_cjk = cjk.join("cubelink-lan-agent.jar");
        std::fs::copy(&src, &jar_cjk).unwrap();
        let arg = javaagent_arg(&jar_cjk, None);
        assert!(arg.is_ascii(), "{arg}");
        let (code, out) = run_with_agent(&java, &arg, &jar);
        assert_eq!(code, Some(0), "{out}");
        assert!(out.contains("[Cubelink] LAN:"), "{out}");
        let (raw_code, raw_out) = run_with_agent(&java, &format!("-javaagent:{}", jar_cjk.display()), &jar);
        eprintln!("chemin CJK brut (sans copie) : code {raw_code:?} {}", raw_out.lines().last().unwrap_or(""));
        // 3. jar corrompu : la JVM échoue et la sortie est reconnue par le garde-fou
        let bad = sub.join("corrompu.jar");
        std::fs::write(&bad, b"pas un jar").unwrap();
        let (code, out) = run_with_agent(&java, &javaagent_arg(&bad, None), &jar);
        assert_ne!(code, Some(0), "{out}");
        assert!(agent_crash_marker(&out), "{out}");
    }

    #[test]
    fn noauth_jamais_sans_mode_test_et_variable() {
        assert!(!noauth_enabled(false, Some("1")), "hors mode test");
        assert!(!noauth_enabled(true, None), "sans variable");
        assert!(!noauth_enabled(true, Some("0")));
        assert!(noauth_enabled(true, Some("1")));
        let base = "-javaagent:C:/x/a.jar".to_string();
        assert_eq!(with_test_noauth(base.clone(), false), base, "désactivé : ligne inchangée");
        assert_eq!(with_test_noauth(base.clone(), true), format!("{base}=noauth"));
        assert_eq!(with_test_noauth(format!("{base}=map=m.tiny"), true), format!("{base}=noauth,map=m.tiny"));
    }

    #[test]
    fn option_join_de_l_agent_invite() {
        let base = "-javaagent:C:/x/a.jar".to_string();
        assert_eq!(with_join(base.clone(), "127.0.0.1", 60061), format!("{base}=join=127.0.0.1:60061"));
        // caractères hors liste retirés (pas d'injection d'options dans la ligne de lancement)
        assert_eq!(with_join(base.clone(), "1.2.3.4,noauth", 1), format!("{base}=join=1.2.3.4noauth:1"));
        let j = with_join(base.clone(), "127.0.0.1", 5);
        assert_eq!(with_map(j, std::path::Path::new("m.tiny")), format!("{base}=join=127.0.0.1:5,map=m.tiny"));
        assert_eq!(with_map(base.clone(), std::path::Path::new("m.tiny")), format!("{base}=map=m.tiny"));
        assert!(!with_join(base, "127.0.0.1", 5).contains("noauth"), "invité : jamais noauth");
    }

    #[test]
    fn chemin_agent_sans_prefixe_windows_etendu() {
        use std::path::Path;
        // Bug réel : « Unable to add \\?\C:\...\cubelink-lan-agent.jar to system class path » (la JVM plante).
        assert_eq!(plain_windows_path(Path::new(r"\\?\C:\Users\x\AppData\Local\Cubelink\a.jar")), Path::new(r"C:\Users\x\AppData\Local\Cubelink\a.jar"));
        assert_eq!(plain_windows_path(Path::new(r"\\?\UNC\srv\share\a.jar")), Path::new(r"\\srv\share\a.jar"));
        assert_eq!(plain_windows_path(Path::new(r"C:\a\b.jar")), Path::new(r"C:\a\b.jar"));
        assert_eq!(plain_windows_path(Path::new("/tmp/a.jar")), Path::new("/tmp/a.jar"));
    }

    fn vanilla() -> LoaderSpec {
        LoaderSpec { loader: Loader::Vanilla, version: String::new() }
    }

    /// Réel (disque, vraie RAM de la machine, vrai java) : la mémoire « Automatique » suit le nombre de jars de `mods/`,
    /// l'instance et le réglage manuel l'emportent, et la JVM accepte vraiment le `-Xmx` calculé.
    #[test]
    fn memoire_automatique_au_lancement() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        assert_eq!(st.settings_get().default_ram_mb, 0, "Automatique par défaut");
        let phys = core_lib::sysmem::physical_ram_mb();
        let mut inst = st.instances.create("Auto", "1.21.1", vanilla(), core_lib::instance::RAM_DEFAULT).unwrap();
        assert_eq!(st.launch_ram_mb(&inst), core_lib::instance::auto_ram_mb(0, phys));
        let mods = st.instances.game_dir(&inst.id).unwrap().join("mods");
        std::fs::create_dir_all(&mods).unwrap();
        for n in 0..120 {
            std::fs::write(mods.join(format!("m{n}.jar")), b"x").unwrap();
        }
        // `inst` est l'ancienne copie (0 mod) : le lancement recompte sur le disque.
        let auto = st.launch_ram_mb(&inst);
        assert_eq!(auto, core_lib::instance::auto_ram_mb(120, phys));
        assert!(auto >= core_lib::instance::auto_ram_mb(0, phys) && auto.is_multiple_of(256) && auto >= 1024, "{auto}");
        if phys.is_some_and(|p| p >= 10 * 1024) {
            assert_eq!(auto, 6144, "120 mods sur un PC de 10 Go ou plus");
        }
        // Réglage global manuel, puis réglage de l'instance.
        lock(&st.settings).default_ram_mb = 3072;
        assert_eq!(st.launch_ram_mb(&inst), 3072);
        inst.ram_mb = 2048;
        assert_eq!(st.launch_ram_mb(&inst), 2048);

        // Vrai java : le tas maximal obtenu est bien celui demandé.
        let Ok(out) = std::process::Command::new("java").args([format!("-Xmx{auto}M"), "-XX:+PrintFlagsFinal".into(), "-version".into()]).output() else {
            return; // pas de java sur cette machine
        };
        let text = String::from_utf8_lossy(&out.stdout);
        let heap: u64 = text.lines().find(|l| l.contains(" MaxHeapSize "))
            .and_then(|l| l.split('=').nth(1)).and_then(|v| v.split_whitespace().next()).and_then(|v| v.parse().ok())
            .unwrap_or_else(|| panic!("MaxHeapSize introuvable : {text}"));
        let want = u64::from(auto) * 1024 * 1024;
        assert!(heap.abs_diff(want) <= want / 50, "java a pris {heap} octets au lieu de {want}");
    }

    /// Réel (disque, sockets, tâche de lancement) : le jeu à télécharger ne répond jamais ; « Annuler le lancement »
    /// interrompt la préparation tout de suite, émet `exited` (jamais `failed`), libère l'instance, qui se relance aussitôt.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn annuler_pendant_la_preparation_libere_l_instance() {
        use std::time::{Duration, Instant};
        let dir = tempfile::tempdir().unwrap();
        let (st, events) = test_state(dir.path());
        // Serveur muet : la connexion TLS vers le jar du jeu reste en attente.
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let srv = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((s, _)) = l.accept().await {
                held.push(s);
            }
        });
        let inst = st.instances.create("Lent", "1.21.1", vanilla(), 2048).unwrap();
        let vdir = st.paths.versions.join("1.21.1");
        std::fs::create_dir_all(&vdir).unwrap();
        std::fs::write(
            vdir.join("1.21.1.json"),
            serde_json::json!({
                "id": "1.21.1", "mainClass": "net.minecraft.client.main.Main",
                "assetIndex": { "id": "17", "url": format!("https://127.0.0.1:{port}/i.json"), "sha1": "0".repeat(40), "size": 1 },
                "downloads": { "client": { "url": format!("https://127.0.0.1:{port}/c.jar"), "sha1": "0".repeat(40), "size": 10 } },
                "javaVersion": { "majorVersion": 21 }
            })
            .to_string(),
        )
        .unwrap();
        // Java imposé par le réglage : la recherche de Java ne touche pas au réseau.
        std::fs::create_dir_all(dir.path().join("faux-java")).unwrap();
        let java = dir.path().join("faux-java").join("java");
        std::fs::write(&java, b"#!/bin/sh\n").unwrap();
        lock(&st.settings).java_path = Some(java.display().to_string());
        *lock(&st.profile) = Some(Profile::Offline { name: "Test".into(), uuid: "00000000-0000-0000-0000-000000000000".into() });

        let states = |events: &crate::state::test_support::Events| -> Vec<(String, Option<String>)> {
            lock(events)
                .iter()
                .filter(|(n, _)| n == "launch://state")
                .map(|(_, v)| (v["state"].as_str().unwrap().to_string(), v["message"].as_str().map(str::to_string)))
                .collect()
        };
        for tour in 1..=2 {
            let before = states(&events).len();
            st.launch_with(&inst.id, None).await.unwrap_or_else(|e| panic!("tour {tour} : {e}"));
            tokio::time::sleep(Duration::from_millis(400)).await;
            assert_eq!(lock(&st.game).as_deref(), Some(inst.id.as_str()), "préparation en cours");
            let t = Instant::now();
            st.instances_kill(&inst.id).await.unwrap();
            loop {
                if states(&events)[before..].iter().any(|(s, _)| s == "exited") {
                    break;
                }
                assert!(t.elapsed() < Duration::from_secs(3), "pas d'exited : {:?}", states(&events));
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            let seq = states(&events)[before..].to_vec();
            assert!(seq.iter().all(|(s, _)| s != "failed"), "{seq:?}");
            assert_eq!(seq.last().unwrap(), &("exited".to_string(), None), "{seq:?}");
            assert!(seq.iter().any(|(s, m)| s == "preparing" && m.as_deref() == Some("Annulation en cours…")), "{seq:?}");
            // `exited` part après la libération : un nouveau clic sur « Jouer » passe tout de suite.
            assert!(lock(&st.game).is_none() && lock(&st.launch_cancel).is_none() && lock(&st.game_handle).is_none());
        }
        srv.abort();
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

    /// Changement de modpack dont le lancement suivant est REFUSE (chaque refus de `launch_with`) : le rideau, reste ferme a
    /// mi-barre apres la fin de l'ancien jeu, se rouvre sur le launcher, et l'erreur revient a l'interface.
    #[tokio::test(flavor = "multi_thread")]
    async fn changement_de_modpack_lancement_refuse_rouvre_le_rideau() {
        use crate::stage::fake::{switch_pending, FakeHost};
        use cubelink_host::flow::Flow;
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let fake = FakeHost::new();
        fake.install(&st.stage);
        let inst = st.instances.create("Neuf", "0.0-inexistante", vanilla(), 2048).unwrap();
        let check = |what: &str, r: Result<(), String>| {
            assert!(r.is_err(), "{what} : refus attendu");
            assert!(!fake.curtain_stuck(), "{what} : rideau reste ferme ({:?})", fake.flow());
            assert_eq!(fake.flow(), Flow::Launcher, "{what}");
        };
        switch_pending(&st.stage, &fake);
        check("identifiant invalide", st.launch_with("../x", None).await);
        switch_pending(&st.stage, &fake);
        check("instance inconnue", st.launch_with("inconnue", None).await);
        switch_pending(&st.stage, &fake);
        check("pas de compte (jeton refuse)", st.launch_with(&inst.id, None).await);
        *lock(&st.profile) = Some(Profile::Offline { name: "Steve".into(), uuid: "0".repeat(32) });
        switch_pending(&st.stage, &fake);
        *lock(&st.game) = Some("autre".into());
        check("verrou de jeu deja pris", st.launch_with(&inst.id, None).await);
        *lock(&st.game) = None;
        switch_pending(&st.stage, &fake);
        st.quit.begin_quit();
        let r = st.launch_with(&inst.id, None).await;
        assert!(r.as_ref().is_err_and(|e| e.contains("en train de se fermer")), "{r:?}");
        check("launcher en cours de fermeture", r);
        assert!(lock(&st.game).is_none(), "aucun lancement engage pendant la fermeture");
    }

    /// Lancement ACCEPTE apres un changement de modpack, mais qui echoue pendant la preparation (version introuvable) : la
    /// tache de jeu s'arrete avant `watch`, le rideau se rouvre sur le launcher et l'echec est annonce a l'interface.
    #[tokio::test(flavor = "multi_thread")]
    async fn changement_de_modpack_echec_de_preparation_rouvre_le_rideau() {
        use crate::stage::fake::{switch_pending, FakeHost};
        use cubelink_host::flow::Flow;
        let dir = tempfile::tempdir().unwrap();
        let (st, events) = test_state(dir.path());
        let fake = FakeHost::new();
        fake.install(&st.stage);
        let inst = st.instances.create("Neuf", "0.0-inexistante", vanilla(), 2048).unwrap();
        *lock(&st.profile) = Some(Profile::Offline { name: "Steve".into(), uuid: "0".repeat(32) });
        switch_pending(&st.stage, &fake);
        st.launch_with(&inst.id, None).await.unwrap();
        assert_eq!(fake.count("begin"), 2, "le lancement est entre dans la zone integree");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while lock(&st.game).is_some() {
            assert!(std::time::Instant::now() < deadline, "la tache de jeu ne s'arrete pas");
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(!fake.curtain_stuck(), "{:?}", fake.flow());
        assert_eq!(fake.flow(), Flow::Launcher);
        assert_eq!(fake.count("watch"), 1, "le nouveau jeu n'a jamais demarre");
        let failed = lock(&events).iter().any(|(n, v)| n == "launch://state" && v["id"] == inst.id.as_str() && v["state"] == "failed" && v["message"].is_string());
        assert!(failed, "l'echec reste visible du joueur : {:?}", lock(&events));
    }

    /// Le jeu suivant s'ouvre HORS de la zone integree (reglage coupe entre la fermeture et le lancement) : plus personne ne
    /// rouvrirait le rideau du changement de modpack.
    #[tokio::test(flavor = "multi_thread")]
    async fn changement_de_modpack_vers_un_jeu_hors_scene_rouvre_le_rideau() {
        use crate::stage::fake::{switch_pending, FakeHost};
        use cubelink_host::flow::Flow;
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let fake = FakeHost::new();
        fake.install(&st.stage);
        let inst = st.instances.create("Neuf", "0.0-inexistante", vanilla(), 2048).unwrap();
        *lock(&st.profile) = Some(Profile::Offline { name: "Steve".into(), uuid: "0".repeat(32) });
        switch_pending(&st.stage, &fake);
        st.stage.set_wanted(false);
        st.launch_with(&inst.id, None).await.unwrap();
        assert_eq!(fake.count("begin"), 1, "ce lancement n'entre pas dans la zone integree");
        assert!(!fake.curtain_stuck(), "{:?}", fake.flow());
        assert_eq!(fake.flow(), Flow::Launcher);
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

/// Met `earlyWindowControl = <on>` dans le `fml.toml` de l'instance (cree le fichier au besoin). Sans effet s'il l'est deja.
fn set_early_window(path: &std::path::Path, on: bool) -> std::io::Result<()> {
    let cur = std::fs::read_to_string(path).unwrap_or_default();
    let out = early_window(&cur, on);
    if out != cur {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(path, out)?;
    }
    Ok(())
}

/// Pur : texte du `fml.toml` avec `earlyWindowControl = <on>` (ligne remplacee, ou ajoutee en tete du fichier).
fn early_window(toml: &str, on: bool) -> String {
    let line = format!("earlyWindowControl = {on}");
    let is_key = |l: &str| l.trim_start().strip_prefix("earlyWindowControl").is_some_and(|r| r.trim_start().starts_with('='));
    if toml.lines().any(is_key) {
        let mut out: Vec<String> = toml.lines().map(|l| if is_key(l) { line.clone() } else { l.to_string() }).collect();
        if toml.ends_with('\n') {
            out.push(String::new());
        }
        return out.join("\n");
    }
    format!("{line}\n{toml}")
}

#[cfg(test)]
mod early_window_tests {
    use super::early_window;

    #[test]
    fn ajoute_remplace_et_est_idempotent() {
        assert_eq!(early_window("", false), "earlyWindowControl = false\n");
        assert_eq!(early_window("a = 1\n", false), "earlyWindowControl = false\na = 1\n");
        let r = early_window("x = 2\n#c\nearlyWindowControl = true\ny = 3\n", false);
        assert_eq!(r, "x = 2\n#c\nearlyWindowControl = false\ny = 3\n");
        assert_eq!(early_window(&r, false), r);
        assert_eq!(early_window(&r, true), "x = 2\n#c\nearlyWindowControl = true\ny = 3\n");
    }
}
