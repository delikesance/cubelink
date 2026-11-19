//! Endpoint iroh sans aucun tiers (#55) et observation du chemin d'une connexion.
//!
//! - [`build_endpoint`] : crée l'endpoint **sans relais et sans service de découverte d'adresses** (ni relais
//!   publics n0, ni pkarr, ni DNS n0) : il ne contacte aucun serveur tiers et ne publie rien. Un pair n'est joignable
//!   que par les indices de son code ami (adresses IP : LAN, IPv6, adresse ouverte sur la box par UPnP/NAT-PMP/PCP,
//!   voir `portmap`) et par les adresses mémorisées. Deux amis derrière un NAT strict ou un CGNAT, sans IPv6 ni UPnP,
//!   peuvent donc ne pas se joindre, en attendant la découverte par DHT Mainline (#136) et des relais Cubelink.
//! - [`PathObserver`] : expose le type de chemin d'une connexion (direct / relais) et ses bascules.
//! - [`SlowRelayAdvisor`] : décide d'afficher « plus lentement » (fonction pure, temps injecté).
//!
//! Les indices de relais d'un code ami ou d'une adresse mémorisée sont ignorés : un ancien code peut encore porter un
//! relais n0, et on ne contacte jamais un relais que l'utilisateur n'a pas choisi.

use std::time::Duration;

use core_lib::types::{Error, Result};
use iroh::endpoint::{presets, Connection, PathList};
use iroh::{Endpoint, RelayMode, SecretKey};
use n0_future::StreamExt;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::friend_code::{self, FriendCodePayload};

/// Crée l'endpoint iroh : aucun relais, aucune découverte d'adresses (voir l'en-tête du module). `secret` est la clé
/// secrète de l'identité.
pub async fn build_endpoint(secret: SecretKey, alpns: Vec<Vec<u8>>) -> Result<Endpoint> {
    Endpoint::builder(presets::Minimal)
        .secret_key(secret)
        .relay_mode(RelayMode::Disabled)
        .alpns(alpns)
        .bind()
        .await
        .map_err(|e| Error::Network(format!("création de l'endpoint : {e}")))
}

// ---------- code ami ----------

/// Adresses IP d'un endpoint, prêtes à mettre dans un code ami ([`FriendCodePayload`]) : bornées aux limites du
/// format (chaînes <= `MAX_STR_LEN`, au plus `MAX_ADDRS` adresses). Jamais de relais : le code n'en porte plus.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EndpointAddrInfo {
    /// Adresses IP `ip:port`, les non-loopback d'abord.
    pub addrs: Vec<String>,
}

impl EndpointAddrInfo {
    /// Construit le payload de code ami (à passer à `friend_code::encode`).
    pub fn into_payload(self, public_key: [u8; 32], epoch: u32) -> FriendCodePayload {
        FriendCodePayload { public_key, epoch, relay: None, addrs: self.addrs }
    }
}

/// Extrait les adresses IP d'une `EndpointAddr` (typiquement `endpoint.addr()`) pour le code ami. Ce que cela
/// révèle : tes adresses IP à quiconque reçoit le code (un ami).
pub fn endpoint_addr_info(addr: &iroh::EndpointAddr) -> EndpointAddrInfo {
    let ok = |s: &str| s.len() <= friend_code::MAX_STR_LEN;
    let mut ips: Vec<std::net::SocketAddr> = addr.ip_addrs().copied().collect();
    ips.sort_by_key(|a| (a.ip().is_loopback() || a.ip().is_unspecified(), a.is_ipv6()));
    let addrs = ips.iter().map(|a| a.to_string()).filter(|s| ok(s)).take(friend_code::MAX_ADDRS).collect();
    EndpointAddrInfo { addrs }
}

