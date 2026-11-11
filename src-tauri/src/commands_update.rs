//! Mise à jour automatique du launcher (tauri-plugin-updater, GitHub Releases).
//!
//! Au démarrage (après quelques secondes) puis toutes les 6 h, on cherche une version plus récente ; si elle existe,
//! elle est TÉLÉCHARGÉE en arrière-plan et l'état passe à `ready` : l'utilisateur n'a plus qu'à cliquer « Redémarrer
//! et installer ». Le bouton « Chercher une mise à jour » (`update_check`) fait la même chose à la demande.
//! L'installation est refusée tant qu'une partie est en cours. État publié par `update://state`, relu par `update_state`.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::state::{lock, AppState};

const FIRST_CHECK_DELAY: Duration = Duration::from_secs(10);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);
const PROGRESS_EVERY: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdatePhase {
    Idle,
    Checking,
    UpToDate,
    Available,
    Downloading,
    Ready,
    Error,
}

/// Contrat avec le front (`UpdateState` dans `src/services/types.ts`).
#[derive(Debug, Clone, Serialize)]
pub struct UpdateState {
    pub phase: UpdatePhase,
    pub current: String,
    pub version: Option<String>,
    pub notes: Option<String>,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub message: Option<String>,
}

impl UpdateState {
    fn new(current: String) -> Self {
        Self { phase: UpdatePhase::Idle, current, version: None, notes: None, downloaded: 0, total: None, message: None }
    }
}

/// État partagé, géré par Tauri (`app.manage`).
pub struct UpdateCtl {
    state: Mutex<UpdateState>,
    update: Mutex<Option<Update>>,
    bytes: Mutex<Option<Vec<u8>>>,
    busy: AtomicBool,
    /// Mode test : aucune requête réseau, l'état reste `idle` / `up_to_date`.
    disabled: bool,
}

impl UpdateCtl {
    pub fn new(current: String, disabled: bool) -> Self {
        Self { state: Mutex::new(UpdateState::new(current)), update: Mutex::new(None), bytes: Mutex::new(None), busy: AtomicBool::new(false), disabled }
    }

    fn snapshot(&self) -> UpdateState {
        lock(&self.state).clone()
    }

    fn set(&self, app: &AppHandle, f: impl FnOnce(&mut UpdateState)) -> UpdateState {
        let s = {
            let mut g = lock(&self.state);
            f(&mut g);
            g.clone()
        };
        let _ = app.emit("update://state", &s);
        s
    }
}

fn friendly(e: &dyn std::fmt::Display) -> String {
    let raw = e.to_string().to_lowercase();
    if raw.contains("signature") {
        "La signature de la mise à jour est invalide : elle n'a pas été installée.".into()
    } else {
        "Impossible de joindre le serveur de mises à jour. Vérifie ta connexion Internet.".into()
    }
}

