//! Mise à jour automatique du launcher (tauri-plugin-updater, GitHub Releases).
//!
//! Au démarrage (après quelques secondes) puis toutes les 6 h, on cherche une version plus récente ; si elle existe,
//! elle est TÉLÉCHARGÉE en arrière-plan et l'état passe à `ready` : l'utilisateur n'a plus qu'à cliquer « Redémarrer
//! et installer ». Le bouton « Chercher une mise à jour » (`update_check`) fait la même chose à la demande.
//! Le paquet téléchargé est écrit dans le dossier de cache (pas gardé en RAM : l'app reste des jours dans la zone de
//! notification) et relu à l'installation, où `Update::install` revérifie sa signature. En phase `ready`, les contrôles
//! continuent : une version plus récente remplace le paquet prêt, un contrôle sans résultat le laisse intact.
//! L'installation est refusée tant qu'une partie est en cours. État publié par `update://state`, relu par `update_state`.
use std::path::{Path, PathBuf};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum UpdatePhase {
    Idle,
    Checking,
    UpToDate,
    Available,
    Downloading,
    Ready,
    /// Paquet géré ailleurs (Nix, dossier en lecture seule) : version vérifiée, mais ni téléchargée ni installée.
    AvailableManual,
    Error,
    /// Installé via le Microsoft Store : c'est lui qui livre les mises à jour, Cubelink ne vérifie rien lui-même.
    ManagedByStore,
}

/// Page de la dernière release (bouton « Voir la release » côté front).
const RELEASE_URL: &str = "https://github.com/delikesance/cubelink/releases/latest";

/// Où et comment le launcher est installé : décide si l'updater peut remplacer le binaire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallKind {
    /// Windows (MSI) ou autre : comportement historique de l'updater.
    Standard,
    /// Paquet MSIX du Microsoft Store : le Store gère les mises à jour.
    Store,
    /// AppImage inscriptible : l'updater remplace `$APPIMAGE`.
    AppImage,
    /// Linux géré ailleurs : on vérifie la version mais on n'installe pas.
    Packaged(Packager),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Packager {
    Nix,
    Autre,
}

/// Pure : `linux` (cible), `appimage` (variable `APPIMAGE`), chemin de l'exécutable, `writable` (l'AppImage peut être remplacée).
pub fn install_kind(linux: bool, appimage: Option<&str>, exe_path: &str, writable: bool) -> InstallKind {
    if exe_path.to_lowercase().contains("\\windowsapps\\") {
        return InstallKind::Store;
    }
    if !linux {
        return InstallKind::Standard;
    }
    if exe_path.starts_with("/nix/store/") || appimage.is_some_and(|a| a.starts_with("/nix/store/")) {
        return InstallKind::Packaged(Packager::Nix);
    }
    match appimage {
        Some(a) if !a.is_empty() && writable => InstallKind::AppImage,
        _ => InstallKind::Packaged(Packager::Autre),
    }
}

/// Message affiché pour un paquet qu'on ne met pas à jour nous-mêmes.
fn packaged_message(p: Packager) -> String {
    match p {
        Packager::Nix => "Cubelink est installé via Nix : mets à jour ton paquet ou ton flake pour passer à cette version.".into(),
        // Le .rpm n'est qu'un asset de la release GitHub (aucun dépôt dnf) : `dnf upgrade` ne le trouverait pas.
        Packager::Autre => "Cubelink n'est pas une AppImage modifiable : télécharge la nouvelle version (le .rpm ou l'AppImage) depuis la page de la release.".into(),
    }
}

/// Message quand `update.install` échoue (sous Linux, lien explicite vers la release).
fn install_failed_message(linux: bool) -> String {
    if linux {
        format!("L'installation a échoué (l'AppImage est peut-être en lecture seule). Télécharge la nouvelle version : {RELEASE_URL}")
    } else {
        "L'installation de la mise à jour a échoué. Réessaie, ou télécharge la dernière version depuis GitHub.".into()
    }
}

