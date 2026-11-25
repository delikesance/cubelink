//! Commandes amis : code ami, liste, demandes. Le transport passe par `P2pBackend` (provisoire).

use std::sync::Arc;

use core_lib::types::{Error, Friend, FriendCode, FriendRequest, Presence, VerifyCode};
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

/// Écrit le jeton (hex) de façon atomique : fichier temporaire puis renommage.
fn write_token(path: &std::path::Path, token: &friend_code::Token) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, hex::encode(token))?;
    std::fs::rename(&tmp, path)
}

impl AppState {
    /// Jeton courant du code ami. En mémoire d'abord ; sinon lu dans le fichier ; sinon (premier lancement, ou
    /// migration depuis l'ancien `friend_code_epoch`) tiré au hasard : les anciens codes sont alors périmés, ce
    /// qui est voulu.
    pub fn code_token(&self) -> friend_code::Token {
        let mut cur = lock(&self.code_token);
        if let Some(t) = *cur {
            return t;
        }
        let stored = std::fs::read_to_string(&self.paths.code_token_file)
            .ok()
            .and_then(|s| hex::decode(s.trim()).ok())
            .and_then(|b| friend_code::Token::try_from(b).ok());
        let token = stored.unwrap_or_else(|| {
            let t = friend_code::new_token();
            // Écriture ratée : le jeton reste valable pour cette session (le code affiché et le jeton
            // accepté restent identiques) ; au prochain lancement, un nouveau sera tiré.
            let _ = write_token(&self.paths.code_token_file, &t);
            let _ = std::fs::remove_file(&self.paths.legacy_epoch_file);
            t
        });
        *cur = Some(token);
        token
    }

    fn code_payload(&self, token: friend_code::Token) -> FriendCodePayload {
        let (relay, addrs) = self.backend().code_hints();
        FriendCodePayload { public_key: self.identity.public_key().to_bytes(), token, relay, addrs }
    }

    fn encode_code(&self, token: friend_code::Token) -> FriendCode {
        FriendCode(friend_code::encode(&self.code_payload(token)))
    }

    /// Mon lien ami `cubelink://add/<code>` : même charge utile et même jeton que `friend_my_code`.
    pub fn friend_my_link(&self) -> String {
        friend_code::encode_link(&self.code_payload(self.code_token()))
    }

    /// Reprend (une seule fois) le lien ami reçu du système et pas encore lu par l'interface.
    pub fn friend_take_pending_link(&self) -> Option<String> {
        lock(&self.pending_link).take()
    }

    pub fn profile_name(&self) -> Option<String> {
        match lock(&self.profile).clone() {
            Some(core_lib::types::Profile::Online { name, .. }) | Some(core_lib::types::Profile::Offline { name, .. }) => Some(name),
            None => None,
        }
    }

    pub fn friend_my_code(&self) -> FriendCode {
        self.encode_code(self.code_token())
    }

