//! Commandes amis : code ami, liste, demandes. Le transport passe par `P2pBackend` (provisoire).

use std::sync::Arc;

use core_lib::types::{Friend, FriendCode, FriendRequest, Presence, VerifyCode};
use p2p_lib::friend_code::{self, FriendCodePayload};
use serde::Serialize;
use tauri::State;

use crate::error::user_msg;
use crate::state::{lock, now_secs, AppState};

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct FriendWithPresence {
    pub friend: Friend,
    pub presence: Presence,
    /// Un jeu tourne chez cet ami : seul cas où on peut lui demander à rejoindre (faux hors ligne et pour une ancienne version).
    pub in_game: bool,
    /// Dernière fois vu en ligne (secondes Unix), si connu.
    pub last_seen: Option<i64>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct RequestsPage {
    pub items: Vec<FriendRequest>,
    pub total: usize,
}

const MAX_PAGE: usize = 500;

fn valid_key(k: &str) -> Result<(), String> {
    if k.len() == 64 && k.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        Ok(())
    } else {
        Err("clé publique invalide".into())
    }
}

impl AppState {
    fn code_epoch(&self) -> u32 {
        std::fs::read_to_string(&self.paths.epoch_file)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or_else(|| {
                let e = now_secs() as u32;
                let _ = std::fs::write(&self.paths.epoch_file, e.to_string());
                e
            })
    }

    fn encode_code(&self, epoch: u32) -> FriendCode {
        let (relay, addrs) = self.backend().code_hints();
        FriendCode(friend_code::encode(&FriendCodePayload {
            public_key: self.identity.public_key().to_bytes(),
            epoch,
            relay,
            addrs,
        }))
    }

    pub fn profile_name(&self) -> Option<String> {
        match lock(&self.profile).clone() {
            Some(core_lib::types::Profile::Online { name, .. }) | Some(core_lib::types::Profile::Offline { name, .. }) => Some(name),
            None => None,
        }
    }

    pub fn friend_my_code(&self) -> FriendCode {
        self.encode_code(self.code_epoch())
    }

    pub fn friend_regenerate_code(&self) -> Result<FriendCode, String> {
        let next = (now_secs() as u32).max(self.code_epoch().saturating_add(1));
        std::fs::write(&self.paths.epoch_file, next.to_string())
            .map_err(|_| "impossible d'enregistrer le nouveau code".to_string())?;
        Ok(self.encode_code(next))
    }

    pub fn friend_list(&self) -> Result<Vec<FriendWithPresence>, String> {
        let list = self.friends.list_friends().map_err(|e| user_msg(&e))?;
        Ok(list
            .into_iter()
            .map(|f| {
                let presence = self.backend().presence(&f.public_key);
                if presence == Presence::Online {
                    self.last_seen.touch(&f.public_key, now_secs());
                }
                let last_seen = self.last_seen.get(&f.public_key);
                let in_game = presence == Presence::Online && self.backend().in_game(&f.public_key);
                FriendWithPresence { friend: f, presence, in_game, last_seen }
            })
            .collect())
    }

    /// Validation locale du code (sans réseau) : format, pas soi-même, pas déjà ami.
    pub fn check_friend_code(&self, code: &str) -> Result<FriendCodePayload, String> {
        let payload = friend_code::parse(code).map_err(|_| "code ami invalide : vérifiez qu'il commence par mcl1-".to_string())?;
        let key = payload.public_key.iter().map(|b| format!("{b:02x}")).collect::<String>();
        if key == self.identity.public_key_hex() {
            return Err("c'est votre propre code ami".into());
        }
        if self.friends.is_friend(&key).map_err(|e| user_msg(&e))? {
            return Err("cette personne est déjà dans vos amis".into());
        }
        Ok(payload)
    }

    pub async fn friend_send_request(&self, code: &str) -> Result<VerifyCode, String> {
        let payload = self.check_friend_code(code)?;
        let Some(my_name) = self.profile_name() else {
            return Err("connectez-vous avant d'ajouter un ami".into());
        };
        self.backend().set_display_name(&my_name);
        self.backend().send_request(payload, my_name).await
    }

