//! Tâche N17 : configuration et observation du relais.
//!
//! - [`RelaySettings`] / [`build_endpoint`] : applique « direct seul », « relais seul »,
//!   relais par défaut (n0) ou auto-hébergé (URL validée).
//! - [`PathObserver`] : expose le type de chemin d'une connexion (direct / relais) et ses bascules.
//! - [`SlowRelayAdvisor`] : décide d'afficher « plus lentement » (fonction pure, temps injecté).
//! - [`relay_hint_from_code`] : relais d'attache éventuel d'un code ami.
//!
//! Honnêteté : le relais ne déchiffre pas le trafic QUIC (chiffré de bout en bout), mais il voit
//! les adresses IP des deux pairs et le volume échangé. Les messages n'affirment pas le contraire.

use std::net::IpAddr;
use std::time::Duration;

use core_lib::types::{Error, Result};
use iroh::endpoint::{presets, Connection, PathList};
use iroh::{Endpoint, RelayMode, RelayUrl, SecretKey};
use n0_future::StreamExt;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::friend_code::{self, FriendCodePayload};

// ---------- réglages et mapping des modes ----------

/// Relais utilisé quand il est autorisé.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayChoice {
    /// Relais publics n0 (best effort, ~1 Mo/s : inadapté aux mods).
    Default,
    /// Relais auto-hébergé (`https://…`, `http://` seulement en loopback pour les tests).
    Custom(String),
    /// Aucun relais.
    Disabled,
}

/// Réglages relais ; `allow_relay` / `relay_only` correspondent aux champs de `Settings`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelaySettings {
    pub mode: RelayChoice,
    /// « Passer toujours par le relais » : aucun transport IP.
    pub relay_only: bool,
    pub allow_relay: bool,
    /// Service de découverte d'adresses public (n0 : pkarr + DNS). Désactivé dans `Default` (tests, hors
    /// réseau) ; l'application le pilote par le réglage `Settings.discovery` (activé par défaut).
    /// Activé, l'endpoint PUBLIE auprès de n0 (`dns.iroh.link`) sa clé publique, son relais
    /// d'attache et ses adresses IP, et interroge ce service pour retrouver un pair par sa seule
    /// clé : n0 voit donc qui tu es (clé) et d'où tu te connectes (IP), et peut observer qui tu
    /// cherches. Désactivé, un pair n'est joignable que via les adresses/relais de son code ami
    /// (voir [`endpoint_addr_info`]) : aucune fuite vers un tiers.
    pub address_lookup: bool,
}

impl Default for RelaySettings {
    fn default() -> Self {
        Self { mode: RelayChoice::Default, relay_only: false, allow_relay: true, address_lookup: false }
    }
}

/// Résultat du mapping, sans réseau : ce qui sera appliqué au builder iroh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointPlan {
    pub relay_mode: RelayMode,
    /// `false` : `clear_ip_transports()` (relais seul).
    pub ip_transports: bool,
}

fn invalid(msg: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("relais : {msg}"))
}

fn is_loopback_host(host: &str) -> bool {
    let h = host.trim_start_matches('[').trim_end_matches(']');
    h.eq_ignore_ascii_case("localhost") || h.parse::<IpAddr>().map(|ip| ip.is_loopback()).unwrap_or(false)
}

