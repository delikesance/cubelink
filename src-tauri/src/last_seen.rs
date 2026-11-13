//! Dernière fois qu'un ami a été vu en ligne (secondes Unix), mémorisée dans `p2p/last_seen.json`.
//!
//! Alimentée par les événements de présence : un ami qui passe en ligne ou hors ligne était là à cet instant.
//! Écrit au plus une fois par minute et par ami pour ne pas user le disque quand la présence clignote.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// Écart minimal entre deux écritures pour un même ami.
const WRITE_EVERY_SECS: i64 = 60;

pub struct LastSeen {
    path: PathBuf,
    map: Mutex<HashMap<String, i64>>,
}

impl LastSeen {
    pub fn open(path: PathBuf) -> Self {
        let map = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<HashMap<String, i64>>(&b).ok())
            .unwrap_or_default();
        Self { path, map: Mutex::new(map) }
    }

    pub fn get(&self, key: &str) -> Option<i64> {
        self.map.lock().ok()?.get(key).copied()
    }

    /// Note que `key` était en ligne à `now`. N'écrit le fichier que si la valeur avance d'au moins une minute.
    pub fn touch(&self, key: &str, now: i64) {
        let Ok(mut m) = self.map.lock() else { return };
        let prev = m.get(key).copied().unwrap_or(i64::MIN);
        if now <= prev {
            return;
        }
        m.insert(key.to_string(), now);
        if now.saturating_sub(prev) < WRITE_EVERY_SECS {
            return;
        }
        if let Ok(json) = serde_json::to_vec(&*m) {
            let tmp = self.path.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memorise_et_relit() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("last_seen.json");
        let ls = LastSeen::open(p.clone());
        assert_eq!(ls.get("a"), None);
        ls.touch("a", 1000);
        ls.touch("a", 1010); // en mémoire, pas encore écrit
        ls.touch("a", 900); // jamais en arrière
        assert_eq!(ls.get("a"), Some(1010));
        assert_eq!(LastSeen::open(p.clone()).get("a"), Some(1000));
        ls.touch("a", 1100);
        assert_eq!(LastSeen::open(p).get("a"), Some(1100));
    }
}