    pub fn friend_requests(&self, offset: usize, limit: usize, query: Option<String>) -> Result<RequestsPage, String> {
        let limit = limit.min(MAX_PAGE);
        let q = query.as_deref().map(str::trim).filter(|q| !q.is_empty());
        let items = self.friends.list_requests(offset, limit, q).map_err(|e| user_msg(&e))?;
        let total = match q {
            None => self.friends.request_count().map_err(|e| user_msg(&e))?,
            Some(_) => self.friends.list_requests(0, 1_000_000, q).map_err(|e| user_msg(&e))?.len(),
        };
        Ok(RequestsPage { items, total })
    }

    pub async fn friend_accept(&self, public_key: &str) -> Result<(), String> {
        valid_key(public_key)?;
        if let Some(name) = self.profile_name() {
            self.backend().set_display_name(&name);
        }
        if self.backend().accept_request(public_key.to_string()).await.map_err(|e| crate::error::sanitize(&e))?.is_some() {
            return Ok(());
        }
        self.friends.accept_request(public_key).map_err(|e| user_msg(&e))?;
        Ok(())
    }

    pub async fn friend_refuse(&self, public_key: &str) -> Result<(), String> {
        valid_key(public_key)?;
        if self.backend().refuse_request(public_key.to_string()).await.map_err(|e| crate::error::sanitize(&e))? {
            return Ok(());
        }
        self.friends.remove_request(public_key).map_err(|e| user_msg(&e))?;
        Ok(())
    }

    /// Retire un ami : il disparaît de la base (adresses comprises), perd l'accès d'invité en cours, et tout ce qui
    /// venait de lui est oublié (invitations, demandes à rejoindre, demande d'ami en file, dernière présence).
    /// Sans l'amitié, la sonde de présence est refusée et la revérification du tunnel (500 ms) coupe sa connexion.
    /// Idempotent : un ami déjà retiré n'est pas une erreur. Les groupes Hearth ne sont pas touchés.
    pub fn friend_remove(&self, public_key: &str) -> Result<(), String> {
        valid_key(public_key)?;
        let key = public_key;
        // Base d'abord : sans l'amitié, `may_join` refuse déjà, même si la révocation arrivait après.
        self.friends.remove_friend(key).map_err(|e| user_msg(&e))?;
        if let Some(k) = hex::decode(key).ok().and_then(|b| <[u8; 32]>::try_from(b).ok()) {
            self.access.revoke_guest(&k);
        }
        self.last_seen.forget(key);
        self.invites_forget_friend(key);
        self.friends.remove_request(key).map_err(|e| user_msg(&e))?;
        Ok(())
    }

    /// Refuse toutes les demandes. Les 20 premiers demandeurs sont prévenus (refus signé via le protocole ami,
    /// 3 s max chacun, au mieux) ; les autres, et ceux injoignables, sont simplement retirés localement.
    pub async fn friend_refuse_all(&self) -> Result<usize, String> {
        const NOTIFY_MAX: usize = 20;
        let total = self.friends.request_count().map_err(|e| user_msg(&e))?;
        let pending = self.friends.list_requests(0, NOTIFY_MAX, None).map_err(|e| user_msg(&e))?;
        for r in pending {
            let backend = self.backend();
            let fut = backend.refuse_request(r.public_key.clone());
            let _ = tokio::time::timeout(std::time::Duration::from_secs(3), fut).await;
        }
        self.friends.remove_all_requests().map_err(|e| user_msg(&e))?;
        Ok(total)
    }
}

/// Test de connexion réseau (Paramètres → « Tester ma connexion »).
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn net_test(state: State<'_, Arc<AppState>>) -> Result<crate::p2p_backend::NetTest, String> {
    state.backend().net_test().await
}

/// Mon code ami (`mcl1-…`).
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub fn friend_my_code(state: State<'_, Arc<AppState>>) -> Result<FriendCode, String> {
    Ok(state.friend_my_code())
}

/// Génère un nouveau code ami (l'ancien ne marche plus).
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub fn friend_regenerate_code(state: State<'_, Arc<AppState>>) -> Result<FriendCode, String> {
    state.friend_regenerate_code()
}

