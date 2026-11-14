//! Adaptateurs d'autorisation unifiés : `FriendsDb` + `SessionAccess` -> `PeerAuthorizer` / `TunnelAuthorizer` / `FriendLookup`.
//!
//! Règle de jonction (docs/hearth.md § 2) : être ami ne suffit plus. L'hôte n'ouvre le tunnel, le manifeste et les
//! fichiers qu'aux pairs de `SessionAccess` : amis invités pour la partie en cours, amis dont la demande à rejoindre a
//! été acceptée, et (plus tard) membres du groupe hébergé. Tout autre pair n'obtient rien.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use p2p_lib::friends_db::FriendsDb;
use p2p_lib::hearth::model::GroupId;
use p2p_lib::presence::{FriendLookup, PeerKey};
use p2p_lib::transfer::send::PeerAuthorizer;
use p2p_lib::tunnel::TunnelAuthorizer;

use crate::state::lock;

/// Ce que l'hôte sert en ce moment : un monde solo partagé, ou le serveur d'un groupe. L'accès se donne et se lit
/// toujours pour une cible précise : un ami invité à un monde solo n'a rien sur le serveur d'un groupe, et inversement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SessionTarget {
    Solo,
    Group(GroupId),
}

#[derive(Default)]
struct AccessSets {
    /// Cible `Solo` : amis invités ou acceptés, l'accès vaut tant qu'ils sont encore amis acceptés.
    guests: HashSet<PeerKey>,
    /// Cible `Group` : membres d'un groupe hébergé (Hearth), l'accès ne dépend pas d'une amitié avec l'hôte.
    groups: HashMap<GroupId, HashSet<PeerKey>>,
    /// Le groupe que sert le tunnel en ce moment (un seul port local par hôte). Absent : le monde solo, s'il y en a un.
    serving: Option<GroupId>,
}

/// Pairs autorisés à rejoindre la partie en cours de l'hôte, par cible. Vidé à `host_stop_sharing` : l'accès ne survit
/// jamais à la partie pour laquelle il a été donné.
#[derive(Default)]
pub struct SessionAccess {
    inner: Mutex<AccessSets>,
}

impl SessionAccess {
    /// Un ami invité ou dont la demande est acceptée, pour le monde solo. Vrai si l'accès est nouveau. Sans effet tant
    /// que l'hôte sert un groupe : un non-membre n'entre jamais dans un groupe par une demande à rejoindre.
    pub fn allow_friend(&self, key: &PeerKey) -> bool {
        let mut sets = lock(&self.inner);
        if sets.serving.is_some() {
            return false;
        }
        sets.guests.insert(*key)
    }

    /// Ouvre l'accès au serveur du groupe `group` aux membres donnés (ajoute aux membres déjà autorisés) et en fait la
    /// cible servie par le tunnel.
    pub fn allow_members(&self, group: GroupId, keys: impl IntoIterator<Item = PeerKey>) {
        let mut sets = lock(&self.inner);
        sets.groups.entry(group).or_default().extend(keys);
        sets.serving = Some(group);
    }

    /// Retire l'accès d'un membre au groupe `group` (retrait, départ).
    pub fn revoke_member(&self, group: &GroupId, key: &PeerKey) {
        if let Some(set) = lock(&self.inner).groups.get_mut(group) {
            set.remove(key);
        }
    }

    /// Retire l'accès à ce pair, pour toutes les cibles (ami retiré, invitation annulée).
    pub fn revoke(&self, key: &PeerKey) {
        let mut sets = lock(&self.inner);
        sets.guests.remove(key);
        for set in sets.groups.values_mut() {
            set.remove(key);
        }
    }

    /// Fin de la partie d'un groupe : plus aucun accès à ce groupe. Le monde solo partagé en parallèle n'est pas touché.
    pub fn close_group(&self, group: &GroupId) {
        let mut sets = lock(&self.inner);
        sets.groups.remove(group);
        if sets.serving.as_ref() == Some(group) {
            sets.serving = None;
        }
    }

