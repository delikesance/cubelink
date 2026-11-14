//! Interface vers le réseau P2P. `IrohBackend` (iroh_backend.rs) est l'implémentation réelle ;
//! `UnavailableBackend` reste pour les tests unitaires (erreurs claires, aucun réseau).

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use std::path::PathBuf;

use core_lib::unknown_mods::UnknownReport;
use core_lib::types::{Error, Friend, Manifest, ManifestFile, Presence, TransferProgress, VerifyCode};
use p2p_lib::transfer::plan::Plan;
use p2p_lib::friend_code::FriendCodePayload;
use p2p_lib::invite::InviteMessage;
use p2p_lib::join_request::JoinAnswer;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
pub type ProgressFn = Arc<dyn Fn(TransferProgress) + Send + Sync>;

/// Commande de contrôle d'un transfert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    Pause,
    Resume,
    Cancel,
}

type Hook = Box<dyn Fn(Cmd) + Send + Sync>;

/// Drapeaux de contrôle partagés avec le transfert en cours ; un crochet transmet les commandes au transfert.
#[derive(Default)]
pub struct TransferControl {
    pub paused: AtomicBool,
    pub cancelled: AtomicBool,
    hook: std::sync::Mutex<Option<Hook>>,
}

impl std::fmt::Debug for TransferControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TransferControl")
    }
}

impl TransferControl {
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
    fn call(&self, c: Cmd) {
        if let Some(h) = self.hook.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            h(c);
        }
    }
    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
        self.call(Cmd::Pause);
    }
    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
        self.call(Cmd::Resume);
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.paused.store(false, Ordering::SeqCst);
        self.call(Cmd::Cancel);
    }
    /// Branche le transfert ; rejoue l'état déjà demandé.
    pub fn set_hook(&self, h: Hook) {
        if self.is_cancelled() {
            h(Cmd::Cancel);
        } else if self.is_paused() {
            h(Cmd::Pause);
        }
        *self.hook.lock().unwrap_or_else(|e| e.into_inner()) = Some(h);
    }
}

/// Manifeste de l'hôte.
#[derive(Debug, Clone)]
pub struct HostManifest {
    pub manifest: Manifest,
}

/// Résultat du test « Ta connexion avec tes amis » (Paramètres).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NetTest {
    /// Connecté à un relais (secours quand la connexion directe échoue).
    pub relay_connected: bool,
    /// Nom d'hôte du relais (« euc1-1.relay.n0.iroh.link »).
    pub relay: Option<String>,
    /// Latence vers ce relais, en millisecondes.
    pub relay_ms: Option<u32>,
    /// L'UDP passe (indispensable à la connexion directe).
    pub udp: bool,
    pub ipv6: bool,
    /// Connexion directe : `Some(true)` facile, `Some(false)` routeur strict (adresse publique changeante), `None` inconnu.
    pub direct: Option<bool>,
    /// Réglage « Passer toujours par le relais ».
    pub relay_only: bool,
    /// Le rapport réseau n'est pas arrivé à temps : `udp` et `direct` ne sont pas fiables.
    pub partial: bool,
}

