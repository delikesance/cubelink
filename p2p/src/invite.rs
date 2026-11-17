//! Invitation à rejoindre le monde d'un ami, ALPN `cubelink/invite/1`.
//!
//! Protocole minuscule, sans donnée personnelle (même esprit que `presence.rs`) : l'émetteur ouvre un flux
//! bidirectionnel, écrit un JSON `{world_name, minecraft, expires_at, pack?, icon?}` (au plus `MAX_INVITE_BYTES`) puis
//! termine son flux ; le récepteur répond un octet `ACK` s'il a pris l'invitation en compte.
//!
//! Sécurité : l'émetteur est `conn.remote_id()` (jamais le contenu du message) ; un non-ami est refusé sans
//! un octet de réponse ; message borné, valide ou rejeté ; au plus une invitation par ami et par
//! `min_interval` (anti-spam, la suivante est ignorée).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::{Endpoint, EndpointAddr};
use serde::{Deserialize, Serialize};
use tokio::time::timeout;

use crate::presence::{FriendLookup, PeerKey};

/// ALPN du protocole d'invitation.
pub const INVITE_ALPN: &[u8] = b"cubelink/invite/1";
/// Taille maximale du message (octets) : l'icône du monde (PNG en base64) en prend l'essentiel.
pub const MAX_INVITE_BYTES: usize = 16 * 1024;
/// Taille maximale acceptée par les versions précédentes (sans icône) : au-delà, elles refusent le message.
pub const LEGACY_INVITE_BYTES: usize = 1024;
/// Longueur maximale de l'icône en base64.
pub const MAX_ICON_CHARS: usize = 14 * 1024;
/// Intervalle minimal entre deux invitations acceptées d'un même ami.
pub const DEFAULT_MIN_INTERVAL: Duration = Duration::from_secs(10);
const MAX_WORLD_CHARS: usize = 64;
const MAX_MC_CHARS: usize = 32;
const ACK: u8 = 1;
const IO_TIMEOUT: Duration = Duration::from_secs(5);

const CLOSE_DENIED: u32 = 1;
const CLOSE_BAD: u32 = 2;

/// Contenu d'une invitation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InviteMessage {
    pub world_name: String,
    pub minecraft: String,
    /// Secondes depuis l'epoch Unix.
    pub expires_at: i64,
    /// Annule les invitations précédentes de l'émetteur (il ne partage plus son monde).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub revoke: bool,
    /// Nom du modpack (instance) de l'émetteur.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack: Option<String>,
    /// Icône du monde : PNG en base64 (standard). Contrôlée par le récepteur (format, dimensions).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

impl InviteMessage {
    /// Annulation (sans texte) des invitations déjà envoyées.
    pub fn revocation() -> Self {
        Self { world_name: String::new(), minecraft: String::new(), expires_at: 0, revoke: true, pack: None, icon: None }
    }

    /// Texte sans caractère de contrôle, non vide, borné (le contenu vient d'un pair : pas de confiance).
    pub fn validate(&self) -> Result<(), String> {
        if self.revoke {
            return Ok(());
        }
        let ok = |s: &str, max: usize| !s.trim().is_empty() && s.chars().count() <= max && !s.chars().any(char::is_control);
        if !ok(&self.world_name, MAX_WORLD_CHARS) {
            return Err("nom de monde invalide".into());
        }
        if !ok(&self.minecraft, MAX_MC_CHARS) {
            return Err("version de Minecraft invalide".into());
        }
        if self.pack.as_deref().is_some_and(|p| !ok(p, MAX_WORLD_CHARS)) {
            return Err("nom de modpack invalide".into());
        }
        let b64 = |s: &str| !s.is_empty() && s.len() <= MAX_ICON_CHARS && s.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'/' | b'='));
        if self.icon.as_deref().is_some_and(|i| !b64(i)) {
            return Err("icône invalide".into());
        }
        Ok(())
    }
}

/// Décode et valide un message d'invitation (octets reçus du réseau).
pub fn parse_invite(bytes: &[u8]) -> Result<InviteMessage, String> {
    if bytes.len() > MAX_INVITE_BYTES {
        return Err("message trop gros".into());
    }
    let m: InviteMessage = serde_json::from_slice(bytes).map_err(|_| "message invalide".to_string())?;
    m.validate()?;
    Ok(m)
}

/// Destinataire des invitations (appelé de façon synchrone, ne doit pas bloquer) : clé de l'émetteur + message.
pub type InviteSink = Arc<dyn Fn(PeerKey, InviteMessage) + Send + Sync>;

