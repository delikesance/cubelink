//! Dernière fois qu'un ami a été vu en ligne (secondes Unix), mémorisée dans `p2p/last_seen.json`.
//!
//! Alimentée par les événements de présence : un ami qui passe en ligne ou hors ligne était là à cet instant.
//! Le fichier est écrit au plus une fois par minute, au total (il est réécrit en entier, un seuil par ami
//! multiplierait les écritures). `flush` écrit ce qui reste en attente : à la sortie de l'app et quand un
//! ami passe hors ligne.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// Écart minimal entre deux écritures du fichier.
const WRITE_EVERY_SECS: i64 = 60;

struct Inner {
    map: HashMap<String, i64>,
    /// Instant (injecté par l'appelant) de la dernière écriture réussie.
    last_write: i64,
    /// Des valeurs en mémoire ne sont pas encore sur le disque.
    dirty: bool,
}

pub struct LastSeen {
    path: PathBuf,
    inner: Mutex<Inner>,
}

impl LastSeen {
    pub fn open(path: PathBuf) -> Self {
        let map = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<HashMap<String, i64>>(&b).ok())
            .unwrap_or_default();
        Self { path, inner: Mutex::new(Inner { map, last_write: i64::MIN, dirty: false }) }
    }

    pub fn get(&self, key: &str) -> Option<i64> {
        self.inner.lock().ok()?.map.get(key).copied()
    }

    /// Note que `key` était en ligne à `now`. N'écrit le fichier que si la dernière écriture date d'au moins une minute.
    pub fn touch(&self, key: &str, now: i64) {
        let Ok(mut g) = self.inner.lock() else { return };
        let prev = g.map.get(key).copied().unwrap_or(i64::MIN);
        if now <= prev {
            return;
        }
        g.map.insert(key.to_string(), now);
        g.dirty = true;
        if now.saturating_sub(g.last_write) >= WRITE_EVERY_SECS {
            self.write(&mut g, now);
        }
    }

    /// Écrit tout de suite ce qui n'est pas encore sur le disque (sans effet si tout est à jour).
    pub fn flush(&self) {
        let Ok(mut g) = self.inner.lock() else { return };
        if g.dirty {
            let now = g.map.values().copied().max().unwrap_or(g.last_write);
            self.write(&mut g, now);
        }
    }

    /// Écriture atomique (fichier temporaire puis renommage). En cas d'échec, `dirty` reste vrai : le prochain appel réessaie.
    fn write(&self, g: &mut Inner, now: i64) {
        let Ok(json) = serde_json::to_vec(&g.map) else { return };
        let tmp = self.path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() && std::fs::rename(&tmp, &self.path).is_ok() {
            g.last_write = now;
            g.dirty = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fichier() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("last_seen.json");
        (dir, p)
    }

    #[test]
    fn memorise_et_relit() {
        let (_d, p) = fichier();
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

    /// Scénario du ticket : une mise à jour toutes les 15 s (annonce de monde) ne doit pas bloquer l'écriture.
    #[test]
    fn ecrit_malgre_des_mises_a_jour_rapprochees() {
        let (_d, p) = fichier();
        let ls = LastSeen::open(p.clone());
        let mut t = 1000;
        while t <= 1120 {
            ls.touch("a", t);
            t += 15;
        }
        // Écritures à 1000 et 1060 (puis 1120 avec 60 s d'écart) : le fichier ne doit pas rester à 1000.
        assert!(LastSeen::open(p).get("a").unwrap() >= 1060);
    }

    #[test]
    fn seuil_global_pas_par_ami() {
        let (_d, p) = fichier();
        let ls = LastSeen::open(p.clone());
        ls.touch("a", 1000);
        ls.touch("b", 1010); // moins d'une minute après l'écriture : en attente
        assert_eq!(LastSeen::open(p.clone()).get("b"), None);
        ls.flush();
        assert_eq!(LastSeen::open(p).get("b"), Some(1010));
    }

    #[test]
    fn flush_ecrit_le_reste_une_seule_fois() {
        let (_d, p) = fichier();
        let ls = LastSeen::open(p.clone());
        ls.touch("a", 1000);
        ls.touch("a", 1010);
        assert_eq!(LastSeen::open(p.clone()).get("a"), Some(1000));
        ls.flush();
        assert_eq!(LastSeen::open(p.clone()).get("a"), Some(1010));
        // Rien de nouveau : un second flush ne réécrit pas.
        std::fs::remove_file(&p).unwrap();
        ls.flush();
        assert!(!p.exists());
    }

    #[test]
    fn retour_en_arriere_ne_marque_pas_a_ecrire() {
        let (_d, p) = fichier();
        let ls = LastSeen::open(p.clone());
        ls.touch("a", 1000);
        std::fs::remove_file(&p).unwrap();
        ls.touch("a", 900);
        ls.flush();
        assert!(!p.exists());
    }

    #[test]
    fn echec_d_ecriture_reessaye_au_touch_suivant() {
        let dir = tempfile::tempdir().unwrap();
        let sous = dir.path().join("p2p");
        let p = sous.join("last_seen.json");
        let ls = LastSeen::open(p.clone());
        ls.touch("a", 1000); // dossier absent : échec
        assert!(!p.exists());
        std::fs::create_dir_all(&sous).unwrap();
        ls.touch("a", 1001); // pas attendre 60 s
        assert_eq!(LastSeen::open(p).get("a"), Some(1001));
    }
}
