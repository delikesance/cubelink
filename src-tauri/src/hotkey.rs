//! Raccourci global du panneau « en jeu » (amis, invitations, modpacks), même quand le jeu a le clavier ; ne fait rien hors du jeu. Notifications système des invitations, demandes d'ami et demandes à rejoindre (limitées en fréquence, un clic rouvre Cubelink). Rien ici ne doit empêcher l'application de démarrer.
//!
//! Le raccourci n'est enregistré que pendant qu'un jeu tourne (il ne vole donc pas `Ctrl+Maj+C` aux autres applications).

use std::collections::{HashMap, VecDeque};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use tauri::{AppHandle, Listener, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use tauri_plugin_notification::NotificationExt;

use crate::state::{lock, AppState};

/// État partagé (géré par Tauri).
#[derive(Default)]
pub struct HotkeyCtl {
    game_running: AtomicBool,
    registered: Mutex<Option<Shortcut>>,
}

/// Pur : `launch://state` -> le jeu tourne-t-il ? `None` si l'état ne change rien (préparation).
pub fn running_after(state: &str) -> Option<bool> {
    match state {
        "running" => Some(true),
        "exited" | "failed" => Some(false),
        _ => None,
    }
}

/// Pur : charge utile de `launch://state` -> le jeu tourne-t-il ? `None` si elle est illisible ou ne change rien.
pub fn running_from_payload(payload: &Value) -> Option<bool> {
    payload.get("state").and_then(Value::as_str).and_then(running_after)
}

/// État de lancement d'un jeu (appelé par Rust seul, depuis le sink d'événements) : enregistre ou libère le raccourci.
pub fn on_launch_state(app: &AppHandle, payload: &Value) {
    let Some(now) = running_from_payload(payload) else { return };
    let Some(ctl) = app.try_state::<Arc<HotkeyCtl>>() else { return };
    if ctl.game_running.swap(now, Ordering::SeqCst) != now {
        let h = app.clone();
        // Ne pas bloquer l'émetteur.
        std::thread::spawn(move || apply_settings(&h));
    }
}

/// Valide un raccourci (format du plugin) : erreur française claire sinon.
pub fn validate_hotkey(h: &str) -> Result<(), String> {
    let sc = Shortcut::from_str(h.trim())
        .map_err(|_| format!("raccourci invalide « {h} » : utilisez par exemple Ctrl+Shift+C ou Alt+F9"))?;
    let is_fkey = {
        let k = format!("{:?}", sc.key);
        k.len() >= 2 && k.starts_with('F') && k[1..].chars().all(|c| c.is_ascii_digit())
    };
    if sc.mods.is_empty() && !is_fkey {
        return Err("le raccourci doit contenir au moins une touche Ctrl, Alt, Maj ou Windows (ou être une touche F1 à F24)".into());
    }
    Ok(())
}

/// Fenêtre de la limitation de fréquence des notifications.
const NOTIFY_WINDOW: Duration = Duration::from_secs(60);
/// Plafond de notifications par fenêtre, tous pairs confondus : une rafale de clés différentes n'inonde pas le bureau.
/// Les demandes d'ami viennent d'inconnus : elles ont leur propre plafond, pour qu'une rafale de leur part ne rende pas
/// muettes les invitations et demandes à rejoindre des amis.
const NOTIFY_GLOBAL_MAX: usize = 5;

/// Titre de la notification d'une demande d'ami reçue.
const FRIEND_REQUEST_TITLE: &str = "Demande d'ami";
/// Titre de la notification d'une demande à rejoindre reçue.
const JOIN_REQUEST_TITLE: &str = "Demande à rejoindre";
/// Fin du titre de la notification d'une invitation reçue (« Léa t'invite à rejoindre son monde »).
const INVITE_TITLE_SUFFIX: &str = " t'invite à rejoindre son monde";

/// Pur : nom d'ami affichable (caractères de contrôle retirés, 48 caractères au plus). Le nom vient du réseau.
fn clean_name(raw: &str) -> String {
    raw.chars().filter(|c| !c.is_control()).take(48).collect()
}

/// Pur : titre et corps de la notification d'une demande d'ami reçue (`None` si la charge utile est inutilisable).
/// Le code de vérification n'y figure jamais : il se compare à l'écran, pas sur un bandeau visible à distance. Pas de
/// raccourci cité : le panneau « en jeu » ne traite pas les demandes d'ami, elles se traitent dans le launcher.
pub fn friend_request_notification(payload: &Value) -> Option<(String, String)> {
    let name = clean_name(payload.get("name")?.as_str()?);
    if name.trim().is_empty() {
        return None;
    }
    Some((FRIEND_REQUEST_TITLE.into(), format!("{name} veut t'ajouter en ami. Ouvre Cubelink pour répondre.")))
}

/// Titre de la notification d'une liaison perdue avec l'hôte.
const TUNNEL_LOST_TITLE: &str = "Connexion perdue";

/// Pur : notification d'une liaison perdue avec l'hôte (`tunnel://state` `lost`) ; `None` pour les autres états. Avec la
/// Scène, le jeu recouvre le bandeau du launcher : sans notification système, l'invité ne le voit pas.
pub fn tunnel_lost_notification(payload: &Value) -> Option<(String, String)> {
    (payload.get("state")?.as_str()? == "lost")
        .then(|| (TUNNEL_LOST_TITLE.into(), "La liaison avec ton ami est perdue : il a fermé son monde ou sa connexion a coupé. Ouvre Cubelink.".into()))
}

/// Les sortes de notification d'un ami.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Kind {
    Invite,
    Request,
    JoinRequest,
    TunnelLost,
}