    pub fn friend_regenerate_code(&self) -> Result<FriendCode, String> {
        let next = friend_code::new_token();
        // Le verrou est pris avant l'écriture : un `code_token()` concurrent (premier lancement) ne peut pas
        // écraser le fichier avec un autre jeton que celui de la mémoire.
        let mut cur = lock(&self.code_token);
        // Écrit d'abord : si le disque refuse, ni le code affiché ni le jeton accepté ne changent.
        write_token(&self.paths.code_token_file, &next).map_err(|_| "impossible d'enregistrer le nouveau code".to_string())?;
        let _ = std::fs::remove_file(&self.paths.legacy_epoch_file);
        *cur = Some(next);
        drop(cur);
        // Effet immédiat sur le réseau : l'ancien code ne permet plus de NOUVELLE demande. Les amis déjà
        // acceptés et les demandes déjà admises ne sont pas touchés.
        self.backend().set_code_token(next);
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
        let payload = friend_code::parse(code).map_err(|e| match e {
            // Code d'une autre version : le dire plutôt que « invalide » (le code est correct, le format a changé).
            Error::Invalid(m) if m == friend_code::LEGACY_CODE_MSG || m == friend_code::NEWER_CODE_MSG => m,
            _ => "code ami invalide : vérifiez qu'il commence par mcl1-".to_string(),
        })?;
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

    /// Annule la demande d'ami envoyée avec ce code : le pair est libéré tout de suite et la demande disparaît chez
    /// lui. Idempotent (rien en cours, ou déjà décidée : `Ok`). Pas de vérification « déjà ami » ici : on peut annuler
    /// juste après une acceptation.
    pub fn friend_cancel_request(&self, code: &str) -> Result<(), String> {
        let payload = friend_code::parse(code).map_err(|_| "code ami invalide : vérifiez qu'il commence par mcl1-".to_string())?;
        let key = hex::encode(payload.public_key);
        self.backend().cancel_request(&key);
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

    /// Règle le niveau de confiance d'un ami : `Always` = « Installer ses mods sans me demander » (jars inconnus ou non
    /// vérifiés de CET ami, voir `join_plan`). Idempotent ; ami inconnu : refusé.
    pub fn friend_set_trust(&self, public_key: &str, trust: core_lib::types::TrustLevel) -> Result<(), String> {
        valid_key(public_key)?;
        self.friends.set_trust(public_key, trust).map_err(|e| match e {
            core_lib::types::Error::NotFound(_) => "cet ami est inconnu".to_string(),
            e => user_msg(&e),
        })
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

/// Mon code ami (`mcl1-…`, format v2) : clé publique, **jeton aléatoire de 16 octets** (fichier `p2p/friend_code_token`, créé au premier appel), indices de connexion. Ce n'est pas un secret d'authentification (le code se partage) : le jeton sert à couper les anciens codes.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub fn friend_my_code(state: State<'_, Arc<AppState>>) -> Result<FriendCode, String> {
    Ok(state.friend_my_code())
}

/// Mon lien ami `cubelink://add/<code>` (bouton « Copier mon lien »). Même charge utile que `friend_my_code`, donc même jeton : « Générer un nouveau code » invalide aussi les anciens liens. Un clic sur ce lien ne fait que préremplir « Ajouter un ami » chez celui qui le reçoit.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub fn friend_my_link(state: State<'_, Arc<AppState>>) -> Result<String, String> {
    Ok(state.friend_my_link())
}

/// Lien ami `cubelink://add/…` ouvert depuis le système (clic, `xdg-open`, argument de lancement) et pas encore repris : le rend puis l'oublie (`null` si rien n'attend). Appelé au démarrage de l'interface et à chaque événement `friend://link`. Le lien n'est jamais validé ni envoyé ici : l'interface le met dans « Ajouter un ami », l'envoi reste un geste de l'utilisateur.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub fn friend_take_pending_link(state: State<'_, Arc<AppState>>) -> Result<Option<String>, String> {
    Ok(state.friend_take_pending_link())
}

/// Génère un nouveau jeton (écrit sur disque avant tout changement, puis pris en compte tout de suite par le réseau) et renvoie le nouveau code. L'ancien code est refusé pour les **nouvelles** demandes d'ami ; les amis déjà acceptés et les demandes déjà reçues ne sont pas touchés. La clé publique ne change pas : l'appareil reste joignable par la connexion de base (présence). L'ancien fichier `friend_code_epoch` est ignoré et supprimé : les codes `mcl1-` d'avant le jeton (version 1) sont périmés.
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

/// Envoie une demande d'ami à partir de son code ; renvoie le code de vérification à comparer. Erreurs propres au jeton : code d'une ancienne version (sans jeton, refusé sans connexion : « Ce code ami vient d'une ancienne version de Cubelink… »), code d'une version plus récente, code révoqué par l'hôte (« Ce code ami n'est plus valide : demande à ton ami de te renvoyer son code actuel. »).
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn friend_send_request(state: State<'_, Arc<AppState>>, code: String) -> Result<VerifyCode, String> {
    state.friend_send_request(&code).await
}

/// Annule la demande d'ami envoyée avec ce code (celui passé à `friend_send_request`) : le pair est libéré tout de suite (une nouvelle demande est possible) et la demande disparaît chez lui. Idempotent : rien en cours n'est pas une erreur. Aucun événement `friend://accepted` / `friend://refused` n'est émis pour une demande annulée.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub fn friend_cancel_request(state: State<'_, Arc<AppState>>, code: String) -> Result<(), String> {
    state.friend_cancel_request(&code)
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

/// Règle la confiance envers un ami : `always` = « Installer ses mods sans me demander » (ses jars inconnus ne demandent plus l'accord ; les scripts KubeJS et les exécutables restent bloqués ou demandés). Idempotent ; ami inconnu refusé.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub fn friend_set_trust(state: State<'_, Arc<AppState>>, public_key: String, trust: core_lib::types::TrustLevel) -> Result<(), String> {
    state.friend_set_trust(&public_key, trust)
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

