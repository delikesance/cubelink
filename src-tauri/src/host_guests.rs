//! Hôte : invités de la partie en cours, avec l'origine et l'échéance de leur accès (`host_guests`, événement
//! `host://guests`), et retrait d'un accès (`host_guest_revoke`). Invité : demandes à rejoindre envoyées qui attendent
//! encore la réponse de l'ami (`friend_join_asks`, événement `friend://join_asks`), pour griser « Demander ».

use std::sync::Arc;

use p2p_lib::invite::InviteMessage;
use tauri::State;

use crate::guest_pass::HostGuest;
use crate::invites::key_bytes;
use crate::state::{lock, AppState};

impl AppState {
    /// Invités du monde solo dont l'accès vaut encore.
    pub fn host_guests(&self) -> Vec<HostGuest> {
        self.access.guests()
    }

    /// Retire l'accès de l'ami `public_key` à mon monde solo : il ne peut plus entrer, son invitation disparaît chez lui,
    /// et « tous mes amis » ne le réinvite pas pendant cette partie. Une connexion en cours est coupée par le tunnel.
    pub fn host_guest_revoke(&self, public_key: &str) -> Result<(), String> {
        self.check_friend(public_key)?;
        let key = key_bytes(public_key).ok_or("cet ami est inconnu")?;
        self.access.remove_guest(&key);
        lock(&self.invites_sent).remove(public_key);
        let (backend, k) = (self.backend(), public_key.to_string());
        tauri::async_runtime::spawn(async move {
            let _ = backend.send_invite(k, InviteMessage::revocation()).await;
        });
        Ok(())
    }

    /// Amis à qui j'ai demandé à rejoindre et qui n'ont pas encore répondu (clés hex, triées).
    pub fn friend_join_asks(&self) -> Vec<String> {
        let mut keys: Vec<String> = lock(&self.join_requests_out).iter().cloned().collect();
        keys.sort();
        keys
    }

    pub(crate) fn emit_join_asks(&self) {
        self.emit_event("friend://join_asks", &self.friend_join_asks());
    }
}

/// Invités de ma partie en cours (monde solo), avec l'origine et l'échéance de leur accès.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub fn host_guests(state: State<'_, Arc<AppState>>) -> Result<Vec<HostGuest>, String> {
    Ok(state.host_guests())
}

/// Retire l'accès d'un ami à mon monde solo (invitation ou demande acceptée).
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub fn host_guest_revoke(
    state: State<'_, Arc<AppState>>,
    public_key: String,
) -> Result<(), String> {
    state.host_guest_revoke(&public_key)
}