impl Kind {
    /// Événement Tauri écouté (docs/contrats.md).
    fn event(self) -> &'static str {
        match self {
            Kind::Invite => "friend://invite",
            Kind::Request => "friend://request",
            Kind::JoinRequest => "friend://join_request",
            Kind::TunnelLost => "tunnel://state",
        }
    }

    fn build(self, payload: &Value, hotkey: Option<&str>) -> Option<(String, String)> {
        match self {
            Kind::Invite => invite_notification(payload, hotkey),
            Kind::Request => friend_request_notification(payload),
            Kind::JoinRequest => join_request_notification(payload, hotkey),
            Kind::TunnelLost => tunnel_lost_notification(payload),
        }
    }
}

/// Limiteur de notifications : au plus une par pair et par type par fenêtre (une minute), et `NOTIFY_GLOBAL_MAX` par minute en tout.
/// Il ne limite que le popup système : les événements arrivent toujours à la webview (la demande reste visible dans l'app).
#[derive(Default)]
pub struct NotifyLimiter {
    inner: Mutex<LimiterState>,
}

#[derive(Default)]
struct LimiterState {
    per_peer: HashMap<(Kind, String), Instant>,
    /// Envois récents : [0] les demandes d'ami (inconnus), [1] le reste (amis).
    global: [VecDeque<Instant>; 2],
}

impl NotifyLimiter {
    /// Une notification de ce `kind` pour ce pair (`key` : sa clé publique, vide si inconnue : limité alors seulement
    /// globalement) peut-elle partir à l'instant `now` ? Si oui, elle est comptée. Les entrées de plus d'une fenêtre sont
    /// purgées à chaque appel : la table reste bornée.
    fn allow(&self, kind: Kind, key: &str, now: Instant) -> bool {
        let mut st = lock(&self.inner);
        let fresh = |t: &Instant| now.saturating_duration_since(*t) < NOTIFY_WINDOW;
        st.per_peer.retain(|_, t| fresh(t));
        for g in st.global.iter_mut() {
            g.retain(fresh);
        }
        let class = usize::from(kind != Kind::Request);
        if !key.is_empty() && st.per_peer.contains_key(&(kind, key.to_string())) {
            return false;
        }
        if st.global[class].len() >= NOTIFY_GLOBAL_MAX {
            return false;
        }
        if !key.is_empty() {
            st.per_peer.insert((kind, key.to_string()), now);
        }
        st.global[class].push_back(now);
        true
    }
}

/// Pur : la notification à montrer pour cet événement, ou `None` (charge inutilisable, ou limitée en fréquence).
fn plan_notification(
    limiter: &NotifyLimiter,
    kind: Kind,
    payload: &Value,
    hotkey: Option<&str>,
    now: Instant,
) -> Option<(String, String)> {
    let notif = kind.build(payload, hotkey)?;
    let key = payload.get("public_key").or_else(|| payload.get("friend_public_key")).and_then(Value::as_str).unwrap_or_default();
    limiter.allow(kind, key, now).then_some(notif)
}

