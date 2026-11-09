//! Adaptateurs d'autorisation unifiés : `FriendsDb` -> `PeerAuthorizer` / `TunnelAuthorizer` / `FriendLookup`.
//! Seuls les amis ACCEPTÉS passent (les demandes en attente ne sont pas des amis).

use std::sync::Arc;

use p2p_lib::friends_db::FriendsDb;
use p2p_lib::presence::{FriendLookup, PeerKey};
use p2p_lib::transfer::send::PeerAuthorizer;
use p2p_lib::tunnel::TunnelAuthorizer;

/// Source de vérité : la liste d'amis. `share_mods` ne concerne que les fichiers (jamais le tunnel).
#[derive(Clone)]
pub struct FriendsAuthz {
    db: Arc<FriendsDb>,
    share_mods: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl FriendsAuthz {
    pub fn new(db: Arc<FriendsDb>, share_mods: Arc<dyn Fn() -> bool + Send + Sync>) -> Self {
        Self { db, share_mods }
    }

    pub fn is_friend_hex(&self, key: &str) -> bool {
        key.len() == 64 && self.db.is_friend(key).unwrap_or(false)
    }
}

/// Fichiers et manifeste : ami accepté ET « envoyer des mods à mes amis » activé.
impl PeerAuthorizer for FriendsAuthz {
    fn is_allowed(&self, public_key: &str) -> bool {
        (self.share_mods)() && self.is_friend_hex(&public_key.to_lowercase())
    }
}

/// Tunnel de jeu : ami accepté suffit.
impl TunnelAuthorizer for FriendsAuthz {
    fn is_allowed(&self, public_key: &[u8; 32]) -> bool {
        self.is_friend_hex(&hex::encode(public_key))
    }
}

impl FriendLookup for FriendsAuthz {
    fn friend_keys(&self) -> Vec<PeerKey> {
        FriendLookup::friend_keys(self.db.as_ref())
    }
    fn is_friend(&self, key: &PeerKey) -> bool {
        self.is_friend_hex(&hex::encode(key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_lib::types::{Friend, TrustLevel};

    #[test]
    fn amis_seulement_et_share_mods() {
        let db = Arc::new(FriendsDb::open_in_memory().unwrap());
        let key = "ab".repeat(32);
        db.add_friend(&Friend { public_key: key.clone(), name: "B".into(), trust: TrustLevel::Ask, added_at: 1 }).unwrap();
        let flag = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let f2 = flag.clone();
        let a = FriendsAuthz::new(db, Arc::new(move || f2.load(std::sync::atomic::Ordering::SeqCst)));
        assert!(PeerAuthorizer::is_allowed(&a, &key));
        assert!(!PeerAuthorizer::is_allowed(&a, &"cd".repeat(32)));
        assert!(TunnelAuthorizer::is_allowed(&a, &[0xab; 32]));
        assert!(!TunnelAuthorizer::is_allowed(&a, &[1; 32]));
        flag.store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(!PeerAuthorizer::is_allowed(&a, &key));
        assert!(TunnelAuthorizer::is_allowed(&a, &[0xab; 32]), "le tunnel ne dépend pas de share_mods");
    }
}