/// Côté serveur : à enregistrer dans le `Router` sous [`INVITE_ALPN`].
#[derive(Clone)]
pub struct InviteHandler {
    friends: Arc<dyn FriendLookup>,
    sink: InviteSink,
    min_interval: Duration,
    last: Arc<Mutex<HashMap<PeerKey, Instant>>>,
}

impl std::fmt::Debug for InviteHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("InviteHandler")
    }
}

impl InviteHandler {
    pub fn new(friends: Arc<dyn FriendLookup>, sink: InviteSink) -> Self {
        Self::with_interval(friends, sink, DEFAULT_MIN_INTERVAL)
    }

    pub fn with_interval(friends: Arc<dyn FriendLookup>, sink: InviteSink, min_interval: Duration) -> Self {
        Self { friends, sink, min_interval, last: Arc::default() }
    }

    /// Vrai (et date retenue) si cet ami n'a pas déjà invité il y a moins de `min_interval`.
    fn allow(&self, key: PeerKey) -> bool {
        let now = Instant::now();
        let mut last = crate::util::lock(&self.last);
        last.retain(|_, t| now.duration_since(*t) < self.min_interval);
        if last.contains_key(&key) {
            return false;
        }
        last.insert(key, now);
        true
    }
}

impl ProtocolHandler for InviteHandler {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let key: PeerKey = *conn.remote_id().as_bytes();
        if !self.friends.is_friend(&key) {
            conn.close(CLOSE_DENIED.into(), b"");
            return Ok(());
        }
        let served = async {
            let (mut send, mut recv) = conn.accept_bi().await.ok()?;
            let bytes = recv.read_to_end(MAX_INVITE_BYTES).await.ok()?;
            let msg = parse_invite(&bytes).ok()?;
            // Une annulation n'est pas limitée (elle réduit l'état chez l'ami) ; une invitation l'est.
            if !msg.revoke && !self.allow(key) {
                return None;
            }
            (self.sink)(key, msg);
            send.write_all(&[ACK]).await.ok()?;
            send.finish().ok()?;
            // Laisse à l'émetteur le temps de lire l'accusé avant de fermer.
            let _ = timeout(IO_TIMEOUT, conn.closed()).await;
            Some(())
        };
        match timeout(IO_TIMEOUT * 2, served).await {
            Ok(Some(())) => {}
            Ok(None) => conn.close(CLOSE_BAD.into(), b""),
            Err(_) => conn.close(CLOSE_BAD.into(), b""),
        }
        Ok(())
    }
}

/// Envoie une invitation à `addr` ; Ok seulement si l'ami l'a prise en compte (accusé reçu).
pub async fn send_invite(endpoint: &Endpoint, addr: impl Into<EndpointAddr>, msg: &InviteMessage) -> Result<(), String> {
    msg.validate()?;
    let addr: EndpointAddr = addr.into();
    match send_once(endpoint, addr.clone(), msg).await {
        // Une version précédente refuse un message de plus de 1 Kio sans répondre : on renvoie sans l'icône
        // (son limiteur de débit n'a pas compté la tentative refusée).
        Err(SendError::NoAck) if msg.icon.is_some() => {
            let bare = InviteMessage { icon: None, ..msg.clone() };
            if serde_json::to_vec(&bare).map_or(true, |b| b.len() > LEGACY_INVITE_BYTES) {
                return Err(SendError::NoAck.to_string());
            }
            send_once(endpoint, addr, &bare).await.map_err(|e| e.to_string())
        }
        r => r.map_err(|e| e.to_string()),
    }
}

#[derive(Debug)]
enum SendError {
    Unreachable,
    Failed,
    NoAck,
    Other(String),
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unreachable => "l'ami est injoignable",
            Self::Failed => "envoi impossible",
            Self::NoAck => "l'ami n'a pas pu recevoir l'invitation (trop d'invitations ou pas ami)",
            Self::Other(s) => s,
        })
    }
}