/// Où un clic sur une notification ramène l'utilisateur.
#[derive(Debug, PartialEq, Eq)]
enum ClickTarget {
    /// L'accueil du launcher (qui affiche les demandes d'ami, les invitations et le bandeau des demandes à rejoindre).
    Home,
    /// Le panneau « en jeu » quand la Scène est active (il porte les invitations et les demandes à rejoindre), sinon l'accueil.
    Panel,
}

/// Pur : cible du clic d'après le titre de la notification (`None` : inconnu, donc l'accueil).
fn click_target(title: Option<&str>) -> ClickTarget {
    match title {
        Some(t) if t == JOIN_REQUEST_TITLE || t.ends_with(INVITE_TITLE_SUFFIX) => ClickTarget::Panel,
        _ => ClickTarget::Home,
    }
}

/// Clic sur une notification de Cubelink : montre la fenêtre ; avec la Scène active, ouvre le panneau « en jeu » pour
/// les invitations et les demandes à rejoindre.
fn on_notification_click(app: &AppHandle, title: Option<&str>) {
    crate::background::show_main(app);
    if click_target(title) != ClickTarget::Panel {
        return;
    }
    let (Some(st), Some(w)) = (app.try_state::<Arc<AppState>>(), app.get_webview_window("main")) else { return };
    if st.stage.staged() && !st.stage.state().panel {
        st.stage.set_panel(&w, true);
    }
}

/// Pur : titre et corps de la notification d'invitation (`None` si la charge utile est inutilisable). `hotkey` : `None` quand
/// le raccourci n'existera pas (pas de Scene : le jeu a sa propre fenetre), le texte renvoie alors vers le launcher.
pub fn invite_notification(payload: &Value, hotkey: Option<&str>) -> Option<(String, String)> {
    let name = clean_name(payload.get("name")?.as_str()?);
    if name.trim().is_empty() {
        return None;
    }
    let world = payload.get("world_name").and_then(Value::as_str).map(clean_name).filter(|w| !w.trim().is_empty());
    let mut body = match world {
        Some(w) => format!("Monde : {w}. "),
        None => String::new(),
    };
    match hotkey {
        Some(hotkey) => body.push_str(&format!("Appuie sur {hotkey} en jeu pour répondre.")),
        None => body.push_str("Ouvre Cubelink pour répondre."),
    }
    Some((format!("{name}{INVITE_TITLE_SUFFIX}"), body))
}

/// Pur : le raccourci est-il (ou sera-t-il) enregistré ? Source unique pour l'enregistrement réel et pour le texte de la
/// notification d'invitation, afin qu'ils ne divergent plus. `staged` = le jeu en cours est dans la Scène (jamais « le
/// prochain jeu irait dans la Scène », qui reste vrai après un repli).
pub fn hotkey_available(enabled: bool, staged: bool, game_running: bool) -> bool {
    enabled && staged && game_running
}

/// Le raccourci à annoncer maintenant : `Some` seulement si `apply_settings` l'enregistre (ou va le faire).
fn hotkey_now(app: &AppHandle) -> Option<String> {
    let ctl = app.try_state::<Arc<HotkeyCtl>>()?;
    let (enabled, hotkey) = settings_of(app);
    let staged = app.try_state::<Arc<AppState>>().is_some_and(|s| s.stage.staged());
    hotkey_available(enabled, staged, ctl.game_running.load(Ordering::SeqCst)).then_some(hotkey)
}

/// Pur : titre et corps de la notification d'une demande à rejoindre reçue (`None` si la charge utile est inutilisable).
/// `hotkey` : comme pour l'invitation. Avec la Scene, le jeu recouvre le bandeau du launcher tant que le panneau est fermé :
/// le texte indique alors le raccourci qui l'ouvre.
pub fn join_request_notification(payload: &Value, hotkey: Option<&str>) -> Option<(String, String)> {
    let name = clean_name(payload.get("name")?.as_str()?);
    if name.trim().is_empty() {
        return None;
    }
    let reply = match hotkey {
        Some(hotkey) => format!("Appuie sur {hotkey} en jeu pour répondre."),
        None => "Ouvre Cubelink pour répondre.".to_string(),
    };
    Some((JOIN_REQUEST_TITLE.into(), format!("{name} veut rejoindre ta partie. {reply}")))
}

