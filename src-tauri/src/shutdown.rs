//! Fin de session et signaux d'arrêt (docs/hearth.md § 7.3, tâche n° 38).
//!
//! Quand la machine ou la session s'arrête, le système ne laisse que quelques secondes. Cubelink fait alors le strict
//! nécessaire pour un serveur de groupe (annonce aux joueurs, `stop`, bail relâché avec `Shutdown`, rien de publié : voir
//! `hearth::StopMode::Shutdown`) et pour le jeu intégré, par la même routine que « Quitter » (`background::graceful_exit`).
//!
//! * Linux et autres Unix : `SIGTERM` (envoyé par le système à l'arrêt de la machine) lance l'arrêt court, sans publication.
//!   `SIGINT` et `SIGHUP` (Ctrl-C, terminal fermé : l'utilisateur ferme volontairement l'application) lancent une
//!   fermeture normale, comme « Quitter ». Un second signal pendant l'arrêt force la sortie (un Ctrl-C ne doit jamais
//!   rendre l'application insensible).
//! * Windows : une fenêtre cachée de premier niveau (pas `HWND_MESSAGE`, qui ne reçoit pas les diffusions) traite
//!   `WM_QUERYENDSESSION` et `WM_ENDSESSION` (`shutdown_win.rs`).
//! * Dernier filet : la dernière fenêtre détruite par le système (`RunEvent::ExitRequested` sans code, voir `lib.rs`).
//!
//! Rien ici n'est actif en mode test.

use std::sync::Arc;
use std::time::Duration;

use tauri::AppHandle;

use crate::background::{self, ExitCause};
use crate::state::AppState;

/// Texte montré par Windows dans l'écran « cette application empêche l'arrêt » (`ShutdownBlockReasonCreate`).
pub const BLOCK_REASON: &str = "Cubelink enregistre la partie du groupe…";

/// Temps pendant lequel `WM_ENDSESSION` attend la fin de la routine avant de rendre la main au système.
pub const END_SESSION_WAIT: Duration = Duration::from_secs(25);

/// Que répondre à `WM_QUERYENDSESSION` ?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryAction {
    /// Rien à protéger : répondre TRUE, le système peut continuer.
    Allow,
    /// Un serveur ou un jeu est ouvert : annoncer la raison du blocage, lancer la routine, répondre FALSE (TRUE ne garantit
    /// aucun délai). L'utilisateur garde « Arrêter quand même ».
    BlockAndStop,
}

/// Pur : la décision de `WM_QUERYENDSESSION` ne dépend que de ce qui tourne.
pub fn query_action(busy: bool) -> QueryAction {
    if busy {
        QueryAction::BlockAndStop
    } else {
        QueryAction::Allow
    }
}

/// Ce que la fenêtre cachée de Windows demande à l'application. Séparé de Tauri pour rester testable.
pub trait SessionEnd: Send + Sync + 'static {
    /// Un serveur de groupe ou un jeu intégré tourne.
    fn busy(&self) -> bool;
    /// Lance la routine d'arrêt (idempotent, ne bloque pas).
    fn begin(&self);
    /// Attend, au plus `max`, que la routine termine l'application. Le processus sort de lui-même à la fin de la routine :
    /// revenir sans qu'il soit sorti veut dire que le délai est écoulé.
    fn wait(&self, max: Duration);
}

/// Branchement sur l'application : la même routine que « Quitter », en mode arrêt de la machine.
pub struct AppSessionEnd {
    pub app: AppHandle,
    pub st: Arc<AppState>,
}

impl SessionEnd for AppSessionEnd {
    fn busy(&self) -> bool {
        !self.st.hearth.active_groups().is_empty() || self.st.stage.staged()
    }

    fn begin(&self) {
        background::graceful_exit(&self.app, &self.st, ExitCause::Shutdown);
    }

    fn wait(&self, max: Duration) {
        std::thread::sleep(max);
    }
}

