//! Commandes d'instances : liste, création (avec installation), import .mrpack, lancement.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use core_lib::instance::{MAX_RAM_MB, MIN_RAM_MB};
use core_lib::launch_args::{build_command, LaunchSpec, LaunchVersion};
use core_lib::launch_run::{self, LaunchEvent, LaunchPhase, LaunchStateEvent};
use core_lib::mojang::libraries::{Arch, Os};
use core_lib::mojang::versions::release_ids;
use core_lib::mrpack::{self, ImportOptions, PackKind, PlainSpec};
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
                eprintln!("[Cubelink] LAN: table SRG illisible ({e})");
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
                    eprintln!("[Cubelink] LAN: jar intermediary introuvable, noms Fabric non traduits");
                    return None;
                }
            }
        }
        Loader::NeoForge | Loader::Forge => return None,
    };
    let mappings = downloads.client_mappings.as_ref()?; // absent : jeu non obfusqué (26.x)
    match ensure_lanmap(&dir, &reqwest::Client::new(), &inst.minecraft, runtime, mappings, inter.as_deref()).await {
        Ok(p) => Some(p),
        Err(e) => {
            eprintln!("[Cubelink] LAN: correspondance de noms indisponible ({e}), l'ouverture auto peut échouer");
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
        let _ = self.instances.mark_custom(&inst.id);
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

    /// Importe un modpack : `.mrpack` (Modrinth), zip CurseForge (`manifest.json`, sans clé API ; les mods introuvables sont signalés par `import://warning`) ou
    /// zip générique (contenu d'instance brut, avec nom / version de Minecraft / loader choisis par l'utilisateur).
    pub async fn instances_import_mrpack(
        &self,
        path: &str,
        name: Option<String>,
        minecraft: Option<String>,
        loader: Option<LoaderSpec>,
    ) -> Result<Instance, String> {
        let (inst, missing) = self.import_pack_file(path, name, minecraft, loader, None).await?;
        if !missing.is_empty() {
            self.emit_event("import://warning", &serde_json::json!({ "missing": missing }));
        }
        Ok(inst)
    }

    /// Cœur de l'import : renvoie l'instance et les mods CurseForge introuvables (sans émettre d'événement).
    pub(crate) async fn import_pack_file(
        &self,
        path: &str,
        name: Option<String>,
        minecraft: Option<String>,
        loader: Option<LoaderSpec>,
        progress: Option<core_lib::mrpack::FilesProgress>,
    ) -> Result<(Instance, Vec<String>), String> {
        let file = validate_mrpack_path(path)?;
        let mut opts = ImportOptions { ram_mb: self.settings_get().default_ram_mb, ..ImportOptions::default() };
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
        let noop: Progress = Arc::new(|_| {});
        match ensure_game(&self.paths, &inst, self.settings_get().java_path, noop).await {
            Ok(_) => Ok((inst, missing)),
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
                        eprintln!("[Cubelink] fml.toml : {e}");
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
        // Runtime de Tauri (et non `tokio::spawn`) : valable même appelée hors d'un runtime tokio.
        tauri::async_runtime::spawn(async move {
            let _guard = GameGuard(me.clone());
            let joined = join.is_some();
            me.run_game(inst, profile, token, join).await;
            me.stage.run_finished();
            if joined {
                me.join_close_tunnel().await;
            }
        });
        Ok(())
    }

    fn emit_state(&self, id: &str, state: LaunchPhase, message: Option<String>) {
        self.emit_event("launch://state", &LaunchStateEvent { id: id.to_string(), state, message });
    }

    /// Lance le jeu ; si l'agent LAN fait planter la JVM au chargement (erreur native impossible à attraper côté
    /// Java), relance UNE fois la même instance sans agent et désactive l'agent pour la session.
    async fn run_game(self: &Arc<Self>, inst: Instance, profile: Profile, token: Option<String>, join_server: Option<(String, u16)>) {
        let mut crashed = false;
        self.run_game_once(inst.clone(), profile.clone(), token.clone(), join_server.clone(), &mut crashed).await;
        if crashed {
            self.lan_agent_off.store(true, std::sync::atomic::Ordering::SeqCst);
            eprintln!("[Cubelink] LAN: l'agent a fait planter la JVM, relance sans agent");
            self.run_game_once(inst, profile, token, join_server, &mut false).await;
        }
    }

    async fn run_game_once(self: &Arc<Self>, inst: Instance, profile: Profile, token: Option<String>, join_server: Option<(String, u16)>, crashed: &mut bool) {
        let id = inst.id.clone();
        self.emit_state(&id, LaunchPhase::Preparing, None);
        let me = self.clone();
        let pid = id.clone();
        let progress: Progress = Arc::new(move |m| me.emit_state(&pid, LaunchPhase::Preparing, Some(m)));
        let java_setting = inst.java_path.clone().filter(|j| !j.trim().is_empty()).or_else(|| self.settings_get().java_path);
        let prepared = match ensure_game(&self.paths, &inst, java_setting, progress).await {
            Ok(p) => p,
            Err(e) => return self.emit_state(&id, LaunchPhase::Failed, Some(user_msg(&e))),
        };
        let game_dir = match self.instances.game_dir(&id) {
            Ok(d) => d,
            Err(e) => return self.emit_state(&id, LaunchPhase::Failed, Some(user_msg(&e))),
        };
        // Partage / agent LAN : seulement pour nos propres instances, jamais en rejoignant un ami.
        let own = join_server.is_none() && self.instances.kind(&id).map(|k| k == core_lib::instance::InstanceKind::Own).unwrap_or(false);
        let agent_arg = lan_agent_jar(self, own, join_server.as_ref()).and_then(|jar| {
            let java = core_lib::java::probe_with(&prepared.java, core_lib::java::run_java_version).map(|i| i.major);
            let has_srg = core_lib::lan_map::srg_table(&inst.minecraft).is_some();
            match agent_supported(java, core_lib::lan_map::jar_min_java(&jar), &inst.minecraft, inst.loader.loader, join_server.is_some(), has_srg) {
                Ok(()) => Some(lan_agent_arg(&jar, join_server.as_ref())),
                Err(why) => {
                    eprintln!("[Cubelink] LAN: agent non injecté ({why}) : {} {:?}, Java {java:?}", inst.minecraft, inst.loader.loader);
                    None
                }
            }
        });
        let agent_on = agent_arg.is_some();
        let lan_map = if agent_on && join_server.is_none() { lan_map_path(self, &inst, &prepared.version, &prepared.libs_dir).await } else { None };
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
            extra_jvm_args: {
                let mut a = core_lib::instance::split_jvm_args(&inst.jvm_args);
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
            Err(e) => return self.emit_state(&id, LaunchPhase::Failed, Some(user_msg(&e))),
        };
        drop(spec);
        // Essai : fenetre du jeu cachee des sa creation (jamais hors de la zone integree, voir `Stage::hide_start`).
        cmd.hide_first_window = self.stage.hide_start(crate::stage::hide_start_switch());
        let log_dir = self.paths.logs.join(&id);
        // Le launcher se ferme (fenetre fermee pendant la preparation) : on ne demarre pas un jeu que plus rien ne suivrait.
        // `pass` est garde jusqu'a ce que le jeu soit suivi : une fermeture qui arrive entre-temps attend (voir `QuitGate`).
        let Some(pass) = self.quit.enter_spawn() else {
            self.stage.note("fermeture du launcher pendant la preparation : le jeu n'est pas demarre");
            return self.emit_state(&id, LaunchPhase::Failed, Some("Cubelink est en train de se fermer".into()));
        };
        // Le jeu reste vivant (via `game_handle`) jusqu'à la fin de la boucle : le lâcher l'arrêterait.
        let mut game = match launch_run::spawn(&cmd, &log_dir).await {
            Ok(g) => g,
            Err(e) => return self.emit_state(&id, LaunchPhase::Failed, Some(user_msg(&e))),
        };
        if self.quit.quitting() {
            // La fermeture est arrivee pendant le demarrage du processus : il est arrete tout de suite, avant toute fenetre.
            self.stage.note("fermeture du launcher pendant le demarrage du jeu : processus arrete");
            let _ = game.kill().await;
            return self.emit_state(&id, LaunchPhase::Failed, Some("Cubelink est en train de se fermer".into()));
        }
        let Some(mut rx) = game.events() else {
            return self.emit_state(&id, LaunchPhase::Failed, Some("suivi du jeu indisponible".into()));
        };
        let game = Arc::new(game);
        // Invité : Dynamic FPS (et autres) bride le rendu hors premier plan et retarde le quickPlay (voir `foreground`).
        if join_server.is_some() {
            crate::foreground::bring_game_to_front(game.pid);
        }
        *lock(&self.game_handle) = Some((id.clone(), game.clone()));
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
    }
}

#[tauri::command(rename_all = "snake_case")]
pub fn instances_list(state: State<'_, Arc<AppState>>) -> Result<Vec<Instance>, String> {
    // Les modpacks reçus d'un ami (instances d'invité) restent hors de la liste : ils ne sont lancés que via « Rejoindre ».
    Ok(state.instances_list().into_iter().filter(|i| !core_lib::guest::is_guest_instance(&state.instances, &i.id)).collect())
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
pub async fn instances_import_mrpack(
    state: State<'_, Arc<AppState>>,
    path: String,
    name: Option<String>,
    minecraft: Option<String>,
    loader: Option<LoaderSpec>,
) -> Result<Instance, String> {
    state.instances_import_mrpack(&path, name, minecraft, loader).await
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
    #[cfg(windows)]
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
    #[cfg(windows)]
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
    #[cfg(windows)]
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