async fn send_once(endpoint: &Endpoint, addr: EndpointAddr, msg: &InviteMessage) -> Result<(), SendError> {
    let body = serde_json::to_vec(msg).map_err(|_| SendError::Other("message invalide".into()))?;
    if body.len() > MAX_INVITE_BYTES {
        return Err(SendError::Other("message trop gros".into()));
    }
    let work = async {
        let conn = endpoint.connect(addr, INVITE_ALPN).await.map_err(|_| SendError::Unreachable)?;
        let res = async {
            let (mut send, mut recv) = conn.open_bi().await.map_err(|_| SendError::Unreachable)?;
            send.write_all(&body).await.map_err(|_| SendError::Failed)?;
            send.finish().map_err(|_| SendError::Failed)?;
            let mut ack = [0u8; 1];
            match recv.read_exact(&mut ack).await {
                Ok(()) if ack[0] == ACK => Ok(()),
                _ => Err(SendError::NoAck),
            }
        }
        .await;
        conn.close(0u32.into(), b"");
        res
    };
    timeout(Duration::from_secs(15), work).await.map_err(|_| SendError::Unreachable)?
}

#[cfg(test)]
mod tests {
    use super::*;

    use iroh::endpoint::presets;
    use iroh::protocol::Router;
    use iroh::RelayMode;

    struct Friends(Mutex<Vec<PeerKey>>);
    impl FriendLookup for Friends {
        fn friend_keys(&self) -> Vec<PeerKey> {
            self.0.lock().unwrap().clone()
        }
        fn is_friend(&self, key: &PeerKey) -> bool {
            self.0.lock().unwrap().contains(key)
        }
    }

    type Got = Arc<Mutex<Vec<(PeerKey, InviteMessage)>>>;