/// Un arrêt est demandé (signal, fin de session) : la routine d'arrêt de la machine.
pub fn begin_shutdown(app: &AppHandle, st: &Arc<AppState>) {
    background::graceful_exit(app, st, ExitCause::Shutdown);
}

/// Installe ce qui écoute la fin de session du système. Sans effet en mode test.
pub fn install(app: &AppHandle, st: &Arc<AppState>) {
    if crate::test_hooks::hooks().test_mode {
        return;
    }
    #[cfg(unix)]
    {
        let (app, st) = (app.clone(), st.clone());
        tauri::async_runtime::spawn(async move {
            let signals = match unix::Signals::register() {
                Ok(s) => s,
                Err(e) => return eprintln!("[Cubelink] signaux d'arrêt non écoutés : {e}"),
            };
            signals
                .run(move |n, sig| {
                    if n == 1 {
                        match sig {
                            unix::Sig::Term => begin_shutdown(&app, &st),
                            unix::Sig::Int | unix::Sig::Hup => background::quit_app(&app, &st),
                        }
                    } else {
                        eprintln!("[Cubelink] second signal d'arrêt : sortie immédiate");
                        std::process::exit(130);
                    }
                })
                .await;
        });
    }
    #[cfg(windows)]
    win::spawn_session_watcher(Arc::new(AppSessionEnd { app: app.clone(), st: st.clone() }));
    #[cfg(not(any(unix, windows)))]
    let _ = (app, st);
}

#[cfg(unix)]
pub mod unix {
    use tokio::signal::unix::{signal, Signal, SignalKind};

    /// Quel signal a été reçu.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Sig {
        Term,
        Int,
        Hup,
    }

    /// `SIGTERM`, `SIGINT` et `SIGHUP`. Les signaux doivent être enregistrés dans un runtime tokio.
    pub struct Signals {
        term: Signal,
        int: Signal,
        hup: Signal,
    }

    impl Signals {
        pub fn register() -> std::io::Result<Signals> {
            Ok(Signals { term: signal(SignalKind::terminate())?, int: signal(SignalKind::interrupt())?, hup: signal(SignalKind::hangup())? })
        }

        /// Appelle `on_signal(n, signal)` à chaque signal reçu, `n` valant 1 au premier, 2 au suivant, etc. Ne rend jamais la
        /// main.
        pub async fn run(mut self, mut on_signal: impl FnMut(u32, Sig)) {
            let mut n = 0u32;
            loop {
                let sig = tokio::select! {
                    _ = self.term.recv() => Sig::Term,
                    _ = self.int.recv() => Sig::Int,
                    _ = self.hup.recv() => Sig::Hup,
                };
                n += 1;
                on_signal(n, sig);
            }
        }
    }
}

#[cfg(windows)]
#[path = "shutdown_win.rs"]
mod win;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requete_de_fin_de_session_bloque_seulement_si_quelque_chose_tourne() {
        assert_eq!(query_action(true), QueryAction::BlockAndStop);
        assert_eq!(query_action(false), QueryAction::Allow);
    }

    /// Un vrai SIGTERM envoyé au processus de test : le premier est compté 1, le second 2. Le gestionnaire est installé
    /// avant l'envoi, donc le signal n'est pas fatal pour le test.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sigterm_declenche_l_arret_une_fois_puis_signale_le_second() {
        let signals = unix::Signals::register().unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(signals.run(move |n, sig| {
            assert_eq!(sig, unix::Sig::Term);
            let _ = tx.send(n);
        }));
        let me = std::process::id().to_string();
        for expected in [1u32, 2] {
            let status = std::process::Command::new("kill").args(["-TERM", &me]).status().unwrap();
            assert!(status.success());
            let n = tokio::time::timeout(Duration::from_secs(10), rx.recv()).await.expect("signal non vu").unwrap();
            assert_eq!(n, expected);
        }
        task.abort();
    }
}
