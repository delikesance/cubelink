//! Windows : Job Object qui regroupe le jeu et ses descendants, pour les arrêter tous d'un coup (voir `launch_run`).

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW, TerminateJobObject};

/// Job Object anonyme, sans `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` : le fermer ne tue rien.
pub struct Job(HANDLE);

// SAFETY : un handle de Job Object s'utilise depuis n'importe quel fil ; `Job` en est le seul propriétaire.
unsafe impl Send for Job {}
// SAFETY : `terminate` ne fait qu'un appel système sur le handle, sans état partagé côté Rust.
unsafe impl Sync for Job {}

impl Job {
    /// Crée un Job Object et y place `process` (handle valide pendant l'appel). `None` en cas d'échec.
    pub fn for_process(process: HANDLE) -> Option<Self> {
        // SAFETY : pas d'attributs de sécurité ni de nom : deux pointeurs nuls sont acceptés.
        let h = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if h.is_null() {
            return None;
        }
        let job = Job(h);
        // SAFETY : `job.0` vient d'être créé, `process` est fourni valide par l'appelant.
        (unsafe { AssignProcessToJobObject(job.0, process) } != 0).then_some(job)
    }

    /// Tue tous les processus du Job (le jeu et ses descendants).
    pub fn terminate(&self) {
        // SAFETY : handle possédé par `self`, valide tant que `self` vit.
        unsafe { TerminateJobObject(self.0, 1) };
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY : handle possédé par `self`, fermé une seule fois.
        unsafe { CloseHandle(self.0) };
    }
}