    async fn ep() -> Endpoint {
        Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled).bind().await.unwrap()
    }

    fn msg(world: &str) -> InviteMessage {
        InviteMessage { world_name: world.into(), minecraft: "1.21.1".into(), expires_at: 1_900_000_000, revoke: false, pack: None, icon: None }
    }

    /// Serveur A (amis : `friends`), reçoit dans `Got`.
    async fn server(friends: Vec<PeerKey>, interval: Duration) -> (Endpoint, Router, Got) {
        let ep_a = ep().await;
        let got: Got = Arc::default();
        let g2 = Arc::clone(&got);
        let sink: InviteSink = Arc::new(move |k, m| g2.lock().unwrap().push((k, m)));
        let h = InviteHandler::with_interval(Arc::new(Friends(Mutex::new(friends))), sink, interval);
        let router = Router::builder(ep_a.clone()).accept(INVITE_ALPN, h).spawn();
        (ep_a, router, got)
    }

    #[test]
    fn parse_borne_et_valide() {
        assert!(parse_invite(br#"{"world_name":"Monde","minecraft":"1.21.1","expires_at":5}"#).is_ok());
        assert!(parse_invite(b"pas du json").is_err());
        assert!(parse_invite(br#"{"world_name":"","minecraft":"1.21.1","expires_at":5}"#).is_err());
        assert!(parse_invite(br#"{"world_name":"a\u0000b","minecraft":"1.21.1","expires_at":5}"#).is_err());
        assert!(parse_invite(br#"{"world_name":"x","minecraft":"1.21.1"}"#).is_err());
        let big = vec![b' '; MAX_INVITE_BYTES + 1];
        assert!(parse_invite(&big).is_err());
        let long = format!(r#"{{"world_name":"{}","minecraft":"1","expires_at":5}}"#, "a".repeat(65));
        assert!(parse_invite(long.as_bytes()).is_err());
    }

    #[tokio::test]
    async fn ami_accepte_emetteur_identifie_par_la_connexion() {
        let ep_b = ep().await;
        let (ep_a, router, got) = server(vec![*ep_b.id().as_bytes()], Duration::from_millis(0)).await;
        send_invite(&ep_b, ep_a.addr(), &msg("Monde")).await.unwrap();
        let g = got.lock().unwrap().clone();
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].0, *ep_b.id().as_bytes());
        assert_eq!(g[0].1, msg("Monde"));
        router.shutdown().await.unwrap();
    }

    #[test]
    fn pack_et_icone_optionnels_et_bornes() {
        let m = parse_invite(br#"{"world_name":"M","minecraft":"1","expires_at":5,"pack":"ATM10","icon":"iVBORw0KGgo="}"#).unwrap();
        assert_eq!((m.pack.as_deref(), m.icon.as_deref()), (Some("ATM10"), Some("iVBORw0KGgo=")));
        assert!(parse_invite(br#"{"world_name":"M","minecraft":"1","expires_at":5,"icon":"pas du base64 !"}"#).is_err());
        assert!(parse_invite(br#"{"world_name":"M","minecraft":"1","expires_at":5,"pack":""}"#).is_err());
        // Sans les champs optionnels, le JSON reste celui des versions précédentes.
        assert_eq!(serde_json::to_string(&msg("M")).unwrap(), r#"{"world_name":"M","minecraft":"1.21.1","expires_at":1900000000}"#);
    }

    /// Ancien récepteur : lit au plus 1 Kio et ne connaît pas l'icône.
    #[derive(Debug, Clone)]
    struct Legacy(Got);
    impl ProtocolHandler for Legacy {
        async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
            if let Ok((mut send, mut recv)) = conn.accept_bi().await {
                if let Ok(bytes) = recv.read_to_end(LEGACY_INVITE_BYTES).await {
                    if let Ok(m) = parse_invite(&bytes) {
                        self.0.lock().unwrap().push(([0; 32], m));
                        let _ = send.write_all(&[ACK]).await;
                        let _ = send.finish();
                        let _ = timeout(IO_TIMEOUT, conn.closed()).await;
                        return Ok(());
                    }
                }
            }
            conn.close(CLOSE_BAD.into(), b"");
            Ok(())
        }
    }

    #[tokio::test]
    async fn ancienne_version_recoit_sans_icone() {
        let ep_a = ep().await;
        let got: Got = Arc::default();
        let router = Router::builder(ep_a.clone()).accept(INVITE_ALPN, Legacy(Arc::clone(&got))).spawn();
        let ep_b = ep().await;
        let with_icon = InviteMessage { pack: Some("ATM10".into()), icon: Some("A".repeat(4000)), ..msg("Monde") };
        send_invite(&ep_b, ep_a.addr(), &with_icon).await.unwrap();
        let g = got.lock().unwrap().clone();
        assert_eq!(g.len(), 1);
        assert_eq!((g[0].1.icon.as_deref(), g[0].1.pack.as_deref()), (None, Some("ATM10")));
        router.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn non_ami_refuse() {
        let ep_c = ep().await;
        let (ep_a, router, got) = server(vec![], Duration::from_millis(0)).await;
        assert!(send_invite(&ep_c, ep_a.addr(), &msg("Monde")).await.is_err());
        assert!(got.lock().unwrap().is_empty());
        router.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn message_trop_gros_ou_invalide_rejete() {
        let ep_b = ep().await;
        let (ep_a, router, got) = server(vec![*ep_b.id().as_bytes()], Duration::from_millis(0)).await;
        for payload in [vec![b'x'; MAX_INVITE_BYTES + 10], b"{pas du json".to_vec(), br#"{"world_name":"","minecraft":"1","expires_at":1}"#.to_vec()] {
            let conn = ep_b.connect(ep_a.addr(), INVITE_ALPN).await.unwrap();
            let (mut s, mut r) = conn.open_bi().await.unwrap();
            let _ = s.write_all(&payload).await;
            let _ = s.finish();
            let mut ack = [0u8; 1];
            assert!(r.read_exact(&mut ack).await.is_err(), "aucun accusé pour un message refusé");
            conn.close(0u32.into(), b"");
        }
        assert!(got.lock().unwrap().is_empty());
        // L'émetteur local refuse aussi un message trop gros.
        let huge = InviteMessage { world_name: "a".repeat(2000), ..msg("x") };
        assert!(send_invite(&ep_b, ep_a.addr(), &huge).await.is_err());
        router.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn annulation_non_limitee_et_sans_texte() {
        let ep_b = ep().await;
        let (ep_a, router, got) = server(vec![*ep_b.id().as_bytes()], Duration::from_secs(60)).await;
        send_invite(&ep_b, ep_a.addr(), &msg("Un")).await.unwrap();
        send_invite(&ep_b, ep_a.addr(), &InviteMessage::revocation()).await.unwrap();
        let g = got.lock().unwrap().clone();
        assert_eq!(g.len(), 2);
        assert!(g[1].1.revoke);
        router.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn limite_de_debit_par_ami() {
        let ep_b = ep().await;
        let (ep_a, router, got) = server(vec![*ep_b.id().as_bytes()], Duration::from_millis(400)).await;
        send_invite(&ep_b, ep_a.addr(), &msg("Un")).await.unwrap();
        assert!(send_invite(&ep_b, ep_a.addr(), &msg("Deux")).await.is_err(), "trop rapide");
        assert_eq!(got.lock().unwrap().len(), 1);
        tokio::time::sleep(Duration::from_millis(500)).await;
        send_invite(&ep_b, ep_a.addr(), &msg("Trois")).await.unwrap();
        let g = got.lock().unwrap().clone();
        assert_eq!(g.iter().map(|(_, m)| m.world_name.as_str()).collect::<Vec<_>>(), ["Un", "Trois"]);
        router.shutdown().await.unwrap();
    }
}
