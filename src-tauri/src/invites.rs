//! Invitations « rejoins mon monde » entre amis : file en mémoire (reçues) et commandes `friend_invite*`.
//!
//! Réception : le handler iroh (`p2p_lib::invite`) identifie l'émetteur par sa connexion, refuse les non-amis
//! et borne le message ; ici on retient l'invitation (une seule par ami : la nouvelle remplace l'ancienne),
//! on émet `friend://invite` (Invite) puis `friend://invites` (Vec<Invite>, à chaque changement de la file).
//! Une invitation disparaît à son échéance (`INVITE_TTL` au plus), quand l'ami annule (il arrête de partager,
//! message `revoke`), ou quand l'utilisateur l'ignore. Aucune persistance.
//!
//! Règle de jonction (docs/hearth.md § 2) : inviter un ami (`friend_invite`) lui ouvre l'accès (`SessionAccess`) à la
//! partie en cours, comme l'acceptation de sa demande à rejoindre. Demande reçue (hôte) : événement
//! `friend://join_request` (JoinRequest) puis `friend://join_requests` (Vec<JoinRequest>, à chaque changement) ;
//! décision par `friend_join_request_answer`. Demande envoyée (invité) : `friend_join_request` attend la décision.
//!
//! Point d'accroche pour une notification système : l'événement `friend://invite` (payload `Invite`), émis
//! une fois par invitation reçue ; la file complète est dans `friend_invites` / `friend://invites`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use core_lib::types::Invite;
use p2p_lib::invite::InviteMessage;
use p2p_lib::join_request::{JoinAnswer, ANSWER_WINDOW};
use serde::Serialize;
use tauri::State;

use base64::Engine;

use crate::error::user_msg;
use crate::state::{lock, now_secs, AppState};

/// Durée de vie d'une invitation (reçue ou émise).
pub const INVITE_TTL: Duration = Duration::from_secs(5 * 60);
/// Intervalle minimal entre deux invitations envoyées au même ami.
pub const SEND_MIN_INTERVAL: Duration = Duration::from_secs(10);
/// Nombre maximal d'invitations en attente (les plus anciennes sont abandonnées).
const MAX_QUEUE: usize = 50;

static SEQ: AtomicU64 = AtomicU64::new(1);

/// Demande à rejoindre reçue d'un ami, en attente de la décision de l'hôte (événement `friend://join_request`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JoinRequest {
    pub public_key: String,
    pub name: String,
    pub received_at: i64,
    /// Fin de la fenêtre de réponse : au-delà, le demandeur est prévenu que sa demande a expiré.
    pub expires_at: i64,
}

fn key_bytes(hex_key: &str) -> Option<[u8; 32]> {
    if hex_key.len() != 64 {
        return None;
    }
    hex::decode(hex_key).ok()?.try_into().ok()
}

impl AppState {
    fn emit_invites(&self) {
        let list = lock(&self.invites).clone();
        self.emit_event("friend://invites", &list);
    }

