//! Endpoint iroh sans aucun tiers (#55) et adresses du code ami.
//!
//! - [`build_endpoint`] : crée l'endpoint **sans relais et sans service de découverte d'adresses** (ni relais
//!   publics n0, ni pkarr, ni DNS n0) : il ne contacte aucun serveur tiers et ne publie rien. Un pair n'est joignable
//!   que par les indices de son code ami (adresses IP : LAN, IPv6, adresse ouverte sur la box par UPnP/NAT-PMP/PCP,
//!   voir `portmap`) et par les adresses mémorisées. Deux amis derrière un NAT strict ou un CGNAT, sans IPv6 ni UPnP,
//!   peuvent donc ne pas se joindre, en attendant la découverte par DHT Mainline (#136) et des relais Cubelink.
//! - [`endpoint_addr_info`] : les adresses IP du code ami, les plus joignables d'abord ([`AddrClass`]).
//!
//! Les indices de relais d'un code ami ou d'une adresse mémorisée sont ignorés : un ancien code peut encore porter un
//! relais n0, et on ne contacte jamais un relais que l'utilisateur n'a pas choisi.

use std::net::{IpAddr, SocketAddr};

use core_lib::types::{Error, Result};
use iroh::endpoint::presets;
use iroh::{Endpoint, RelayMode, SecretKey};

use crate::friend_code::{self, FriendCodePayload};

async fn bind(secret: SecretKey, alpns: Vec<Vec<u8>>, ports: Option<(u16, bool)>) -> Result<Endpoint> {
    let mut b = Endpoint::builder(presets::Minimal).secret_key(secret).relay_mode(RelayMode::Disabled).alpns(alpns);
    if let Some((port, v6)) = ports {
        let bad = |e: &dyn std::fmt::Display| Error::Network(format!("adresse de liaison : {e}"));
        b = b.bind_addr(format!("0.0.0.0:{port}")).map_err(|e| bad(&e))?;
        if v6 {
            // Port voisin : sur certains systèmes `[::]:p` entre en conflit avec `0.0.0.0:p` (double pile).
            b = b.bind_addr(format!("[::]:{}", port.wrapping_add(1))).map_err(|e| bad(&e))?;
        }
    }
    b.bind().await.map_err(|e| Error::Network(format!("création de l'endpoint : {e}")))
}

/// Crée l'endpoint iroh : aucun relais, aucune découverte d'adresses (voir l'en-tête du module). `secret` est la clé
/// secrète de l'identité. `port` : port UDP fixe voulu (IPv4 ; IPv6 sur le port suivant), pour que les adresses mémorisées par les amis
/// ne se périment pas à chaque démarrage ; occupé ou refusé (pas d'IPv6 : IPv4 seul d'abord), on se replie sur un port
/// libre. `None` ou `0` : port libre.
pub async fn build_endpoint(secret: SecretKey, alpns: Vec<Vec<u8>>, port: Option<u16>) -> Result<Endpoint> {
    if let Some(p) = port.filter(|p| *p != 0) {
        // IPv6 seulement si le système en a (sinon la liaison explicite échoue), et une pause entre deux essais : une
        // liaison ratée libère ses sockets de façon asynchrone.
        let attempts: &[bool] = if std::net::UdpSocket::bind("[::]:0").is_ok() { &[true, false] } else { &[false] };
        for (i, &v6) in attempts.iter().enumerate() {
            if i > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            }
            match bind(secret.clone(), alpns.clone(), Some((p, v6))).await {
                Ok(ep) => return Ok(ep),
                Err(e) => tracing::warn!(port = p, v6, "port UDP fixe indisponible : {e}"),
            }
        }
    }
    bind(secret, alpns, None).await
}

/// Plage des ports UDP fixes tirés au premier lancement : sous les plages éphémères de Linux (32768+) et de Windows
/// (49152+), donc rarement prise par une connexion sortante, et loin de 25565 (Minecraft).
const PORT_MIN: u16 = 30000;
const PORT_SPAN: u16 = 2700;

/// Pur : port UDP tiré d'octets aléatoires, dans la plage ci-dessus.
pub fn port_from_entropy(b: [u8; 2]) -> u16 {
    PORT_MIN + u16::from_le_bytes(b) % PORT_SPAN
}