/// Cherche une mise à jour ; si elle existe, lance son téléchargement en arrière-plan. `manual` : une erreur
/// réseau est alors montrée à l'utilisateur (sinon on reste silencieux).
async fn check(app: &AppHandle, ctl: &Arc<UpdateCtl>, manual: bool) -> UpdateState {
    if ctl.disabled {
        return ctl.set(app, |s| s.phase = UpdatePhase::UpToDate);
    }
    // Déjà en cours, ou mise à jour déjà prête : rien à refaire.
    if matches!(ctl.snapshot().phase, UpdatePhase::Checking | UpdatePhase::Downloading | UpdatePhase::Ready) {
        return ctl.snapshot();
    }
    if ctl.busy.swap(true, Ordering::SeqCst) {
        return ctl.snapshot();
    }
    ctl.set(app, |s| {
        s.phase = UpdatePhase::Checking;
        s.message = None;
    });
    let found = match app.updater() {
        Ok(u) => u.check().await.map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    match found {
        Ok(Some(update)) => {
            let (version, notes) = (update.version.clone(), update.body.clone());
            *lock(&ctl.update) = Some(update);
            let s = ctl.set(app, |s| {
                s.phase = UpdatePhase::Available;
                s.version = Some(version);
                s.notes = notes;
                s.downloaded = 0;
                s.total = None;
            });
            let (app2, ctl2) = (app.clone(), ctl.clone());
            tauri::async_runtime::spawn(async move {
                download(&app2, &ctl2).await;
                ctl2.busy.store(false, Ordering::SeqCst);
            });
            s
        }
        Ok(None) => {
            ctl.busy.store(false, Ordering::SeqCst);
            ctl.set(app, |s| s.phase = if manual { UpdatePhase::UpToDate } else { UpdatePhase::Idle })
        }
        Err(e) => {
            eprintln!("[Cubelink] mise à jour : vérification impossible : {e}");
            ctl.busy.store(false, Ordering::SeqCst);
            let msg = friendly(&e);
            ctl.set(app, |s| {
                if manual {
                    s.phase = UpdatePhase::Error;
                    s.message = Some(msg);
                } else {
                    s.phase = UpdatePhase::Idle;
                }
            })
        }
    }
}

async fn download(app: &AppHandle, ctl: &Arc<UpdateCtl>) {
    let Some(update) = lock(&ctl.update).take() else { return };
    ctl.set(app, |s| s.phase = UpdatePhase::Downloading);
    let mut done: u64 = 0;
    let mut last_emit = Instant::now();
    let (a2, c2) = (app.clone(), ctl.clone());
    let res = update
        .download(
            move |chunk, total| {
                done += chunk as u64;
                if last_emit.elapsed() >= PROGRESS_EVERY {
                    last_emit = Instant::now();
                    c2.set(&a2, |s| {
                        s.downloaded = done;
                        s.total = total;
                    });
                }
            },
            || {},
        )
        .await;
    match res {
        Ok(bytes) => {
            let n = bytes.len() as u64;
            *lock(&ctl.bytes) = Some(bytes);
            *lock(&ctl.update) = Some(update);
            ctl.set(app, |s| {
                s.phase = UpdatePhase::Ready;
                s.downloaded = n;
                s.total = Some(n);
            });
        }
        Err(e) => {
            eprintln!("[Cubelink] mise à jour : téléchargement impossible : {e}");
            let msg = friendly(&e);
            ctl.set(app, |s| {
                s.phase = UpdatePhase::Error;
                s.message = Some(msg);
            });
        }
    }
}

/// Lance la vérification périodique (à appeler une fois au démarrage, hors mode test).
pub fn start_background_checks(app: &AppHandle, ctl: Arc<UpdateCtl>) {
    if ctl.disabled {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK_DELAY).await;
        loop {
            check(&app, &ctl, false).await;
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

#[tauri::command(rename_all = "snake_case")]
pub fn update_state(ctl: State<'_, Arc<UpdateCtl>>) -> UpdateState {
    ctl.snapshot()
}

#[tauri::command(rename_all = "snake_case")]
pub async fn update_check(app: AppHandle, ctl: State<'_, Arc<UpdateCtl>>) -> Result<UpdateState, String> {
    Ok(check(&app, ctl.inner(), true).await)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn update_install(app: AppHandle, ctl: State<'_, Arc<UpdateCtl>>, st: State<'_, Arc<AppState>>) -> Result<(), String> {
    if lock(&st.game).is_some() {
        return Err("Une partie est en cours : ferme Minecraft avant d'installer la mise à jour.".into());
    }
    let (Some(update), Some(bytes)) = (lock(&ctl.update).take(), lock(&ctl.bytes).take()) else {
        return Err("Aucune mise à jour prête à installer.".into());
    };
    if let Err(e) = update.install(&bytes) {
        eprintln!("[Cubelink] mise à jour : installation impossible : {e}");
        // On garde la mise à jour pour pouvoir réessayer.
        *lock(&ctl.update) = Some(update);
        *lock(&ctl.bytes) = Some(bytes);
        return Err("L'installation de la mise à jour a échoué. Réessaie, ou télécharge la dernière version depuis GitHub.".into());
    }
    // Windows : l'installeur ferme déjà l'application ; ailleurs on relance sur la nouvelle version.
    app.restart();
}

/// Pour `lib.rs` : enregistre l'état et démarre les vérifications.
pub fn setup(app: &tauri::App, test_mode: bool) {
    // Version de l'application (tauri.conf.json, fixée par la CI à chaque release), celle que compare l'updater.
    // Installée par le Microsoft Store (paquet MSIX sous ...\WindowsApps\) : le Store gère les mises à jour, pas nous.
    let store = std::env::current_exe().map(|p| p.to_string_lossy().to_lowercase().contains("\\windowsapps\\")).unwrap_or(false);
    let ctl = Arc::new(UpdateCtl::new(app.package_info().version.to_string(), test_mode || store));
    app.manage(ctl.clone());
    start_background_checks(app.handle(), ctl);
}
