//! Journal applicatif (`logs/cubelink.log`) : tout ce que l'application dit par `tracing`, y compris Iroh, le coeur et le
//! P2P. Sous Windows, l'application de production n'a pas de console : `eprintln!` y part dans le vide, et un joueur qui
//! signale « ca ne marche pas » n'avait aucune trace. Le journal est donc toujours ecrit.
//!
//! L'ecriture reutilise [`crate::stage_log::Outbox`] (fil dedie, file bornee, rotation a 1 Mo avec un seul ancien fichier
//! `cubelink.log.1` : ~2 Mo au plus sur le disque). Chaque ligne passe par [`crate::error::sanitize`] : ni chemin personnel
//! ni jeton dans le fichier qu'un joueur copie dans un ticket.
//!
//! Niveau : `info` pour Cubelink, `warn` pour le reste (Iroh, quinn...). `CUBELINK_LOG` le remplace (syntaxe `EnvFilter`,
//! ex. `CUBELINK_LOG=debug` ou `CUBELINK_LOG=warn,iroh=debug`) ; une valeur invalide est ignoree.

use std::fmt::{self, Write as _};
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::{EnvFilter, Layer};

use crate::stage_log::Outbox;

pub const FILE_NAME: &str = "cubelink.log";
/// Variable d'environnement qui remplace le filtre par defaut.
pub const ENV_VAR: &str = "CUBELINK_LOG";
/// Cubelink (l'app, `core_lib`, `p2p_lib`, l'hote de la Scene) en `info`, tout le reste (Iroh, quinn, hyper...) en `warn`.
pub const DEFAULT_FILTER: &str = "warn,cubelink_app=info,core_lib=info,p2p_lib=info,cubelink_host=info";

static OUTBOX: OnceLock<Arc<Outbox>> = OnceLock::new();

/// Pur : le filtre a utiliser. Une valeur absente ou invalide donne le filtre par defaut (jamais une panique au demarrage).
pub fn filter_from(spec: Option<&str>) -> EnvFilter {
    spec.map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| EnvFilter::try_new(s).ok())
        .unwrap_or_else(|| EnvFilter::new(DEFAULT_FILTER))
}

/// Pur : le texte d'une ligne (`message champ=valeur ...`), masque par `sanitize`.
pub fn format_event(message: &str, fields: &str) -> String {
    crate::error::sanitize(&format!("{message}{fields}"))
}

/// Recolte le message et les champs structures d'un evenement `tracing`.
#[derive(Default)]
struct Fields {
    message: String,
    extra: String,
}

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else {
            let _ = write!(self.extra, " {}={value:?}", field.name());
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            let _ = write!(self.extra, " {}={value}", field.name());
        }
    }
}

/// Couche qui poste chaque evenement au fil d'ecriture du journal.
struct FileLayer {
    out: Arc<Outbox>,
}

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for FileLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut f = Fields::default();
        event.record(&mut f);
        let meta = event.metadata();
        let source = format!("{} {}", meta.level(), meta.target());
        self.out.line(&source, &format_event(&f.message, &f.extra));
    }
}

/// Le subscriber complet : filtre + ecriture dans `out`.
fn subscriber(out: Arc<Outbox>, filter: EnvFilter) -> impl Subscriber + Send + Sync {
    tracing_subscriber::registry().with(filter).with(FileLayer { out })
}

/// Installe le journal dans `<root>/logs/cubelink.log`. A appeler une fois, des que le dossier de donnees est connu ; sans
/// effet si deja fait (tests) ou si le fichier ne peut pas etre cree (les `tracing::*!` ne font alors rien).
pub fn init(root: &Path) {
    if OUTBOX.get().is_some() {
        return;
    }
    let Ok(out) = Outbox::start(root.join("logs").join(FILE_NAME)) else { return };
    let out = Arc::new(out);
    if OUTBOX.set(out.clone()).is_err() {
        return;
    }
    let filter = filter_from(std::env::var(ENV_VAR).ok().as_deref());
    if tracing::subscriber::set_global_default(subscriber(out, filter)).is_err() {
        return;
    }
    // Une panique n'ecrit rien sans console : elle va au journal, puis au gestionnaire d'avant.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("panique : {info}");
        flush();
        previous(info);
    }));
}

