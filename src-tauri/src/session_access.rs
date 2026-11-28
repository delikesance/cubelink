//! Pairs autorisés à rejoindre la partie en cours de l'hôte, par cible (monde solo ou serveur d'un groupe).
//!
//! Monde solo : un laissez-passer par ami (`guest_pass::GuestPass`), qui périme s'il ne sert pas (`PASS_TTL`). Les
//! changements de la liste des invités sont annoncés à l'observateur (`host://guests` dans l'application).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use p2p_lib::hearth::model::GroupId;
use p2p_lib::presence::PeerKey;

use crate::guest_pass::{GuestPass, HostGuest, PassOrigin, PASS_TTL};
use crate::state::{lock, now_secs};

/// Ce que l'hôte sert en ce moment : un monde solo partagé, ou le serveur d'un groupe. L'accès se donne et se lit
/// toujours pour une cible précise : un ami invité à un monde solo n'a rien sur le serveur d'un groupe, et inversement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SessionTarget {
    Solo,
    Group(GroupId),
}

/// Reçoit la liste des invités du monde solo à chaque changement (appelé hors verrou, ne doit pas bloquer).
pub type GuestsObserver = Arc<dyn Fn(Vec<HostGuest>) + Send + Sync>;

#[derive(Default)]
struct AccessSets {
    /// Cible `Solo` : laissez-passer des amis invités ou acceptés (ils doivent rester amis acceptés).
    guests: HashMap<PeerKey, GuestPass>,
    /// Amis dont l'hôte a retiré l'accès pendant cette partie : « tous mes amis » ne les réinvite pas.
    removed: HashSet<PeerKey>,
    /// Cible `Group` : membres d'un groupe hébergé (Hearth), l'accès ne dépend pas d'une amitié avec l'hôte.
    groups: HashMap<GroupId, HashSet<PeerKey>>,
    /// Le groupe que sert le tunnel en ce moment (un seul port local par hôte). Absent : le monde solo, s'il y en a un.
    serving: Option<GroupId>,
}

/// Accès de la partie en cours. Vidé à `host_stop_sharing` : l'accès ne survit jamais à la partie pour laquelle il a
/// été donné.
pub struct SessionAccess {
    inner: Mutex<AccessSets>,
    ttl: Duration,
    observer: Mutex<Option<GuestsObserver>>,
    /// Sérialise les annonces : la dernière liste émise est toujours la plus récente.
    announcing: Mutex<()>,
}

impl Default for SessionAccess {
    fn default() -> Self {
        Self::with_ttl(PASS_TTL)
    }
}

