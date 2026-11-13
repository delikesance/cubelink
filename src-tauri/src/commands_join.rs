//! Commandes « rejoindre le monde d'un ami » : plan, transfert, pause, reprise, annulation.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use core_lib::guest;
use core_lib::types::{Error, Manifest, TransferPhase, TransferProgress};
use p2p_lib::transfer::plan::{compute_plan, to_join_summary, DiskCache, Plan};
use serde::Serialize;
use tauri::State;

use crate::error::{sanitize, user_msg};
use crate::p2p_backend::{HostManifest, ProgressFn, TransferControl};
use crate::state::{lock, AppState, JoinSession};

#[derive(Debug, Clone, Serialize)]
pub struct JoinPlan {
    pub manifest: Manifest,
    pub cached: usize,
    pub to_receive: usize,
    pub to_receive_bytes: u64,
    /// Mods que Modrinth ne reconnaît pas, calculés avant téléchargement sur les sha1 annoncés par l'hôte.
    pub unknown_mods: Vec<String>,
    /// Jars non vérifiés (sha1 absent, réseau, limite de débit) : ni reconnus ni signalés inconnus.
    #[serde(default)]
    pub unknown_unchecked: Vec<String>,
}

fn progress(phase: TransferPhase) -> TransferProgress {
    TransferProgress { phase, files_done: 0, files_total: 0, bytes_done: 0, bytes_total: 0, speed_bps: 0, via_relay: false, error: None }
}

/// Étape où une erreur survient (change le sens d'une erreur réseau).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Receive,
    Install,
}

fn disk_full(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::StorageFull || matches!(e.raw_os_error(), Some(112) | Some(28) | Some(39) | Some(122))
}

/// Message d'échec reconnaissable par l'interface (phase `failed`).
pub fn failure_message(e: &Error, stage: Stage) -> String {
    match e {
        Error::Io(io) if disk_full(io) => "disque plein : libérez de l'espace puis reprenez".into(),
        Error::Integrity { .. } => "fichier corrompu : le contenu reçu ne correspond pas à l'empreinte annoncée".into(),
        Error::Cancelled => "annulé".into(),
        Error::Network(_) if stage == Stage::Receive => "l'ami s'est déconnecté : réessayez quand il est en ligne".into(),
        Error::Auth(_) if stage == Stage::Receive => "l'ami a refusé le transfert".into(),
        Error::NotFound(m) | Error::Other(m) | Error::Invalid(m) if m.to_lowercase().contains("java") => {
            "Java introuvable : installez-le ou indiquez son chemin dans les paramètres".into()
        }
        other => user_msg(other),
    }
}

/// Date UTC `AAAA-MM-JJ hh:mm:ss` (sans dépendance), pour le journal des échecs.
fn utc_now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Algorithme de Howard Hinnant (jours depuis 1970 -> date civile).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Texte technique complet d'un échec d'installation (étape, détail, cause) : jamais montré tel quel dans l'interface
/// sans action de l'utilisateur (bloc « Détails »), toujours écrit dans `join-errors.log`.
pub fn technical_details(step: &str, detail: &str, e: &Error) -> String {
    format!("étape : {step}\ndétail : {detail}\nerreur : {e}\ndebug : {e:?}")
}

fn failed(msg: String, last: Option<TransferProgress>) -> TransferProgress {
    let base = last.unwrap_or_else(|| progress(TransferPhase::Failed));
    TransferProgress { phase: TransferPhase::Failed, error: Some(msg), speed_bps: 0, ..base }
}

