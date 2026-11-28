//! Laissez-passer d'un invité pour le monde solo de l'hôte : d'où vient l'accès, jusqu'à quand il vaut, s'il a servi.
//!
//! Un accès donné par une invitation ou une demande acceptée ne vaut que `PASS_TTL` tant que l'ami n'est pas entré
//! (manifeste, fichiers ou tunnel). Une fois entré, il vaut jusqu'à la fin de la partie (`SessionAccess::clear_guests`).
//! Sans cette échéance, une invitation vieille d'une heure laissait l'ami entrer, et sa demande à rejoindre était
//! acceptée d'office sans que l'hôte ne voie rien.

use std::time::{Duration, Instant};

use serde::Serialize;

/// Validité d'un accès pas encore utilisé : la durée de vie d'une invitation.
pub const PASS_TTL: Duration = Duration::from_secs(5 * 60);

/// Origine d'un accès, montrée à l'hôte.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum PassOrigin {
    /// Invitation envoyée par l'hôte (« Inviter »).
    Invite,
    /// Demande à rejoindre acceptée par l'hôte.
    Request,
    /// Réglage « Laisser tous mes amis rejoindre mes mondes solo sans invitation ».
    AllFriends,
}

/// Accès d'un invité au monde solo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GuestPass {
    pub origin: PassOrigin,
    /// Échéance tant que l'invité n'est pas entré.
    pub until: Instant,
    /// L'invité a déjà utilisé l'accès : il ne périme plus avant la fin de la partie.
    pub entered: bool,
}

impl GuestPass {
    pub fn new(origin: PassOrigin, now: Instant, ttl: Duration) -> Self {
        Self { origin, until: now + ttl, entered: false }
    }

    /// L'accès vaut-il encore à `now` ?
    pub fn is_valid(&self, now: Instant) -> bool {
        self.entered || now < self.until
    }

    /// Invitation envoyée et pas encore utilisée (le bouton « Inviter » est alors grisé).
    pub fn is_pending(&self, now: Instant) -> bool {
        !self.entered && now < self.until
    }

    /// Renouvelle l'accès (nouvelle invitation, nouvelle acceptation) : l'échéance repart, l'origine d'origine est gardée
    /// (c'est elle que l'hôte voit), un invité entré le reste.
    pub fn renewed(self, now: Instant, ttl: Duration) -> Self {
        if self.entered {
            return self;
        }
        Self { until: now + ttl, ..self }
    }

    /// Invitation envoyée par l'hôte et pas encore utilisée.
    pub fn is_pending_invite(&self, now: Instant) -> bool {
        self.origin == PassOrigin::Invite && self.is_pending(now)
    }

    /// Secondes avant l'échéance (`None` une fois entré).
    pub fn remaining_secs(&self, now: Instant) -> Option<u64> {
        (!self.entered).then(|| self.until.saturating_duration_since(now).as_secs())
    }
}

/// Un invité vu par l'hôte (`host_guests`, événement `host://guests`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, specta::Type)]
pub struct HostGuest {
    pub public_key: String,
    pub origin: PassOrigin,
    /// Vrai une fois que l'ami a commencé à rejoindre (manifeste, fichiers ou tunnel).
    pub entered: bool,
    /// Échéance (secondes Unix) tant que l'ami n'est pas entré, sinon `null`.
    pub expires_at: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const TTL: Duration = Duration::from_secs(300);

    #[test]
    fn un_acces_inutilise_perime_a_l_echeance() {
        let t0 = Instant::now();
        let p = GuestPass::new(PassOrigin::Invite, t0, TTL);
        assert!(p.is_valid(t0) && p.is_pending(t0));
        assert!(p.is_valid(t0 + TTL - Duration::from_secs(1)));
        assert!(!p.is_valid(t0 + TTL), "périmé à l'échéance");
        assert!(!p.is_pending(t0 + TTL));
        assert_eq!(p.remaining_secs(t0 + Duration::from_secs(60)), Some(240));
    }

    #[test]
    fn un_acces_utilise_ne_perime_plus() {
        let t0 = Instant::now();
        let p = GuestPass { entered: true, ..GuestPass::new(PassOrigin::Request, t0, TTL) };
        assert!(p.is_valid(t0 + TTL * 10));
        assert!(!p.is_pending(t0), "un invité entré n'a plus d'invitation en attente");
        assert_eq!(p.remaining_secs(t0), None);
    }

    #[test]
    fn renouveler_repart_de_zero_sauf_pour_un_invite_entre() {
        let t0 = Instant::now();
        let later = t0 + Duration::from_secs(200);
        let p = GuestPass::new(PassOrigin::Invite, t0, TTL).renewed(later, TTL);
        assert_eq!((p.origin, p.until), (PassOrigin::Invite, later + TTL), "origine gardée");
        assert!(p.is_pending_invite(later));
        assert!(!GuestPass::new(PassOrigin::AllFriends, t0, TTL).is_pending_invite(t0), "seule une invitation grise « Inviter »");
        let entered = GuestPass { entered: true, ..GuestPass::new(PassOrigin::Request, t0, TTL) };
        assert_eq!(entered.renewed(later, TTL), entered);
    }
}