impl SessionAccess {
    /// Comme `default`, avec une validité des laissez-passer réglable (tests).
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            inner: Mutex::default(),
            ttl,
            observer: Mutex::new(None),
            announcing: Mutex::new(()),
        }
    }

    /// Branche l'observateur de la liste des invités (un seul ; le dernier l'emporte).
    pub fn observe(&self, observer: GuestsObserver) {
        *lock(&self.observer) = Some(observer);
    }

    fn guests_changed(&self) {
        let _order = lock(&self.announcing);
        let observer = lock(&self.observer).clone();
        if let Some(f) = observer {
            f(self.guests());
        }
    }

    /// Donne (ou renouvelle) l'accès au monde solo à un ami. Vrai si l'accès est nouveau (aucun accès valide avant).
    /// Sans effet tant que l'hôte sert un groupe, et pour « tous mes amis » envers un ami que l'hôte a retiré.
    pub fn allow_friend(&self, key: &PeerKey, origin: PassOrigin) -> bool {
        let now = Instant::now();
        let newly = {
            let mut sets = lock(&self.inner);
            if sets.serving.is_some()
                || (origin == PassOrigin::AllFriends && sets.removed.contains(key))
            {
                return false;
            }
            if origin != PassOrigin::AllFriends {
                sets.removed.remove(key);
            }
            let prev = sets.guests.get(key).copied().filter(|p| p.is_valid(now));
            let pass = prev.map_or(GuestPass::new(origin, now, self.ttl), |p| {
                p.renewed(now, self.ttl)
            });
            sets.guests.insert(*key, pass);
            prev.is_none()
        };
        self.guests_changed();
        newly
    }

    /// Une invitation envoyée à cet ami (bouton « Inviter ») attend encore qu'il entre.
    pub fn has_pending_invite(&self, key: &PeerKey) -> bool {
        lock(&self.inner)
            .guests
            .get(key)
            .is_some_and(|p| p.is_pending_invite(Instant::now()))
    }

    /// Retire l'accès de cet ami s'il ne l'a pas encore utilisé (invitation qui n'a pas pu partir).
    pub fn revoke_unused_pass(&self, key: &PeerKey) {
        let removed = {
            let mut sets = lock(&self.inner);
            let unused = sets.guests.get(key).is_some_and(|p| !p.entered);
            unused && sets.guests.remove(key).is_some()
        };
        if removed {
            self.guests_changed();
        }
    }

    /// L'hôte a retiré l'accès de cet ami pendant cette partie (`remove_guest`).
    pub fn was_removed(&self, key: &PeerKey) -> bool {
        lock(&self.inner).removed.contains(key)
    }

    /// L'ami utilise son accès (manifeste, fichiers, tunnel) : il ne périme plus avant la fin de la partie.
    pub fn mark_entered(&self, key: &PeerKey) {
        let changed = {
            let mut sets = lock(&self.inner);
            let solo = sets.serving.is_none();
            match sets.guests.get_mut(key) {
                Some(p) if solo && !p.entered && p.is_valid(Instant::now()) => {
                    p.entered = true;
                    true
                }
                _ => false,
            }
        };
        if changed {
            self.guests_changed();
        }
    }

    /// L'hôte retire l'accès de cet ami au monde solo (« Retirer ») : « tous mes amis » ne le réinvite plus.
    pub fn remove_guest(&self, key: &PeerKey) {
        {
            let mut sets = lock(&self.inner);
            sets.guests.remove(key);
            sets.removed.insert(*key);
        }
        self.guests_changed();
    }

    /// Invités du monde solo dont l'accès vaut encore, triés par clé.
    pub fn guests(&self) -> Vec<HostGuest> {
        let (now, secs) = (Instant::now(), now_secs());
        let sets = lock(&self.inner);
        let mut list: Vec<HostGuest> = sets
            .guests
            .iter()
            .filter(|(_, p)| p.is_valid(now))
            .map(|(k, p)| HostGuest {
                public_key: hex::encode(k),
                origin: p.origin,
                entered: p.entered,
                expires_at: p.remaining_secs(now).map(|r| secs + r as i64),
            })
            .collect();
        list.sort_by(|a, b| a.public_key.cmp(&b.public_key));
        list
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
        {
            let mut sets = lock(&self.inner);
            sets.guests.remove(key);
            for set in sets.groups.values_mut() {
                set.remove(key);
            }
        }
        self.guests_changed();
    }

    /// Retire l'accès d'invité (monde solo) de ce pair. Les groupes Hearth ne sont pas touchés : l'appartenance à un
    /// groupe ne dépend pas de l'amitié (voir `docs/hearth.md`).
    pub fn revoke_guest(&self, key: &PeerKey) {
        lock(&self.inner).guests.remove(key);
        self.guests_changed();
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
        {
            let mut sets = lock(&self.inner);
            sets.guests.clear();
            sets.removed.clear();
            sets.groups.clear();
            sets.serving = None;
        }
        self.guests_changed();
    }

    /// Le monde solo est fermé : plus aucun invité pour lui. L'accès des groupes n'est pas touché.
    pub fn clear_guests(&self) {
        {
            let mut sets = lock(&self.inner);
            sets.guests.clear();
            sets.removed.clear();
        }
        self.guests_changed();
    }

    /// Vrai si ce pair a un accès valide, pour une cible quelconque (sans tenir compte de l'amitié).
    pub fn contains(&self, key: &PeerKey) -> bool {
        let sets = lock(&self.inner);
        sets.guests
            .get(key)
            .is_some_and(|p| p.is_valid(Instant::now()))
            || sets.groups.values().any(|s| s.contains(key))
    }

    /// Ce pair a-t-il accès à la cible `target` ?
    pub fn allows(&self, target: &SessionTarget, key: &PeerKey) -> bool {
        let sets = lock(&self.inner);
        match target {
            SessionTarget::Solo => sets
                .guests
                .get(key)
                .is_some_and(|p| p.is_valid(Instant::now())),
            SessionTarget::Group(g) => sets.groups.get(g).is_some_and(|s| s.contains(key)),
        }
    }

    /// La cible que sert le tunnel en ce moment, et si ce pair y a accès (un seul verrou : pas de cible qui change entre
    /// les deux lectures).
    pub fn served_for(&self, key: &PeerKey) -> (SessionTarget, bool) {
        let sets = lock(&self.inner);
        match sets.serving {
            Some(g) => (
                SessionTarget::Group(g),
                sets.groups.get(&g).is_some_and(|s| s.contains(key)),
            ),
            None => (
                SessionTarget::Solo,
                sets.guests
                    .get(key)
                    .is_some_and(|p| p.is_valid(Instant::now())),
            ),
        }
    }

    /// Vrai si l'hôte sert le serveur d'un groupe (les demandes à rejoindre ne s'appliquent alors pas).
    pub fn serving_group(&self) -> bool {
        lock(&self.inner).serving.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const K: PeerKey = [0xab; 32];

    #[test]
    fn un_acces_inutilise_perime_et_ne_laisse_plus_entrer() {
        let access = SessionAccess::with_ttl(Duration::ZERO);
        assert!(access.allow_friend(&K, PassOrigin::Invite));
        assert!(
            !access.allows(&SessionTarget::Solo, &K),
            "invitation échue : plus d'accès"
        );
        assert!(!access.contains(&K));
        assert!(access.guests().is_empty());
        access.mark_entered(&K);
        assert!(
            !access.allows(&SessionTarget::Solo, &K),
            "un accès échu ne se rattrape pas en entrant"
        );
    }

    #[test]
    fn entrer_fige_l_acces_jusqu_a_la_fin_de_partie() {
        let access = SessionAccess::default();
        access.allow_friend(&K, PassOrigin::Invite);
        assert!(access.has_pending_invite(&K));
        let g = access.guests();
        assert_eq!((g[0].origin, g[0].entered), (PassOrigin::Invite, false));
        assert!(g[0].expires_at.is_some_and(|t| t > now_secs()));
        access.mark_entered(&K);
        assert!(
            !access.has_pending_invite(&K),
            "entré : plus d'invitation en attente"
        );
        access.revoke_unused_pass(&K);
        assert!(
            access.contains(&K),
            "un invité entré garde son accès si une invitation échoue"
        );
        assert_eq!(
            (access.guests()[0].entered, access.guests()[0].expires_at),
            (true, None)
        );
        access.clear_guests();
        assert!(!access.contains(&K));
    }

    #[test]
    fn un_ami_retire_par_l_hote_n_est_pas_reinvite_d_office() {
        let access = SessionAccess::default();
        access.allow_friend(&K, PassOrigin::AllFriends);
        access.remove_guest(&K);
        assert!(!access.contains(&K));
        assert!(
            !access.allow_friend(&K, PassOrigin::AllFriends),
            "« tous mes amis » ne le réinvite pas"
        );
        assert!(!access.contains(&K));
        assert!(
            access.allow_friend(&K, PassOrigin::Invite),
            "une invitation explicite le réinvite"
        );
        assert!(access.contains(&K));
        access.remove_guest(&K);
        access.clear_guests();
        assert!(
            access.allow_friend(&K, PassOrigin::AllFriends),
            "partie suivante : la liste des retraits est vidée"
        );
    }

    #[test]
    fn l_observateur_recoit_la_liste_a_chaque_changement() {
        let access = SessionAccess::default();
        let seen = Arc::new(Mutex::new(Vec::<usize>::new()));
        let s = seen.clone();
        access.observe(Arc::new(move |l| lock(&s).push(l.len())));
        access.allow_friend(&K, PassOrigin::Invite);
        access.allow_friend(&[1; 32], PassOrigin::Invite);
        access.mark_entered(&K);
        access.mark_entered(&K); // déjà entré : pas d'événement
        access.revoke_guest(&K);
        access.clear();
        assert_eq!(*lock(&seen), vec![1, 2, 2, 1, 0]);
    }
}
