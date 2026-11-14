//! Hôte : partage d'une instance et du monde LAN (`host_start_sharing` / `host_stop_sharing`).
//!
//! En usage réel, le partage est automatique : `HostAuto` partage l'instance (manifeste, sans port) dès que le jeu
//! tourne, détecte l'ouverture au réseau local dans la sortie du jeu, invite alors automatiquement les amis en ligne
//! (invitations renouvelées tant que le jeu tourne, annulées à la fin), puis arrête le partage à la fin du jeu.

use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use crate::error::user_msg;
use crate::state::{lock, AppState};
use crate::test_hooks::{set_tunnel_port, validate_world_port};

/// Port du monde LAN annoncé par Minecraft, p. ex. `[Server thread/INFO] [minecraft/IntegratedServer]: Started
/// serving on 54321`, ou par l'agent LAN (`[Cubelink] LAN: Published LAN server on port 54321`). `None` si la ligne
/// n'en contient pas.
///
/// Sécurité : seul le message d'un journal est lu, jamais une ligne de chat. Le message doit COMMENCER par le marqueur,
/// juste après l'en-tête du journal (voir [`log_message`]) : `<Bob> Started serving on 5432` (chat journalisé par le
/// serveur), `[CHAT] Local game hosted on port 5432` (chat affiché au client) ou `[Bob] …` (`/say`) sont ignorés.
/// Sans cela, un joueur pourrait faire relayer le tunnel vers un autre service local de l'hôte.
pub fn parse_world_port(line: &str) -> Option<u16> {
    let msg = log_message(line.trim_end());
    // L'agent LAN écrit sur la sortie standard, que certaines versions journalisent en `[STDOUT]: …` (ou
    // `[STDOUT]: [Classe:méthode:ligne]: …`). Un joueur ne fabrique pas ce jeton : son message commence par `<Pseudo>`.
    let msg = msg.strip_prefix("[STDOUT]:").map(str::trim_start).unwrap_or(msg);
    let msg = match msg.strip_prefix('[').and_then(|r| r.split_once("]: ")) {
        Some((inner, rest)) if inner.contains(':') && !inner.contains(' ') => rest,
        _ => msg,
    };
    let msg = msg.strip_prefix("[Cubelink] LAN: ").unwrap_or(msg);
    for marker in ["Started serving on", "Local game hosted on port", "Published LAN server on port"] {
        let Some(rest) = msg.strip_prefix(marker) else { continue };
        let rest = rest.trim_start();
        // Hôte optionnel avant le port : `127.0.0.1:54321`.
        let token = rest.split_whitespace().next().unwrap_or("");
        let rest = match token.rsplit_once(':') {
            Some((h, _)) if !h.is_empty() && h.chars().all(|c| c.is_ascii_alphanumeric() || c == '.') => &rest[h.len() + 1..],
            _ => rest,
        };
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if (4..=5).contains(&digits.len()) {
            return digits.parse::<u32>().ok().and_then(|p| validate_world_port(p).ok());
        }
        return None;
    }
    None
}

/// Message d'une ligne de journal, sans son en-tête : jetons `[12:00:01]`, `[Server thread/INFO]`,
/// `[minecraft/IntegratedServer]` (log4j, terminés par `:`) ou `(Minecraft)` (Fabric). Un jeton entre crochets n'est
/// un en-tête que s'il contient `/` ou n'est qu'une heure : `[CHAT]`, `[Not Secure]` ou `[Bob]` arrêtent la lecture
/// et restent dans le message. Un pseudo ne contient ni `/`, ni crochets, ni parenthèses : un joueur ne forge pas
/// d'en-tête, et son message commence toujours par `<Pseudo>`, `[CHAT]` ou `[Pseudo]`.
/// Heure d'un en-tête de journal : `HH:MM:SS`, éventuellement suivie de `.mmm`. Un pseudo numérique (`[1234]`) n'en
/// est pas une.
fn is_clock(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 8
        && b[2] == b':'
        && b[5] == b':'
        && [0, 1, 3, 4, 6, 7].iter().all(|&i| b[i].is_ascii_digit())
        && (b.len() == 8 || (b[8] == b'.' && b.len() > 9 && b[9..].iter().all(u8::is_ascii_digit)))
}

/// Fin du serveur intégré (monde quitté) : un monde rouvert ensuite au réseau local aura un nouveau port.
fn is_server_stop(line: &str) -> bool {
    log_message(line.trim_end()).starts_with("Stopping server")
}