    /// Invitation reçue de l'ami `key` (hex). Ignorée si `key` n'est pas un ami ou si elle est déjà échue.
    /// Une annulation (`revoke`) retire les invitations de cet ami.
    pub fn invite_received(self: &Arc<Self>, key: &str, msg: InviteMessage) {
        if msg.revoke {
            self.invite_remove(|i| i.public_key == key);
            return;
        }
        let Some(friend) = self.friends.get_friend(key).ok().flatten() else { return };
        let now = now_secs();
        let expires_at = msg.expires_at.min(now + INVITE_TTL.as_secs() as i64);
        if msg.validate().is_err() || expires_at <= now {
            return;
        }
        let world_icon = msg.icon.as_deref().and_then(icon_data_url);
        let inv = Invite {
            id: format!("inv-{now}-{}", SEQ.fetch_add(1, Ordering::Relaxed)),
            public_key: key.to_string(),
            name: friend.name,
            world_name: msg.world_name,
            minecraft: msg.minecraft,
            received_at: now,
            expires_at,
            pack: msg.pack,
            world_icon,
        };
        {
            let mut q = lock(&self.invites);
            q.retain(|i| i.public_key != key);
            q.push(inv.clone());
            let extra = q.len().saturating_sub(MAX_QUEUE);
            q.drain(..extra);
        }
        self.emit_event("friend://invite", &inv);
        self.emit_invites();
        // Expiration : un réveil à l'échéance (la lecture de la file purge aussi, voir `friend_invites`).
        let me = Arc::downgrade(self);
        let wait = Duration::from_secs((expires_at - now).max(0) as u64) + Duration::from_millis(100);
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(wait).await;
            if let Some(st) = me.upgrade() {
                st.invites_purge_expired(now_secs());
            }
        });
    }

    fn emit_join_requests(&self) {
        let list = lock(&self.join_requests).clone();
        self.emit_event("friend://join_requests", &list);
    }

    /// Demande à rejoindre reçue de l'ami `key` (hex) : la retient (une seule par ami) et prévient l'interface.
    pub fn join_request_received(&self, key: &str) {
        let Some(friend) = self.friends.get_friend(key).ok().flatten() else { return };
        let now = now_secs();
        let req = JoinRequest { public_key: key.to_string(), name: friend.name, received_at: now, expires_at: now + ANSWER_WINDOW.as_secs() as i64 };
        {
            let mut q = lock(&self.join_requests);
            q.retain(|r| r.public_key != key);
            q.push(req.clone());
            let extra = q.len().saturating_sub(MAX_QUEUE);
            q.drain(..extra);
        }
        self.emit_event("friend://join_request", &req);
        self.emit_join_requests();
    }

    /// La demande de l'ami `key` n'attend plus rien (décidée, expirée, retirée ou annulée).
    pub fn join_request_ended(&self, key: &str) {
        let changed = {
            let mut q = lock(&self.join_requests);
            let before = q.len();
            q.retain(|r| r.public_key != key);
            q.len() != before
        };
        if changed {
            self.emit_join_requests();
        }
    }

    /// Le partage s'arrête : les demandes reçues ne valent plus rien.
    pub(crate) fn join_requests_clear(&self) {
        let had = {
            let mut q = lock(&self.join_requests);
            let had = !q.is_empty();
            q.clear();
            had
        };
        if had {
            self.emit_join_requests();
        }
    }

    /// Demandes à rejoindre en attente d'une décision.
    pub fn friend_join_requests(&self) -> Vec<JoinRequest> {
        lock(&self.join_requests).clone()
    }

    /// Hôte : accepte ou refuse la demande de l'ami `public_key`. Accepter ouvre l'accès à la partie en cours.
    pub fn friend_join_request_answer(&self, public_key: &str, accept: bool) -> Result<(), String> {
        self.check_friend(public_key)?;
        let decided = self.backend().answer_join_request(public_key, accept);
        self.join_request_ended(public_key);
        if decided {
            Ok(())
        } else {
            Err("cette demande n'est plus valable (expirée, retirée, ou rien n'est partagé)".into())
        }
    }

    /// Invité : demande à l'ami `public_key` de rejoindre sa partie ; attend sa décision.
    pub async fn friend_join_request(&self, public_key: &str) -> Result<JoinAnswer, String> {
        self.check_friend(public_key)?;
        if !lock(&self.join_requests_out).insert(public_key.to_string()) {
            return Err("une demande est déjà en attente chez cet ami".into());
        }
        let res = self.backend().send_join_request(public_key.to_string()).await.map_err(|e| crate::error::sanitize(&e));
        lock(&self.join_requests_out).remove(public_key);
        res
    }

    fn invite_remove(&self, pred: impl Fn(&Invite) -> bool) -> bool {
        let changed = {
            let mut q = lock(&self.invites);
            let before = q.len();
            q.retain(|i| !pred(i));
            q.len() != before
        };
        if changed {
            self.emit_invites();
        }
        changed
    }

    /// Retire les invitations échues à `now` (secondes) ; émet `friend://invites` si la file a changé.
    pub fn invites_purge_expired(&self, now: i64) {
        self.invite_remove(|i| i.expires_at <= now);
    }

    /// Invitations en attente (les échues sont purgées d'abord).
    pub fn friend_invites(&self) -> Vec<Invite> {
        self.invites_purge_expired(now_secs());
        lock(&self.invites).clone()
    }

    /// Ignore (ou consomme) une invitation ; sans effet si elle n'existe plus.
    pub fn friend_invite_dismiss(&self, id: &str) {
        self.invite_remove(|i| i.id == id);
    }

    /// Envoie à l'ami `public_key` une invitation à rejoindre notre monde ouvert.
    pub async fn friend_invite(&self, public_key: &str) -> Result<(), String> {
        let host = self.host_state().filter(|h| h.world_port.is_some()).ok_or("ouvrez d'abord votre monde au réseau local pour inviter un ami")?;
        self.check_friend(public_key)?;
        if self.access.serving_group() {
            return Err("ce serveur est celui d'un groupe : ajoute cet ami au groupe pour qu'il y joue".into());
        }
        let inst = self.instances.get(&host.instance_id).map_err(|e| user_msg(&e))?;
        {
            let mut sent = lock(&self.invites_sent);
            sent.retain(|_, t| t.elapsed() < INVITE_TTL);
            if sent.get(public_key).is_some_and(|t| t.elapsed() < SEND_MIN_INTERVAL) {
                return Err("invitation déjà envoyée à cet ami, patientez quelques secondes".into());
            }
            sent.insert(public_key.to_string(), Instant::now());
        }
        // Monde réellement ouvert (dernier `session.lock`) et son icône ; à défaut, le nom de l'instance.
        let world = self.instances.game_dir(&host.instance_id).ok().and_then(|d| core_lib::world::current_world(&d));
        let (world_name, icon) = match world {
            Some(w) => (clip(&w.name), w.icon_png.map(|b| B64.encode(b))),
            None => (clip(&inst.name), None),
        };
        let msg = InviteMessage {
            world_name,
            minecraft: inst.minecraft,
            expires_at: now_secs() + INVITE_TTL.as_secs() as i64,
            revoke: false,
            pack: Some(clip(&inst.name)),
            icon,
        };
        // L'accès précède l'invitation : l'ami qui la reçoit peut rejoindre aussitôt.
        let key = key_bytes(public_key).ok_or("cet ami est inconnu")?;
        let newly = self.access.allow_friend(&key);
        let res = self.backend().send_invite(public_key.to_string(), msg).await.map_err(|e| crate::error::sanitize(&e));
        if res.is_err() {
            lock(&self.invites_sent).remove(public_key);
            if newly {
                self.access.revoke(&key);
            }
        }
        res
    }

    /// Le partage s'arrête : prévient (au mieux) les amis invités récemment que l'invitation n'est plus valable.
    pub(crate) fn invites_revoke_sent(&self) {
        let keys: Vec<String> = {
            let mut sent = lock(&self.invites_sent);
            sent.retain(|_, t| t.elapsed() < INVITE_TTL);
            sent.drain().map(|(k, _)| k).collect()
        };
        if keys.is_empty() {
            return;
        }
        let backend = self.backend();
        tauri::async_runtime::spawn(async move {
            for k in keys {
                let _ = backend.send_invite(k, InviteMessage::revocation()).await;
            }
        });
    }
}

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// Texte d'invitation : sans caractère de contrôle, au plus 64 caractères.
fn clip(s: &str) -> String {
    let t: String = s.chars().filter(|c| !c.is_control()).take(64).collect();
    if t.trim().is_empty() { "Monde".into() } else { t }
}