impl AppState {
    /// Échec d'une étape de `run_join` : journalise TOUT (fichier + stderr), prévient l'interface (événement
    /// `transfer://error_details`) et renvoie le message court affichable (« Étape : cause » si la cause est générique).
    fn join_fail(&self, step: &str, detail: &str, e: &Error, stage: Stage) -> String {
        let base = failure_message(e, stage);
        let short = if base == user_msg(e) { format!("{step} : {base}") } else { base };
        let details = technical_details(step, detail, e);
        eprintln!("[Cubelink] join échoué ({step}) : {e:?} ({detail})");
        let log = self.paths.logs.join("join-errors.log");
        let _ = std::fs::create_dir_all(&self.paths.logs);
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log) {
            use std::io::Write;
            let _ = writeln!(f, "[{}] {}\nmessage affiché : {short}\n", utc_now(), details.replace('\n', "\n  "));
        }
        self.emit_event(
            "transfer://error_details",
            &serde_json::json!({ "step": step, "message": short, "details": details, "log_file": "join-errors.log" }),
        );
        short
    }

    pub(crate) fn check_friend(&self, key: &str) -> Result<(), String> {
        let known = key.len() == 64 && self.friends.is_friend(key).map_err(|e| user_msg(&e))?;
        if known {
            Ok(())
        } else {
            Err("cet ami est inconnu".into())
        }
    }

    async fn host_plan(&self, key: &str) -> Result<(HostManifest, Plan), String> {
        let hm = self.backend().fetch_manifest(key.to_string()).await.map_err(|e| sanitize(&e))?;
        let plan = compute_plan(&hm.manifest, &DiskCache { index: &self.store, root: &self.paths.store }).map_err(|e| user_msg(&e))?;
        Ok((hm, plan))
    }

    pub async fn join_plan(&self, friend_public_key: &str) -> Result<JoinPlan, String> {
        self.check_friend(friend_public_key)?;
        let (hm, plan) = self.host_plan(friend_public_key).await?;
        let (cached, to_receive, to_receive_bytes) = to_join_summary(&plan);
        // Revendication de l'hôte (sha1 du manifeste), re-vérifiée à la réception puis localement
        // (`transfer://unknown_mods`, voir `run_join`). Échec réseau : liste vide + `unknown_unchecked`.
        let report = self.backend().find_unknown_from_manifest(hm.manifest.files.clone()).await;
        Ok(JoinPlan {
            manifest: hm.manifest,
            cached,
            to_receive,
            to_receive_bytes,
            unknown_mods: report.unknown,
            unknown_unchecked: report.unchecked,
        })
    }

    /// `ram_mb` : RAM de l'instance invitée (`None` ou 0 : « Par défaut », valeur globale).
    pub async fn join_start(self: &Arc<Self>, friend_public_key: &str, ram_mb: Option<u32>) -> Result<(), String> {
        self.check_friend(friend_public_key)?;
        let ram_mb = ram_mb.unwrap_or(core_lib::instance::RAM_DEFAULT);
        if ram_mb != core_lib::instance::RAM_DEFAULT && !(core_lib::instance::MIN_RAM_MB..=core_lib::instance::MAX_RAM_MB).contains(&ram_mb) {
            return Err("mémoire invalide".into());
        }
        if lock(&self.join).is_some() {
            return Err("un transfert est déjà en cours".into());
        }
        let (hm, plan) = self.host_plan(friend_public_key).await?;
        let control = Arc::new(TransferControl::default());
        let last = Arc::new(Mutex::new(None));
        {
            let mut j = lock(&self.join);
            if j.is_some() {
                return Err("un transfert est déjà en cours".into());
            }
            *j = Some(JoinSession { control: control.clone(), last: last.clone() });
        }
        *lock(&self.last_join_instance) = None;
        let me = self.clone();
        let key = friend_public_key.to_string();
        let (l2, me2) = (last.clone(), self.clone());
        // Les phases terminales (Done/Failed) de la réception ne sont pas les nôtres : l'installation suit.
        let cb: ProgressFn = Arc::new(move |mut p| {
            match p.phase {
                TransferPhase::Failed => return,
                TransferPhase::Done => p.phase = TransferPhase::Verifying,
                _ => {}
            }
            *lock(&l2) = Some(p.clone());
            me2.emit_event("transfer://progress", &p);
        });
        tokio::spawn(async move {
            me.emit_event("transfer://progress", &progress(TransferPhase::Preparing));
            let outcome = me.run_join(&key, hm, plan, cb, control, &last).await;
            // RAM choisie avant de rejoindre (aussi pour une instance invitée réutilisée : même ami, même pack).
            if outcome.is_ok() {
                if let Some(id) = lock(&me.last_join_instance).clone() {
                    let _ = me.instances.update_ram(&id, ram_mb);
                }
            }
            let last_p = lock(&last).clone();
            match outcome {
                Ok(()) => {
                    let base = last_p.unwrap_or_else(|| progress(TransferPhase::Done));
                    me.emit_event("transfer://progress", &TransferProgress { phase: TransferPhase::Done, speed_bps: 0, error: None, ..base });
                }
                Err(msg) => me.emit_event("transfer://progress", &failed(msg, last_p)),
            }
            *lock(&me.join) = None;
        });
        Ok(())
    }

    /// Réception, instance invitée, installation du loader/Java. Erreur = message d'échec affichable.
    async fn run_join(
        &self,
        key: &str,
        hm: HostManifest,
        plan: Plan,
        cb: ProgressFn,
        control: Arc<TransferControl>,
        last: &Arc<Mutex<Option<TransferProgress>>>,
    ) -> Result<(), String> {
        let manifest = hm.manifest;
        let mh = core_lib::manifest::manifest_hash(&manifest);
        let id = format!("{}-{}", &key[..16], &mh[..16]);
        let res = self.backend().receive_files(key.to_string(), id.clone(), plan, cb.clone(), control.clone()).await;
        if control.is_cancelled() {
            return Err(failure_message(&Error::Cancelled, Stage::Receive));
        }
        res.map_err(|e| self.join_fail("Réception des fichiers", &format!("{} fichier(s) du plan", manifest.files.len()), &e, Stage::Receive))?;
        // Filet : un objet que l'index croit présent mais absent du disque est purgé puis re-reçu UNE fois.
        let missing = compute_plan(&manifest, &DiskCache { index: &self.store, root: &self.paths.store })
            .map_err(|e| self.join_fail("Contrôle du cache", "plan après réception", &e, Stage::Install))?;
        if missing.to_receive_count > 0 {
            eprintln!("[Cubelink] join : {} objet(s) manquant(s) dans le cache après réception, nouvelle réception", missing.to_receive_count);
            let again = self.backend().receive_files(key.to_string(), format!("{id}-r"), missing, cb, control.clone()).await;
            if control.is_cancelled() {
                return Err(failure_message(&Error::Cancelled, Stage::Receive));
            }
            again.map_err(|e| self.join_fail("Réception des fichiers (reprise)", "objets manquants du cache", &e, Stage::Receive))?;
        }

        let phase = |ph: TransferPhase| {
            let base = lock(last).clone().unwrap_or_else(|| progress(ph));
            self.emit_event("transfer://progress", &TransferProgress { phase: ph, ..base });
        };
        phase(TransferPhase::Verifying);
        let host_label = self.friends.get_friend(key).ok().flatten().map(|f| f.name).unwrap_or_else(|| "ami".into());
        let inst = guest::create_guest_instance(&self.instances, &manifest, &host_label, &self.paths.store, &self.store)
            .map_err(|e| self.join_fail("Création de l'instance invitée", &format!("monde {:?}, minecraft {}", manifest.name, manifest.minecraft), &e, Stage::Install))?;
        if let Err(e) = guest::install_manifest_files(&self.instances, &inst, &manifest, &self.paths.store, &self.store) {
            let _ = guest::remove_guest_instance(&self.instances, &inst.id, &self.store);
            return Err(self.join_fail("Installation des fichiers", &format!("instance {}, {} fichier(s) du manifeste", inst.id, manifest.files.len()), &e, Stage::Install));
        }
        // Avertissement fiable : sha1 locaux des jars reçus, côté invité.
        if let Ok(gd) = self.instances.game_dir(&inst.id) {
            let unknown = self.backend().find_unknown_mods(manifest.files.clone(), gd).await;
            self.emit_event("transfer://unknown_mods", &serde_json::json!({ "instance_id": inst.id, "unknown_mods": unknown }));
        }
        if !self.skip_game_install.load(Ordering::SeqCst) {
            phase(TransferPhase::Launching);
            let java = lock(&self.settings).java_path.clone();
            let prog: crate::install::Progress = Arc::new(|_| {});
            if let Err(e) = crate::install::ensure_game(&self.paths, &inst, java, prog).await {
                let _ = guest::remove_guest_instance(&self.instances, &inst.id, &self.store);
                return Err(self.join_fail("Installation du jeu (loader, Java)", &format!("instance {}, minecraft {}", inst.id, inst.minecraft), &e, Stage::Install));
            }
        }
        *lock(&self.last_join_instance) = Some(inst.id.clone());
        Ok(())
    }

    /// Invité : ouvre le tunnel vers le monde de l'ami et renvoie le port local à donner à Minecraft
    /// (« Connexion directe » 127.0.0.1:port). Un seul tunnel à la fois : le précédent est fermé.
    pub async fn join_open_tunnel(&self, friend_public_key: &str) -> Result<u16, String> {
        self.check_friend(friend_public_key)?;
        let port = self.backend().join_open_tunnel(friend_public_key.to_string()).await.map_err(|e| sanitize(&e))?;
        crate::test_hooks::set_guest_tunnel_port(port);
        Ok(port)
    }

    pub async fn join_close_tunnel(&self) {
        self.backend().join_close_tunnel().await;
        crate::test_hooks::set_guest_tunnel_port(0);
    }

    /// Invité : « Rejoindre et jouer ». Ouvre le tunnel vers le monde de l'ami puis lance l'instance invitée avec
    /// connexion automatique à `127.0.0.1:<port du tunnel>`. Le tunnel est refermé à la fin du jeu (ou si le lancement échoue).
    pub async fn join_play(self: &Arc<Self>, friend_public_key: &str, instance_id: &str) -> Result<(), String> {
        self.check_friend(friend_public_key)?;
        if !crate::commands_instances::valid_id(instance_id) {
            return Err("identifiant d'instance invalide".into());
        }
        self.instances.get(instance_id).map_err(|e| user_msg(&e))?;
        if lock(&self.game).is_some() {
            return Err("une partie est déjà en cours de préparation ou de jeu".into());
        }
        let port = self.join_open_tunnel(friend_public_key).await?;
        if let Err(e) = self.launch_with(instance_id, Some(("127.0.0.1".to_string(), port))).await {
            self.join_close_tunnel().await;
            return Err(e);
        }
        Ok(())
    }

    fn with_session<R>(&self, f: impl FnOnce(&JoinSession) -> R) -> Result<R, String> {
        lock(&self.join).as_ref().map(f).ok_or_else(|| "aucun transfert en cours".to_string())
    }

    pub fn join_pause(&self) -> Result<(), String> {
        let last = self.with_session(|s| {
            s.control.pause();
            lock(&s.last).clone()
        })?;
        let p = TransferProgress { phase: TransferPhase::Paused, ..last.unwrap_or_else(|| progress(TransferPhase::Paused)) };
        self.emit_event("transfer://progress", &p);
        Ok(())
    }

    pub fn join_resume(&self) -> Result<(), String> {
        let last = self.with_session(|s| {
            s.control.resume();
            lock(&s.last).clone()
        })?;
        let p = TransferProgress { phase: TransferPhase::Receiving, ..last.unwrap_or_else(|| progress(TransferPhase::Receiving)) };
        self.emit_event("transfer://progress", &p);
        Ok(())
    }

    pub fn join_cancel(&self) -> Result<(), String> {
        self.with_session(|s| s.control.cancel())
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn join_plan(state: State<'_, Arc<AppState>>, friend_public_key: String) -> Result<JoinPlan, String> {
    state.join_plan(&friend_public_key).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn join_start(state: State<'_, Arc<AppState>>, friend_public_key: String, ram_mb: Option<u32>) -> Result<(), String> {
    state.inner().join_start(&friend_public_key, ram_mb).await
}

#[tauri::command(rename_all = "snake_case")]
pub fn join_pause(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    state.join_pause()
}

#[tauri::command(rename_all = "snake_case")]
pub fn join_resume(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    state.join_resume()
}

#[tauri::command(rename_all = "snake_case")]
pub fn join_cancel(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    state.join_cancel()
}

#[tauri::command(rename_all = "snake_case")]
pub async fn join_open_tunnel(state: State<'_, Arc<AppState>>, friend_public_key: String) -> Result<u16, String> {
    state.join_open_tunnel(&friend_public_key).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn join_play(state: State<'_, Arc<AppState>>, friend_public_key: String, instance_id: String) -> Result<(), String> {
    // Vide = l'instance invitée que le dernier transfert vient de préparer (elle n'apparaît pas dans `instances_list`).
    let id = if instance_id.is_empty() {
        lock(&state.last_join_instance).clone().ok_or("la préparation du monde de l'ami a échoué")?
    } else {
        instance_id
    };
    state.inner().join_play(&friend_public_key, &id).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn join_close_tunnel(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    state.join_close_tunnel().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{HostManifest, ProgressFn, TransferControl};
    use crate::state::test_support::test_state;
    use core_lib::types::{Friend, TrustLevel};
    use std::sync::Arc;

    #[tokio::test]
    async fn ami_inconnu_et_backend_indisponible() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        assert!(st.join_plan("zz").await.unwrap_err().contains("inconnu"));
        let key = "a".repeat(64);
        st.friends
            .add_friend(&Friend { public_key: key.clone(), name: "Bob".into(), trust: TrustLevel::Ask, added_at: 1 })
            .unwrap();
        assert!(st.join_plan(&key).await.unwrap_err().contains("pas encore disponible"));
        assert!(st.join_start(&key, None).await.is_err());
        assert!(st.join_pause().unwrap_err().contains("aucun transfert"));
        assert!(st.join_cancel().is_err());
    }

    struct TunnelBackend;
    impl crate::p2p_backend::P2pBackend for TunnelBackend {
        fn presence(&self, _k: &str) -> core_lib::types::Presence {
            core_lib::types::Presence::Offline
        }
        fn send_request(
            &self,
            _t: p2p_lib::friend_code::FriendCodePayload,
            _n: String,
        ) -> crate::p2p_backend::BoxFuture<'_, Result<core_lib::types::VerifyCode, String>> {
            Box::pin(async { Err("non".into()) })
        }
        fn notify_accepted(&self, _f: Friend) -> crate::p2p_backend::BoxFuture<'_, Result<(), String>> {
            Box::pin(async { Ok(()) })
        }
        fn fetch_manifest(&self, _k: String) -> crate::p2p_backend::BoxFuture<'_, Result<HostManifest, String>> {
            Box::pin(async { Err("non".into()) })
        }
        fn receive_files(
            &self,
            _k: String,
            _i: String,
            _p: p2p_lib::transfer::plan::Plan,
            _pr: ProgressFn,
            _c: Arc<TransferControl>,
        ) -> crate::p2p_backend::BoxFuture<'_, Result<(), core_lib::types::Error>> {
            Box::pin(async { Ok(()) })
        }
        fn join_open_tunnel(&self, _k: String) -> crate::p2p_backend::BoxFuture<'_, Result<u16, String>> {
            Box::pin(async { Ok(40001) })
        }
    }

    #[tokio::test]
    async fn tunnel_invite_port_distinct_du_port_hote() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let key = "b".repeat(64);
        // Ami inconnu et réseau indisponible : refus clairs.
        assert!(st.join_open_tunnel(&key).await.unwrap_err().contains("inconnu"));
        st.friends
            .add_friend(&Friend { public_key: key.clone(), name: "Bob".into(), trust: TrustLevel::Ask, added_at: 1 })
            .unwrap();
        assert!(st.join_open_tunnel(&key).await.unwrap_err().contains("pas encore disponible"));
        st.set_backend(Arc::new(TunnelBackend));
        let host_before = st.test_state_info().tunnel_port;
        assert_eq!(st.join_open_tunnel(&key).await.unwrap(), 40001);
        let info = st.test_state_info();
        assert_eq!(info.guest_tunnel_port, 40001);
        assert_eq!(info.tunnel_port, host_before, "le port du monde hôte n'est pas touché");
        st.join_close_tunnel().await;
        assert_eq!(st.test_state_info().guest_tunnel_port, 0);
    }

    #[tokio::test]
    async fn join_play_refus_avant_tunnel_et_fermeture_si_lancement_echoue() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let key = "c".repeat(64);
        assert!(st.join_play("zz", "x").await.unwrap_err().contains("inconnu"));
        st.friends
            .add_friend(&Friend { public_key: key.clone(), name: "Bob".into(), trust: TrustLevel::Ask, added_at: 1 })
            .unwrap();
        assert!(st.join_play(&key, "../x").await.unwrap_err().contains("invalide"));
        assert!(st.join_play(&key, "inconnue").await.is_err());
        st.set_backend(Arc::new(TunnelBackend));
        let inst = st
            .instances
            .create("Inv", "1.21.1", core_lib::types::LoaderSpec { loader: core_lib::types::Loader::Vanilla, version: String::new() }, 2048)
            .unwrap();
        // Pas de profil connecté : le lancement est refusé, le tunnel ouvert est refermé.
        assert!(st.join_play(&key, &inst.id).await.unwrap_err().contains("connectez-vous"));
        assert_eq!(st.test_state_info().guest_tunnel_port, 0);
    }

    #[test]
    fn messages_d_echec_reconnaissables() {
        use super::{failure_message, technical_details, utc_now, Stage};
        let d = technical_details("Installation des fichiers", "mods/x.jar", &Error::NotFound("mods/x.jar absent du cache".into()));
        assert!(d.contains("étape : Installation des fichiers") && d.contains("mods/x.jar") && d.contains("NotFound"), "{d}");
        let t = utc_now();
        assert!(t.len() == 23 && t.ends_with("UTC") && t.starts_with("20"), "{t}");
        use core_lib::types::Error;
        let full = Error::Io(std::io::Error::from(std::io::ErrorKind::StorageFull));
        assert!(failure_message(&full, Stage::Receive).contains("disque plein"));
        assert!(failure_message(&Error::Io(std::io::Error::from_raw_os_error(112)), Stage::Receive).contains("disque plein"));
        let bad = Error::Integrity { expected: "a".into(), actual: "b".into() };
        assert!(failure_message(&bad, Stage::Receive).contains("fichier corrompu"));
        assert!(failure_message(&Error::Network("x".into()), Stage::Receive).contains("l'ami s'est déconnecté"));
        assert!(!failure_message(&Error::Network("x".into()), Stage::Install).contains("l'ami"));
        assert!(failure_message(&Error::NotFound("Java 21 introuvable".into()), Stage::Install).contains("Java introuvable"));
    }
}