fn log_message(line: &str) -> &str {
    let mut s = line.trim_start();
    loop {
        let close = match s.as_bytes().first() {
            Some(b'[') => ']',
            Some(b'(') => ')',
            _ => break,
        };
        let Some(end) = s.find(close) else { break };
        let inner = &s[1..end];
        let header = !inner.is_empty()
            && if close == ']' {
                inner.contains('/') || is_clock(inner)
            } else {
                inner.chars().all(|c| c.is_ascii_alphanumeric() || "_.$-".contains(c))
            };
        if !header {
            break;
        }
        s = s[end + 1..].trim_start();
        if let Some(rest) = s.strip_prefix(':') {
            return rest.trim_start();
        }
    }
    s
}

/// État du partage hôte (événement `host://state`, commande `host_state`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct HostState {
    pub instance_id: String,
    pub sharing: bool,
    /// Port du monde LAN détecté ; `None` tant que le monde n'est pas ouvert au réseau local.
    pub world_port: Option<u16>,
}

impl AppState {
    /// `None` si rien n'est partagé.
    pub fn host_state(&self) -> Option<HostState> {
        lock(&self.host).clone().filter(|h| h.sharing)
    }

    /// Met à jour l'état et émet `host://state` (si l'état est partagé ou vient de s'arrêter).
    fn host_set(&self, st: HostState, emit: bool) {
        *lock(&self.host) = Some(st.clone());
        if emit {
            self.emit_event("host://state", &st);
        }
    }

    /// `world_port` : port du monde LAN local (1024 à 65535), ou 0 si le monde n'est pas encore ouvert au réseau
    /// local (partage des fichiers seulement). L'instance doit exister. Le dernier appel l'emporte.
    pub async fn host_start_sharing(&self, instance_id: &str, world_port: u32) -> Result<(), String> {
        let port = if world_port == 0 { 0 } else { validate_world_port(world_port)? };
        if !crate::commands_instances::valid_id(instance_id) {
            return Err("identifiant d'instance invalide".into());
        }
        self.instances.get(instance_id).map_err(|e| user_msg(&e))?;
        self.backend().host_start_sharing(instance_id.to_string(), port).await?;
        if port != 0 {
            set_tunnel_port(port);
        }
        // Un port déjà détecté pour cette instance n'est pas perdu par un repartage sans port.
        let kept = lock(&self.host).as_ref().filter(|h| h.instance_id == instance_id).and_then(|h| h.world_port);
        let world_port = if port != 0 { Some(port) } else { kept };
        self.host_set(HostState { instance_id: instance_id.to_string(), sharing: true, world_port }, true);
        Ok(())
    }

    /// Le monde LAN de `instance_id` s'est ouvert sur `port` : les amis peuvent désormais s'y connecter.
    pub fn host_world_opened(&self, instance_id: &str, port: u16) {
        self.backend().host_set_world_port(port);
        set_tunnel_port(port);
        // Le partage peut ne pas être encore prêt : le port est retenu, l'événement part quand il l'est.
        let sharing = lock(&self.host).as_ref().is_some_and(|h| h.instance_id == instance_id && h.sharing);
        self.host_set(HostState { instance_id: instance_id.to_string(), sharing, world_port: Some(port) }, sharing);
    }

    pub fn host_stop_sharing(&self) {
        self.backend().host_stop_sharing();
        set_tunnel_port(0);
        // Les invitations envoyées ne valent plus rien : on prévient les amis invités (au mieux).
        self.invites_revoke_sent();
        let prev = lock(&self.host).take();
        if let Some(p) = prev.filter(|p| p.sharing) {
            self.emit_event("host://state", &HostState { instance_id: p.instance_id, sharing: false, world_port: None });
        }
    }
}

/// Partage automatique lié à la vie d'une partie.
pub struct HostAuto {
    st: Arc<AppState>,
    id: String,
    share: Option<tauri::async_runtime::JoinHandle<()>>,
    port: Option<u16>,
    /// Annonce du monde ouvert aux amis en ligne (invitations renouvelées tant que le jeu tourne).
    announce: Option<tauri::async_runtime::JoinHandle<()>>,
}

/// Renouvellement des invitations automatiques, avant leur échéance (`INVITE_TTL`).
const ANNOUNCE_RENEW: std::time::Duration = std::time::Duration::from_secs(4 * 60);
/// Fréquence de vérification (amis qui viennent de se connecter, partage enfin prêt).
const ANNOUNCE_TICK: std::time::Duration = std::time::Duration::from_secs(15);

