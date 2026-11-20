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
        Packager::Autre => "Cubelink n'est pas une AppImage modifiable : mets à jour ton paquet, ou télécharge la nouvelle AppImage depuis GitHub.".into(),
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
}

impl UpdateState {
    fn new(current: String) -> Self {
        Self { phase: UpdatePhase::Idle, current, version: None, notes: None, downloaded: 0, total: None, message: None, url: None }
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
    /// Mode test : aucune requête réseau, l'état reste `idle` / `up_to_date`.
    disabled: bool,
    /// Paquet géré ailleurs : on vérifie, on ne télécharge pas.
    packaged: Option<Packager>,
}

impl UpdateCtl {
    pub fn new(current: String, disabled: bool, packaged: Option<Packager>, cache_dir: PathBuf) -> Self {
        Self { state: Mutex::new(UpdateState::new(current)), update: Mutex::new(None), pkg: Mutex::new(None), cache_dir, busy: AtomicBool::new(false), disabled, packaged }
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
    // Déjà en cours : rien à refaire. En phase `ready` on revérifie (une version plus récente peut être sortie).
    if matches!(ctl.snapshot().phase, UpdatePhase::Checking | UpdatePhase::Downloading) {
        return ctl.snapshot();
    }
    if ctl.busy.swap(true, Ordering::SeqCst) {
        return ctl.snapshot();
    }
    // En `ready` on ne touche pas à la phase pendant la requête (sinon le bouton « Redémarrer et installer » clignote)
    // et un contrôle sans résultat ou en échec laisse la mise à jour prête intacte.
    let was_ready = ctl.snapshot().phase == UpdatePhase::Ready;
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
                    return snap;
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
                });
            }
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
        Ok(None) if was_ready => {
            ctl.busy.store(false, Ordering::SeqCst);
            ctl.snapshot()
        }
        Ok(None) => {
            ctl.busy.store(false, Ordering::SeqCst);
            ctl.set(app, |s| s.phase = if manual { UpdatePhase::UpToDate } else { UpdatePhase::Idle })
        }
        Err(e) => {
            eprintln!("[Cubelink] mise à jour : vérification impossible : {e}");
            ctl.busy.store(false, Ordering::SeqCst);
            if was_ready {
                return ctl.snapshot();
            }
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
    let ctl = Arc::new(UpdateCtl::new(app.package_info().version.to_string(), test_mode || kind == InstallKind::Store, packaged, cache_dir));
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
        let ctl = UpdateCtl::new("0.1.0".into(), false, None, std::env::temp_dir().join("cubelink-test-update"));
        assert!(lock(&ctl.pkg).is_none());
        *lock(&ctl.state) = UpdateState { phase: UpdatePhase::Ready, version: Some("0.2.0".into()), ..UpdateState::new("0.1.0".into()) };
        assert_eq!(ctl.snapshot().phase, UpdatePhase::Ready);
    }

    #[test]
    fn messages() {
        assert!(packaged_message(Packager::Nix).contains("Nix"));
        assert!(packaged_message(Packager::Nix).contains("flake"));
        assert!(install_failed_message(true).contains(RELEASE_URL));
        assert!(!install_failed_message(false).contains("http"));
    }
}