fn settings_of(app: &AppHandle) -> (bool, String) {
    match app.try_state::<Arc<AppState>>() {
        Some(st) => {
            let s = st.settings_get();
            (s.overlay_enabled, s.overlay_hotkey)
        }
        None => (true, core_lib::types::DEFAULT_OVERLAY_HOTKEY.to_string()),
    }
}

/// Ouvre ou ferme le panneau « en jeu ». Sans effet hors du jeu (chargement, rideau, launcher visible).
pub fn toggle(app: &AppHandle) {
    let (Some(st), Some(w)) = (app.try_state::<Arc<AppState>>(), app.get_webview_window("main")) else { return };
    let open = !st.stage.state().panel;
    st.stage.set_panel(&w, open);
}

/// Aligne l'enregistrement du raccourci sur l'état (réglages + jeu en cours + jeu affiché dans la fenêtre du launcher).
pub fn apply_settings(app: &AppHandle) {
    let Some(ctl) = app.try_state::<Arc<HotkeyCtl>>() else { return };
    let (enabled, hotkey) = settings_of(app);
    let staged = app.try_state::<Arc<AppState>>().is_some_and(|s| s.stage.staged());
    let available = hotkey_available(enabled, staged, ctl.game_running.load(Ordering::SeqCst));
    let gs = app.global_shortcut();
    let mut reg = lock(&ctl.registered);
    if let Some(old) = reg.take() {
        let _ = gs.unregister(old);
    }
    if !available {
        return;
    }
    match Shortcut::from_str(hotkey.trim()) {
        Ok(sc) => {
            let h = app.clone();
            let res = gs.on_shortcut(sc, move |_, _, ev| {
                if ev.state == ShortcutState::Pressed {
                    let h = h.clone();
                    std::thread::spawn(move || toggle(&h));
                }
            });
            match res {
                Ok(()) => *reg = Some(sc),
                Err(e) => tracing::warn!("raccourci « {hotkey} » non enregistré : {e}"),
            }
        }
        Err(e) => tracing::warn!("raccourci « {hotkey} » invalide : {e}"),
    }
}