impl AppState {
    /// Invite chaque ami en ligne pas encore invité (ou dont l'invitation arrive à échéance).
    /// Sans effet tant que le monde n'est pas ouvert et partagé.
    pub(crate) async fn announce_world(&self, sent: &mut std::collections::HashMap<String, std::time::Instant>) {
        if self.host_state().and_then(|h| h.world_port).is_none() {
            return;
        }
        let Ok(friends) = self.friend_list() else { return };
        for f in friends {
            let key = f.friend.public_key;
            if f.presence != core_lib::types::Presence::Online {
                sent.remove(&key);
                continue;
            }
            if sent.get(&key).is_some_and(|t| t.elapsed() < ANNOUNCE_RENEW) {
                continue;
            }
            match self.friend_invite(&key).await {
                Ok(()) => {
                    sent.insert(key, std::time::Instant::now());
                }
                Err(e) => eprintln!("[Cubelink] annonce du monde à un ami impossible : {e}"),
            }
        }
    }
}

impl HostAuto {
    /// Le jeu tourne : partage l'instance en tâche de fond (le calcul du manifeste peut être long).
    pub fn start(st: &Arc<AppState>, instance_id: &str) -> Self {
        let (s, id) = (st.clone(), instance_id.to_string());
        let share = tauri::async_runtime::spawn(async move {
            if let Err(e) = s.host_start_sharing(&id, 0).await {
                eprintln!("partage automatique impossible : {e}");
            }
        });
        let s = st.clone();
        let announce = tauri::async_runtime::spawn(async move {
            let mut sent = std::collections::HashMap::new();
            loop {
                s.announce_world(&mut sent).await;
                tokio::time::sleep(ANNOUNCE_TICK).await;
            }
        });
        Self { st: st.clone(), id: instance_id.to_string(), share: Some(share), port: None, announce: Some(announce) }
    }

    /// Une ligne de sortie du jeu : détecte l'ouverture du monde au réseau local. Le port détecté est définitif tant
    /// que ce monde tourne : une ligne plus tardive (même forgée) ne détourne pas le tunnel. Après l'arrêt du serveur
    /// intégré (`Stopping server`, monde quitté), le monde suivant ouvert au réseau local donne le nouveau port.
    pub fn on_line(&mut self, line: &str) {
        if self.port.is_some() {
            // Monde quitté (serveur intégré arrêté) : le prochain monde ouvert au réseau local pourra donner son port.
            if is_server_stop(line) {
                self.port = None;
            }
            return;
        }
        if let Some(p) = parse_world_port(line) {
            self.port = Some(p);
            self.st.host_world_opened(&self.id, p);
        }
    }

    /// Fin du jeu : arrête l'annonce, attend la fin d'un partage encore en cours puis l'arrête.
    pub async fn finish(mut self) {
        if let Some(a) = self.announce.take() {
            a.abort();
        }
        if let Some(h) = self.share.take() {
            let _ = h.await;
        }
        self.st.host_stop_sharing();
    }
}

impl Drop for HostAuto {
    fn drop(&mut self) {
        if let Some(a) = self.announce.take() {
            a.abort();
        }
    }
}

/// Partage le monde LAN avec les amis (`world_port` 0 : fichiers seulement). Normalement automatique.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn host_start_sharing(state: State<'_, Arc<AppState>>, instance_id: String, world_port: u32) -> Result<(), String> {
    state.host_start_sharing(&instance_id, world_port).await
}

/// État du partage hôte, `None` si rien n'est partagé.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub fn host_state(state: State<'_, Arc<AppState>>) -> Result<Option<HostState>, String> {
    Ok(state.host_state())
}