    /// Fin de partie : plus aucun accès, pour aucune cible.
    pub fn clear(&self) {
        let mut sets = lock(&self.inner);
        sets.guests.clear();
        sets.groups.clear();
        sets.serving = None;
    }

    /// Vrai si ce pair a une entrée, pour une cible quelconque (sans tenir compte de l'amitié).
    pub fn contains(&self, key: &PeerKey) -> bool {
        let sets = lock(&self.inner);
        sets.guests.contains(key) || sets.groups.values().any(|s| s.contains(key))
    }

    /// Ce pair a-t-il accès à la cible `target` ?
    pub fn allows(&self, target: &SessionTarget, key: &PeerKey) -> bool {
        let sets = lock(&self.inner);
        match target {
            SessionTarget::Solo => sets.guests.contains(key),
            SessionTarget::Group(g) => sets.groups.get(g).is_some_and(|s| s.contains(key)),
        }
    }

    /// La cible que sert le tunnel en ce moment, et si ce pair y a accès (un seul verrou : pas de cible qui change entre
    /// les deux lectures).
    pub fn served_for(&self, key: &PeerKey) -> (SessionTarget, bool) {
        let sets = lock(&self.inner);
        match sets.serving {
            Some(g) => (SessionTarget::Group(g), sets.groups.get(&g).is_some_and(|s| s.contains(key))),
            None => (SessionTarget::Solo, sets.guests.contains(key)),
        }
    }

    /// Vrai si l'hôte sert le serveur d'un groupe (les demandes à rejoindre ne s'appliquent alors pas).
    pub fn serving_group(&self) -> bool {
        lock(&self.inner).serving.is_some()
    }
}

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
}

/// Tunnel de jeu : pair autorisé pour la partie suffit.
impl TunnelAuthorizer for FriendsAuthz {
    fn is_allowed(&self, public_key: &[u8; 32]) -> bool {
        self.may_join(public_key)
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
        assert!(access.allow_friend(&[0xab; 32]));
        assert!(PeerAuthorizer::is_allowed(&a, &key));
        assert!(PeerAuthorizer::is_allowed(&a, &key.to_uppercase()));
        assert!(TunnelAuthorizer::is_allowed(&a, &[0xab; 32]));
        assert!(!access.allow_friend(&[0xab; 32]), "déjà autorisé");
    }

    #[test]
    fn inconnu_refuse_meme_dans_la_liste_d_acces_s_il_n_est_pas_ami() {
        let (a, access, _, key) = authz(true);
        access.allow_friend(&[0xcd; 32]);
        assert!(!PeerAuthorizer::is_allowed(&a, &"cd".repeat(32)));
        assert!(!TunnelAuthorizer::is_allowed(&a, &[0xcd; 32]));
        assert!(!PeerAuthorizer::is_allowed(&a, "pas une clé"));
        assert!(!PeerAuthorizer::is_allowed(&a, &key));
    }

    #[test]
    fn share_mods_ne_concerne_que_les_fichiers() {
        let (a, access, flag, key) = authz(true);
        access.allow_friend(&[0xab; 32]);
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
        access.allow_friend(&[0xab; 32]);
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
        access.allow_friend(&[0xab; 32]);
        assert!(TunnelAuthorizer::is_allowed(&a, &[0xab; 32]));
        access.allow_members(g, [[1u8; 32]]);
        assert!(!TunnelAuthorizer::is_allowed(&a, &[0xab; 32]), "un invité solo n'atteint pas le serveur du groupe");
        assert!(!PeerAuthorizer::is_allowed(&a, &"ab".repeat(32)), "ni son manifeste");
        assert!(TunnelAuthorizer::is_allowed(&a, &[1; 32]));
        // Une demande à rejoindre acceptée pendant le groupe n'ouvre rien : ni maintenant, ni pour le groupe.
        assert!(!access.allow_friend(&[0xcd; 32]));
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
    fn ami_retire_perd_son_acces_d_invite() {
        let (a, access, _, key) = authz(true);
        access.allow_friend(&[0xab; 32]);
        assert!(a.db.remove_friend(&key).is_ok());
        assert!(!TunnelAuthorizer::is_allowed(&a, &[0xab; 32]));
    }
}