    #[tokio::test]
    async fn mon_lien_porte_le_meme_code_et_le_meme_jeton() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        st.auth_offline_login("Alice").unwrap();
        let (code, link) = (st.friend_my_code().0, st.friend_my_link());
        assert_eq!(link, format!("cubelink://add/{code}"));
        assert_eq!(friend_code::parse(&link).unwrap(), friend_code::parse(&code).unwrap());
        // Le lien est accepté partout où le code l'est (ici : reconnu comme le propre code de l'utilisateur).
        assert!(st.friend_send_request(&link).await.unwrap_err().contains("propre code"));
        // « Générer un nouveau code » change aussi le lien.
        st.friend_regenerate_code().unwrap();
        assert_ne!(st.friend_my_link(), link);
        assert_ne!(friend_code::parse(&st.friend_my_link()).unwrap().token, friend_code::parse(&link).unwrap().token);
    }

    #[test]
    fn lien_en_attente_repris_une_seule_fois() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        assert_eq!(st.friend_take_pending_link(), None);
        *lock(&st.pending_link) = Some("cubelink://add/mcl1-abcd".into());
        assert_eq!(st.friend_take_pending_link().as_deref(), Some("cubelink://add/mcl1-abcd"));
        assert_eq!(st.friend_take_pending_link(), None);
    }

    #[test]
    fn annuler_une_demande_est_idempotent_et_rejette_un_code_invalide() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let other_dir = tempfile::tempdir().unwrap();
        let (other, _) = test_state(other_dir.path());
        let code = other.friend_my_code().0;
        // Rien en cours : pas une erreur, et aussi quand on recommence.
        st.friend_cancel_request(&code).unwrap();
        st.friend_cancel_request(&code).unwrap();
        for bad in ["", "abc", "mcl1-zzzz"] {
            assert!(st.friend_cancel_request(bad).unwrap_err().contains("code ami invalide"), "{bad}");
        }
    }

    #[test]
    fn regeneration_change_le_code_et_le_jeton() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let a = st.friend_my_code();
        assert_eq!(a, st.friend_my_code());
        let t_a = friend_code::parse(&a.0).unwrap().token;
        let b = st.friend_regenerate_code().unwrap();
        assert_ne!(a, b);
        assert_eq!(b, st.friend_my_code());
        let t_b = friend_code::parse(&b.0).unwrap().token;
        assert_ne!(t_a, t_b);
        // Persisté : un nouvel état sur le même dossier retrouve le même jeton.
        assert_eq!(std::fs::read_to_string(&st.paths.code_token_file).unwrap(), hex::encode(t_b));
        let (st2, _) = test_state(dir.path());
        assert_eq!(friend_code::parse(&st2.friend_my_code().0).unwrap().token, t_b);
    }

    #[test]
    fn migration_de_l_ancien_fichier_epoch_donne_un_jeton_neuf() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        std::fs::write(&st.paths.legacy_epoch_file, "1700000000").unwrap();
        let t = friend_code::parse(&st.friend_my_code().0).unwrap().token;
        assert_eq!(std::fs::read_to_string(&st.paths.code_token_file).unwrap(), hex::encode(t));
        assert!(!st.paths.legacy_epoch_file.exists(), "ancien fichier supprimé");
        // Un fichier de jeton illisible est remplacé, jamais interprété.
        let dir2 = tempfile::tempdir().unwrap();
        let (st2, _) = test_state(dir2.path());
        std::fs::write(&st2.paths.code_token_file, "pas un jeton").unwrap();
        let t2 = friend_code::parse(&st2.friend_my_code().0).unwrap().token;
        assert_eq!(std::fs::read_to_string(&st2.paths.code_token_file).unwrap(), hex::encode(t2));
    }

    #[test]
    fn regeneration_previent_le_backend_et_echec_d_ecriture_ne_change_rien() {
        use p2p_lib::friend_code::Token;
        use std::sync::Mutex;
        struct Rec(Mutex<Vec<Token>>);
        impl crate::p2p_backend::P2pBackend for Rec {
            fn presence(&self, _k: &str) -> Presence {
                Presence::Offline
            }
            fn set_code_token(&self, t: Token) {
                lock(&self.0).push(t);
            }
            fn send_request(&self, _t: FriendCodePayload, _n: String) -> crate::p2p_backend::BoxFuture<'_, Result<VerifyCode, String>> {
                Box::pin(async { Err("non".to_string()) })
            }
            fn fetch_manifest(&self, _k: String) -> crate::p2p_backend::BoxFuture<'_, Result<crate::p2p_backend::HostManifest, String>> {
                Box::pin(async { Err("non".to_string()) })
            }
            fn receive_files(
                &self,
                _k: String,
                _id: String,
                _p: p2p_lib::transfer::plan::Plan,
                _pr: crate::p2p_backend::ProgressFn,
                _c: Arc<crate::p2p_backend::TransferControl>,
            ) -> crate::p2p_backend::BoxFuture<'_, Result<(), Error>> {
                Box::pin(async { Err(Error::Other("non".to_string())) })
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let rec = Arc::new(Rec(Mutex::new(vec![])));
        st.set_backend(rec.clone());
        assert_eq!(lock(&rec.0).len(), 1, "le backend reçoit le jeton courant à son branchement");
        let before = st.friend_my_code();
        let next = st.friend_regenerate_code().unwrap();
        let sent = lock(&rec.0).clone();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[1], friend_code::parse(&next.0).unwrap().token);
        // Disque qui refuse : le code et le jeton accepté ne changent pas, le backend n'est pas prévenu.
        std::fs::remove_file(&st.paths.code_token_file).unwrap();
        std::fs::create_dir(&st.paths.code_token_file).unwrap();
        std::fs::create_dir(st.paths.code_token_file.with_extension("tmp")).unwrap();
        let e = st.friend_regenerate_code().unwrap_err();
        assert!(e.contains("impossible d'enregistrer"), "{e}");
        assert_eq!(st.friend_my_code(), next);
        assert_ne!(before, next);
        assert_eq!(lock(&rec.0).len(), 2);
    }

    #[test]
    fn code_d_une_ancienne_version_recoit_un_message_dedie() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        // Code v1 valide (CRC correct) : version 1, epoch u32, clé, relais vide, 0 adresse.
        let mut buf = vec![1u8, 0, 0, 0, 7];
        buf.extend_from_slice(&[5; 32]);
        buf.extend_from_slice(&[0, 0]);
        let mut crc = 0xFFFF_FFFFu32;
        for &b in &buf {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
            }
        }
        buf.extend_from_slice(&(!crc).to_be_bytes());
        // Base32 minuscule sans remplissage (RFC 4648).
        let alphabet = b"abcdefghijklmnopqrstuvwxyz234567";
        let bits: String = buf.iter().map(|b| format!("{b:08b}")).collect();
        let body: String = bits
            .as_bytes()
            .chunks(5)
            .map(|c| {
                let v = c.iter().fold(0usize, |acc, &x| acc << 1 | usize::from(x == b'1')) << (5 - c.len());
                alphabet[v] as char
            })
            .collect();
        let code = format!("mcl1-{body}");
        let e = st.check_friend_code(&code).unwrap_err();
        assert_eq!(e, friend_code::LEGACY_CODE_MSG);
        assert!(e.contains("ancienne version") && e.contains("mettre l'application à jour"));
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
    fn confiance_ami_reglable_idempotente_et_validee() {
        use core_lib::types::TrustLevel::{Always, Ask};
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let a = add_friend(&st, 'a');
        assert_eq!(st.friend_list().unwrap()[0].friend.trust, Ask, "refus par défaut");
        st.friend_set_trust(&a, Always).unwrap();
        st.friend_set_trust(&a, Always).unwrap();
        assert_eq!(st.friend_list().unwrap()[0].friend.trust, Always);
        st.friend_set_trust(&a, Ask).unwrap();
        assert_eq!(st.friend_list().unwrap()[0].friend.trust, Ask);
        assert!(st.friend_set_trust(&"c".repeat(64), Always).unwrap_err().contains("inconnu"));
        assert!(st.friend_set_trust("pas-une-cle", Always).unwrap_err().contains("invalide"));
        // Retirer puis ré-ajouter l'ami : la confiance ne survit pas.
        st.friend_set_trust(&a, Always).unwrap();
        st.friend_remove(&a).unwrap();
        let a = add_friend(&st, 'a');
        assert_eq!(st.friends.get_friend(&a).unwrap().unwrap().trust, Ask);
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
