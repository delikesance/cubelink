//! Petits outils partagés par les modules du crate.
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

/// Verrou tolérant à l'empoisonnement : chaque section critique met l'état à jour d'un bloc, il reste cohérent.
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Secondes depuis l'époque Unix (0 si l'horloge est avant 1970).
pub(crate) fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
