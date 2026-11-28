//! Adaptateurs d'autorisation unifiés : `FriendsDb` + `SessionAccess` -> `PeerAuthorizer` / `TunnelAuthorizer` / `FriendLookup`.
//!
//! Règle de jonction (docs/hearth.md § 2) : être ami ne suffit plus. L'hôte n'ouvre le tunnel, le manifeste et les
//! fichiers qu'aux pairs de `SessionAccess` : amis invités pour la partie en cours, amis dont la demande à rejoindre a
//! été acceptée, et (plus tard) membres du groupe hébergé. Tout autre pair n'obtient rien.

use std::sync::Arc;

use p2p_lib::friends_db::FriendsDb;
use p2p_lib::presence::{FriendLookup, PeerKey};
use p2p_lib::reach::Reach;
use p2p_lib::transfer::send::PeerAuthorizer;
use p2p_lib::tunnel::TunnelAuthorizer;

pub use crate::session_access::{SessionAccess, SessionTarget};

fn decode_key(hex_key: &str) -> Option<PeerKey> {
    if hex_key.len() != 64 {
        return None;
    }
    hex::decode(hex_key).ok()?.try_into().ok()
}

/// Source de vérité : la liste d'amis et l'accès de la partie en cours. `share_mods` ne concerne que les fichiers
/// (jamais le tunnel).
#[derive(Clone)]
pub struct FriendsAuthz {
    db: Arc<FriendsDb>,
    share_mods: Arc<dyn Fn() -> bool + Send + Sync>,
    access: Arc<SessionAccess>,
}

impl FriendsAuthz {
    pub fn new(db: Arc<FriendsDb>, share_mods: Arc<dyn Fn() -> bool + Send + Sync>, access: Arc<SessionAccess>) -> Self {
        Self { db, share_mods, access }
    }

    pub fn is_friend_hex(&self, key: &str) -> bool {
        key.len() == 64 && self.db.is_friend(key).unwrap_or(false)
    }

    /// L'accès de la partie en cours (partagé avec l'état de l'application).
    pub fn access(&self) -> &Arc<SessionAccess> {
        &self.access
    }

    /// Pourquoi ce pair ne peut pas rejoindre la partie : `NotFriend` s'il n'est pas ami (la seule raison qui ne
    /// révèle rien de la partie : un inconnu qui connaît la clé de l'hôte n'apprend ni qu'il sert un groupe ni son
    /// état), sinon `NotInvited` : ami non invité ou invitation retirée, ou ami retiré du groupe servi. Un ancien
    /// membre qui n'est pas ami n'est pas distinguable d'un inconnu : il reçoit `NotFriend`.
    pub fn join_denial(&self, key: &PeerKey) -> Reach {
        if self.is_friend_hex(&hex::encode(key)) {
            Reach::NotInvited
        } else {
            Reach::NotFriend
        }
    }

    /// Ce pair peut-il rejoindre la partie en cours (tunnel, manifeste, fichiers) ? Seule la cible servie compte : un
    /// invité (qui doit encore être ami accepté) pour un monde solo, un membre pour un groupe, personne d'autre.
    pub fn may_join(&self, key: &PeerKey) -> bool {
        match self.access.served_for(key) {
            (SessionTarget::Group(_), allowed) => allowed,
            (SessionTarget::Solo, allowed) => allowed && self.is_friend_hex(&hex::encode(key)),
        }
    }
}

/// Fichiers et manifeste : pair autorisé pour la partie ET « envoyer des mods à mes amis » activé.
impl PeerAuthorizer for FriendsAuthz {
    fn is_allowed(&self, public_key: &str) -> bool {
        (self.share_mods)() && decode_key(&public_key.to_lowercase()).is_some_and(|k| self.may_join(&k))
    }
    fn admitted(&self, public_key: &str) {
        if let Some(k) = decode_key(&public_key.to_lowercase()) {
            self.access.mark_entered(&k);
        }
    }
}