/// Inverse : `EndpointAddr` joignable depuis un code ami. Le relais éventuel du code est ignoré (voir l'en-tête) ;
/// une adresse illisible aussi.
pub fn endpoint_addr_from_payload(payload: &FriendCodePayload) -> Result<iroh::EndpointAddr> {
    let id = iroh::EndpointId::from_bytes(&payload.public_key)
        .map_err(|e| Error::Invalid(format!("clé publique : {e}")))?;
    let mut addr = iroh::EndpointAddr::new(id);
    for a in &payload.addrs {
        if let Ok(sa) = a.parse::<std::net::SocketAddr>() {
            addr = addr.with_ip_addr(sa);
        }
    }
    Ok(addr)
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

    /// Non-régression #55 : l'endpoint n'a ni relais (aucun relais n0, aucun relais du tout) ni service de
    /// découverte (n0, pkarr, DNS) : il ne publie rien, n'interroge aucun annuaire et ne contacte aucun relais.
    #[tokio::test]
    async fn endpoint_has_no_relay_and_no_address_lookup() {
        let ep = build_endpoint(SecretKey::generate(), vec![]).await.unwrap();
        use iroh::Watcher as _;
        assert!(ep.home_relay_status().get().is_empty(), "aucun relais attendu");
        assert!(ep.addr().relay_urls().next().is_none(), "aucun relais d'attache");
        assert!(ep.address_lookup().unwrap().is_empty(), "aucun service de découverte attendu");
        ep.close().await;
    }

    /// Connexion directe loopback : sans relais ni annuaire, un pair se joint par les seuls indices de son code ami
    /// (adresses IP), et le chemin est direct.
    #[tokio::test]
    async fn friend_code_hints_alone_connect_directly() {
        let a = build_endpoint(SecretKey::generate(), vec![TEST_ALPN.to_vec()]).await.unwrap();
        let b = build_endpoint(SecretKey::generate(), vec![]).await.unwrap();
        let a2 = a.clone();
        let acc = tokio::spawn(async move {
            let c = a2.accept().await.unwrap().await.unwrap();
            c.closed().await;
        });
        let payload = endpoint_addr_info(&a.addr()).into_payload(*a.id().as_bytes(), 0);
        assert!(!payload.addrs.is_empty(), "le code doit porter les IP");
        assert!(payload.relay.is_none(), "le code ne porte plus de relais");
        let target = endpoint_addr_from_payload(&friend_code::parse(&friend_code::encode(&payload)).unwrap()).unwrap();
        let conn = tokio::time::timeout(Duration::from_secs(15), b.connect(target, TEST_ALPN)).await.unwrap().unwrap();
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

    #[test]
    fn endpoint_addr_info_roundtrip() {
        let id = SecretKey::from_bytes(&[7u8; 32]).public();
        let addr = iroh::EndpointAddr::new(id)
            .with_relay_url("https://relay.example.org".parse().unwrap())
            .with_ip_addr("127.0.0.1:4000".parse().unwrap())
            .with_ip_addr("192.168.1.5:4001".parse().unwrap());
        let info = endpoint_addr_info(&addr);
        assert_eq!(info.addrs, ["192.168.1.5:4001", "127.0.0.1:4000"], "non-loopback d'abord");
        let payload = info.into_payload(*id.as_bytes(), 0);
        assert!(payload.relay.is_none(), "un relais de l'adresse n'entre jamais dans le code");
        let parsed = friend_code::parse(&friend_code::encode(&payload)).unwrap();
        let back = endpoint_addr_from_payload(&parsed).unwrap();
        assert_eq!(back.id, id);
        assert_eq!(back.ip_addrs().count(), 2);
        assert_eq!(back.relay_urls().count(), 0);
    }

    /// Un ancien code ami porte le relais n0 de son émetteur : il est ignoré, seules les IP servent.
    #[test]
    fn old_friend_code_relay_is_ignored() {
        let payload = FriendCodePayload {
            public_key: [3u8; 32],
            epoch: 1,
            relay: Some("euc1-1.relay.n0.iroh.link".into()),
            addrs: vec!["192.168.1.5:4001".into()],
        };
        let parsed = friend_code::parse(&friend_code::encode(&payload)).unwrap();
        assert_eq!(parsed.relay.as_deref(), Some("euc1-1.relay.n0.iroh.link"), "le format reste lisible");
        let addr = endpoint_addr_from_payload(&parsed).unwrap();
        assert_eq!(addr.relay_urls().count(), 0);
        assert_eq!(addr.ip_addrs().count(), 1);
    }
}