/// `$APPIMAGE` et son dossier sont-ils modifiables (l'updater écrit un fichier voisin puis renomme) ?
fn appimage_writable(appimage: &str) -> bool {
    let p = std::path::Path::new(appimage);
    let dir_ok = p.parent().and_then(|d| std::fs::metadata(d).ok()).is_some_and(|m| !m.permissions().readonly());
    dir_ok && std::fs::OpenOptions::new().append(true).open(p).is_ok()
}

/// Nom (sans dossier) du paquet en cache pour `version` : seuls les caractères sûrs sont gardés.
fn package_name(version: &str, ext: &str) -> String {
    let v: String = version.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+') { c } else { '_' }).collect();
    format!("cubelink-{v}.{ext}")
}

/// Pure (E/S disque) : écrit `bytes` dans `dir/cubelink-<version>.pkg` (fichier `.part` puis renommage, donc jamais de
/// paquet tronqué). Crée le dossier au besoin.
fn store_package(dir: &Path, version: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let (part, pkg) = (dir.join(package_name(version, "part")), dir.join(package_name(version, "pkg")));
    let res = std::fs::write(&part, bytes).and_then(|()| std::fs::rename(&part, &pkg));
    if res.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    res.map(|()| pkg)
}

/// Supprime les paquets (`cubelink-*.pkg` / `.part`) de `dir`, sauf `keep`. Les autres fichiers ne sont pas touchés.
fn purge_cache(dir: &Path, keep: Option<&Path>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let path = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        let ours = name.starts_with("cubelink-") && (name.ends_with(".pkg") || name.ends_with(".part"));
        if ours && keep != Some(path.as_path()) {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Pure : en phase `ready` (version `ready`), faut-il remplacer le paquet par la version trouvée `found` ?
/// Rien trouvé ou même version : non. Version différente (le manifeste ne propose que du plus récent) : oui.
fn needs_redownload(ready: Option<&str>, found: Option<&str>) -> bool {
    match found {
        None => false,
        Some(f) => ready != Some(f),
    }
}

/// Contrat avec le front (`UpdateState` dans `src/services/types.ts`).
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct UpdateState {
    pub phase: UpdatePhase,
    pub current: String,
    pub version: Option<String>,
    pub notes: Option<String>,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub message: Option<String>,
    /// Page de la release (phase `available_manual`).
    pub url: Option<String>,
    /// Date (secondes Unix, UTC) du dernier contrôle terminé avec un verdict ; `None` tant qu'aucun n'a abouti.
    pub last_checked: Option<i64>,
}

impl UpdateState {
    fn new(current: String, phase: UpdatePhase) -> Self {
        Self { phase, current, version: None, notes: None, downloaded: 0, total: None, message: None, url: None, last_checked: None }
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

/// Pure : verdict de `check` quand les contrôles sont désactivés. Le mode test garde `up_to_date` (E2E sans réseau) ;
/// sous le Store, Cubelink ne sait pas si le Store a livré la dernière version : il ne dit pas « à jour ».
fn disabled_phase(store: bool) -> UpdatePhase {
    if store {
        UpdatePhase::ManagedByStore
    } else {
        UpdatePhase::UpToDate
    }
}

/// Pure : un contrôle terminé sans nouvelle version (manuel ou de fond) : « à jour », daté.
fn after_no_update(s: &mut UpdateState, now: i64) {
    s.phase = UpdatePhase::UpToDate;
    s.message = None;
    s.last_checked = Some(now);
}

/// Pure : un contrôle en échec. Manuel : l'erreur est montrée. De fond : on revient à l'état d'avant (`prev`), sans
/// effacer un « à jour » ni une version trouvée, et sans prétendre qu'aucun contrôle n'a eu lieu.
fn after_failure(s: &mut UpdateState, prev: &UpdateState, manual: bool, msg: String) {
    if manual {
        s.phase = UpdatePhase::Error;
        s.message = Some(msg);
    } else {
        s.phase = prev.phase;
        s.message = prev.message.clone();
    }
}

/// État partagé, géré par Tauri (`app.manage`).
pub struct UpdateCtl {
    state: Mutex<UpdateState>,
    update: Mutex<Option<Update>>,
    /// Paquet téléchargé, sur disque (phase `ready`).
    pkg: Mutex<Option<PathBuf>>,
    cache_dir: PathBuf,
    busy: AtomicBool,
    /// Aucune requête réseau : mode test (l'état reste `idle` / `up_to_date`) ou Store.
    disabled: bool,
    /// Installé via le Microsoft Store (implique `disabled`) : phase `managed_by_store`.
    store: bool,
    /// Paquet géré ailleurs : on vérifie, on ne télécharge pas.
    packaged: Option<Packager>,
}

impl UpdateCtl {
    pub fn new(current: String, disabled: bool, store: bool, packaged: Option<Packager>, cache_dir: PathBuf) -> Self {
        let phase = if store { UpdatePhase::ManagedByStore } else { UpdatePhase::Idle };
        Self { state: Mutex::new(UpdateState::new(current, phase)), update: Mutex::new(None), pkg: Mutex::new(None), cache_dir, busy: AtomicBool::new(false), disabled: disabled || store, store, packaged }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailureKind {
    Signature,
    /// Le serveur répond, mais sans version utilisable (plateforme absente de `latest.json`, 404, JSON invalide).
    NoRelease,
    Network,
    Unknown,
}

/// Pure : famille d'une erreur de tauri-plugin-updater / reqwest, d'après son texte (fragile : repli neutre).
fn classify(raw: &str) -> FailureKind {
    let raw = raw.to_lowercase();
    let has = |pats: &[&str]| pats.iter().any(|p| raw.contains(p));
    if raw.contains("signature") {
        FailureKind::Signature
    } else if has(&["release json", "was not found in the response", "fallback platforms", "unsupported os", "unsupported application architecture", "404", "expected value at line", "invalid type", "missing field", "eof while parsing"]) {
        FailureKind::NoRelease
    } else if has(&["error sending request", "dns", "connect", "timed out", "timeout", "network is unreachable"]) {
        FailureKind::Network
    } else {
        FailureKind::Unknown
    }
}

fn friendly(e: &dyn std::fmt::Display) -> String {
    match classify(&e.to_string()) {
        FailureKind::Signature => "La signature de la mise à jour est invalide : elle n'a pas été installée.".into(),
        FailureKind::NoRelease => "Le serveur de mises à jour n'a pas de version pour ton système. Réessaie plus tard, ou télécharge la dernière version depuis GitHub.".into(),
        FailureKind::Network => "Impossible de joindre le serveur de mises à jour. Vérifie ta connexion Internet.".into(),
        FailureKind::Unknown => "La vérification de la mise à jour a échoué. Réessaie dans un instant.".into(),
    }
}

/// Cherche une mise à jour ; si elle existe, lance son téléchargement en arrière-plan. `manual` : une erreur
/// réseau est alors montrée à l'utilisateur (sinon on reste silencieux).
async fn check(app: &AppHandle, ctl: &Arc<UpdateCtl>, manual: bool) -> UpdateState {
    if ctl.disabled {
        return ctl.set(app, |s| s.phase = disabled_phase(ctl.store));
    }
    // Déjà en cours : rien à refaire. En phase `ready` on revérifie (une version plus récente peut être sortie).
    if matches!(ctl.snapshot().phase, UpdatePhase::Checking | UpdatePhase::Downloading) {
        return ctl.snapshot();
    }
    if ctl.busy.swap(true, Ordering::SeqCst) {
        return ctl.snapshot();
    }
    // En `ready` on ne touche pas à la phase pendant la requête (sinon le bouton « Redémarrer et installer » clignote)
    // et un contrôle sans résultat ou en échec laisse la mise à jour prête intacte.
    let prev = ctl.snapshot();
    let was_ready = prev.phase == UpdatePhase::Ready;
    if !was_ready {
        ctl.set(app, |s| {
            s.phase = UpdatePhase::Checking;
            s.message = None;
        });
    }
    let found = match app.updater() {
        Ok(u) => u.check().await.map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    match found {
        Ok(Some(update)) => {
            let (version, notes) = (update.version.clone(), update.body.clone());
            if was_ready {
                // L'utilisateur a pu installer ou relancer entre-temps : on ne remplace que si c'est toujours `ready`.
                let snap = ctl.snapshot();
                if snap.phase != UpdatePhase::Ready || !needs_redownload(snap.version.as_deref(), Some(&version)) {
                    ctl.busy.store(false, Ordering::SeqCst);
                    return if snap.phase == UpdatePhase::Ready { ctl.set(app, |s| s.last_checked = Some(now_secs())) } else { snap };
                }
                if let Some(old) = lock(&ctl.pkg).take() {
                    let _ = std::fs::remove_file(old);
                }
            }
            if let Some(p) = ctl.packaged {
                ctl.busy.store(false, Ordering::SeqCst);
                return ctl.set(app, |s| {
                    s.phase = UpdatePhase::AvailableManual;
                    s.version = Some(version);
                    s.notes = notes;
                    s.message = Some(packaged_message(p));
                    s.url = Some(RELEASE_URL.into());
                    s.last_checked = Some(now_secs());
                });
            }
            *lock(&ctl.update) = Some(update);
            let s = ctl.set(app, |s| {
                s.phase = UpdatePhase::Available;
                s.version = Some(version);
                s.notes = notes;
                s.downloaded = 0;
                s.total = None;
                s.last_checked = Some(now_secs());
            });
            let (app2, ctl2) = (app.clone(), ctl.clone());
            tauri::async_runtime::spawn(async move {
                download(&app2, &ctl2).await;
                ctl2.busy.store(false, Ordering::SeqCst);
            });
            s
        }
        Ok(None) if was_ready => {
            ctl.busy.store(false, Ordering::SeqCst);
            // La mise à jour prête reste intacte (phase inchangée) ; seule la date du contrôle avance.
            ctl.set(app, |s| s.last_checked = Some(now_secs()))
        }
        Ok(None) => {
            ctl.busy.store(false, Ordering::SeqCst);
            ctl.set(app, |s| after_no_update(s, now_secs()))
        }
        Err(e) => {
            eprintln!("[Cubelink] mise à jour : vérification impossible : {e}");
            ctl.busy.store(false, Ordering::SeqCst);
            if was_ready {
                return ctl.snapshot();
            }
            let msg = friendly(&e);
            ctl.set(app, |s| after_failure(s, &prev, manual, msg))
        }
    }
}

async fn download(app: &AppHandle, ctl: &Arc<UpdateCtl>) {
    let Some(update) = lock(&ctl.update).take() else { return };
    // Anciens paquets orphelins (version remplacée, plantage en cours d'écriture) : on repart d'un cache vide.
    purge_cache(&ctl.cache_dir, None);
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
            // Les octets partent sur disque puis sont libérés : rien ne reste en RAM pendant que le paquet attend.
            let (dir, version) = (ctl.cache_dir.clone(), update.version.clone());
            let stored = tauri::async_runtime::spawn_blocking(move || store_package(&dir, &version, &bytes)).await;
            match stored {
                Ok(Ok(path)) => {
                    *lock(&ctl.pkg) = Some(path);
                    *lock(&ctl.update) = Some(update);
                    ctl.set(app, |s| {
                        s.phase = UpdatePhase::Ready;
                        s.downloaded = n;
                        s.total = Some(n);
                    });
                }
                other => {
                    let why = match other {
                        Ok(Err(e)) => e.to_string(),
                        Err(e) => e.to_string(),
                        Ok(Ok(_)) => unreachable!(),
                    };
                    eprintln!("[Cubelink] mise à jour : écriture du paquet impossible : {why}");
                    purge_cache(&ctl.cache_dir, None);
                    ctl.set(app, |s| {
                        s.phase = UpdatePhase::Error;
                        s.message = Some("Impossible d'enregistrer la mise à jour sur le disque (espace insuffisant ?).".into());
                    });
                }
            }
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
        purge_cache(&ctl.cache_dir, None);
        tokio::time::sleep(FIRST_CHECK_DELAY).await;
        loop {
            check(&app, &ctl, false).await;
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

/// État de la mise à jour automatique du launcher.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub fn update_state(ctl: State<'_, Arc<UpdateCtl>>) -> UpdateState {
    ctl.snapshot()
}

/// Vérifie s'il existe une mise à jour ; si oui, le téléchargement démarre en arrière-plan.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn update_check(app: AppHandle, ctl: State<'_, Arc<UpdateCtl>>) -> Result<UpdateState, String> {
    Ok(check(&app, ctl.inner(), true).await)
}

/// Redémarre et installe la mise à jour téléchargée (refusé si une partie est en cours).
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn update_install(app: AppHandle, ctl: State<'_, Arc<UpdateCtl>>, st: State<'_, Arc<AppState>>) -> Result<(), String> {
    if lock(&st.game).is_some() {
        return Err("Une partie est en cours : ferme Minecraft avant d'installer la mise à jour.".into());
    }
    let (Some(update), Some(path)) = (lock(&ctl.update).take(), lock(&ctl.pkg).take()) else {
        return Err("Aucune mise à jour prête à installer.".into());
    };
    let read_path = path.clone();
    let bytes = match tauri::async_runtime::spawn_blocking(move || std::fs::read(read_path)).await {
        Ok(Ok(b)) => b,
        other => {
            eprintln!("[Cubelink] mise à jour : paquet en cache illisible : {other:?}");
            // Le cache a été vidé (nettoyeur de disque…) : on repart de zéro et on relance une recherche.
            ctl.set(&app, |s| {
                s.phase = UpdatePhase::Idle;
                s.version = None;
                s.notes = None;
                s.downloaded = 0;
                s.total = None;
                s.message = None;
            });
            let (app2, ctl2) = (app.clone(), ctl.inner().clone());
            tauri::async_runtime::spawn(async move {
                check(&app2, &ctl2, false).await;
            });
            return Err("Le paquet téléchargé a disparu : Cubelink le télécharge de nouveau.".into());
        }
    };
    if let Err(e) = update.install(&bytes) {
        eprintln!("[Cubelink] mise à jour : installation impossible : {e}");
        // On garde la mise à jour pour pouvoir réessayer.
        *lock(&ctl.update) = Some(update);
        *lock(&ctl.pkg) = Some(path);
        return Err(install_failed_message(cfg!(target_os = "linux")));
    }
    // Windows : l'installeur a déjà fermé l'app (le cache est purgé au démarrage suivant) ; ailleurs on nettoie ici.
    drop(bytes);
    let _ = std::fs::remove_file(&path);
    // Windows : l'installeur ferme déjà l'application ; ailleurs on relance sur la nouvelle version.
    app.restart();
}

/// Pour `lib.rs` : enregistre l'état et démarre les vérifications.
pub fn setup(app: &tauri::App, test_mode: bool) {
    // Version de l'application (tauri.conf.json, fixée par la CI à chaque release), celle que compare l'updater.
    // MSIX (Store) : le Store gère les mises à jour. Linux non-AppImage / lecture seule (Nix) : vérification seule.
    let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let appimage = std::env::var("APPIMAGE").ok();
    let writable = appimage.as_deref().is_some_and(appimage_writable);
    let kind = install_kind(cfg!(target_os = "linux"), appimage.as_deref(), &exe, writable);
    let packaged = if let InstallKind::Packaged(p) = kind { Some(p) } else { None };
    let cache_dir = app.path().app_cache_dir().unwrap_or_else(|_| std::env::temp_dir()).join("update");
    let ctl = Arc::new(UpdateCtl::new(app.package_info().version.to_string(), test_mode, kind == InstallKind::Store, packaged, cache_dir));
    app.manage(ctl.clone());
    start_background_checks(app.handle(), ctl);
}

#[cfg(test)]
mod tests {
    use super::*;

    const NIX: &str = "/nix/store/abc-cubelink-0.1.14-extracted/bin/cubelink";

    #[test]
    fn windows_inchange() {
        assert_eq!(install_kind(false, None, "C:\\Program Files\\Cubelink\\cubelink.exe", true), InstallKind::Standard);
        assert_eq!(install_kind(false, None, "C:\\Program Files\\WindowsApps\\X\\cubelink.exe", true), InstallKind::Store);
    }

    #[test]
    fn appimage_inscriptible() {
        assert_eq!(install_kind(true, Some("/home/u/Cubelink.AppImage"), "/tmp/.mount_x/usr/bin/cubelink", true), InstallKind::AppImage);
    }

    #[test]
    fn appimage_lecture_seule_ou_absente() {
        assert_eq!(install_kind(true, Some("/opt/Cubelink.AppImage"), "/tmp/.mount_x/cubelink", false), InstallKind::Packaged(Packager::Autre));
        assert_eq!(install_kind(true, None, "/usr/bin/cubelink", true), InstallKind::Packaged(Packager::Autre));
    }

    #[test]
    fn paquet_nix() {
        assert_eq!(install_kind(true, None, NIX, false), InstallKind::Packaged(Packager::Nix));
        assert_eq!(install_kind(true, Some("/nix/store/x.AppImage"), "/tmp/y", true), InstallKind::Packaged(Packager::Nix));
    }

    #[test]
    fn paquet_ecrit_puis_relu_identique() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("pas/encore/la");
        let bytes: Vec<u8> = (0..4_000_000u32).map(|i| (i % 251) as u8).collect();
        let p = store_package(&dir, "0.2.0", &bytes).unwrap();
        assert_eq!(p, dir.join("cubelink-0.2.0.pkg"));
        assert_eq!(std::fs::read(&p).unwrap(), bytes);
        assert!(!dir.join("cubelink-0.2.0.part").exists(), "aucun .part ne doit rester");
    }

    #[test]
    fn nom_de_paquet_sans_separateur() {
        let n = package_name("../1.0/x y", "pkg");
        assert!(!n.contains('/') && !n.contains(' '));
        assert_eq!(package_name("1.2.3-beta.1+b", "part"), "cubelink-1.2.3-beta.1+b.part");
    }

    #[test]
    fn ecriture_impossible_donne_une_erreur() {
        let tmp = tempfile::tempdir().unwrap();
        let fichier = tmp.path().join("fichier");
        std::fs::write(&fichier, b"x").unwrap();
        // Le « dossier » de cache est sous un fichier : création impossible.
        assert!(store_package(&fichier.join("update"), "1.0.0", b"abc").is_err());
    }

    #[test]
    fn purge_ne_touche_que_les_paquets() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        for n in ["cubelink-1.0.0.pkg", "cubelink-1.1.0.pkg", "cubelink-1.1.0.part", "autre.pkg", "cubelink-notes.txt"] {
            std::fs::write(d.join(n), b"x").unwrap();
        }
        purge_cache(d, Some(&d.join("cubelink-1.1.0.pkg")));
        let mut restants: Vec<String> = std::fs::read_dir(d).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        restants.sort();
        assert_eq!(restants, ["autre.pkg", "cubelink-1.1.0.pkg", "cubelink-notes.txt"]);
        purge_cache(d, None);
        assert!(!d.join("cubelink-1.1.0.pkg").exists());
        // Dossier absent : pas de panique.
        purge_cache(&d.join("absent"), None);
    }

    #[test]
    fn ready_ne_remplace_que_par_une_autre_version() {
        assert!(!needs_redownload(Some("0.2.0"), None), "rien trouvé : on garde le paquet prêt");
        assert!(!needs_redownload(Some("0.2.0"), Some("0.2.0")), "même version : pas de retéléchargement");
        assert!(needs_redownload(Some("0.2.0"), Some("0.2.2")), "version plus récente : on remplace");
        assert!(needs_redownload(None, Some("0.2.2")));
    }

    #[test]
    fn ctl_ne_garde_que_le_chemin_du_paquet() {
        // UpdateCtl ne porte plus que le chemin du paquet (aucun Vec<u8> en mémoire).
        let ctl = UpdateCtl::new("0.1.0".into(), false, false, None, std::env::temp_dir().join("cubelink-test-update"));
        assert!(lock(&ctl.pkg).is_none());
        *lock(&ctl.state) = UpdateState { phase: UpdatePhase::Ready, version: Some("0.2.0".into()), ..UpdateState::new("0.1.0".into(), UpdatePhase::Idle) };
        assert_eq!(ctl.snapshot().phase, UpdatePhase::Ready);
    }

    #[test]
    fn messages() {
        assert!(packaged_message(Packager::Nix).contains("Nix"));
        assert!(packaged_message(Packager::Nix).contains("flake"));
        // .rpm = asset de la release, pas de dépôt dnf : aucune promesse de mise à jour par le gestionnaire de paquets.
        let autre = packaged_message(Packager::Autre);
        assert!(autre.contains("release") && autre.contains(".rpm") && !autre.contains("dnf") && !autre.contains("mets à jour ton paquet"));
        assert!(install_failed_message(true).contains(RELEASE_URL));
        assert!(!install_failed_message(false).contains("http"));
    }

    fn etat(phase: UpdatePhase) -> UpdateState {
        UpdateState::new("0.1.0".into(), phase)
    }

    #[test]
    fn controle_de_fond_sans_nouveaute_donne_a_jour_et_date() {
        let mut s = etat(UpdatePhase::Checking);
        after_no_update(&mut s, 1_700_000_000);
        assert_eq!((s.phase, s.last_checked), (UpdatePhase::UpToDate, Some(1_700_000_000)));
    }

    #[test]
    fn echec_de_fond_garde_l_etat_precedent() {
        // Contrôle de fond raté après un « à jour » : on ne retombe pas sur « aucune vérification ».
        let mut prev = etat(UpdatePhase::UpToDate);
        prev.last_checked = Some(42);
        let mut s = prev.clone();
        s.phase = UpdatePhase::Checking;
        after_failure(&mut s, &prev, false, "x".into());
        assert_eq!((s.phase, s.last_checked, s.message), (UpdatePhase::UpToDate, Some(42), None));
        // Avant tout contrôle : reste `idle`, sans date.
        let prev = etat(UpdatePhase::Idle);
        let mut s = etat(UpdatePhase::Checking);
        after_failure(&mut s, &prev, false, "x".into());
        assert_eq!((s.phase, s.last_checked), (UpdatePhase::Idle, None));
        // Version trouvée mais gérée ailleurs : le message d'origine revient.
        let mut prev = etat(UpdatePhase::AvailableManual);
        prev.message = Some("m".into());
        let mut s = etat(UpdatePhase::Checking);
        after_failure(&mut s, &prev, false, "x".into());
        assert_eq!((s.phase, s.message.as_deref()), (UpdatePhase::AvailableManual, Some("m")));
    }

    #[test]
    fn echec_manuel_montre_l_erreur() {
        let mut s = etat(UpdatePhase::Checking);
        after_failure(&mut s, &etat(UpdatePhase::UpToDate), true, "boum".into());
        assert_eq!((s.phase, s.message.as_deref()), (UpdatePhase::Error, Some("boum")));
    }

    #[test]
    fn store_ne_dit_jamais_a_jour_mais_le_mode_test_si() {
        assert_eq!(disabled_phase(true), UpdatePhase::ManagedByStore);
        assert_eq!(disabled_phase(false), UpdatePhase::UpToDate, "les E2E simulés attendent `up_to_date`");
        let tmp = std::env::temp_dir().join("cubelink-test-update");
        // Store : état initial géré par le Store dès le démarrage (rien n'est vérifié, donc pas de « aucune vérification »).
        let store = UpdateCtl::new("0.1.0".into(), false, true, None, tmp.clone());
        assert!(store.disabled && store.store);
        assert_eq!(store.snapshot().phase, UpdatePhase::ManagedByStore);
        // Mode test : état initial inchangé.
        let test = UpdateCtl::new("0.1.0".into(), true, false, None, tmp);
        assert!(test.disabled && !test.store);
        assert_eq!(test.snapshot().phase, UpdatePhase::Idle);
    }

    #[test]
    fn phase_et_date_serialisees_pour_le_front() {
        let mut s = etat(UpdatePhase::ManagedByStore);
        s.last_checked = Some(7);
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(j["phase"], "managed_by_store");
        assert_eq!(j["last_checked"], 7);
        assert!(serde_json::to_value(etat(UpdatePhase::Idle)).unwrap()["last_checked"].is_null());
    }

    #[test]
    fn erreurs_classees_par_famille() {
        use FailureKind::*;
        assert_eq!(classify("The signature abc could not be decoded"), Signature);
        assert_eq!(classify("error sending request for url (https://github.com/x/latest.json)"), Network);
        assert_eq!(classify("dns error: failed to lookup address"), Network);
        assert_eq!(classify("operation timed out"), Network);
        // Le serveur répond mais n'a rien d'utilisable : pas un problème de connexion.
        assert_eq!(classify("the platform `linux-x86_64` was not found in the response `platforms` object"), NoRelease);
        assert_eq!(classify("Could not fetch a valid release JSON from the remote"), NoRelease);
        assert_eq!(classify("HTTP status client error (404 Not Found) for url (https://x)"), NoRelease);
        assert_eq!(classify("expected value at line 1 column 1"), NoRelease);
        assert_eq!(classify("quelque chose d'inattendu"), Unknown);
        assert!(!friendly(&"quelque chose d'inattendu").contains("connexion"));
        assert!(!friendly(&"the platform `x` was not found in the response").contains("connexion"));
        assert!(friendly(&"error sending request").contains("connexion"));
        assert!(friendly(&"bad signature").contains("signature"));
    }

    /// Vraies erreurs : types du plugin et refus de connexion réel (port loopback fermé), pas seulement des chaînes recopiées.
    #[test]
    fn vraies_erreurs_du_plugin_et_du_reseau() {
        use tauri_plugin_updater::Error;
        assert_eq!(classify(&Error::ReleaseNotFound.to_string()), FailureKind::NoRelease);
        assert_eq!(classify(&Error::TargetNotFound("linux-x86_64".into()).to_string()), FailureKind::NoRelease);
        assert_eq!(classify(&Error::TargetsNotFound(vec!["linux-x86_64".into()]).to_string()), FailureKind::NoRelease);
        let json = Error::Serialization(serde_json::from_str::<serde_json::Value>("").unwrap_err());
        assert_eq!(classify(&json.to_string()), FailureKind::NoRelease, "{json}");
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port(); // libéré aussitôt : fermé
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let err = rt.block_on(reqwest::Client::new().get(format!("http://127.0.0.1:{port}/latest.json")).send()).unwrap_err();
        assert_eq!(classify(&err.to_string()), FailureKind::Network);
    }
}