/// Tunnel de jeu : pair autorisé pour la partie suffit.
impl TunnelAuthorizer for FriendsAuthz {
    fn is_allowed(&self, public_key: &[u8; 32]) -> bool {
        self.may_join(public_key)
    }
    fn admitted(&self, public_key: &[u8; 32]) {
        self.access.mark_entered(public_key);
    }
    fn denial(&self, public_key: &[u8; 32]) -> Reach {
        self.join_denial(public_key)
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
    use p2p_lib::hearth::model::GroupId;
    use core_lib::types::{Friend, TrustLevel};

    fn authz(share: bool) -> (FriendsAuthz, Arc<SessionAccess>, Arc<std::sync::atomic::AtomicBool>, String) {
        let db = Arc::new(FriendsDb::open_in_memory().unwrap());
        let key = "ab".repeat(32);
        db.add_friend(&Friend { public_key: key.clone(), name: "B".into(), trust: TrustLevel::Ask, added_at: 1 }).unwrap();
        let flag = Arc::new(std::sync::atomic::AtomicBool::new(share));
        let f2 = flag.clone();
        let access = Arc::new(SessionAccess::default());
        let a = FriendsAuthz::new(db, Arc::new(move || f2.load(std::sync::atomic::Ordering::SeqCst)), access.clone());
        (a, access, flag, key)
    }

    #[test]
    fn ami_non_invite_n_a_rien() {
        let (a, access, _, key) = authz(true);
        assert!(!PeerAuthorizer::is_allowed(&a, &key));
        assert!(!TunnelAuthorizer::is_allowed(&a, &[0xab; 32]));
        assert!(FriendLookup::is_friend(&a, &[0xab; 32]), "reste ami : présence et invitations marchent");
        assert!(access.allow_friend(&[0xab; 32], crate::guest_pass::PassOrigin::Invite));
        assert!(PeerAuthorizer::is_allowed(&a, &key));
        assert!(PeerAuthorizer::is_allowed(&a, &key.to_uppercase()));
        assert!(TunnelAuthorizer::is_allowed(&a, &[0xab; 32]));
        assert!(!access.allow_friend(&[0xab; 32], crate::guest_pass::PassOrigin::Invite), "déjà autorisé");
    }

    #[test]
    fn raison_du_refus_ami_non_invite_ou_non_ami_ou_membre_retire() {
        let (a, access, _, _) = authz(true);
        // Ami non invité : « pas invité » (il peut demander à rejoindre), pas « pas ami ».
        assert_eq!(TunnelAuthorizer::denial(&a, &[0xab; 32]), Reach::NotInvited);
        assert_eq!(a.join_denial(&[0xab; 32]), Reach::NotInvited);
        // Non-ami : « pas ami », même s'il figure dans la liste d'accès.
        access.allow_friend(&[0xcd; 32], crate::guest_pass::PassOrigin::Invite);
        assert_eq!(TunnelAuthorizer::denial(&a, &[0xcd; 32]), Reach::NotFriend);
        // Invité révoqué (fin de partie, invitation annulée) : redevient « pas invité ».
        assert!(access.allow_friend(&[0xab; 32], crate::guest_pass::PassOrigin::Invite));
        access.revoke(&[0xab; 32]);
        assert_eq!(TunnelAuthorizer::denial(&a, &[0xab; 32]), Reach::NotInvited);
        // Groupe servi : un inconnu ne doit rien apprendre (« pas ami »), même retiré du groupe ; un ami retiré est « pas invité ».
        let g = GroupId([7; 16]);
        access.allow_members(g, [[0xcd; 32], [0xab; 32]]);
        access.revoke_member(&g, &[0xcd; 32]);
        access.revoke_member(&g, &[0xab; 32]);
        assert_eq!(TunnelAuthorizer::denial(&a, &[0xcd; 32]), Reach::NotFriend);
        assert_eq!(TunnelAuthorizer::denial(&a, &[0xee; 32]), Reach::NotFriend);
        assert_eq!(TunnelAuthorizer::denial(&a, &[0xab; 32]), Reach::NotInvited);
    }

    #[test]
    fn inconnu_refuse_meme_dans_la_liste_d_acces_s_il_n_est_pas_ami() {
        let (a, access, _, key) = authz(true);
        access.allow_friend(&[0xcd; 32], crate::guest_pass::PassOrigin::Invite);
        assert!(!PeerAuthorizer::is_allowed(&a, &"cd".repeat(32)));
        assert!(!TunnelAuthorizer::is_allowed(&a, &[0xcd; 32]));
        assert!(!PeerAuthorizer::is_allowed(&a, "pas une clé"));
        assert!(!PeerAuthorizer::is_allowed(&a, &key));
    }

    #[test]
    fn share_mods_ne_concerne_que_les_fichiers() {
        let (a, access, flag, key) = authz(true);
        access.allow_friend(&[0xab; 32], crate::guest_pass::PassOrigin::Invite);
        assert!(PeerAuthorizer::is_allowed(&a, &key));
        flag.store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(!PeerAuthorizer::is_allowed(&a, &key));
        assert!(TunnelAuthorizer::is_allowed(&a, &[0xab; 32]), "le tunnel ne dépend pas de share_mods");
    }

    #[test]
    fn membres_de_groupe_sans_amitie_puis_acces_vide() {
        let (a, access, _, key) = authz(true);
        let g = GroupId([9; 16]);
        access.allow_members(g, [[1u8; 32], [2u8; 32]]);
        assert!(TunnelAuthorizer::is_allowed(&a, &[1; 32]));
        assert!(PeerAuthorizer::is_allowed(&a, &"02".repeat(32)));
        assert!(!TunnelAuthorizer::is_allowed(&a, &[3; 32]));
        access.revoke(&[2; 32]);
        assert!(!TunnelAuthorizer::is_allowed(&a, &[2; 32]));
        access.allow_friend(&[0xab; 32], crate::guest_pass::PassOrigin::Invite);
        access.clear();
        assert!(!access.contains(&[1; 32]) && !access.contains(&[0xab; 32]));
        assert!(!TunnelAuthorizer::is_allowed(&a, &[1; 32]));
        assert!(!PeerAuthorizer::is_allowed(&a, &key));
    }

    #[test]
    fn la_cible_servie_decide_qui_entre() {
        let (a, access, _, _) = authz(true);
        let g = GroupId([7; 16]);
        // Un monde solo partagé : Dave (ami invité) entre. Puis l'hôte sert un groupe dont Dave n'est pas membre.
        access.allow_friend(&[0xab; 32], crate::guest_pass::PassOrigin::Invite);
        assert!(TunnelAuthorizer::is_allowed(&a, &[0xab; 32]));
        access.allow_members(g, [[1u8; 32]]);
        assert!(!TunnelAuthorizer::is_allowed(&a, &[0xab; 32]), "un invité solo n'atteint pas le serveur du groupe");
        assert!(!PeerAuthorizer::is_allowed(&a, &"ab".repeat(32)), "ni son manifeste");
        assert!(TunnelAuthorizer::is_allowed(&a, &[1; 32]));
        // Une demande à rejoindre acceptée pendant le groupe n'ouvre rien : ni maintenant, ni pour le groupe.
        assert!(!access.allow_friend(&[0xcd; 32], crate::guest_pass::PassOrigin::Invite));
        assert!(!access.contains(&[0xcd; 32]));
        // Fin de la partie du groupe : le monde solo partagé en parallèle garde ses invités.
        access.close_group(&g);
        assert!(!TunnelAuthorizer::is_allowed(&a, &[1; 32]), "le membre n'a plus rien une fois le groupe fermé");
        assert!(TunnelAuthorizer::is_allowed(&a, &[0xab; 32]), "l'invité solo n'est pas vidé par la fin du groupe");
    }

    #[test]
    fn retrait_d_un_membre_ne_touche_que_son_groupe() {
        let (a, access, _, _) = authz(true);
        let (g1, g2) = (GroupId([1; 16]), GroupId([2; 16]));
        access.allow_members(g2, [[5u8; 32]]);
        access.allow_members(g1, [[5u8; 32], [6u8; 32]]);
        access.revoke_member(&g1, &[5; 32]);
        assert!(!TunnelAuthorizer::is_allowed(&a, &[5; 32]));
        assert!(TunnelAuthorizer::is_allowed(&a, &[6; 32]));
        assert!(access.allows(&SessionTarget::Group(g2), &[5; 32]), "l'accès à l'autre groupe est intact");
        assert_eq!(access.served_for(&[6; 32]), (SessionTarget::Group(g1), true));
    }

    #[test]
    fn tester_l_acces_ne_fait_pas_entrer_seul_le_service_le_fait() {
        let (a, access, _, key) = authz(true);
        access.allow_friend(&[0xab; 32], crate::guest_pass::PassOrigin::Invite);
        assert!(PeerAuthorizer::is_allowed(&a, &key));
        assert!(TunnelAuthorizer::is_allowed(&a, &[0xab; 32]));
        assert!(!access.guests()[0].entered, "un simple test ne fige pas l'accès");
        TunnelAuthorizer::admitted(&a, &[0xab; 32]);
        assert!(access.guests()[0].entered);
    }

    #[test]
    fn ami_retire_perd_son_acces_d_invite() {
        let (a, access, _, key) = authz(true);
        access.allow_friend(&[0xab; 32], crate::guest_pass::PassOrigin::Invite);
        assert!(a.db.remove_friend(&key).is_ok());
        assert!(!TunnelAuthorizer::is_allowed(&a, &[0xab; 32]));
    }
}