/// Port UDP fixe de l'appareil : relu dans `dir/udp_port`, sinon tiré au hasard et enregistré (premier lancement). Un
/// contenu illisible ou hors plage est remplacé. Le port ne change donc plus à chaque démarrage : les adresses que les
/// amis ont mémorisées (et celle ouverte sur la box) restent valables.
pub fn load_or_pick_port(dir: &std::path::Path) -> u16 {
    let file = dir.join("udp_port");
    let saved = std::fs::read_to_string(&file).ok().and_then(|t| t.trim().parse::<u16>().ok());
    if let Some(p) = saved.filter(|p| (PORT_MIN..PORT_MIN + PORT_SPAN).contains(p)) {
        return p;
    }
    let k = SecretKey::generate().to_bytes();
    let p = port_from_entropy([k[0], k[1]]);
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(&file, p.to_string());
    p
}

// ---------- code ami ----------

/// Rang d'une adresse IP pour le code ami (limité à `MAX_ADDRS` adresses) : les plus joignables de l'extérieur d'abord.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AddrClass {
    /// Adresse ouverte sur la box (UPnP / NAT-PMP / PCP).
    Mapped,
    /// IPv6 globale.
    GlobalV6,
    /// IPv4 publique (joignable directement si rien ne la filtre).
    PublicV4,
    /// Réseau local : IPv4 privée, CGNAT (100.64/10), IPv6 ULA.
    Private,
    /// Lien local (169.254/16, fe80::/10).
    LinkLocal,
    /// Loopback, non spécifiée, multicast : inutile hors de cette machine.
    Loopback,
}

/// Classe une adresse IP (jamais [`AddrClass::Mapped`] : cela dépend de l'endpoint).
pub fn classify(ip: IpAddr) -> AddrClass {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            if v4.is_loopback() || v4.is_unspecified() || v4.is_multicast() || v4.is_broadcast() {
                AddrClass::Loopback
            } else if v4.is_link_local() {
                AddrClass::LinkLocal
            } else if v4.is_private() || (o[0] == 100 && (64..128).contains(&o[1])) || o[0] == 0 {
                AddrClass::Private
            } else {
                AddrClass::PublicV4
            }
        }
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            if v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() {
                AddrClass::Loopback
            } else if first & 0xffc0 == 0xfe80 {
                AddrClass::LinkLocal
            } else if first & 0xfe00 == 0xfc00 {
                AddrClass::Private
            } else {
                AddrClass::GlobalV6
            }
        }
    }
}

/// Une adresse joignable de l'extérieur sans NAT : IPv6 globale ou IPv4 publique.
pub fn is_public(ip: IpAddr) -> bool {
    matches!(classify(ip), AddrClass::GlobalV6 | AddrClass::PublicV4)
}

/// Adresses IP d'un endpoint, prêtes à mettre dans un code ami ([`FriendCodePayload`]) : bornées aux limites du
/// format (chaînes <= `MAX_STR_LEN`, au plus `MAX_ADDRS` adresses). Jamais de relais : le code n'en porte plus.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EndpointAddrInfo {
    /// Adresses IP `ip:port`, les plus joignables d'abord (voir [`AddrClass`]).
    pub addrs: Vec<String>,
}

impl EndpointAddrInfo {
    /// Construit le payload de code ami (à passer à `friend_code::encode`).
    pub fn into_payload(self, public_key: [u8; 32], epoch: u32) -> FriendCodePayload {
        FriendCodePayload { public_key, epoch, relay: None, addrs: self.addrs }
    }
}

/// Pur : choisit et ordonne les adresses du code. Ordre : adresse ouverte sur la box (`mapped`), IPv6 globale, IPv4
/// publique, puis réseau local, lien local, loopback. Sans ce tri, la coupe à `MAX_ADDRS` pourrait écarter l'adresse
/// qui compte au profit d'IPv4 privées. À rang égal, l'ordre d'origine est gardé.
pub fn pick_addrs(ips: &[SocketAddr], mapped: Option<SocketAddr>) -> Vec<String> {
    let ok = |s: &str| s.len() <= friend_code::MAX_STR_LEN;
    let mut all: Vec<SocketAddr> = ips.to_vec();
    if let Some(m) = mapped {
        if !all.contains(&m) {
            all.push(m);
        }
    }
    let rank = |a: &SocketAddr| if Some(*a) == mapped { AddrClass::Mapped } else { classify(a.ip()) };
    all.sort_by_key(rank);
    all.iter().map(ToString::to_string).filter(|s| ok(s)).take(friend_code::MAX_ADDRS).collect()
}