/// Valide l'URL d'un relais : https obligatoire, http toléré uniquement en loopback,
/// hôte requis, pas d'identifiants dans l'URL.
pub fn validate_relay_url(s: &str) -> Result<RelayUrl> {
    let url: RelayUrl = s.trim().parse().map_err(|_| invalid(format!("URL invalide « {s} »")))?;
    let host = url.host_str().ok_or_else(|| invalid("hôte manquant"))?.to_owned();
    if host.is_empty() {
        return Err(invalid("hôte manquant"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(invalid("identifiants interdits dans l'URL"));
    }
    match url.scheme() {
        "https" => {}
        "http" if is_loopback_host(&host) => {}
        "http" => return Err(invalid("http refusé (https obligatoire hors loopback)")),
        other => return Err(invalid(format!("schéma « {other} » non supporté"))),
    }
    Ok(url)
}

/// Mapping pur des réglages vers la configuration iroh.
/// - `allow_relay=false` => relais désactivé (direct uniquement) ;
/// - `relay_only` => pas de transport IP ; contradictoire avec `allow_relay=false` ou `Disabled`.
pub fn plan(settings: &RelaySettings) -> Result<EndpointPlan> {
    let disabled = !settings.allow_relay || settings.mode == RelayChoice::Disabled;
    if settings.relay_only && disabled {
        return Err(invalid(
            "contradiction : « toujours passer par le relais » est activé alors que le relais est désactivé",
        ));
    }
    let relay_mode = if disabled {
        RelayMode::Disabled
    } else {
        match &settings.mode {
            RelayChoice::Default => RelayMode::Default,
            RelayChoice::Custom(url) => RelayMode::custom([validate_relay_url(url)?]),
            RelayChoice::Disabled => RelayMode::Disabled,
        }
    };
    Ok(EndpointPlan { relay_mode, ip_transports: !settings.relay_only })
}

/// Crée l'endpoint iroh selon les réglages. `secret` est la clé secrète de l'identité
/// (`Identity` n'expose pas encore ses octets secrets : voir rapport N17).
pub async fn build_endpoint(secret: SecretKey, settings: &RelaySettings, alpns: Vec<Vec<u8>>) -> Result<Endpoint> {
    let plan = plan(settings)?;
    let mut b = Endpoint::builder(presets::Minimal).secret_key(secret).relay_mode(plan.relay_mode);
    if !plan.ip_transports {
        b = b.clear_ip_transports();
    }
    if settings.address_lookup {
        use iroh::address_lookup::{DnsAddressLookup, PkarrPublisher, PkarrResolver};
        b = b
            .address_lookup(PkarrPublisher::n0_dns())
            .address_lookup(PkarrResolver::n0_dns())
            .address_lookup(DnsAddressLookup::n0_dns());
    }
    b.alpns(alpns).bind().await.map_err(|e| Error::Network(format!("création de l'endpoint : {e}")))
}

// ---------- code ami ----------

/// Adresses d'un endpoint, prêtes à mettre dans un code ami ([`FriendCodePayload`]) : bornées
/// aux limites du format (chaînes <= `MAX_STR_LEN`, au plus `MAX_ADDRS` adresses).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EndpointAddrInfo {
    /// URL du relais d'attache.
    pub relay: Option<String>,
    /// Adresses IP `ip:port`, les non-loopback d'abord.
    pub addrs: Vec<String>,
}

impl EndpointAddrInfo {
    /// Construit le payload de code ami (à passer à `friend_code::encode`).
    pub fn into_payload(self, public_key: [u8; 32], epoch: u32) -> FriendCodePayload {
        FriendCodePayload { public_key, epoch, relay: self.relay, addrs: self.addrs }
    }
}

/// Extrait relais + adresses IP d'une `EndpointAddr` (typiquement `endpoint.addr()`) pour le code
/// ami. Ce que cela révèle : tes adresses IP à quiconque reçoit le code (un ami).
pub fn endpoint_addr_info(addr: &iroh::EndpointAddr) -> EndpointAddrInfo {
    let ok = |s: &str| s.len() <= friend_code::MAX_STR_LEN;
    let relay = addr.relay_urls().map(|u| u.to_string()).find(|u| ok(u));
    let mut ips: Vec<std::net::SocketAddr> = addr.ip_addrs().copied().collect();
    ips.sort_by_key(|a| (a.ip().is_loopback() || a.ip().is_unspecified(), a.is_ipv6()));
    let addrs = ips.iter().map(|a| a.to_string()).filter(|s| ok(s)).take(friend_code::MAX_ADDRS).collect();
    EndpointAddrInfo { relay, addrs }
}

/// Inverse : `EndpointAddr` joignable depuis un code ami. Relais invalide ou adresse illisible :
/// ignorés (indices seulement).
pub fn endpoint_addr_from_payload(payload: &FriendCodePayload) -> Result<iroh::EndpointAddr> {
    let id = iroh::EndpointId::from_bytes(&payload.public_key).map_err(|e| invalid(format!("clé publique : {e}")))?;
    let mut addr = iroh::EndpointAddr::new(id);
    if let Some(r) = relay_hint(payload) {
        addr = addr.with_relay_url(r);
    }
    for a in &payload.addrs {
        if let Ok(sa) = a.parse::<std::net::SocketAddr>() {
            addr = addr.with_ip_addr(sa);
        }
    }
    Ok(addr)
}

/// Relais d'attache d'un code ami déjà parsé (`host[:port]` ou URL). Invalide => `None`.
/// Ce n'est qu'un indice : s'y connecter expose l'IP au relais d'un tiers, à l'appelant de décider.
pub fn relay_hint(payload: &FriendCodePayload) -> Option<RelayUrl> {
    let r = payload.relay.as_deref()?.trim();
    let candidate = if r.contains("://") { r.to_owned() } else { format!("https://{r}") };
    validate_relay_url(&candidate).ok()
}

/// Parse un code ami (ou lien) puis extrait son relais d'attache.
pub fn relay_hint_from_code(code: &str) -> Result<Option<RelayUrl>> {
    Ok(relay_hint(&friend_code::parse(code)?))
}

// ---------- observation du chemin ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    Direct,
    Relay,
    Unknown,
}