/// Icône reçue (base64) → data URL, seulement si c'est un PNG d'au plus 64 px de côté.
fn icon_data_url(b64: &str) -> Option<String> {
    let bytes = B64.decode(b64).ok()?;
    core_lib::world::is_valid_icon(&bytes).then(|| format!("data:image/png;base64,{}", B64.encode(&bytes)))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn friend_invite(state: State<'_, Arc<AppState>>, public_key: String) -> Result<(), String> {
    state.friend_invite(&public_key).await
}

#[tauri::command(rename_all = "snake_case")]
pub fn friend_join_requests(state: State<'_, Arc<AppState>>) -> Result<Vec<JoinRequest>, String> {
    Ok(state.friend_join_requests())
}

#[tauri::command(rename_all = "snake_case")]
pub fn friend_join_request_answer(state: State<'_, Arc<AppState>>, public_key: String, accept: bool) -> Result<(), String> {
    state.friend_join_request_answer(&public_key, accept)
}

/// Renvoie `accepted`, `refused` ou `expired`.
#[tauri::command(rename_all = "snake_case")]
pub async fn friend_join_request(state: State<'_, Arc<AppState>>, public_key: String) -> Result<JoinAnswer, String> {
    state.friend_join_request(&public_key).await
}

#[tauri::command(rename_all = "snake_case")]
pub fn friend_invites(state: State<'_, Arc<AppState>>) -> Result<Vec<Invite>, String> {
    Ok(state.friend_invites())
}

#[tauri::command(rename_all = "snake_case")]
pub fn friend_invite_dismiss(state: State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
    state.friend_invite_dismiss(&id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands_host::HostState;
    use crate::p2p_backend::{BoxFuture, P2pBackend, ProgressFn, TransferControl};
    use crate::state::test_support::test_state;
    use core_lib::types::{Error, Friend, Loader, LoaderSpec, Presence, TrustLevel, VerifyCode};
    use p2p_lib::friend_code::FriendCodePayload;
    use p2p_lib::transfer::plan::Plan;
    use std::sync::Mutex;

    fn key(n: u8) -> String {
        hex::encode([n; 32])
    }

    fn add_friend(st: &AppState, n: u8, name: &str) {
        st.friends.add_friend(&Friend { public_key: key(n), name: name.into(), trust: TrustLevel::Ask, added_at: 1 }).unwrap();
    }

    fn msg(world: &str, ttl: i64) -> InviteMessage {
        InviteMessage { world_name: world.into(), minecraft: "1.21.1".into(), expires_at: now_secs() + ttl, revoke: false, pack: None, icon: None }
    }

    fn names(events: &crate::state::test_support::Events, ev: &str) -> usize {
        lock(events).iter().filter(|(n, _)| n == ev).count()
    }

    #[tokio::test]
    async fn modpack_et_icone_controlee_a_la_reception() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _events) = test_state(dir.path());
        add_friend(&st, 1, "Alice");
        add_friend(&st, 2, "Bob");
        // PNG 1×1 valide.
        let png = B64.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==").unwrap();
        st.invite_received(&key(1), InviteMessage { pack: Some("ATM10 Sky".into()), icon: Some(B64.encode(&png)), ..msg("Îles", 60) });
        st.invite_received(&key(2), InviteMessage { icon: Some(B64.encode(b"pas un png")), ..msg("Base", 60) });
        let q = st.friend_invites();
        assert_eq!(q[0].pack.as_deref(), Some("ATM10 Sky"));
        assert!(q[0].world_icon.as_deref().is_some_and(|u| u.starts_with("data:image/png;base64,")));
        assert_eq!((q[1].pack.as_deref(), q[1].world_icon.as_deref()), (None, None));
    }

    #[tokio::test]
    async fn reception_remplacement_dismiss_et_evenements() {
        let dir = tempfile::tempdir().unwrap();
        let (st, events) = test_state(dir.path());
        add_friend(&st, 1, "Alice");
        add_friend(&st, 2, "Bob");
        st.invite_received(&key(1), msg("Monde A", 60));
        let q = st.friend_invites();
        assert_eq!(q.len(), 1);
        assert_eq!((q[0].name.as_str(), q[0].world_name.as_str(), q[0].public_key.as_str()), ("Alice", "Monde A", key(1).as_str()));
        // Événement `friend://invite` : payload = Invite sérialisé avec les noms de champs du contrat.
        let first = lock(&events).iter().find(|(n, _)| n == "friend://invite").unwrap().1.clone();
        for f in ["id", "public_key", "name", "world_name", "minecraft", "received_at", "expires_at"] {
            assert!(first.get(f).is_some(), "champ {f}");
        }
        // Même ami : remplace.
        st.invite_received(&key(1), msg("Monde A2", 60));
        st.invite_received(&key(2), msg("Monde B", 60));
        let q = st.friend_invites();
        assert_eq!(q.iter().map(|i| i.world_name.as_str()).collect::<Vec<_>>(), ["Monde A2", "Monde B"]);
        // Non-ami ignoré ; message déjà échu ignoré.
        st.invite_received(&key(9), msg("Inconnu", 60));
        st.invite_received(&key(1), msg("Echu", -5));
        assert_eq!(st.friend_invites().len(), 2);
        assert_eq!(names(&events, "friend://invite"), 3);
        // Dismiss : un événement `friend://invites` de plus, id inconnu sans effet.
        let before = names(&events, "friend://invites");
        let id = q[0].id.clone();
        st.friend_invite_dismiss(&id);
        st.friend_invite_dismiss("nope");
        assert_eq!(st.friend_invites().len(), 1);
        assert_eq!(names(&events, "friend://invites"), before + 1);
        let last = lock(&events).iter().rev().find(|(n, _)| n == "friend://invites").unwrap().1.clone();
        assert_eq!(last.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn expiration_plafonnee_et_purge() {
        let dir = tempfile::tempdir().unwrap();
        let (st, events) = test_state(dir.path());
        add_friend(&st, 1, "Alice");
        // Une échéance lointaine est ramenée à la durée de vie maximale.
        st.invite_received(&key(1), msg("Monde", 24 * 3600));
        let inv = st.friend_invites().remove(0);
        assert!(inv.expires_at - inv.received_at <= INVITE_TTL.as_secs() as i64);
        let before = names(&events, "friend://invites");
        st.invites_purge_expired(inv.expires_at - 1);
        assert_eq!(st.friend_invites().len(), 1);
        st.invites_purge_expired(inv.expires_at);
        assert!(st.friend_invites().is_empty());
        assert_eq!(names(&events, "friend://invites"), before + 1);
    }

    #[tokio::test]
    async fn expiration_par_minuteur() {
        let dir = tempfile::tempdir().unwrap();
        let (st, events) = test_state(dir.path());
        add_friend(&st, 1, "Alice");
        st.invite_received(&key(1), msg("Monde", 1));
        assert_eq!(lock(&st.invites).len(), 1);
        for _ in 0..40 {
            if lock(&st.invites).is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(lock(&st.invites).is_empty(), "purgée par le minuteur");
        assert!(names(&events, "friend://invites") >= 2);
    }

    #[tokio::test]
    async fn annulation_de_l_ami_purge_ses_invitations() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        add_friend(&st, 1, "Alice");
        add_friend(&st, 2, "Bob");
        st.invite_received(&key(1), msg("A", 60));
        st.invite_received(&key(2), msg("B", 60));
        st.invite_received(&key(1), InviteMessage::revocation());
        let q = st.friend_invites();
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].public_key, key(2));
    }

    #[derive(Default)]
    struct Rec(Mutex<Vec<(String, InviteMessage)>>);
    impl P2pBackend for Rec {
        fn presence(&self, _k: &str) -> Presence {
            Presence::Online
        }
        fn send_request(&self, _t: FriendCodePayload, _n: String) -> BoxFuture<'_, Result<VerifyCode, String>> {
            Box::pin(async { Err("x".into()) })
        }
        fn fetch_manifest(&self, _k: String) -> BoxFuture<'_, Result<crate::p2p_backend::HostManifest, String>> {
            Box::pin(async { Err("x".into()) })
        }
        fn send_invite(&self, k: String, m: InviteMessage) -> BoxFuture<'_, Result<(), String>> {
            self.0.lock().unwrap().push((k, m));
            Box::pin(async { Ok(()) })
        }
        fn receive_files(&self, _k: String, _i: String, _p: Plan, _pr: ProgressFn, _c: Arc<TransferControl>) -> BoxFuture<'_, Result<(), Error>> {
            Box::pin(async { Ok(()) })
        }
    }

    #[tokio::test]
    async fn envoi_refuse_sans_monde_ouvert_puis_envoye_limite_et_annule_a_l_arret() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let rec = Arc::new(Rec::default());
        st.set_backend(rec.clone());
        add_friend(&st, 1, "Alice");
        let inst = st.instances.create("Mon Monde", "1.21.1", LoaderSpec { loader: Loader::Vanilla, version: String::new() }, 2048).unwrap();

        // Rien de partagé.
        let e = st.friend_invite(&key(1)).await.unwrap_err();
        assert!(e.contains("monde"), "{e}");
        // Partage de fichiers seulement (monde pas encore ouvert) : refusé aussi.
        *lock(&st.host) = Some(HostState { instance_id: inst.id.clone(), sharing: true, world_port: None });
        assert!(st.friend_invite(&key(1)).await.unwrap_err().contains("monde"));
        // Monde ouvert, mais ami inconnu.
        *lock(&st.host) = Some(HostState { instance_id: inst.id.clone(), sharing: true, world_port: Some(54321) });
        assert!(st.friend_invite(&key(7)).await.unwrap_err().contains("inconnu"));
        assert!(rec.0.lock().unwrap().is_empty());

        st.friend_invite(&key(1)).await.unwrap();
        {
            let sent = rec.0.lock().unwrap();
            assert_eq!(sent.len(), 1);
            assert_eq!((sent[0].0.as_str(), sent[0].1.world_name.as_str(), sent[0].1.minecraft.as_str()), (key(1).as_str(), "Mon Monde", "1.21.1"));
            assert!(!sent[0].1.revoke);
        }
        // Anti-spam côté émetteur.
        assert!(st.friend_invite(&key(1)).await.unwrap_err().contains("patientez"));
        assert_eq!(rec.0.lock().unwrap().len(), 1);

        // Arrêt du partage : annulation envoyée à l'ami invité.
        st.host_stop_sharing();
        for _ in 0..50 {
            if rec.0.lock().unwrap().len() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let sent = rec.0.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert!(sent[1].1.revoke && sent[1].0 == key(1));
    }

    #[tokio::test]
    async fn annonce_automatique_aux_amis_en_ligne_une_fois_le_monde_ouvert() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let rec = Arc::new(Rec::default());
        st.set_backend(rec.clone());
        add_friend(&st, 1, "Alice");
        add_friend(&st, 2, "Bob");
        let inst = st.instances.create("ATM10 Sky", "1.21.1", LoaderSpec { loader: Loader::NeoForge, version: "21.1.1".into() }, 0).unwrap();
        let mut sent = std::collections::HashMap::new();

        // Partage prêt mais monde pas encore ouvert : rien n'est annoncé.
        *lock(&st.host) = Some(HostState { instance_id: inst.id.clone(), sharing: true, world_port: None });
        st.announce_world(&mut sent).await;
        assert!(rec.0.lock().unwrap().is_empty());

        // Monde ouvert, mais « inviter tous mes amis » désactivé (défaut) : personne n'est invité d'office.
        *lock(&st.host) = Some(HostState { instance_id: inst.id.clone(), sharing: true, world_port: Some(54321) });
        assert!(!lock(&st.settings).open_to_all_friends);
        st.announce_world(&mut sent).await;
        assert!(rec.0.lock().unwrap().is_empty());
        assert!(!st.access.contains(&[1; 32]));

        // Réglage activé : chaque ami en ligne reçoit une invitation, une seule fois.
        lock(&st.settings).open_to_all_friends = true;
        st.announce_world(&mut sent).await;
        st.announce_world(&mut sent).await;
        let got = rec.0.lock().unwrap();
        let mut keys: Vec<_> = got.iter().map(|(k, _)| k.clone()).collect();
        keys.sort();
        assert_eq!(keys, vec![key(1), key(2)]);
        assert!(got.iter().all(|(_, m)| !m.revoke && m.pack.as_deref() == Some("ATM10 Sky")));
    }

    #[tokio::test]
    async fn inviter_ouvre_l_acces_et_l_arret_du_partage_le_retire() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let rec = Arc::new(Rec::default());
        st.set_backend(rec.clone());
        add_friend(&st, 1, "Alice");
        add_friend(&st, 2, "Bob");
        let inst = st.instances.create("Mon Monde", "1.21.1", LoaderSpec { loader: Loader::Vanilla, version: String::new() }, 2048).unwrap();
        *lock(&st.host) = Some(HostState { instance_id: inst.id.clone(), sharing: true, world_port: Some(54321) });

        assert!(!st.access.contains(&[1; 32]));
        st.friend_invite(&key(1)).await.unwrap();
        assert!(st.access.contains(&[1; 32]), "l'invité a accès à la partie");
        assert!(!st.access.contains(&[2; 32]), "les autres amis non");

        // Une invitation refusée (anti-spam) ne donne rien à un autre ami ; une erreur d'envoi reprend l'accès donné.
        assert!(st.friend_invite(&key(7)).await.is_err());
        assert!(!st.access.contains(&[7; 32]));

        st.host_stop_sharing();
        assert!(!st.access.contains(&[1; 32]), "accès vidé à l'arrêt du partage");
    }

    #[tokio::test]
    async fn demandes_a_rejoindre_file_evenements_et_nettoyage() {
        let dir = tempfile::tempdir().unwrap();
        let (st, events) = test_state(dir.path());
        add_friend(&st, 1, "Alice");
        add_friend(&st, 2, "Bob");
        st.join_request_received(&key(1));
        st.join_request_received(&key(9)); // non-ami : ignoré
        st.join_request_received(&key(2));
        st.join_request_received(&key(1)); // même ami : remplace
        let q = st.friend_join_requests();
        assert_eq!(q.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["Bob", "Alice"]);
        assert!(q.iter().all(|r| r.expires_at > r.received_at));
        assert_eq!(names(&events, "friend://join_request"), 3);
        let first = lock(&events).iter().find(|(n, _)| n == "friend://join_request").unwrap().1.clone();
        for f in ["public_key", "name", "received_at", "expires_at"] {
            assert!(first.get(f).is_some(), "champ {f}");
        }
        let before = names(&events, "friend://join_requests");
        st.join_request_ended(&key(1));
        st.join_request_ended(&key(1)); // déjà retirée : sans effet ni événement
        assert_eq!(names(&events, "friend://join_requests"), before + 1);
        assert_eq!(st.friend_join_requests().len(), 1);
        st.join_requests_clear();
        assert!(st.friend_join_requests().is_empty());
        let last = lock(&events).iter().rev().find(|(n, _)| n == "friend://join_requests").unwrap().1.clone();
        assert_eq!(last.as_array().unwrap().len(), 0);
        // Sans backend réel, répondre à une demande échoue clairement ; un inconnu aussi.
        assert!(st.friend_join_request_answer(&key(2), true).unwrap_err().contains("plus valable"));
        assert!(st.friend_join_request_answer(&key(9), true).unwrap_err().contains("inconnu"));
    }
}