/// Écoute le jeu, les invitations et les demandes. Appelée une fois depuis `setup` ; n'échoue jamais.
pub fn setup(app: &AppHandle, test_mode: bool) {
    app.manage(Arc::new(HotkeyCtl::default()));
    // `launch://state` n'est plus écouté : le webview ne peut plus émettre d'événement (voir capabilities/default.json) et l'état
    // arrive par appel direct depuis le sink d'événements (`on_launch_state`). Seul le mode test l'écoute, car les E2E
    // simulent l'état du jeu depuis la page.
    if test_mode {
        let h = app.clone();
        app.listen("launch://state", move |ev| {
            if let Ok(v) = serde_json::from_str::<Value>(ev.payload()) {
                on_launch_state(&h, &v);
            }
        });
        // Les E2E émettent des événements depuis la page (`plugin:event|emit`) : permission réservée au mode test.
        let cap = r#"{"identifier":"test-emit","windows":["main"],"permissions":["core:event:allow-emit","core:event:allow-emit-to"]}"#;
        if let Err(e) = app.add_capability(cap) {
            eprintln!("[Cubelink] capacité de test non ajoutée : {e}");
        }
    }
    // Repli : le jeu a repris sa propre fenêtre, le panneau « en jeu » n'existe plus pour lui ; le raccourci est rendu
    // aux autres applications.
    let h = app.clone();
    app.listen("stage://fallback", move |_| {
        let h = h.clone();
        std::thread::spawn(move || apply_settings(&h));
    });
    if test_mode {
        return;
    }
    // Un clic sur une notification de Cubelink ramène à la fenêtre (enregistré avant la première notification : le plugin
    // ne rapporte les clics que pour celles montrées après).
    let h = app.clone();
    if let Err(e) = app.notification().on_action(move |a| {
        if a.action_id() == "tap" {
            on_notification_click(&h, a.notification().and_then(|n| n.title()));
        }
    }) {
        eprintln!("[Cubelink] clic sur notification non suivi : {e}");
    }
    // Invitation, demande d'ami et demande à rejoindre : sans notification, un hôte ou un ami dont la fenêtre n'est pas à
    // l'écran ne les voit jamais, et elles expirent.
    let limiter = Arc::new(NotifyLimiter::default());
    for kind in [Kind::Invite, Kind::Request, Kind::JoinRequest, Kind::TunnelLost] {
        let h = app.clone();
        let limiter = limiter.clone();
        app.listen(kind.event(), move |ev| {
            let Ok(v) = serde_json::from_str::<Value>(ev.payload()) else { return };
            let Some((title, body)) = plan_notification(&limiter, kind, &v, hotkey_now(&h).as_deref(), Instant::now()) else {
                return;
            };
            if let Err(e) = h.notification().builder().title(title).body(body).show() {
                tracing::warn!("notification ({}) : {e}", kind.event());
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn etat_du_jeu_depuis_launch_state() {
        assert_eq!(running_after("running"), Some(true));
        assert_eq!(running_after("exited"), Some(false));
        assert_eq!(running_after("failed"), Some(false));
        assert_eq!(running_after("preparing"), None);
    }

    #[test]
    fn etat_du_jeu_depuis_la_charge_utile() {
        let p = |s: &str| serde_json::json!({ "id": "x", "state": s, "message": "" });
        assert_eq!(running_from_payload(&p("running")), Some(true));
        assert_eq!(running_from_payload(&p("exited")), Some(false));
        assert_eq!(running_from_payload(&p("failed")), Some(false));
        assert_eq!(running_from_payload(&p("preparing")), None);
        assert_eq!(running_from_payload(&serde_json::json!({ "state": 3 })), None);
        assert_eq!(running_from_payload(&serde_json::json!("running")), None);
    }

    #[test]
    fn raccourcis_valides_et_invalides() {
        for ok in ["Ctrl+Shift+C", "Alt+F9", "F8", "CommandOrControl+Shift+K", "Ctrl+Alt+Space"] {
            assert!(validate_hotkey(ok).is_ok(), "{ok}");
        }
        for bad in ["", "Ctrl+Shift+", "Ctrl+Pas", "C", "Espace", "Ctrl+A+B"] {
            let e = validate_hotkey(bad).unwrap_err();
            assert!(e.contains("raccourci"), "{bad}: {e}");
        }
    }

    #[test]
    fn hotkey_available_exige_les_trois_conditions() {
        for bits in 0..8u8 {
            let (e, s, r) = (bits & 1 != 0, bits & 2 != 0, bits & 4 != 0);
            assert_eq!(hotkey_available(e, s, r), bits == 7, "enabled={e} staged={s} running={r}");
        }
    }

    #[test]
    fn notification_sans_raccourci_quand_il_n_est_pas_enregistre() {
        let v = serde_json::json!({"name": "Alex"});
        // (panneau désactivé), (hors Scène / repli), (jeu non lancé) : le texte ne cite pas le raccourci.
        for (e, s, r) in [(false, true, true), (true, false, true), (true, true, false)] {
            let hk = hotkey_available(e, s, r).then_some("Ctrl+Shift+C");
            let (_, b) = invite_notification(&v, hk).unwrap();
            assert!(b.contains("Ouvre Cubelink") && !b.contains("Ctrl+Shift+C"), "{e} {s} {r}: {b}");
        }
        let hk = hotkey_available(true, true, true).then_some("Ctrl+Shift+C");
        assert!(invite_notification(&v, hk).unwrap().1.contains("Ctrl+Shift+C"));
    }

    #[test]
    fn notification_de_demande_a_rejoindre() {
        let v = serde_json::json!({"public_key": "ab", "name": "Alex", "received_at": 1, "expires_at": 121});
        let (t, b) = join_request_notification(&v, None).unwrap();
        assert_eq!(t, "Demande à rejoindre");
        assert_eq!(b, "Alex veut rejoindre ta partie. Ouvre Cubelink pour répondre.");
        let (_, b) = join_request_notification(&v, Some("Ctrl+Shift+C")).unwrap();
        assert_eq!(b, "Alex veut rejoindre ta partie. Appuie sur Ctrl+Shift+C en jeu pour répondre.");
        assert!(join_request_notification(&serde_json::json!({"public_key": "ab"}), None).is_none());
        assert!(join_request_notification(&serde_json::json!({"name": " \n "}), None).is_none());
        let long = serde_json::json!({"name": "a".repeat(500)});
        assert!(join_request_notification(&long, Some("F9")).unwrap().1.len() < 160);
    }

    #[test]
    fn notification_d_invitation() {
        let v = serde_json::json!({"name": "Alex", "world_name": "Survie"});
        let (t, b) = invite_notification(&v, Some("Ctrl+Shift+C")).unwrap();
        assert_eq!(t, "Alex t'invite à rejoindre son monde");
        assert!(b.contains("Survie") && b.contains("Ctrl+Shift+C"));
        let (_, b) = invite_notification(&v, None).unwrap();
        assert!(b.contains("Survie") && !b.contains("Ctrl") && b.contains("Ouvre Cubelink"));
        assert!(invite_notification(&serde_json::json!({"world_name": "x"}), Some("F9")).is_none());
        assert!(invite_notification(&serde_json::json!({"name": "  "}), Some("F9")).is_none());
        let long = serde_json::json!({"name": "a".repeat(500)});
        assert!(invite_notification(&long, Some("F9")).unwrap().0.len() < 100);
    }

    #[test]
    fn notification_de_demande_d_ami() {
        let v = serde_json::json!({"public_key": "ab", "name": "Léa", "verify_code": "123456", "received_at": 1});
        let (t, b) = friend_request_notification(&v).unwrap();
        assert_eq!(t, "Demande d'ami");
        assert_eq!(b, "Léa veut t'ajouter en ami. Ouvre Cubelink pour répondre.");
        assert!(!b.contains("123456") && !b.contains("ab veut"), "ni code de vérification ni clé : {b}");
        assert!(friend_request_notification(&serde_json::json!({"public_key": "ab"})).is_none());
        assert!(friend_request_notification(&serde_json::json!({"name": " \n "})).is_none());
        let sale = serde_json::json!({"name": "Al\u{0007}ex\n"});
        assert_eq!(friend_request_notification(&sale).unwrap().1, "Alex veut t'ajouter en ami. Ouvre Cubelink pour répondre.");
        let long = serde_json::json!({"name": "a".repeat(500)});
        assert!(friend_request_notification(&long).unwrap().1.len() < 120);
    }

    #[test]
    fn notification_de_liaison_perdue() {
        let lost = serde_json::json!({"state": "lost", "friend_public_key": "ab"});
        let (t, b) = tunnel_lost_notification(&lost).unwrap();
        assert_eq!(t, "Connexion perdue");
        assert!(b.contains("Ouvre Cubelink"));
        for s in ["ok", "reconnecting", "closed"] {
            assert!(tunnel_lost_notification(&serde_json::json!({"state": s})).is_none(), "{s}");
        }
        // Limitée par ami (clé `friend_public_key`).
        let l = NotifyLimiter::default();
        let t0 = Instant::now();
        assert!(plan_notification(&l, Kind::TunnelLost, &lost, None, t0).is_some());
        assert!(plan_notification(&l, Kind::TunnelLost, &lost, None, t0 + Duration::from_secs(1)).is_none());
    }

    #[test]
    fn limiteur_un_par_pair_et_par_minute() {
        let l = NotifyLimiter::default();
        let t0 = Instant::now();
        assert!(l.allow(Kind::Request, "a", t0));
        assert!(!l.allow(Kind::Request, "a", t0 + Duration::from_secs(1)));
        assert!(!l.allow(Kind::Request, "a", t0 + Duration::from_secs(59)));
        // Un autre pair, ou un autre type pour le même pair, passe.
        assert!(l.allow(Kind::Request, "b", t0 + Duration::from_secs(2)));
        assert!(l.allow(Kind::JoinRequest, "a", t0 + Duration::from_secs(3)));
        // Une minute plus tard, le pair est de nouveau notifié.
        assert!(l.allow(Kind::Request, "a", t0 + Duration::from_secs(61)));
    }

    #[test]
    fn limiteur_plafond_global_et_cle_vide() {
        let l = NotifyLimiter::default();
        let t0 = Instant::now();
        // Des clés toutes différentes : le plafond global coupe la rafale.
        for i in 0..NOTIFY_GLOBAL_MAX {
            assert!(l.allow(Kind::Request, &format!("k{i}"), t0), "{i}");
        }
        assert!(!l.allow(Kind::Request, "autre", t0 + Duration::from_secs(5)));
        // Les demandes d'ami ont leur propre plafond : celles des amis passent encore, jusqu'au leur.
        for i in 0..NOTIFY_GLOBAL_MAX {
            assert!(l.allow(Kind::JoinRequest, &format!("j{i}"), t0 + Duration::from_secs(5)), "{i}");
        }
        assert!(!l.allow(Kind::JoinRequest, "encore", t0 + Duration::from_secs(5)));
        assert!(!l.allow(Kind::TunnelLost, "encore", t0 + Duration::from_secs(5)));
        assert!(l.allow(Kind::Request, "autre", t0 + Duration::from_secs(61)));
        // Sans clé : pas de limite par pair, seulement le plafond global.
        let l = NotifyLimiter::default();
        for _ in 0..NOTIFY_GLOBAL_MAX {
            assert!(l.allow(Kind::Request, "", t0));
        }
        assert!(!l.allow(Kind::Request, "", t0));
    }

    #[test]
    fn limiteur_purge_et_refus_ne_consomment_pas() {
        let l = NotifyLimiter::default();
        let t0 = Instant::now();
        for i in 0..NOTIFY_GLOBAL_MAX {
            assert!(l.allow(Kind::Request, &format!("k{i}"), t0));
        }
        let t1 = t0 + Duration::from_secs(120);
        assert!(l.allow(Kind::Request, "nouveau", t1));
        {
            let st = lock(&l.inner);
            assert_eq!(st.per_peer.len(), 1, "les entrées expirées sont purgées");
            assert_eq!(st.global[0].len(), 1);
        }
        // Un refus (même pair) ne rallonge pas sa fenêtre : il repasse 60 s après la première notification.
        assert!(!l.allow(Kind::Request, "nouveau", t1 + Duration::from_secs(50)));
        assert!(l.allow(Kind::Request, "nouveau", t1 + Duration::from_secs(61)));
    }

    #[test]
    fn plan_limite_les_trois_sortes_par_pair() {
        let l = NotifyLimiter::default();
        let t0 = Instant::now();
        let req = serde_json::json!({"public_key": "ab", "name": "Léa", "verify_code": "123456"});
        let join = serde_json::json!({"public_key": "ab", "name": "Léa"});
        let inv = serde_json::json!({"public_key": "ab", "name": "Léa", "world_name": "Survie"});
        assert!(plan_notification(&l, Kind::Request, &req, None, t0).is_some());
        assert!(plan_notification(&l, Kind::Request, &req, None, t0).is_none(), "spam de demandes d'ami");
        assert!(plan_notification(&l, Kind::JoinRequest, &join, None, t0).is_some());
        assert!(plan_notification(&l, Kind::JoinRequest, &join, None, t0).is_none());
        assert!(plan_notification(&l, Kind::Invite, &inv, Some("F9"), t0).is_some());
        assert!(plan_notification(&l, Kind::Invite, &inv, Some("F9"), t0).is_none());
        // Une charge inutilisable ne consomme pas de quota.
        let l = NotifyLimiter::default();
        assert!(plan_notification(&l, Kind::Request, &serde_json::json!({"public_key": "ab"}), None, t0).is_none());
        assert!(plan_notification(&l, Kind::Request, &req, None, t0).is_some());
    }

    #[test]
    fn cible_du_clic() {
        assert_eq!(click_target(Some("Demande à rejoindre")), ClickTarget::Panel);
        let (t, _) = invite_notification(&serde_json::json!({"name": "Léa"}), None).unwrap();
        assert_eq!(click_target(Some(&t)), ClickTarget::Panel);
        let (t, _) = friend_request_notification(&serde_json::json!({"name": "Léa"})).unwrap();
        assert_eq!(click_target(Some(&t)), ClickTarget::Home);
        assert_eq!(click_target(Some("Cubelink reste ouvert en arrière-plan")), ClickTarget::Home);
        assert_eq!(click_target(None), ClickTarget::Home);
    }

    #[test]
    fn evenements_ecoutes() {
        assert_eq!(Kind::Invite.event(), "friend://invite");
        assert_eq!(Kind::Request.event(), "friend://request");
        assert_eq!(Kind::JoinRequest.event(), "friend://join_request");
    }
}