/// Type du chemin sélectionné dans un instantané `conn.paths()`.
pub fn kind_of(list: &PathList<'_>) -> PathKind {
    for p in list.iter() {
        if p.is_selected() {
            return if p.is_relay() { PathKind::Relay } else { PathKind::Direct };
        }
    }
    PathKind::Unknown
}

/// Suit le chemin d'une connexion en tâche de fond ; s'arrête à la fermeture de la connexion.
pub struct PathObserver {
    rx: watch::Receiver<PathKind>,
    last: PathKind,
    task: JoinHandle<()>,
}

impl PathObserver {
    pub fn spawn(conn: Connection) -> Self {
        let initial = kind_of(&conn.paths());
        let (tx, rx) = watch::channel(initial);
        let task = tokio::spawn(async move {
            let mut stream = conn.paths_stream();
            while let Some(list) = stream.next().await {
                let k = kind_of(&list);
                tx.send_if_modified(|cur| {
                    let changed = *cur != k;
                    *cur = k;
                    changed
                });
            }
        });
        Self { rx, last: initial, task }
    }

    /// Chemin actuel.
    pub fn current(&self) -> PathKind {
        *self.rx.borrow()
    }

    /// Prochaine bascule `(avant, après)` (direct→relais, relais→direct…) ; `None` quand la connexion est fermée.
    pub async fn changed(&mut self) -> Option<(PathKind, PathKind)> {
        loop {
            self.rx.changed().await.ok()?;
            let now = *self.rx.borrow_and_update();
            if now != self.last {
                let before = std::mem::replace(&mut self.last, now);
                return Some((before, now));
            }
        }
    }

    /// Récepteur indépendant (clonable) pour l'UI.
    pub fn subscribe(&self) -> watch::Receiver<PathKind> {
        self.rx.clone()
    }
}

impl Drop for PathObserver {
    fn drop(&mut self) {
        self.task.abort();
    }
}

// ---------- avertissement « plus lentement » ----------

pub const SLOW_MESSAGE_FR: &str =
    "La connexion passe par un relais, le téléchargement sera plus lent. Si possible, demande à ton ami de se connecter en direct.";

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Advice {
    /// Pas d'avertissement (ou avertissement levé).
    None,
    SlowViaRelay { speed_bps: u64 },
}

