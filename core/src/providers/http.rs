//! Construction des clients HTTP des fournisseurs de mods : un seul endroit pour les délais.
//!
//! - Appels d'API (JSON courts) : délai de connexion, délai de lecture ET délai global.
//! - Téléchargements de fichiers : délai de connexion et de lecture seulement. Un délai global
//!   couperait un gros jar sur un lien lent ; le délai de lecture, lui, est remis à zéro à chaque
//!   morceau reçu et ne coupe qu'un flux réellement muet.

use std::time::Duration;

/// Délai pour établir la connexion.
pub const CONNECT: Duration = Duration::from_secs(15);
/// Délai maximal entre deux lectures (flux muet).
pub const READ: Duration = Duration::from_secs(30);
/// Durée totale maximale d'un appel d'API.
pub const API_TOTAL: Duration = Duration::from_secs(30);

/// Délais d'un client ; `total` à `None` pour un téléchargement.
#[derive(Clone, Copy, Debug)]
pub struct Timeouts {
    pub connect: Duration,
    pub read: Duration,
    pub total: Option<Duration>,
}

impl Timeouts {
    pub const API: Timeouts = Timeouts { connect: CONNECT, read: READ, total: Some(API_TOTAL) };
    pub const DOWNLOAD: Timeouts = Timeouts { connect: CONNECT, read: READ, total: None };
}

fn builder(t: Timeouts, user_agent: Option<&str>, follow_redirects: bool) -> reqwest::ClientBuilder {
    let mut b = reqwest::Client::builder().connect_timeout(t.connect).read_timeout(t.read);
    if let Some(total) = t.total {
        b = b.timeout(total);
    }
    if let Some(ua) = user_agent {
        b = b.user_agent(ua);
    }
    if !follow_redirects {
        b = b.redirect(reqwest::redirect::Policy::none());
    }
    b
}

/// Client pour les appels d'API (délai global compris).
pub fn api_client(user_agent: &str, follow_redirects: bool) -> reqwest::Client {
    client_with(Timeouts::API, Some(user_agent), follow_redirects)
}

/// Client pour télécharger des fichiers (pas de délai global).
pub fn download_client(follow_redirects: bool) -> reqwest::Client {
    client_with(Timeouts::DOWNLOAD, None, follow_redirects)
}

pub fn client_with(t: Timeouts, user_agent: Option<&str>, follow_redirects: bool) -> reqwest::Client {
    builder(t, user_agent, follow_redirects).pool_max_idle_per_host(32).build().unwrap_or_default()
}