/// Arrête le partage hôte.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub fn host_stop_sharing(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    state.host_stop_sharing();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::p2p_backend::{BoxFuture, P2pBackend, ProgressFn, TransferControl};
    use crate::state::test_support::test_state;
    
    use core_lib::types::{Error, Loader, LoaderSpec, Presence, VerifyCode};
    use p2p_lib::friend_code::FriendCodePayload;
    use p2p_lib::transfer::plan::Plan;
    use std::sync::Mutex;

    #[tokio::test]
    async fn refuse_port_invalide_et_instance_inconnue() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        for p in [80, 1023, 65536] {
            assert!(st.host_start_sharing("x", p).await.unwrap_err().contains("port"), "{p}");
        }
        assert!(st.host_start_sharing("../x", 25565).await.unwrap_err().contains("invalide"));
        assert!(st.host_start_sharing("inconnue", 25565).await.is_err());
        assert!(st.host_start_sharing("inconnue", 0).await.is_err());
    }

    #[test]
    fn parse_port_lignes_reelles() {
        let l = "[12:00:01] [Server thread/INFO] [minecraft/IntegratedServer]: Started serving on 54321";
        assert_eq!(parse_world_port(l), Some(54321));
        assert_eq!(parse_world_port("[12:00:01] [Server thread/INFO] (Minecraft) Started serving on 54321"), Some(54321));
        assert_eq!(parse_world_port("[Cubelink] LAN: Published LAN server on port 54321"), Some(54321));
        assert_eq!(parse_world_port("[12:00:01] [Render thread/INFO]: [STDOUT]: [Cubelink] LAN: Published LAN server on port 54321"), Some(54321));
        assert_eq!(
            parse_world_port("[12:00:01] [Client thread/INFO] [STDOUT]: [app.cubelink.agent.LanAgent:log:150]: [Cubelink] LAN: Published LAN server on port 54321"),
            Some(54321)
        );
        assert_eq!(parse_world_port("[12:00:01.123] [Server thread/INFO]: Started serving on 54321"), Some(54321));
        assert_eq!(parse_world_port("Local game hosted on port  1024"), Some(1024));
        assert_eq!(parse_world_port("Started serving on 54321\r"), Some(54321));
        assert_eq!(parse_world_port("[Server thread/INFO]: Started serving on 127.0.0.1:41234"), Some(41234));
        assert_eq!(parse_world_port("[21:24:17] [Server thread/INFO]: Published LAN server on port 60707"), Some(60707));
        assert_eq!(parse_world_port("Published LAN server on port 80"), None);
        assert_eq!(parse_world_port("Started serving on localhost:25565"), Some(25565));
    }

    #[test]
    fn parse_port_faux_positifs() {
        for l in [
            "",
            "[Server thread/INFO]: Starting minecraft server version 1.21.1",
            "Started serving on 80",
            "Started serving on 123456",
            "Started serving on 99999",
            "Started serving on abc",
            "Started serving on 127.0.0.1:80",
            "Started serving on 127.0.0.1:",
            "Local game hosted on port",
            "Done (3.2s)! For help, type \"help\" 54321",
            "Connection to port 54321 refused",
        ] {
            assert_eq!(parse_world_port(l), None, "{l:?}");
        }
    }

    #[test]
    fn arret_du_serveur_integre() {
        assert!(is_server_stop("[12:00:01] [Server thread/INFO] [minecraft/MinecraftServer]: Stopping server"));
        assert!(is_server_stop("[12:00:01] [Server thread/INFO] (Minecraft) Stopping server"));
        assert!(!is_server_stop("[12:00:01] [Server thread/INFO]: <Bob> Stopping server"));
        assert!(!is_server_stop("[12:00:01] [Render thread/INFO]: [CHAT] <Bob> Stopping server"));
    }

    /// S2 : une ligne de chat (ou de `/say`) ne fournit jamais le port du tunnel, quel que soit le format du journal.
    #[test]
    fn parse_port_ignore_le_chat() {
        for l in [
            "[12:00:01] [Render thread/INFO] [minecraft/ChatComponent]: [CHAT] Local game hosted on port 25565",
            "[12:00:01] [Render thread/INFO]: [CHAT] <Bob> Local game hosted on port 5432",
            "[12:00:01] [Server thread/INFO] [minecraft/MinecraftServer]: <Bob> Started serving on 5432",
            "[12:00:01] [Server thread/INFO]: <Bob> Local game hosted on port 5432",
            "[12:00:01] [Server thread/INFO] (Minecraft) <Bob> Started serving on 5432",
            "[12:00:01] [Server thread/INFO]: [Not Secure] <Bob> Started serving on 5432",
            "[12:00:01] [Server thread/INFO]: [Bob] Started serving on 5432",
            "[12:00:01] [Server thread/INFO]: [1234] Started serving on 5432",
            "[12:00:01] [Server thread/INFO]: [12:34] Started serving on 5432",
            "[12:00:01] [Server thread/INFO]: <Bob> [STDOUT]: [Cubelink] LAN: Published LAN server on port 5432",
            "[12:00:01] [Server thread/INFO]: <Bob> ]: Started serving on 5432",
            "[12:00:01] [Server thread/INFO]: <Bob> [Cubelink] LAN: Published LAN server on port 5432",
            "[12:00:01] [Server thread/INFO]: * Bob Started serving on 5432",
            "<Bob> Started serving on 5432",
            "[CHAT] Local game hosted on port 5432",
        ] {
            assert_eq!(parse_world_port(l), None, "{l:?}");
        }
    }

    #[derive(Default)]
    struct Rec(Mutex<Vec<String>>);
    impl Rec {
        fn log(&self, s: String) {
            self.0.lock().unwrap().push(s);
        }
    }
    impl P2pBackend for Rec {
        fn presence(&self, _k: &str) -> Presence {
            Presence::Offline
        }
        fn send_request(&self, _t: FriendCodePayload, _n: String) -> BoxFuture<'_, Result<VerifyCode, String>> {
            Box::pin(async { Err("x".into()) })
        }
        fn fetch_manifest(&self, _k: String) -> BoxFuture<'_, Result<crate::p2p_backend::HostManifest, String>> {
            Box::pin(async { Err("x".into()) })
        }
        fn host_start_sharing(&self, id: String, port: u16) -> BoxFuture<'_, Result<(), String>> {
            self.log(format!("share {id} {port}"));
            Box::pin(async { Ok(()) })
        }
        fn host_set_world_port(&self, port: u16) {
            self.log(format!("port {port}"));
        }
        fn host_stop_sharing(&self) {
            self.log("stop".into());
        }
        fn receive_files(&self, _k: String, _i: String, _p: Plan, _pr: ProgressFn, _c: Arc<TransferControl>) -> BoxFuture<'_, Result<(), Error>> {
            Box::pin(async { Ok(()) })
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cycle_running_partage_port_detecte_arret() {
        let dir = tempfile::tempdir().unwrap();
        let (st, events) = test_state(dir.path());
        let rec = Arc::new(Rec::default());
        st.set_backend(rec.clone());
        let inst = st.instances.create("Hote", "1.21.1", LoaderSpec { loader: Loader::Vanilla, version: String::new() }, 2048).unwrap();

        let mut auto = HostAuto::start(&st, &inst.id);
        auto.on_line("[main/INFO]: Setting user: Steve");
        auto.on_line("[Server thread/INFO] [minecraft/IntegratedServer]: Started serving on 54321");
        auto.on_line("[Render thread/INFO]: [CHAT] Local game hosted on port 54321"); // doublon ignoré
        // S2 : le port est définitif une fois le monde ouvert, même face à une ligne valide plus tardive.
        auto.on_line("[Server thread/INFO] [minecraft/IntegratedServer]: Started serving on 5432");
        auto.on_line("[Server thread/INFO]: <Bob> Stopping server"); // chat : le port reste
        auto.on_line("[Server thread/INFO] [minecraft/IntegratedServer]: Started serving on 5433");
        let want = Some(HostState { instance_id: inst.id.clone(), sharing: true, world_port: Some(54321) });
        for _ in 0..100 {
            if st.host_state() == want {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(st.host_state(), want);
        auto.finish().await;
        assert_eq!(st.host_state(), None);

        let log = rec.0.lock().unwrap().clone();
        // Le partage (tâche de fond) et la détection du port peuvent s'entrelacer ; l'arrêt est toujours en dernier.
        assert_eq!(log.len(), 3, "{log:?}");
        assert!(log.contains(&format!("share {} 0", inst.id)), "{log:?}");
        assert!(log.contains(&"port 54321".to_string()), "{log:?}");
        assert_eq!(log[2], "stop");
        let ev: Vec<_> = events.lock().unwrap().iter().filter(|(n, _)| n == "host://state").map(|(_, v)| v.clone()).collect();
        let id = &inst.id;
        // Prêt (sans port) et port détecté peuvent s'entrelacer : le dernier avant l'arrêt porte le port, l'arrêt clôt.
        assert!(!ev.is_empty(), "{ev:?}");
        assert_eq!(ev.last().unwrap(), &serde_json::json!({"instance_id": id, "sharing": false, "world_port": null}));
        assert!(ev.contains(&serde_json::json!({"instance_id": id, "sharing": true, "world_port": 54321})), "{ev:?}");
    }
}
