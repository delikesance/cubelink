//! Evenements recus de l'hote, communs aux scenarios Win32 (`common`) et X11 (`common_x11`).

use std::sync::{Arc, Mutex};

use cubelink_host::{FallbackReason, HostEvent, Observer};

/// Evenements recus de l'hote (l'observateur est appele depuis ses fils).
#[derive(Clone, Default)]
pub struct Events(Arc<Mutex<Vec<HostEvent>>>);

impl Events {
    pub fn observer(&self) -> Observer {
        let sink = self.0.clone();
        Arc::new(move |ev: &HostEvent| sink.lock().unwrap().push(ev.clone()))
    }

    pub fn all(&self) -> Vec<HostEvent> {
        self.0.lock().unwrap().clone()
    }

    pub fn fallbacks(&self) -> Vec<(FallbackReason, String)> {
        self.all().into_iter().filter_map(|e| if let HostEvent::Fallback { reason, detail } = e { Some((reason, detail)) } else { None }).collect()
    }

    /// Classes des fenetres annoncees par `HostEvent::Adopted`, dans l'ordre.
    pub fn adopted(&self) -> Vec<String> {
        self.all().into_iter().filter_map(|e| if let HostEvent::Adopted { class, .. } = e { Some(class) } else { None }).collect()
    }

    /// Valeurs de `HostEvent::Fullscreen`, dans l'ordre.
    pub fn fullscreens(&self) -> Vec<bool> {
        self.all().into_iter().filter_map(|e| if let HostEvent::Fullscreen(on) = e { Some(on) } else { None }).collect()
    }

    pub fn logs(&self) -> Vec<String> {
        self.all().into_iter().filter_map(|e| if let HostEvent::Log(m) = e { Some(m) } else { None }).collect()
    }

    /// Une ligne du journal contient `needle`.
    pub fn logged(&self, needle: &str) -> bool {
        self.logs().iter().any(|l| l.contains(needle))
    }

    /// Echec du test si le journal ne contient pas `needle` (affiche alors tout le journal).
    #[track_caller]
    pub fn assert_logged(&self, needle: &str) {
        assert!(self.logged(needle), "journal sans {needle:?} :\n{}", self.logs().join("\n"));
    }
}