/// Amis, avec leur présence, `in_game` (un jeu tourne chez l'ami : seul cas où « Demander à rejoindre » est proposé ; faux hors ligne et pour un ami d'une ancienne version) et la dernière fois vus en ligne.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub fn friend_list(state: State<'_, Arc<AppState>>) -> Result<Vec<FriendWithPresence>, String> {
    state.friend_list()
}

/// Envoie une demande d'ami à partir de son code ; renvoie le code de vérification à comparer.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn friend_send_request(state: State<'_, Arc<AppState>>, code: String) -> Result<VerifyCode, String> {
    state.friend_send_request(&code).await
}

/// Demandes d'amis reçues, par page, filtrées par nom.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub fn friend_requests(
    state: State<'_, Arc<AppState>>,
    offset: usize,
    limit: usize,
    query: Option<String>,
) -> Result<RequestsPage, String> {
    state.friend_requests(offset, limit, query)
}

/// Accepte une demande d'ami. Si la demande n'est plus décidable (expirée, redémarrage), la ligne est retirée et l'erreur est « Cette demande a expiré : demande à ton ami de la renvoyer. »
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn friend_accept(state: State<'_, Arc<AppState>>, public_key: String) -> Result<(), String> {
    state.friend_accept(&public_key).await
}

/// Refuse une demande d'ami. Réussit aussi si la demande a expiré (la ligne est simplement retirée).
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn friend_refuse(state: State<'_, Arc<AppState>>, public_key: String) -> Result<(), String> {
    state.friend_refuse(&public_key).await
}

/// Refuse toutes les demandes ; renvoie leur nombre.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn friend_refuse_all(state: State<'_, Arc<AppState>>) -> Result<usize, String> {
    state.friend_refuse_all().await
}