impl Advice {
    /// Message français prêt à afficher.
    pub fn message(&self) -> Option<&'static str> {
        match self {
            Advice::None => Option::None,
            Advice::SlowViaRelay { .. } => Some(SLOW_MESSAGE_FR),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AdvisorConfig {
    /// Seuil de déclenchement, octets/s (2 Mo/s).
    pub threshold_bps: u64,
    /// Durée sous le seuil avant d'avertir (strictement dépassée).
    pub raise_after: Duration,
    /// Levée : débit >= seuil × `clear_ratio_pct`/100 pendant `clear_after`, ou chemin non relais.
    pub clear_ratio_pct: u64,
    pub clear_after: Duration,
    /// Fenêtre de calcul du débit.
    pub window: Duration,
}

impl Default for AdvisorConfig {
    fn default() -> Self {
        Self {
            threshold_bps: 2 * 1024 * 1024,
            raise_after: Duration::from_secs(5),
            clear_ratio_pct: 125,
            clear_after: Duration::from_secs(3),
            window: Duration::from_secs(3),
        }
    }
}

/// Décideur pur : on lui donne (instant, octets cumulés, type de chemin), il signale les transitions.
/// Un transfert à l'arrêt (aucun octet sur la fenêtre) est considéré inactif, pas lent.
#[derive(Debug)]
pub struct SlowRelayAdvisor {
    cfg: AdvisorConfig,
    samples: std::collections::VecDeque<(Duration, u64)>,
    slow_since: Option<Duration>,
    fast_since: Option<Duration>,
    raised: bool,
}

impl Default for SlowRelayAdvisor {
    fn default() -> Self {
        Self::new(AdvisorConfig::default())
    }
}

impl SlowRelayAdvisor {
    pub fn new(cfg: AdvisorConfig) -> Self {
        Self { cfg, samples: Default::default(), slow_since: None, fast_since: None, raised: false }
    }

    pub fn is_raised(&self) -> bool {
        self.raised
    }

    fn speed(&self) -> Option<u64> {
        let (t0, b0) = *self.samples.front()?;
        let (t1, b1) = *self.samples.back()?;
        let dt = t1.checked_sub(t0)?;
        if dt < Duration::from_secs(1) {
            return None; // pas assez de recul
        }
        Some((b1.saturating_sub(b0) as f64 / dt.as_secs_f64()) as u64)
    }

    /// `now` : temps monotone depuis un point de départ quelconque ; `total_bytes` : octets cumulés.
    /// Retourne `Some(..)` uniquement lors d'une transition (avertissement levé ou retiré).
    pub fn update(&mut self, now: Duration, total_bytes: u64, kind: PathKind) -> Option<Advice> {
        if kind != PathKind::Relay {
            self.samples.clear();
            self.slow_since = None;
            self.fast_since = None;
            return self.clear();
        }
        self.samples.push_back((now, total_bytes));
        while self.samples.len() > 1 && now.saturating_sub(self.samples[1].0) >= self.cfg.window {
            self.samples.pop_front();
        }
        let speed = self.speed()?;
        if speed == 0 {
            // inactif : ne déclenche rien et ne lève rien
            self.slow_since = None;
            self.fast_since = None;
            return None;
        }
        let clear_at = self.cfg.threshold_bps * self.cfg.clear_ratio_pct / 100;
        if !self.raised {
            self.fast_since = None;
            if speed < self.cfg.threshold_bps {
                let since = *self.slow_since.get_or_insert(now);
                if now.saturating_sub(since) > self.cfg.raise_after {
                    self.raised = true;
                    return Some(Advice::SlowViaRelay { speed_bps: speed });
                }
            } else {
                self.slow_since = None;
            }
        } else if speed >= clear_at {
            let since = *self.fast_since.get_or_insert(now);
            if now.saturating_sub(since) >= self.cfg.clear_after {
                self.fast_since = None;
                return self.clear();
            }
        } else {
            self.fast_since = None;
        }
        None
    }

    fn clear(&mut self) -> Option<Advice> {
        if self.raised {
            self.raised = false;
            self.slow_since = None;
            Some(Advice::None)
        } else {
            Option::None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(n: u64) -> Duration {
        Duration::from_secs(n)
    }
    const MB: u64 = 1024 * 1024;

    fn settings(mode: RelayChoice, relay_only: bool, allow_relay: bool) -> RelaySettings {
        RelaySettings { mode, relay_only, allow_relay, address_lookup: false }
    }

    // --- mapping pur ---

    #[test]
    fn plan_modes() {
        let p = plan(&settings(RelayChoice::Default, false, true)).unwrap();
        assert_eq!(p, EndpointPlan { relay_mode: RelayMode::Default, ip_transports: true });
        let p = plan(&settings(RelayChoice::Default, false, false)).unwrap();
        assert_eq!(p, EndpointPlan { relay_mode: RelayMode::Disabled, ip_transports: true });
        let p = plan(&settings(RelayChoice::Disabled, false, true)).unwrap();
        assert_eq!(p.relay_mode, RelayMode::Disabled);
        let p = plan(&settings(RelayChoice::Default, true, true)).unwrap();
        assert!(!p.ip_transports);
        let p = plan(&settings(RelayChoice::Custom("https://relay.example.org".into()), false, true)).unwrap();
        assert!(matches!(p.relay_mode, RelayMode::Custom(_)));
        // allow_relay=false l'emporte sur un relais personnalisé
        let p = plan(&settings(RelayChoice::Custom("https://relay.example.org".into()), false, false)).unwrap();
        assert_eq!(p.relay_mode, RelayMode::Disabled);
    }

    #[test]
    fn contradiction_rejected() {
        for (mode, allow) in [(RelayChoice::Default, false), (RelayChoice::Disabled, true)] {
            let e = plan(&settings(mode, true, allow)).unwrap_err().to_string();
            assert!(e.contains("contradiction"), "{e}");
        }
    }

    #[test]
    fn relay_url_validation() {
        assert!(validate_relay_url("https://relay.example.org").is_ok());
        assert!(validate_relay_url("https://relay.example.org:4433/").is_ok());
        assert!(validate_relay_url("http://127.0.0.1:3340").is_ok());
        assert!(validate_relay_url("http://localhost:3340").is_ok());
        assert!(validate_relay_url("http://[::1]:3340").is_ok());
        assert!(validate_relay_url("http://relay.example.org").is_err());
        assert!(validate_relay_url("ftp://relay.example.org").is_err());
        assert!(validate_relay_url("https://user:pw@relay.example.org").is_err());
        assert!(validate_relay_url("relay.example.org").is_err());
        assert!(validate_relay_url("").is_err());
        assert!(plan(&settings(RelayChoice::Custom("http://evil.example".into()), false, true)).is_err());
    }

    #[test]
    fn hint_from_friend_code() {
        let mk = |relay: Option<&str>| {
            friend_code::encode(&FriendCodePayload {
                public_key: [1; 32],
                epoch: 1,
                relay: relay.map(String::from),
                addrs: vec![],
            })
        };
        let h = relay_hint_from_code(&mk(Some("relay.example.org:4433"))).unwrap().unwrap();
        assert_eq!(h.host_str(), Some("relay.example.org"));
        assert_eq!(h.port(), Some(4433));
        assert!(relay_hint_from_code(&mk(None)).unwrap().is_none());
        assert!(relay_hint_from_code(&mk(Some("http://evil.example"))).unwrap().is_none());
        assert!(relay_hint_from_code("n'importe quoi").is_err());
    }

    // --- advisor ---

    /// Simule un transfert à `bps` octets/s, un échantillon par seconde, de t=from à t=to inclus.
    fn feed(a: &mut SlowRelayAdvisor, from: u64, to: u64, bps: u64, base: u64, kind: PathKind) -> Vec<(u64, Advice)> {
        let mut out = vec![];
        for t in from..=to {
            if let Some(ad) = a.update(s(t), base + bps * (t - from), kind) {
                out.push((t, ad));
            }
        }
        out
    }

    #[test]
    fn slow_relay_raises_after_5s() {
        let mut a = SlowRelayAdvisor::default();
        let ev = feed(&mut a, 0, 12, MB, 0, PathKind::Relay);
        assert_eq!(ev.len(), 1, "{ev:?}");
        let (t, ad) = ev[0];
        assert!(t > 5, "pas avant 5 s : {t}");
        match ad {
            Advice::SlowViaRelay { speed_bps } => assert!((MB - 1000..=MB + 1000).contains(&speed_bps)),
            other => panic!("{other:?}"),
        }
        assert_eq!(ad.message(), Some(SLOW_MESSAGE_FR));
        assert!(SLOW_MESSAGE_FR.contains("plus lent"));
        assert!(!SLOW_MESSAGE_FR.to_lowercase().contains("ne voit rien"));
    }

    #[test]
    fn no_warning_when_fast_or_direct_or_idle() {
        let mut a = SlowRelayAdvisor::default();
        assert!(feed(&mut a, 0, 30, 10 * MB, 0, PathKind::Relay).is_empty());
        let mut a = SlowRelayAdvisor::default();
        assert!(feed(&mut a, 0, 30, MB / 10, 0, PathKind::Direct).is_empty());
        let mut a = SlowRelayAdvisor::default();
        assert!(feed(&mut a, 0, 30, MB / 10, 0, PathKind::Unknown).is_empty());
        let mut a = SlowRelayAdvisor::default();
        assert!(feed(&mut a, 0, 30, 0, 5 * MB, PathKind::Relay).is_empty(), "inactif != lent");
    }

    #[test]
    fn short_dip_does_not_trigger() {
        let mut a = SlowRelayAdvisor::default();
        assert!(feed(&mut a, 0, 3, MB, 0, PathKind::Relay).is_empty());
        assert!(feed(&mut a, 4, 30, 8 * MB, 3 * MB, PathKind::Relay).is_empty());
    }

    #[test]
    fn hysteresis_no_flicker() {
        let mut a = SlowRelayAdvisor::default();
        assert_eq!(feed(&mut a, 0, 10, MB, 0, PathKind::Relay).len(), 1);
        // autour du seuil (2,2 Mo/s < 2,5 Mo/s) : l'avertissement reste, aucun événement
        assert!(feed(&mut a, 11, 40, 2 * MB + MB / 5, 11 * MB, PathKind::Relay).is_empty());
        assert!(a.is_raised());
        // nettement plus rapide et durablement : levé une seule fois
        let ev = feed(&mut a, 41, 60, 6 * MB, 100 * MB, PathKind::Relay);
        assert_eq!(ev.len(), 1, "{ev:?}");
        assert_eq!(ev[0].1, Advice::None);
        assert_eq!(Advice::None.message(), None);
        assert!(!a.is_raised());
    }

    #[test]
    fn switching_to_direct_clears() {
        let mut a = SlowRelayAdvisor::default();
        assert_eq!(feed(&mut a, 0, 10, MB, 0, PathKind::Relay).len(), 1);
        assert_eq!(a.update(s(11), 11 * MB, PathKind::Direct), Some(Advice::None));
        assert_eq!(a.update(s(12), 12 * MB, PathKind::Direct), None);
    }

    // --- réseau (loopback) ---

    async fn wait_kind(conn: &Connection) -> PathKind {
        for _ in 0..50 {
            let k = kind_of(&conn.paths());
            if k != PathKind::Unknown {
                return k;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        PathKind::Unknown
    }

    const TEST_ALPN: &[u8] = b"cubelink/relay-test/0";

    #[tokio::test]
    async fn direct_only_connects_direct() {
        let direct = settings(RelayChoice::Default, false, false);
        let a = build_endpoint(SecretKey::generate(), &direct, vec![TEST_ALPN.to_vec()]).await.unwrap();
        let b = build_endpoint(SecretKey::generate(), &direct, vec![]).await.unwrap();
        let a2 = a.clone();
        let acc = tokio::spawn(async move {
            let inc = a2.accept().await.unwrap();
            let c = inc.await.unwrap();
            c.closed().await;
        });
        let conn = b.connect(a.addr(), TEST_ALPN).await.unwrap();
        assert_eq!(wait_kind(&conn).await, PathKind::Direct);
        let mut obs = PathObserver::spawn(conn.clone());
        assert_eq!(obs.current(), PathKind::Direct);
        conn.close(0u32.into(), b"done");
        // la connexion fermée termine l'observateur sans bascule
        assert!(tokio::time::timeout(Duration::from_secs(5), obs.changed()).await.unwrap().is_none());
        let _ = acc.await;
        b.close().await;
        a.close().await;
    }

    #[tokio::test]
    async fn contradiction_and_bad_url_fail_in_build() {
        let k = || SecretKey::generate();
        let e = build_endpoint(k(), &settings(RelayChoice::Default, true, false), vec![]).await.unwrap_err();
        assert!(e.to_string().contains("contradiction"), "{e}");
        for url in ["http://relay.example.org", "pas une url", "https://u:p@relay.example.org"] {
            assert!(build_endpoint(k(), &settings(RelayChoice::Custom(url.into()), false, true), vec![]).await.is_err());
        }
    }

    /// Relais seul. Le crate n'active pas la feature `test-utils` d'iroh (Cargo.toml hors périmètre N17),
    /// donc pas de relais local de test ici : ce test utilise les relais publics n0 (Internet requis).
    /// À lancer à la main : `cargo test -p cubelink-p2p relay_only -- --ignored`.
    /// Avec la feature : remplacer par `iroh::test_utils::run_relay_server()` + `RelayChoice::Custom(url)`.
    #[tokio::test]
    #[ignore = "Internet requis (relais publics n0)"]
    async fn relay_only_uses_relay_path() {
        let ro = settings(RelayChoice::Default, true, true);
        let a = build_endpoint(SecretKey::generate(), &ro, vec![TEST_ALPN.to_vec()]).await.unwrap();
        let b = build_endpoint(SecretKey::generate(), &ro, vec![]).await.unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(15), a.online()).await;
        let a2 = a.clone();
        tokio::spawn(async move {
            if let Some(inc) = a2.accept().await {
                if let Ok(c) = inc.await {
                    c.closed().await;
                }
            }
        });
        let conn = b.connect(a.addr(), TEST_ALPN).await.unwrap();
        assert_eq!(wait_kind(&conn).await, PathKind::Relay);
        conn.close(0u32.into(), b"done");
    }

    #[test]
    fn address_lookup_is_off_by_default() {
        assert!(!RelaySettings::default().address_lookup);
    }

    #[test]
    fn endpoint_addr_info_roundtrip() {
        let id = SecretKey::from_bytes(&[7u8; 32]).public();
        let addr = iroh::EndpointAddr::new(id)
            .with_relay_url("https://relay.example.org".parse().unwrap())
            .with_ip_addr("127.0.0.1:4000".parse().unwrap())
            .with_ip_addr("192.168.1.5:4001".parse().unwrap());
        let info = endpoint_addr_info(&addr);
        assert_eq!(info.addrs, ["192.168.1.5:4001", "127.0.0.1:4000"], "non-loopback d'abord");
        assert!(info.relay.as_deref().unwrap().contains("relay.example.org"));
        let payload = info.into_payload(*id.as_bytes(), 0);
        let parsed = friend_code::parse(&friend_code::encode(&payload)).unwrap();
        let back = endpoint_addr_from_payload(&parsed).unwrap();
        assert_eq!(back.id, id);
        assert_eq!(back.ip_addrs().count(), 2);
        assert_eq!(back.relay_urls().count(), 1);
    }
}
