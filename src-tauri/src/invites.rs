//! Invitations « rejoins mon monde » entre amis : file en mémoire (reçues) et commandes `friend_invite*`.
//!
//! Réception : le handler iroh (`p2p_lib::invite`) identifie l'émetteur par sa connexion, refuse les non-amis
//! et borne le message ; ici on retient l'invitation (une seule par ami : la nouvelle remplace l'ancienne),
//! on émet `friend://invite` (Invite) puis `friend://invites` (Vec<Invite>, à chaque changement de la file).
//! Une invitation disparaît à son échéance (`INVITE_TTL` au plus), quand l'ami annule (il arrête de partager,
//! message `revoke`), ou quand l'utilisateur l'ignore. Aucune persistance.
//!
//! Point d'accroche pour une notification système : l'événement `friend://invite` (payload `Invite`), émis
//! une fois par invitation reçue ; la file complète est dans `friend_invites` / `friend://invites`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use core_lib::types::Invite;
use p2p_lib::invite::InviteMessage;
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
        let res = self.backend().send_invite(public_key.to_string(), msg).await.map_err(|e| crate::error::sanitize(&e));
        if res.is_err() {
            lock(&self.invites_sent).remove(public_key);
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
        fn notify_accepted(&self, _f: Friend) -> BoxFuture<'_, Result<(), String>> {
            Box::pin(async { Ok(()) })
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
}