/// Amis à qui j'ai demandé à rejoindre et qui n'ont pas encore répondu.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub fn friend_join_asks(state: State<'_, Arc<AppState>>) -> Result<Vec<String>, String> {
    Ok(state.friend_join_asks())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands_host::HostState;
    use crate::guest_pass::PassOrigin;
    use crate::p2p_backend::{BoxFuture, P2pBackend, ProgressFn, TransferControl};
    use crate::state::test_support::test_state;
    use core_lib::types::{Error, Friend, Loader, LoaderSpec, Presence, TrustLevel, VerifyCode};
    use p2p_lib::friend_code::FriendCodePayload;
    use p2p_lib::transfer::plan::Plan;
    use std::sync::Mutex;
    use std::time::Duration;

    #[derive(Default)]
    struct Sent(Mutex<Vec<(String, bool)>>);
    impl P2pBackend for Sent {
        fn presence(&self, _k: &str) -> Presence {
            Presence::Online
        }
        fn send_request(
            &self,
            _t: FriendCodePayload,
            _n: String,
        ) -> BoxFuture<'_, Result<VerifyCode, String>> {
            Box::pin(async { Err("x".into()) })
        }
        fn fetch_manifest(
            &self,
            _k: String,
        ) -> BoxFuture<'_, Result<crate::p2p_backend::HostManifest, String>> {
            Box::pin(async { Err("x".into()) })
        }
        fn send_invite(&self, k: String, m: InviteMessage) -> BoxFuture<'_, Result<(), String>> {
            self.0.lock().unwrap().push((k, m.revoke));
            Box::pin(async { Ok(()) })
        }
        fn receive_files(
            &self,
            _k: String,
            _i: String,
            _p: Plan,
            _pr: ProgressFn,
            _c: Arc<TransferControl>,
        ) -> BoxFuture<'_, Result<(), Error>> {
            Box::pin(async { Ok(()) })
        }
    }

    fn key(n: u8) -> String {
        hex::encode([n; 32])
    }

    fn hosting(
        dir: &std::path::Path,
    ) -> (Arc<AppState>, crate::state::test_support::Events, Arc<Sent>) {
        let (st, events) = test_state(dir);
        let sent = Arc::new(Sent::default());
        st.set_backend(sent.clone());
        for n in [1u8, 2] {
            st.friends
                .add_friend(&Friend {
                    public_key: key(n),
                    name: format!("Ami{n}"),
                    trust: TrustLevel::Ask,
                    added_at: 1,
                })
                .unwrap();
        }
        let inst = st
            .instances
            .create(
                "Monde",
                "1.21.1",
                LoaderSpec {
                    loader: Loader::Vanilla,
                    version: String::new(),
                },
                2048,
            )
            .unwrap();
        *lock(&st.host) = Some(HostState {
            instance_id: inst.id,
            sharing: true,
            world_port: Some(54321),
        });
        (st, events, sent)
    }

    fn count(events: &crate::state::test_support::Events, name: &str) -> usize {
        lock(events).iter().filter(|(n, _)| n == name).count()
    }

    #[tokio::test]
    async fn une_invitation_en_attente_ne_se_renvoie_pas_et_apparait_chez_l_hote() {
        let dir = tempfile::tempdir().unwrap();
        let (st, events, _) = hosting(dir.path());
        st.friend_invite(&key(1)).await.unwrap();
        lock(&st.invites_sent).clear(); // l'anti-spam de 10 s n'est pas ce qu'on teste
        let e = st.friend_invite(&key(1)).await.unwrap_err();
        assert!(e.contains("déjà envoyée"), "{e}");
        let g = st.host_guests();
        assert_eq!(g.len(), 1);
        assert_eq!(
            (g[0].public_key.as_str(), g[0].origin, g[0].entered),
            (key(1).as_str(), PassOrigin::Invite, false)
        );
        assert!(
            count(&events, "host://guests") >= 1,
            "la liste des invités est annoncée"
        );
        // Une fois entré, une nouvelle invitation est de nouveau possible (elle ne change rien à son accès).
        st.access.mark_entered(&[1; 32]);
        st.friend_invite(&key(1)).await.unwrap();
        let e = st.friend_invite(&key(1)).await.unwrap_err();
        assert!(e.contains("patientez"), "anti-spam de l'émetteur : {e}");
    }

    #[tokio::test]
    async fn retirer_un_invite_coupe_l_acces_previent_l_ami_et_bloque_tous_mes_amis() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _, sent) = hosting(dir.path());
        st.friend_invite(&key(1)).await.unwrap();
        st.host_guest_revoke(&key(1)).unwrap();
        assert!(!st.access.contains(&[1; 32]));
        assert!(st.host_guests().is_empty());
        for _ in 0..50 {
            if sent
                .0
                .lock()
                .unwrap()
                .iter()
                .any(|(k, r)| *r && *k == key(1))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            sent.0
                .lock()
                .unwrap()
                .iter()
                .any(|(k, r)| *r && *k == key(1)),
            "révocation envoyée à l'ami"
        );
        // « Tous mes amis » ne le réinvite pas ; une invitation explicite, si.
        let before = sent.0.lock().unwrap().len();
        st.invite_friend(&key(1), PassOrigin::AllFriends)
            .await
            .unwrap();
        assert!(!st.access.contains(&[1; 32]));
        assert_eq!(
            sent.0.lock().unwrap().len(),
            before,
            "aucune invitation envoyée"
        );
        st.friend_invite(&key(1)).await.unwrap();
        assert!(st.access.contains(&[1; 32]));
        assert!(st
            .host_guest_revoke(&key(9))
            .unwrap_err()
            .contains("inconnu"));
    }

    #[tokio::test]
    async fn une_demande_envoyee_est_annoncee_puis_retiree() {
        let dir = tempfile::tempdir().unwrap();
        let (st, events, _) = hosting(dir.path());
        assert!(st.friend_join_asks().is_empty());
        let _ = st.friend_join_request(&key(2)).await; // le faux backend répond tout de suite
        assert_eq!(
            count(&events, "friend://join_asks"),
            2,
            "envoi puis réponse"
        );
        let last = lock(&events)
            .iter()
            .rev()
            .find(|(n, _)| n == "friend://join_asks")
            .unwrap()
            .1
            .clone();
        assert_eq!(last.as_array().map(Vec::len), Some(0));
        assert!(st.friend_join_asks().is_empty());
    }
}