/// Extrait les adresses IP d'une `EndpointAddr` (typiquement `endpoint.addr()`) pour le code ami ; `mapped` : adresse
/// ouverte sur la box par le portmapper, si elle existe. Ce que cela révèle : tes adresses IP à quiconque reçoit le
/// code (un ami).
pub fn endpoint_addr_info(addr: &iroh::EndpointAddr, mapped: Option<SocketAddr>) -> EndpointAddrInfo {
    let ips: Vec<SocketAddr> = addr.ip_addrs().copied().collect();
    EndpointAddrInfo { addrs: pick_addrs(&ips, mapped) }
}

/// Inverse : `EndpointAddr` joignable depuis un code ami. Le relais éventuel du code est ignoré (voir l'en-tête) ;
/// une adresse illisible aussi.
pub fn endpoint_addr_from_payload(payload: &FriendCodePayload) -> Result<iroh::EndpointAddr> {
    let id = iroh::EndpointId::from_bytes(&payload.public_key)
        .map_err(|e| Error::Invalid(format!("clé publique : {e}")))?;
    let mut addr = iroh::EndpointAddr::new(id);
    for a in &payload.addrs {
        if let Ok(sa) = a.parse::<SocketAddr>() {
            addr = addr.with_ip_addr(sa);
        }
    }
    Ok(addr)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn a(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    // --- réseau (loopback) ---

    /// Attend un chemin sélectionné ; vrai s'il est direct (pas de relais).
    async fn selected_is_direct(conn: &iroh::endpoint::Connection) -> bool {
        for _ in 0..50 {
            if let Some(direct) = conn.paths().iter().find(|p| p.is_selected()).map(|p| !p.is_relay()) {
                return direct;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        false
    }

    const TEST_ALPN: &[u8] = b"cubelink/relay-test/0";

    /// Non-régression #55 : l'endpoint n'a ni relais (aucun relais n0, aucun relais du tout) ni service de
    /// découverte (n0, pkarr, DNS) : il ne publie rien, n'interroge aucun annuaire et ne contacte aucun relais.
    #[tokio::test]
    async fn endpoint_has_no_relay_and_no_address_lookup() {
        let ep = build_endpoint(SecretKey::generate(), vec![], None).await.unwrap();
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
        let a = build_endpoint(SecretKey::generate(), vec![TEST_ALPN.to_vec()], None).await.unwrap();
        let b = build_endpoint(SecretKey::generate(), vec![], None).await.unwrap();
        let a2 = a.clone();
        let acc = tokio::spawn(async move {
            let c = a2.accept().await.unwrap().await.unwrap();
            c.closed().await;
        });
        let payload = endpoint_addr_info(&a.addr(), None).into_payload(*a.id().as_bytes(), 0);
        assert!(!payload.addrs.is_empty(), "le code doit porter les IP");
        assert!(payload.relay.is_none(), "le code ne porte plus de relais");
        let target = endpoint_addr_from_payload(&friend_code::parse(&friend_code::encode(&payload)).unwrap()).unwrap();
        let conn = tokio::time::timeout(Duration::from_secs(15), b.connect(target, TEST_ALPN)).await.unwrap().unwrap();
        assert!(selected_is_direct(&conn).await, "chemin direct, sans relais");
        conn.close(0u32.into(), b"done");
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
        let info = endpoint_addr_info(&addr, None);
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

    #[test]
    fn classes_d_adresses() {
        use AddrClass::*;
        let c = |s: &str| classify(s.parse().unwrap());
        assert_eq!(c("2a01:e0a::1"), GlobalV6);
        assert_eq!(c("8.8.8.8"), PublicV4);
        for p in ["10.1.2.3", "172.16.0.1", "192.168.1.2", "100.64.0.1", "100.127.255.1", "fd12:3456::1"] {
            assert_eq!(c(p), Private, "{p}");
        }
        assert_eq!(c("100.128.0.1"), PublicV4, "hors du bloc CGNAT");
        assert_eq!(c("169.254.1.1"), LinkLocal);
        assert_eq!(c("fe80::1"), LinkLocal);
        for l in ["127.0.0.1", "::1", "0.0.0.0", "::", "ff02::1", "224.0.0.1"] {
            assert_eq!(c(l), Loopback, "{l}");
        }
        assert!(Mapped < GlobalV6 && GlobalV6 < PublicV4 && PublicV4 < Private && Private < LinkLocal && LinkLocal < Loopback);
        assert!(is_public("2001:db8::1".parse().unwrap()) && is_public("93.184.216.34".parse().unwrap()));
        assert!(!is_public("192.168.1.2".parse().unwrap()) && !is_public("fe80::1".parse().unwrap()));
    }

    /// 10 adresses mélangées : la coupe à 8 garde l'adresse ouverte sur la box, l'IPv6 globale et l'IPv4 publique, et
    /// écarte d'abord les loopback, liens locaux et privées.
    #[test]
    fn code_ami_garde_les_adresses_publiques() {
        let ips = [
            a("127.0.0.1:4000"),
            a("[::1]:4000"),
            a("192.168.1.5:4000"),
            a("10.0.0.5:4000"),
            a("172.16.0.5:4000"),
            a("[fe80::1]:4000"),
            a("169.254.3.4:4000"),
            a("[fd00::5]:4000"),
            a("[2a01:e0a:1:2::3]:4000"),
            a("93.184.216.34:4000"),
        ];
        let mapped = a("203.0.113.7:51820");
        let picked = pick_addrs(&ips, Some(mapped));
        assert_eq!(picked.len(), friend_code::MAX_ADDRS);
        assert_eq!(&picked[..3], ["203.0.113.7:51820", "[2a01:e0a:1:2::3]:4000", "93.184.216.34:4000"], "{picked:?}");
        assert!(!picked.contains(&"127.0.0.1:4000".to_string()), "{picked:?}");
        assert!(!picked.contains(&"[::1]:4000".to_string()), "{picked:?}");
        // Sans adresse mappée connue : IPv6 globale puis IPv4 publique, toujours devant les privées.
        let picked = pick_addrs(&ips, None);
        assert_eq!(&picked[..2], ["[2a01:e0a:1:2::3]:4000", "93.184.216.34:4000"]);
        // L'adresse mappée absente de la liste de l'endpoint y est ajoutée.
        assert_eq!(pick_addrs(&[a("192.168.1.5:1")], Some(mapped)), ["203.0.113.7:51820", "192.168.1.5:1"]);
    }

    #[test]
    fn port_fixe_persiste_et_dans_la_plage() {
        for b in [[0, 0], [255, 255], [1, 2]] {
            let p = port_from_entropy(b);
            assert!((PORT_MIN..PORT_MIN + PORT_SPAN).contains(&p), "{p}");
        }
        let dir = tempfile::tempdir().unwrap();
        let first = load_or_pick_port(dir.path());
        assert_eq!(load_or_pick_port(dir.path()), first, "le même port au démarrage suivant");
        assert_eq!(std::fs::read_to_string(dir.path().join("udp_port")).unwrap(), first.to_string());
        // Fichier illisible ou hors plage : remplacé par un port valide, puis stable.
        std::fs::write(dir.path().join("udp_port"), "80").unwrap();
        let fixed = load_or_pick_port(dir.path());
        assert!((PORT_MIN..PORT_MIN + PORT_SPAN).contains(&fixed));
        assert_eq!(load_or_pick_port(dir.path()), fixed);
    }

    /// Port fixe : le même port est repris au redémarrage ; occupé, repli sur un port libre sans échec.
    #[tokio::test]
    async fn port_fixe_puis_repli_si_occupe() {
        let k = SecretKey::generate().to_bytes();
        let free = port_from_entropy([k[0], k[1]]);
        let ep = build_endpoint(SecretKey::generate(), vec![], Some(free)).await.unwrap();
        let v4 = ep.bound_sockets().into_iter().find(|s| s.is_ipv4()).unwrap();
        assert_eq!(v4.port(), free, "le port demandé est utilisé");
        // Même port encore occupé par `ep` : repli sur un autre port libre.
        let other = build_endpoint(SecretKey::generate(), vec![], Some(free)).await.unwrap();
        let v4b = other.bound_sockets().into_iter().find(|s| s.is_ipv4()).unwrap();
        assert_ne!(v4b.port(), free, "port occupé : repli sur un port libre");
        other.close().await;
        ep.close().await;
    }
}
