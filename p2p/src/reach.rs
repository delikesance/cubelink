//! Raisons distinctes pour lesquelles un ami n'est pas joignable ou ne partage pas, et codes de fermeture
//! QUIC par lesquels l'hôte les annonce à l'invité (aucune donnée n'est échangée dans ces cas).
//!
//! Les messages sont neutres (l'hôte n'est pas nommé) : l'interface les complète avec le nom de l'ami.

use iroh::endpoint::{Connection, ConnectionError};

/// L'hôte refuse : le pair n'est pas son ami (ou n'est plus accepté).
pub const CLOSE_DENIED: u32 = 1;
/// Requête incompréhensible.
pub const CLOSE_BAD_REQUEST: u32 = 2;
/// L'hôte a désactivé l'envoi de mods à ses amis.
pub const CLOSE_SHARING_OFF: u32 = 3;
/// L'hôte est en ligne mais n'a rien ouvert (aucun monde partagé).
pub const CLOSE_NOT_SHARED: u32 = 4;

/// Pourquoi on ne peut pas rejoindre l'ami.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// Injoignable : Cubelink n'est pas ouvert chez lui, ou pas de réseau.
    Offline,
    /// En ligne, mais il ne nous a pas (ou plus) comme ami.
    NotFriend,
    /// En ligne, mais il a désactivé le partage de mods.
    SharingOff,
    /// En ligne, mais aucun monde n'est ouvert.
    NothingShared,
}

impl Reach {
    pub fn message(self) -> &'static str {
        match self {
            Reach::Offline => "Ton ami n'est pas en ligne. Il doit ouvrir Cubelink.",
            Reach::NotFriend => "Ton ami ne t'a pas dans sa liste d'amis. Demande-lui de t'ajouter.",
            Reach::SharingOff => "Ton ami a désactivé le partage de mods. Il doit l'activer dans les Réglages de Cubelink.",
            Reach::NothingShared => "Ton ami n'a pas encore ouvert son monde : il doit lancer le jeu puis l'ouvrir au réseau local.",
        }
    }

    /// Code de fermeture annoncé par l'hôte pour ce refus (`Offline` n'en a pas : rien n'est reçu).
    pub fn close_code(self) -> Option<u32> {
        match self {
            Reach::Offline => None,
            Reach::NotFriend => Some(CLOSE_DENIED),
            Reach::SharingOff => Some(CLOSE_SHARING_OFF),
            Reach::NothingShared => Some(CLOSE_NOT_SHARED),
        }
    }

    pub fn from_code(code: u64) -> Option<Reach> {
        match code {
            c if c == u64::from(CLOSE_DENIED) => Some(Reach::NotFriend),
            c if c == u64::from(CLOSE_SHARING_OFF) => Some(Reach::SharingOff),
            c if c == u64::from(CLOSE_NOT_SHARED) => Some(Reach::NothingShared),
            _ => None,
        }
    }

    /// Raison annoncée par l'hôte si la connexion a été fermée par l'application avec un code connu.
    /// Comme `from_conn`, mais attend brièvement la fermeture : l'erreur d'un flux peut arriver avant que
    /// la raison de fermeture de la connexion soit enregistrée (course constatée).
    pub async fn from_conn_settled(conn: &Connection) -> Option<Reach> {
        if let Some(r) = Reach::from_conn(conn) {
            return Some(r);
        }
        let _ = tokio::time::timeout(std::time::Duration::from_millis(500), conn.closed()).await;
        Reach::from_conn(conn)
    }

    pub fn from_conn(conn: &Connection) -> Option<Reach> {
        match conn.close_reason() {
            Some(ConnectionError::ApplicationClosed(c)) => Reach::from_code(u64::from(c.error_code)),
            _ => None,
        }
    }
}

impl std::fmt::Display for Reach {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for Reach {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_roundtrip_and_messages_are_distinct() {
        let all = [Reach::Offline, Reach::NotFriend, Reach::SharingOff, Reach::NothingShared];
        for r in all {
            if let Some(c) = r.close_code() {
                assert_eq!(Reach::from_code(u64::from(c)), Some(r));
            }
        }
        let msgs: std::collections::HashSet<_> = all.iter().map(|r| r.message()).collect();
        assert_eq!(msgs.len(), all.len());
        assert_eq!(Reach::from_code(u64::from(CLOSE_BAD_REQUEST)), None);
        assert_eq!(Reach::from_code(0), None);
    }
}