/// Retire un ami : accès d'invité en cours révoqué (le tunnel se coupe, la sonde de présence est refusée), invitations, demandes à rejoindre et demande en file de cet ami oubliées. Réussit aussi s'il n'est déjà plus ami ; les groupes Hearth ne sont pas touchés.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub fn friend_remove(state: State<'_, Arc<AppState>>, public_key: String) -> Result<(), String> {
    state.friend_remove(&public_key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::test_state;
    use core_lib::types::VerifyCode as Vc;
    use p2p_lib::friends_db::Direction;

    #[tokio::test]
    async fn code_invalide_refuse() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        st.auth_offline_login("Alice").unwrap();
        for bad in ["", "abc", "mcl1-zzzz", "mcl1-aaaa-bbbb"] {
            let e = st.friend_send_request(bad).await.unwrap_err();
            assert!(e.contains("code ami invalide"), "{bad} -> {e}");
        }
    }

    #[tokio::test]
    async fn propre_code_refuse_et_backend_indisponible() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        st.auth_offline_login("Alice").unwrap();
        let mine = st.friend_my_code();
        assert!(st.friend_send_request(&mine.0).await.unwrap_err().contains("propre code"));
        // Code d'un autre : valide, mais le protocole n'est pas branché.
        let other_dir = tempfile::tempdir().unwrap();
        let (other, _) = test_state(other_dir.path());
        let e = st.friend_send_request(&other.friend_my_code().0).await.unwrap_err();
        assert!(e.contains("pas encore disponible"), "{e}");
    }

    #[test]
    fn regeneration_change_le_code() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let a = st.friend_my_code();
        assert_eq!(a, st.friend_my_code());
        let b = st.friend_regenerate_code().unwrap();
        assert_ne!(a, b);
        assert_eq!(b, st.friend_my_code());
        assert!(friend_code::parse(&b.0).is_ok());
    }

    #[tokio::test]
    async fn demandes_acceptation_refus() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let mk = |c: char, n: &str| FriendRequest {
            public_key: c.to_string().repeat(64),
            name: n.into(),
            verify_code: Vc("3F9A · C21B · 77D0 · 5E14".into()),
            received_at: 10,
        };
        for (c, n) in [('a', "Bob"), ('b', "Carl"), ('c', "Dan")] {
            st.friends.upsert_request(&mk(c, n), Direction::In).unwrap();
        }
        let page = st.friend_requests(0, 2, None).unwrap();
        assert_eq!((page.items.len(), page.total), (2, 3));
        assert_eq!(st.friend_requests(0, 10, Some("Ca".into())).unwrap().total, 1);
        st.friend_accept(&"a".repeat(64)).await.unwrap();
        assert_eq!(st.friend_list().unwrap().len(), 1);
        assert_eq!(st.friend_list().unwrap()[0].presence, Presence::Offline);
        st.friend_refuse(&"b".repeat(64)).await.unwrap();
        assert_eq!(st.friend_refuse_all().await.unwrap(), 1);
        assert!(st.friend_accept("pas-une-cle").await.is_err());
    }

    fn add_friend(st: &AppState, c: char) -> String {
        let key = c.to_string().repeat(64);
        st.friends
            .add_friend(&Friend { public_key: key.clone(), name: format!("Ami-{c}"), trust: core_lib::types::TrustLevel::Ask, added_at: 1 })
            .unwrap();
        key
    }

    #[tokio::test]
    async fn retrait_d_un_ami_oublie_tout_ce_qui_venait_de_lui() {
        let dir = tempfile::tempdir().unwrap();
        let (st, events) = test_state(dir.path());
        let (a, b) = (add_friend(&st, 'a'), add_friend(&st, 'b'));
        let bytes = |c: u8| [c; 32];
        st.friends.set_friend_addr(&a, &p2p_lib::friends_db::PeerAddr { relay: None, addrs: vec!["1.2.3.4:5".into()] }).unwrap();
        st.access.allow_friend(&bytes(0xaa));
        st.access.allow_friend(&bytes(0xbb));
        st.last_seen.touch(&a, 5000);
        st.last_seen.touch(&b, 5000);
        for k in [&a, &b] {
            st.invite_received(k, p2p_lib::invite::InviteMessage { world_name: "M".into(), minecraft: "1.21.1".into(), expires_at: now_secs() + 60, revoke: false, pack: None, icon: None });
            st.join_request_received(k);
        }
        st.friends
            .upsert_request(&FriendRequest { public_key: a.clone(), name: "Ami-a".into(), verify_code: Vc("3F9A · C21B · 77D0 · 5E14".into()), received_at: 1 }, Direction::In)
            .unwrap();

        st.friend_remove(&a).unwrap();

        assert!(!st.friends.is_friend(&a).unwrap());
        assert_eq!(st.friends.get_friend_addr(&a).unwrap(), None, "adresse oubliée");
        assert!(!st.access.contains(&bytes(0xaa)), "accès d'invité révoqué");
        assert!(st.access.contains(&bytes(0xbb)), "l'autre ami n'est pas touché");
        assert_eq!(st.last_seen.get(&a), None);
        assert_eq!(st.last_seen.get(&b), Some(5000));
        assert_eq!(st.friend_invites().iter().map(|i| i.public_key.clone()).collect::<Vec<_>>(), std::slice::from_ref(&b));
        assert_eq!(st.friend_join_requests().iter().map(|r| r.public_key.clone()).collect::<Vec<_>>(), std::slice::from_ref(&b));
        assert_eq!(st.friend_requests(0, 10, None).unwrap().total, 0);
        let list = st.friend_list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].friend.public_key, b);
        assert!(events.lock().unwrap().iter().any(|(n, _)| n == "friend://invites"));
    }

    #[test]
    fn retrait_idempotent_et_cle_invalide_refusee() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let a = add_friend(&st, 'a');
        st.friend_remove(&a).unwrap();
        st.friend_remove(&a).unwrap();
        st.friend_remove(&"c".repeat(64)).unwrap();
        assert!(st.friend_remove("pas-une-cle").unwrap_err().contains("invalide"));
        assert!(st.friend_remove(&"A".repeat(64)).is_err(), "majuscules refusées comme ailleurs");
    }

    #[test]
    fn retrait_garde_l_acces_de_groupe() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let a = add_friend(&st, 'a');
        let g = p2p_lib::hearth::model::GroupId([1; 16]);
        st.access.allow_friend(&[0xaa; 32]);
        st.access.allow_members(g, [[0xaa; 32]]);
        st.friend_remove(&a).unwrap();
        assert!(st.access.contains(&[0xaa; 32]), "il reste membre du groupe");
        assert!(!st.access.allows(&crate::authz::SessionTarget::Solo, &[0xaa; 32]));
    }
}