pub trait P2pBackend: Send + Sync {
    /// Test de la connexion réseau entre amis (relais, UDP, type de routeur).
    fn net_test(&self) -> BoxFuture<'_, Result<NetTest, String>> {
        Box::pin(async { Err("Le réseau entre amis n'est pas démarré.".to_string()) })
    }
    /// Présence d'un ami.
    fn presence(&self, public_key: &str) -> Presence;
    /// Pseudo déclaré aux amis (demandes et acceptations).
    fn set_display_name(&self, _name: &str) {}
    /// Indices à mettre dans notre code ami : (relais, adresses directes).
    fn code_hints(&self) -> (Option<String>, Vec<String>) {
        (None, vec![])
    }
    /// Envoie une demande d'ami ; renvoie le code de vérification à comparer.
    fn send_request(&self, target: FriendCodePayload, my_name: String) -> BoxFuture<'_, Result<VerifyCode, String>>;
    /// Accepte une demande reçue (décision signée + ami créé). `None` : non géré, l'appelant le fait en local.
    fn accept_request(&self, _public_key: String) -> BoxFuture<'_, Result<Option<Friend>, String>> {
        Box::pin(async { Ok(None) })
    }
    /// Refuse une demande reçue. `false` : non géré, l'appelant le fait en local.
    fn refuse_request(&self, _public_key: String) -> BoxFuture<'_, Result<bool, String>> {
        Box::pin(async { Ok(false) })
    }
    /// Envoie une invitation à rejoindre notre monde à un ami (clé hex). Par défaut : sans effet.
    fn send_invite(&self, _public_key: String, _msg: InviteMessage) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
    /// Invité : demande à l'ami (clé hex) de rejoindre sa partie ; attend sa décision (au plus ~2 min).
    fn send_join_request(&self, _public_key: String) -> BoxFuture<'_, Result<JoinAnswer, String>> {
        Box::pin(async { Err(UNAVAILABLE.to_string()) })
    }
    /// Hôte : décide de la demande en attente de l'ami (clé hex). Accepter ouvre l'accès à la partie en cours.
    /// Faux si aucune demande n'attend, ou si rien n'est partagé.
    fn answer_join_request(&self, _public_key: &str, _accept: bool) -> bool {
        false
    }
    /// Récupère le manifeste du monde de l'ami.
    fn fetch_manifest(&self, friend_public_key: String) -> BoxFuture<'_, Result<HostManifest, String>>;
    /// Invité, APRÈS réception : mods (chemins) hors Modrinth, calculés sur les sha1 locaux des jars reçus.
    fn find_unknown_mods(&self, _files: Vec<ManifestFile>, _game_dir: PathBuf) -> BoxFuture<'_, Vec<String>> {
        Box::pin(async { vec![] })
    }
    /// Invité, AVANT téléchargement : analyse sur les sha1 ANNONCÉS par l'hôte (revendication, vérifiée à la
    /// réception). Les jars sans sha1 ou non vérifiables sont dans `unchecked`.
    fn find_unknown_from_manifest(&self, _files: Vec<ManifestFile>) -> BoxFuture<'_, UnknownReport> {
        Box::pin(async { UnknownReport::default() })
    }
    /// Hôte : partage l'instance (manifeste + fichiers) et expose le monde LAN `world_port` (0 : fichiers seulement).
    fn host_start_sharing(&self, _instance_id: String, _world_port: u16) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async { Err(UNAVAILABLE.to_string()) })
    }
    fn host_stop_sharing(&self) {}
    /// Hôte : le monde LAN vient de s'ouvrir sur `port` (127.0.0.1) ; 0 le referme. N'altère pas le partage de fichiers.
    fn host_set_world_port(&self, _port: u16) {}
    /// Invité : ouvre un tunnel local vers le monde de l'ami ; renvoie le port local (127.0.0.1) pour Minecraft.
    fn join_open_tunnel(&self, _friend_public_key: String) -> BoxFuture<'_, Result<u16, String>> {
        Box::pin(async { Err(UNAVAILABLE.to_string()) })
    }
    /// Membre d'un groupe Hearth : ouvre un tunnel local vers le serveur du membre `holder` (clé hex) sans exiger
    /// qu'il soit notre ami. Par défaut comme `join_open_tunnel`.
    fn hearth_open_tunnel(&self, holder: String) -> BoxFuture<'_, Result<u16, String>> {
        self.join_open_tunnel(holder)
    }
    /// Invité : ferme le tunnel ouvert (sans effet s'il n'y en a pas).
    fn join_close_tunnel(&self) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
    /// TEST uniquement (commande `test_cut_guest_connection`, mode test) : coupe de force la connexion iroh du
    /// tunnel invité pour vérifier la reconnexion. Renvoie faux s'il n'y a pas de tunnel.
    fn test_cut_guest_connection(&self) -> BoxFuture<'_, bool> {
        Box::pin(async { false })
    }
    /// Reçoit les fichiers du plan dans le magasin local (reprenable, identifiant stable).
    fn receive_files(
        &self,
        friend_public_key: String,
        transfer_id: String,
        plan: Plan,
        progress: ProgressFn,
        control: Arc<TransferControl>,
    ) -> BoxFuture<'_, Result<(), Error>>;
}

pub const UNAVAILABLE: &str = "fonction pas encore disponible : le réseau entre amis n'est pas encore branché";

/// Implémentation pour les tests unitaires (aucun réseau).
pub struct UnavailableBackend;

impl P2pBackend for UnavailableBackend {
    fn presence(&self, _public_key: &str) -> Presence {
        Presence::Offline
    }
    fn send_request(&self, _t: FriendCodePayload, _n: String) -> BoxFuture<'_, Result<VerifyCode, String>> {
        Box::pin(async { Err(UNAVAILABLE.to_string()) })
    }
    fn fetch_manifest(&self, _k: String) -> BoxFuture<'_, Result<HostManifest, String>> {
        Box::pin(async { Err(UNAVAILABLE.to_string()) })
    }
    fn receive_files(
        &self,
        _k: String,
        _id: String,
        _p: Plan,
        _pr: ProgressFn,
        _c: Arc<TransferControl>,
    ) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async { Err(Error::Other(UNAVAILABLE.to_string())) })
    }
}
