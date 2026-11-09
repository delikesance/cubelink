//! Canal de manifeste (ALPN `cubelink/manifest/1`) : l'invité demande à l'hôte le manifeste validé
//! de l'instance partagée.
//!
//! Protocole : l'invité ouvre un flux bidirectionnel et écrit 1 octet (`b'M'`) ; l'hôte répond
//! le manifeste en JSON puis termine son flux. Tout refus (pair non autorisé, rien de partagé,
//! requête inconnue) = fermeture de connexion avec un code, sans donnée.
//!
//! Sécurité : l'hôte ne sert QUE un manifeste qui a passé `validate_manifest` (à `set`) et
//! seulement aux pairs autorisés ; la taille est bornée des deux côtés (`MAX_MANIFEST_BYTES`) ;
//! l'invité revalide le manifeste reçu (l'hôte n'est pas de confiance).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use core_lib::manifest::validate_manifest;
use core_lib::types::{Error, Manifest, Result};
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::{Endpoint, EndpointAddr};
use tokio::time::timeout;

use crate::transfer::send::PeerAuthorizer;

/// ALPN du canal de manifeste.
pub const MANIFEST_ALPN: &[u8] = b"cubelink/manifest/1";
/// Taille maximale d'un manifeste sérialisé (octets).
pub const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;
const REQUEST: u8 = b'M';
const CLOSE_DENIED: u32 = 1;
const CLOSE_BAD_REQUEST: u32 = 2;
/// Délai d'une requête (connexion + réponse), et durée pendant laquelle l'hôte attend l'invité.
const NET_TIMEOUT: Duration = Duration::from_secs(15);

struct Inner {
    authorizer: Arc<dyn PeerAuthorizer>,
    /// Manifeste sérialisé de l'instance partagée (`None` : rien n'est partagé).
    current: Mutex<Option<Arc<Vec<u8>>>>,
}

/// Côté hôte : détient le manifeste partagé et fournit le `ProtocolHandler` du `Router`.
#[derive(Clone)]
pub struct ManifestServer {
    inner: Arc<Inner>,
}

impl ManifestServer {
    pub fn new(authorizer: Arc<dyn PeerAuthorizer>) -> Self {
        Self { inner: Arc::new(Inner { authorizer, current: Mutex::new(None) }) }
    }

    /// Définit le manifeste partagé (`None` : arrête le partage). Refuse un manifeste invalide ou
    /// trop volumineux : l'ancien reste alors en place.
    pub fn set(&self, manifest: Option<&Manifest>) -> Result<()> {
        let bytes = match manifest {
            None => None,
            Some(m) => {
                validate_manifest(m)?;
                let b = serde_json::to_vec(m).map_err(|e| Error::Other(format!("manifeste : {e}")))?;
                if b.len() > MAX_MANIFEST_BYTES {
                    return Err(Error::Invalid(format!("manifeste trop volumineux : {} octets", b.len())));
                }
                Some(Arc::new(b))
            }
        };
        *self.inner.current.lock().unwrap_or_else(|p| p.into_inner()) = bytes;
        Ok(())
    }

    /// À enregistrer dans le `Router` sous [`MANIFEST_ALPN`].
    pub fn handler(&self) -> ManifestHandler {
        ManifestHandler { inner: self.inner.clone() }
    }
}

/// `ProtocolHandler` du canal de manifeste.
#[derive(Clone)]
pub struct ManifestHandler {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for ManifestHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ManifestHandler")
    }
}

impl ProtocolHandler for ManifestHandler {
    async fn accept(&self, conn: Connection) -> std::result::Result<(), AcceptError> {
        let allowed = self.inner.authorizer.is_allowed(&conn.remote_id().to_string());
        let current = self.inner.current.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let (true, Some(bytes)) = (allowed, current) else {
            conn.close(CLOSE_DENIED.into(), b"denied");
            return Ok(());
        };
        let served = async {
            let (mut send, mut recv) = conn.accept_bi().await.ok()?;
            let mut kind = [0u8; 1];
            recv.read_exact(&mut kind).await.ok()?;
            if kind[0] != REQUEST {
                return None;
            }
            send.write_all(&bytes).await.ok()?;
            send.finish().ok()?;
            // Laisse l'invité lire avant de fermer la connexion.
            let _ = timeout(NET_TIMEOUT, conn.closed()).await;
            Some(())
        };
        if served.await.is_none() {
            conn.close(CLOSE_BAD_REQUEST.into(), b"bad request");
        }
        Ok(())
    }
}

