//! Ouverture automatique d'un port sur la box (UPnP, NAT-PMP, PCP) quand Iroh ne le fait pas lui-même.
//!
//! Iroh ouvre déjà ce port quand un relais est configuré. Sans relais (`RelayMode::Disabled`), il saute toute sa
//! découverte réseau, mappage de port compris : un pair derrière une box n'annonce alors que ses adresses locales et
//! n'est joignable qu'en réseau local ou en IPv6. [`DirectPortmap`] fait ce mappage à sa place et annonce l'adresse
//! publique obtenue (`Endpoint::add_external_addr`), qui part ensuite dans le code ami et les adresses connues des amis.
//!
//! Aucune action de l'utilisateur : une box sans UPnP/NAT-PMP/PCP (ou derrière un CGNAT) ne donne simplement rien.

use std::net::{SocketAddr, SocketAddrV4};
use std::num::NonZeroU16;
use std::time::Duration;

use iroh::Endpoint;
use tokio::task::JoinHandle;

/// Nouvelle demande de mappage (sans effet s'il existe déjà) : rattrape une box redémarrée ou un bail perdu.
const REFRESH: Duration = Duration::from_secs(60);

/// Pur : port UDP local à faire ouvrir, celui de la socket IPv4 de l'endpoint (`None` si aucune).
pub fn local_v4_port(bound: &[SocketAddr]) -> Option<NonZeroU16> {
    bound.iter().find(|a| a.is_ipv4()).and_then(|a| NonZeroU16::new(a.port()))
}

/// Mappage tenu tant que la valeur vit ; le relâcher arrête la tâche et le service (le bail expire seul sur la box).
pub struct DirectPortmap {
    client: portmapper::Client,
    task: JoinHandle<()>,
}

impl DirectPortmap {
    /// Démarre le mappage UDP du port de `endpoint`. `None` si l'endpoint n'a pas de socket IPv4.
    pub fn spawn(endpoint: Endpoint) -> Option<Self> {
        let port = local_v4_port(&endpoint.bound_sockets())?;
        let client = portmapper::Client::new(portmapper::Config::default());
        client.update_local_port(port);
        client.procure_mapping();
        let mut rx = client.watch_external_address();
        let refresh = client.clone();
        let task = tokio::spawn(async move {
            let mut current: Option<SocketAddrV4> = None;
            let mut tick = tokio::time::interval(REFRESH);
            tick.tick().await;
            loop {
                tokio::select! {
                    changed = rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        let next = *rx.borrow_and_update();
                        if next == current {
                            continue;
                        }
                        if let Some(old) = current.take() {
                            endpoint.remove_external_addr(&SocketAddr::V4(old)).await;
                        }
                        if let Some(new) = next {
                            tracing::info!(%new, "port ouvert sur la box (UPnP / NAT-PMP / PCP)");
                            endpoint.add_external_addr(SocketAddr::V4(new)).await;
                        }
                        current = next;
                    }
                    _ = tick.tick() => refresh.procure_mapping(),
                }
            }
        });
        Some(Self { client, task })
    }

    /// Adresse publique obtenue, si la box a accepté.
    pub fn external(&self) -> Option<SocketAddrV4> {
        *self.client.watch_external_address().borrow()
    }
}

impl Drop for DirectPortmap {
    fn drop(&mut self) {
        self.task.abort();
        self.client.deactivate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_local_ipv4() {
        let v6: SocketAddr = "[::]:4000".parse().unwrap();
        let v4: SocketAddr = "0.0.0.0:5000".parse().unwrap();
        assert_eq!(local_v4_port(&[v6, v4]), NonZeroU16::new(5000));
        assert_eq!(local_v4_port(&[v6]), None);
        assert_eq!(local_v4_port(&["0.0.0.0:0".parse().unwrap()]), None);
        assert_eq!(local_v4_port(&[]), None);
    }

    /// Sans box UPnP (CI, conteneur) : le service démarre, ne trouve rien, et l'endpoint reste utilisable.
    #[tokio::test]
    async fn sans_passerelle_ne_bloque_rien() {
        let ep = Endpoint::builder(iroh::endpoint::presets::Minimal)
            .relay_mode(iroh::RelayMode::Disabled)
            .bind()
            .await
            .unwrap();
        let pm = DirectPortmap::spawn(ep.clone()).expect("socket IPv4");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(ep.addr().ip_addrs().count() > 0);
        drop(pm);
        ep.close().await;
    }
}