/// Attend (brievement) que les lignes postees soient ecrites : avant de quitter le processus.
pub fn flush() {
    if let Some(o) = OUTBOX.get() {
        o.flush(Duration::from_millis(500));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Emet `f` avec un subscriber local ecrivant dans `dir/logs/cubelink.log` ; renvoie le contenu du fichier.
    fn journal(dir: &Path, filter: EnvFilter, f: impl FnOnce()) -> String {
        let path = dir.join("logs").join(FILE_NAME);
        let out = Arc::new(Outbox::start(path.clone()).unwrap());
        tracing::subscriber::with_default(subscriber(out.clone(), filter), f);
        out.flush(Duration::from_secs(5));
        std::fs::read_to_string(path).unwrap_or_default()
    }

    #[test]
    fn masque_chemin_avec_espace_et_jeton() {
        let s = format_event("lecture de C:\\Users\\Jean Dupont\\AppData\\x.json : refus, jeton eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abcdefghijk", " code=3");
        assert!(!s.contains("Dupont") && !s.contains("Jean") && !s.contains("eyJ"), "{s}");
        assert!(s.contains("<chemin>") && s.ends_with("code=3"), "{s}");
    }

    #[test]
    fn ecrit_les_evenements_horodates_et_masques() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("Jean Dupont").join("x.json");
        let txt = journal(d.path(), filter_from(None), || {
            tracing::warn!("raccourci « Foo+Bar » invalide : {}", home.display());
            tracing::error!(code = 7, nom = "abc", "échec P2P");
            tracing::info!(target: "core_lib::x", "info du coeur");
            tracing::debug!("trop bavard");
            tracing::info!(target: "iroh::net", "info Iroh coupee");
            tracing::warn!(target: "iroh::net", "relais injoignable");
        });
        assert!(txt.contains("[WARN cubelink_app::app_log::tests] raccourci « Foo+Bar » invalide"), "{txt}");
        assert!(txt.contains("échec P2P code=7 nom=abc"), "{txt}");
        assert!(txt.contains("info du coeur"), "{txt}");
        assert!(txt.contains("relais injoignable"), "iroh en warn est collecte : {txt}");
        assert!(!txt.contains("trop bavard") && !txt.contains("info Iroh coupee"), "{txt}");
        assert!(!txt.contains("Dupont"), "chemin brut dans le journal : {txt}");
        // `2026-11-12 14:03:22.123Z [` : horodatage UTC en tete de chaque ligne.
        assert!(txt.lines().all(|l| l.len() > 24 && l.as_bytes()[4] == b'-' && l.contains("Z [")), "{txt}");
    }

    #[test]
    fn une_ligne_par_evenement_meme_multiligne() {
        let d = tempfile::tempdir().unwrap();
        let txt = journal(d.path(), filter_from(None), || tracing::warn!("a\nb\r\nc"));
        assert_eq!(txt.lines().count(), 1, "{txt}");
    }

    #[test]
    fn variable_de_filtre_valide_ou_invalide() {
        let d = tempfile::tempdir().unwrap();
        let txt = journal(d.path(), filter_from(Some("debug")), || tracing::debug!("détail"));
        assert!(txt.contains("détail"), "{txt}");
        // Invalide, vide ou absente : le filtre par defaut, sans panique.
        for spec in [Some("=[[[ ,, ="), Some("  "), None] {
            let d = tempfile::tempdir().unwrap();
            let txt = journal(d.path(), filter_from(spec), || {
                tracing::debug!("détail");
                tracing::warn!("garde");
            });
            assert!(txt.contains("garde") && !txt.contains("détail"), "{spec:?} : {txt}");
        }
    }

    #[test]
    fn le_journal_reste_borne_par_la_rotation() {
        // Meme mecanique que stage.log : un seul ancien fichier, jamais plus.
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("logs").join(FILE_NAME);
        for i in 0..200 {
            crate::stage_log::append(&path, &format!("ligne {i:04} {}\n", "x".repeat(40)), 1000).unwrap();
        }
        let mut names: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        names.sort();
        assert_eq!(names, ["cubelink.log", "cubelink.log.1"]);
        assert!(std::fs::metadata(&path).unwrap().len() <= 1000);
    }
}