/// Côté invité : récupère le manifeste de l'hôte `peer` (adresse déjà connue, ex. via le code ami).
/// Le manifeste reçu est borné en taille et revalidé.
pub async fn fetch_manifest(endpoint: &Endpoint, peer: impl Into<EndpointAddr>) -> Result<Manifest> {
    let net = |e: String| Error::Network(format!("manifeste : {e}"));
    let work = async {
        let conn = endpoint.connect(peer, MANIFEST_ALPN).await.map_err(|e| net(e.to_string()))?;
        let res = async {
            let (mut send, mut recv) = conn.open_bi().await.map_err(|e| net(e.to_string()))?;
            send.write_all(&[REQUEST]).await.map_err(|e| net(e.to_string()))?;
            send.finish().map_err(|e| net(e.to_string()))?;
            match recv.read_to_end(MAX_MANIFEST_BYTES).await {
                Ok(b) => Ok(b),
                Err(e) => match conn.close_reason() {
                    Some(iroh::endpoint::ConnectionError::ApplicationClosed(c))
                        if u64::from(c.error_code) == u64::from(CLOSE_DENIED) =>
                    {
                        Err(Error::Auth("l'hôte refuse (pair non autorisé ou rien de partagé)".into()))
                    }
                    _ => Err(net(e.to_string())),
                },
            }
        }
        .await;
        conn.close(0u32.into(), b"done");
        let bytes = res?;
        let m: Manifest = serde_json::from_slice(&bytes).map_err(|e| Error::Invalid(format!("manifeste illisible : {e}")))?;
        validate_manifest(&m)?;
        Ok(m)
    };
    timeout(NET_TIMEOUT, work).await.map_err(|_| Error::Network("manifeste : délai dépassé".into()))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transfer::send::AllowList;
    use core_lib::types::{Loader, LoaderSpec, ManifestFile};
    use iroh::endpoint::presets;
    use iroh::protocol::Router;

    fn manifest() -> Manifest {
        Manifest {
            name: "M".into(),
            minecraft: "1.21.1".into(),
            loader: LoaderSpec { loader: Loader::Fabric, version: "0.16.0".into() },
            java_major: 21,
            files: vec![ManifestFile { path: "mods/a.jar".into(), size: 3, sha256: "a".repeat(64), sha1: None }],
        }
    }

    async fn ep(alpn: bool) -> Endpoint {
        let b = Endpoint::builder(presets::Minimal).relay_mode(iroh::RelayMode::Disabled);
        let b = if alpn { b.alpns(vec![MANIFEST_ALPN.to_vec()]) } else { b };
        b.bind().await.unwrap()
    }

    #[test]
    fn set_rejects_invalid_manifest_and_keeps_previous() {
        let srv = ManifestServer::new(Arc::new(AllowList::default()));
        srv.set(Some(&manifest())).unwrap();
        let mut bad = manifest();
        bad.files[0].path = "../evil".into();
        assert!(srv.set(Some(&bad)).is_err());
        assert!(srv.inner.current.lock().unwrap().is_some(), "l'ancien manifeste reste");
        srv.set(None).unwrap();
        assert!(srv.inner.current.lock().unwrap().is_none());
    }

    #[tokio::test]
    #[ignore = "ouvre de vrais sockets UDP locaux ; lancer avec --ignored"]
    async fn manifest_served_to_allowed_peer_only() {
        let (host, guest, stranger) = (ep(true).await, ep(false).await, ep(false).await);
        let srv = ManifestServer::new(Arc::new(AllowList(vec![guest.id().to_string()])));
        let _router = Router::builder(host.clone()).accept(MANIFEST_ALPN, srv.handler()).spawn();

        // Rien de partagé : refusé même pour un pair autorisé.
        assert!(matches!(fetch_manifest(&guest, host.addr()).await, Err(Error::Auth(_))));
        srv.set(Some(&manifest())).unwrap();
        assert_eq!(fetch_manifest(&guest, host.addr()).await.unwrap(), manifest());
        assert!(matches!(fetch_manifest(&stranger, host.addr()).await, Err(Error::Auth(_))));
        // Fin du partage.
        srv.set(None).unwrap();
        assert!(fetch_manifest(&guest, host.addr()).await.is_err());
    }
}
