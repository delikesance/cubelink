//! Serveurs de groupe (Hearth) de bout en bout : trois nœuds loopback (relais désactivé), vrais handlers iroh, vrais
//! magasins de blobs et dossiers sur le disque. Seuls le serveur Minecraft (faux : un écho TCP et un monde de fichiers)
//! et le lancement du jeu (enregistré) sont remplacés ; ils ont leurs propres tests réels (`core/tests`).
//!
//! Scénario : création d'un groupe depuis un monde, invitation, synchro du monde ; Jouer chez A décide Héberger ; B décide
//! Rejoindre et obtient l'accès au tunnel ; la fin de partie publie une version reçue par B ; l'outbox livre plus tard à
//! C, qui était hors ligne ; un bail expiré donne « partie mal terminée » ; ContinueWithout se vote à la majorité ; une
//! fourche se tranche par un choix. Deux candidats simultanés : un seul héberge (prise du bail en deux phases). Après
//! une veille (horloge simulée) : l'hôte ne renouvelle pas, fait une ronde, et reprend ou s'arrête selon ce qu'il voit.

mod common;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{befriend, wait_for, world, Node};
use core_lib::types::HearthStateKind;
use cubelink_app::hearth::{BoxFut, ClientLauncher, LeaveError, RenewTick, RunningServer, ServerRunner, ServerSpec, Timings, EULA_REQUIRED};
use cubelink_app::state::{lock, AppState};
use p2p_lib::hearth::lease::{Lease, ManualClock};
use p2p_lib::hearth::model::{GroupId, GroupOp};
use p2p_lib::hearth::proto::HearthDelegate;
use p2p_lib::hearth::version::VersionKind;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// ---------------------------------------------------------------------------------------------------------
// Faux serveur et faux lanceur
// ---------------------------------------------------------------------------------------------------------

/// Serveur factice : un monde de fichiers dans `dir/world`, un écho TCP en guise de monde, et un fichier de plus à chaque
/// arrêt (le monde « joué » diffère d'une partie à l'autre).
struct FakeServer {
    port: u16,
    dir: PathBuf,
    task: tokio::task::JoinHandle<()>,
    calls: Arc<Mutex<Vec<&'static str>>>,
    /// `save-on` n'est jamais confirmé (serveur chargé) : `flush_end` reste en attente après avoir noté l'appel.
    hang_end: Arc<std::sync::atomic::AtomicBool>,
    /// Code de sortie : `wait_exit` rend dès qu'il est posé (le test fait « mourir » le serveur sans passer par `stop`).
    exit: tokio::sync::watch::Receiver<Option<i32>>,
    /// Sortie du serveur, telle que la lit `recent_lines`.
    log: Arc<Mutex<Vec<String>>>,
}

impl RunningServer for FakeServer {
    fn port(&self) -> u16 {
        self.port
    }
    fn world_dir(&self) -> PathBuf {
        self.dir.join("world")
    }
    fn flush_begin(&self) -> BoxFut<'_, Result<(), String>> {
        self.calls.lock().unwrap().push("save-off");
        Box::pin(async { Ok(()) })
    }
    fn flush_end(&self) -> BoxFut<'_, Result<(), String>> {
        self.calls.lock().unwrap().push("save-on");
        if self.hang_end.load(std::sync::atomic::Ordering::SeqCst) {
            return Box::pin(std::future::pending());
        }
        Box::pin(async { Ok(()) })
    }
    fn stop(&self, _grace: Duration) -> BoxFut<'_, ()> {
        self.calls.lock().unwrap().push("stop");
        let n = std::fs::read_dir(self.world_dir()).map(|d| d.count()).unwrap_or(0);
        std::fs::write(self.world_dir().join(format!("partie-{n}.txt")), format!("jouée {n}")).unwrap();
        self.task.abort();
        Box::pin(async {})
    }
    fn wait_exit(&self) -> BoxFut<'_, i32> {
        let mut rx = self.exit.clone();
        Box::pin(async move {
            let code = rx.wait_for(Option::is_some).await.ok().and_then(|c| *c);
            match code {
                Some(c) => c,
                // L'émetteur est gardé par le lanceur : sans code posé, le serveur tourne.
                None => std::future::pending().await,
            }
        })
    }
    fn recent_lines(&self, n: usize) -> Vec<String> {
        let l = self.log.lock().unwrap();
        l[l.len().saturating_sub(n)..].to_vec()
    }
}

#[derive(Default)]
struct FakeRunner {
    calls: Arc<Mutex<Vec<&'static str>>>,
    specs: Arc<Mutex<Vec<ServerSpec>>>,
    hang_end: Arc<std::sync::atomic::AtomicBool>,
    /// Lignes de log que le prochain serveur « a écrites ».
    log: Arc<Mutex<Vec<String>>>,
    /// Pour faire mourir le serveur en cours avec un code de sortie.
    exit_tx: Arc<Mutex<Option<tokio::sync::watch::Sender<Option<i32>>>>>,
}

impl FakeRunner {
    /// Le serveur en cours meurt seul avec ce code de sortie (sans `stop` demandé).
    fn die(&self, code: i32) {
        self.exit_tx.lock().unwrap().as_ref().expect("aucun serveur lancé").send(Some(code)).unwrap();
    }
}

impl ServerRunner for FakeRunner {
    fn start(&self, spec: ServerSpec) -> BoxFut<'static, Result<Arc<dyn RunningServer>, String>> {
        let (calls, specs, hang_end) = (self.calls.clone(), self.specs.clone(), self.hang_end.clone());
        let (log, exit_slot) = (self.log.clone(), self.exit_tx.clone());
        Box::pin(async move {
            let world = spec.dir.join("world");
            std::fs::create_dir_all(&world).unwrap();
            if !world.join("level.dat").exists() {
                std::fs::write(world.join("level.dat"), b"nouveau monde").unwrap();
            }
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = l.local_addr().unwrap().port();
            let task = tokio::spawn(async move {
                loop {
                    let Ok((mut s, _)) = l.accept().await else { break };
                    tokio::spawn(async move {
                        let mut b = [0u8; 64];
                        while let Ok(n) = s.read(&mut b).await {
                            if n == 0 || s.write_all(&b[..n]).await.is_err() {
                                break;
                            }
                        }
                    });
                }
            });
            calls.lock().unwrap().push("start");
            let dir = spec.dir.clone();
            specs.lock().unwrap().push(spec);
            let (tx, exit) = tokio::sync::watch::channel(None);
            *exit_slot.lock().unwrap() = Some(tx);
            Ok(Arc::new(FakeServer { port, dir, task, calls, hang_end, exit, log }) as Arc<dyn RunningServer>)
        })
    }
}

/// Lanceur factice : note où le jeu se connecte et tient le verrou de jeu comme le ferait un vrai jeu ouvert.
#[derive(Default)]
struct FakeLauncher {
    launched: Arc<Mutex<Vec<(String, u16)>>>,
}

impl ClientLauncher for FakeLauncher {
    fn launch(&self, st: &Arc<AppState>, instance: &str, port: u16) -> BoxFut<'static, Result<(), String>> {
        self.launched.lock().unwrap().push((instance.to_string(), port));
        *lock(&st.game) = Some(instance.to_string());
        Box::pin(async { Ok(()) })
    }
}

/// Emplacement du nœud : vidable, pour libérer les verrous du disque avant un redémarrage.
struct Slot(Option<Node>);

impl std::ops::Deref for Slot {
    type Target = Node;
    fn deref(&self) -> &Node {
        self.0.as_ref().expect("nœud arrêté")
    }
}

struct Peer {
    node: Slot,
    dir: tempfile::TempDir,
    name: &'static str,
    instance: String,
    runner: Arc<FakeRunner>,
    launcher: Arc<FakeLauncher>,
    stopped: std::sync::atomic::AtomicBool,
}

impl Peer {
    async fn start(name: &'static str) -> Peer {
        let dir = tempfile::tempdir().unwrap();
        let node = Node::start(dir.path(), name).await;
        let instance = world(&node);
        let mut p = Peer { node: Slot(Some(node)), dir, name, instance, runner: Arc::default(), launcher: Arc::default(), stopped: Default::default() };
        p.wire();
        p
    }

    fn wire(&mut self) {
        self.runner = Arc::default();
        self.launcher = Arc::default();
        // Hearth est coupé par défaut (docs/hearth.md § 12.1 n° 63) : les scénarios l'activent.
        self.node.st.hearth.set_enabled(true);
        self.node.st.hearth.set_runner(self.runner.clone());
        self.node.st.hearth.set_launcher(self.launcher.clone());
    }

    /// Arrêt puis redémarrage avec le même dossier (même identité, mêmes groupes).
    async fn restart(&mut self) {
        self.stop().await;
        // L'ancien nœud doit disparaître avant le nouveau : le magasin de blobs garde un verrou tant qu'il existe.
        drop(self.node.0.take());
        self.node = Slot(Some(Node::start(self.dir.path(), self.name).await));
        self.stopped.store(false, std::sync::atomic::Ordering::SeqCst);
        self.wire();
    }

    async fn stop(&self) {
        // Un second arrêt du même nœud ne doit pas rouvrir la porte à des tâches de fond : on ne l'arrête qu'une fois.
        if !self.stopped.swap(true, std::sync::atomic::Ordering::SeqCst) {
            self.node.be.shutdown().await;
        }
    }

    fn key(&self) -> [u8; 32] {
        hex::decode(self.node.key()).unwrap().try_into().unwrap()
    }

    fn rt(&self) -> &Arc<cubelink_app::hearth::HearthRuntime> {
        &self.node.st.hearth
    }

    fn state_of(&self, g: &GroupId) -> HearthStateKind {
        self.rt().card(g).unwrap().state
    }

    /// La partie du jeu se termine (le joueur ferme Minecraft).
    fn close_game(&self) {
        *lock(&self.node.st.game) = None;
    }
}

fn save_world(peer: &Peer, name: &str) {
    let gd = peer.node.st.instances.game_dir(&peer.instance).unwrap();
    let w = gd.join("saves").join(name);
    std::fs::create_dir_all(w.join("region")).unwrap();
    std::fs::create_dir_all(w.join("playerdata")).unwrap();
    std::fs::write(w.join("level.dat"), b"level du monde d'Alice").unwrap();
    std::fs::write(w.join("region/r.0.0.mca"), (0..300_000u32).map(|i| (i % 251) as u8).collect::<Vec<u8>>()).unwrap();
    std::fs::write(w.join("playerdata/joueur.dat"), b"inventaire").unwrap();
    std::fs::write(w.join("session.lock"), b"verrou").unwrap();
}

fn read_tree(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            if e.file_type().unwrap().is_dir() {
                stack.push(e.path());
            } else {
                out.push((e.path().strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/"), std::fs::read(e.path()).unwrap()));
            }
        }
    }
    out.sort();
    out
}

async fn echo_works(port: u16) -> bool {
    let ok = async {
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.ok()?;
        s.write_all(b"salut").await.ok()?;
        let mut buf = [0u8; 5];
        tokio::time::timeout(Duration::from_secs(3), s.read_exact(&mut buf)).await.ok()?.ok()?;
        (&buf == b"salut").then_some(())
    }
    .await;
    ok.is_some()
}

// ---------------------------------------------------------------------------------------------------------
// Le scénario
// ---------------------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn serveur_de_groupe_de_la_creation_a_la_fourche() {
    let (mut a, mut b, mut c) = (Peer::start("Alice").await, Peer::start("Bob").await, Peer::start("Chloe").await);
    befriend(&a.node, &b.node).await;
    befriend(&a.node, &c.node).await;
    befriend(&b.node, &c.node).await;
    for p in [&a, &b, &c] {
        let others: Vec<&Peer> = [&a, &b, &c].into_iter().filter(|o| o.key() != p.key()).collect();
        wait_for("amis de tout le monde", 20, || others.iter().all(|o| p.node.st.friends.is_friend(&o.node.key()).unwrap())).await;
    }

    // 1. Création depuis un monde existant de l'instance d'Alice.
    save_world(&a, "Aventure");
    let worlds = a.node.st.hearth_worlds(&a.instance).await.unwrap();
    assert_eq!(worlds.len(), 1);
    assert!(worlds[0].size_bytes > 300_000);
    // Un seul ami ne suffit pas à tromper la règle : un inconnu ne s'invite pas.
    assert!(a.node.st.hearth_create("X", &a.instance, None, vec!["00".repeat(32)]).await.is_err());
    let card = a.node.st.hearth_create("Les copains", &a.instance, Some("Aventure".into()), vec![b.node.key(), c.node.key()]).await.unwrap();
    let g = GroupId::from_hex(&card.id).unwrap();
    assert_eq!((card.members_total, card.version, card.can_host, card.is_creator), (3, Some(1), true, true));
    let v1 = a.rt().head(&g).unwrap();
    assert_eq!(v1.header.number, 1);

    // 2. Invitations (« Rejoindre le groupe »), puis synchro du monde chez B et C.
    for p in [&b, &c] {
        wait_for("invitation de groupe", 30, || p.node.st.hearth_invites().len() == 1).await;
        let inv = p.node.st.hearth_invites().remove(0);
        assert_eq!((inv.name.as_str(), inv.members), ("Les copains", 3));
        assert_eq!(inv.from.public_key, a.node.key());
        p.node.st.hearth_invite_accept(&inv.group_id).unwrap();
        assert!(p.node.st.hearth_invites().is_empty());
        // Sans le modpack, pas de jeu : on le relie une fois installé.
        assert_eq!(p.state_of(&g), HearthStateKind::PackMissing);
        p.node.st.hearth_set_instance(&inv.group_id, &p.instance).await.unwrap();
    }
    for p in [&b, &c] {
        p.rt().probe_all().await;
        wait_for("monde synchronisé", 30, || p.rt().head(&g).is_some_and(|h| p.rt().world_complete(&h))).await;
        assert_eq!(p.rt().head(&g).unwrap().id(), v1.id());
    }
    c.stop().await; // C part hors ligne.

    // 3. Jouer chez A : EULA d'abord, puis Héberger (un membre en ligne, version à jour).
    a.rt().probe_all().await;
    assert_eq!(a.node.st.hearth_play(&card.id).await.unwrap_err(), EULA_REQUIRED);
    assert!(!a.node.st.hearth_eula().accepted);
    a.node.st.hearth_eula_accept();
    a.node.st.hearth_play(&card.id).await.unwrap();
    assert_eq!(a.state_of(&g), HearthStateKind::Hosting);
    let (inst_a, port_a) = a.launcher.launched.lock().unwrap()[0].clone();
    assert_eq!(inst_a, a.instance);
    let spec = a.runner.specs.lock().unwrap()[0].clone();
    assert!(spec.accept_eula && spec.dir.ends_with("server"));
    assert!(spec.mods_src.ends_with("mods"));
    // Le monde copié est celui de la version 1 (sans session.lock).
    let served = read_tree(&spec.dir.join("world"));
    assert!(served.iter().any(|(n, d)| n == "level.dat" && d == b"level du monde d'Alice"));
    assert!(!served.iter().any(|(n, _)| n == "session.lock"));

    // 4. B décide Rejoindre et passe par le tunnel (accès des membres).
    b.rt().probe_all().await;
    assert_eq!(b.state_of(&g), HearthStateKind::Hosted);
    let hosted = b.rt().card(&g).unwrap();
    assert_eq!(hosted.host.as_ref().unwrap().public_key, a.node.key());
    b.node.st.hearth_eula_accept();
    b.node.st.hearth_play(&card.id).await.unwrap();
    let (inst_b, port_b) = b.launcher.launched.lock().unwrap()[0].clone();
    assert_eq!(inst_b, b.instance);
    assert_ne!(port_b, port_a, "B passe par son tunnel local, pas par le port du serveur d'Alice");
    assert!(echo_works(port_b).await, "tunnel de B vers le serveur d'Alice");
    assert!(b.runner.calls.lock().unwrap().is_empty(), "B ne démarre aucun serveur");

    // 5. Fin de partie chez A : version publiée, bail relâché ; B reçoit la passation et la version.
    a.close_game();
    wait_for("serveur d'Alice arrêté", 30, || a.state_of(&g) != HearthStateKind::Hosting).await;
    assert_eq!(a.runner.calls.lock().unwrap().iter().filter(|c| **c == "stop").count(), 1);
    wait_for("version 2 chez B", 30, || b.rt().head(&g).is_some_and(|h| h.header.number == 2 && b.rt().world_complete(&h))).await;
    wait_for("passation chez B", 10, || b.node.count("hearth://handover") == 1).await;
    let v2 = b.rt().head(&g).unwrap();
    assert_eq!(v2.header.kind, VersionKind::Session);
    assert_eq!(v2.header.author, a.key());
    wait_for("file d'envoi d'Alice vidée (B a tout)", 40, || a.rt().pending_outbox() == 0).await;
    b.close_game();

    // 6. Seconde partie : B quitte pendant qu'Alice joue. La version part dans la file, puis arrive chez C.
    a.rt().probe_all().await;
    a.node.st.hearth_play(&card.id).await.unwrap();
    assert_eq!(a.state_of(&g), HearthStateKind::Hosting);
    b.stop().await;
    a.close_game();
    wait_for("serveur d'Alice arrêté (2)", 30, || a.state_of(&g) != HearthStateKind::Hosting).await;
    assert_eq!(a.rt().head(&g).unwrap().header.number, 3);
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(a.rt().pending_outbox(), 1, "personne de joignable : la version 3 attend en file");
    c.restart().await;
    c.rt().probe_all().await;
    wait_for("C a rattrapé la version 3", 40, || c.rt().head(&g).is_some_and(|h| h.header.number == 3 && c.rt().world_complete(&h))).await;
    a.rt().probe_all().await;
    wait_for("file d'envoi d'Alice vidée (C a tout)", 40, || a.rt().pending_outbox() == 0).await;
    assert_eq!(c.rt().head(&g).unwrap().id(), a.rt().head(&g).unwrap().id());

    // 7. Bail expiré : l'hôte a disparu sans relâcher son bail, B voit « partie mal terminée ».
    b.restart().await;
    b.rt().probe_all().await;
    wait_for("B à jour", 40, || b.rt().head(&g).is_some_and(|h| h.header.number == 3 && b.rt().world_complete(&h))).await;
    let head3 = b.rt().head(&g).unwrap();
    // Alice a disparu (PC éteint) : si elle répondait sans bail, Bob effacerait le sien (bail périmé).
    b.node.be.add_peer_addr(c.node.be.endpoint_addr()); // Chloé a changé d'adresse en redémarrant : seule elle reste joignable.
    a.stop().await;
    let lease = Lease { ttl_secs: 1, ..Lease::new(g, a.key(), Some((head3.id(), 3))) }.signed(&*a.node.st.identity);
    b.rt().on_lease(a.key(), lease);
    assert_eq!(b.state_of(&g), HearthStateKind::Hosted);
    tokio::time::sleep(Duration::from_millis(1300)).await;
    let unclean = b.rt().card(&g).unwrap();
    assert_eq!(unclean.state, HearthStateKind::Unclean);
    assert_eq!(unclean.unclean.as_ref().unwrap().holder.public_key, a.node.key());
    assert!(unclean.unclean.as_ref().unwrap().checkpoint_at.is_none(), "aucun point de reprise : « Attendre » ou « dernière version »");
    // Jouer ne devine pas : il faut choisir.
    let refus = b.node.st.hearth_play(&card.id).await.unwrap_err();
    assert!(refus.contains("pas terminée proprement"), "refus inattendu : {refus}");
    // Reprendre depuis la dernière version connue : B héberge, sa version 4 part chez les autres.
    b.node.st.hearth_eula_accept();
    a.restart().await;
    b.node.st.hearth_unclean_choice(&card.id, "last").await.unwrap();
    assert_eq!(b.state_of(&g), HearthStateKind::Hosting);
    b.close_game();
    wait_for("serveur de Bob arrêté", 30, || b.state_of(&g) != HearthStateKind::Hosting).await;
    wait_for("version 4 chez Alice", 40, || {
        a.rt().head(&g).is_some_and(|h| h.header.number == 4 && h.header.author == b.key() && a.rt().world_complete(&h))
    })
    .await;
    b.close_game();

    // 8. Continuer sans C : proposé par A, voté par B (majorité des membres hors C).
    c.stop().await;
    a.rt().probe_all().await;
    b.rt().probe_all().await;
    let head4 = a.rt().head(&g).unwrap();
    // Aucun signe de vie de C depuis 8 jours, chez A comme chez B (la règle des 7 jours se vérifie avant de signer).
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    a.rt().test_set_last_seen_days_ago(&g, &c.key(), 8);
    b.rt().test_set_last_seen_days_ago(&g, &c.key(), 8);
    a.rt().propose(g, GroupOp::ContinueWithout { absent: c.key(), base_version: head4.id(), absent_since: now - 8 * 86_400, at: now }).await.unwrap();
    let detail = a.node.st.hearth_detail(&card.id).await.unwrap();
    assert_eq!(detail.proposals.len(), 1);
    assert_eq!((detail.proposals[0].approvals, detail.proposals[0].needed), (1, 2));
    wait_for("proposition chez Bob", 20, || b.rt().state(&g).is_some() && !b.node.st.hearth.detail(&g).unwrap().proposals.is_empty()).await;
    let pb = b.node.st.hearth.detail(&g).unwrap().proposals.remove(0);
    assert!(pb.can_vote && pb.kind == "continue_without");
    assert_eq!(b.rt().state(&g).unwrap().epoch, 0);
    b.node.st.hearth_vote(&card.id, &pb.id).await.unwrap();
    assert_eq!(b.rt().state(&g).unwrap().epoch, 1, "la majorité des autres (2 sur 2) change d'époque");
    a.rt().probe_all().await;
    wait_for("époque 1 chez Alice", 20, || a.rt().state(&g).unwrap().epoch == 1).await;
    assert_eq!(a.rt().head(&g).unwrap().id(), head4.id(), "on repart de la version la plus récente");

    // 9. Fourche : A et B publient chacun une version 5 depuis la même base. Le groupe en garde une.
    let (da, db) = (a.dir.path().join("fourche"), b.dir.path().join("fourche"));
    for (d, txt) in [(&da, "monde d'Alice"), (&db, "monde de Bob")] {
        std::fs::create_dir_all(d).unwrap();
        std::fs::write(d.join("level.dat"), txt).unwrap();
    }
    let va = a.rt().publish_version(g, &da, VersionKind::Session, Some((head4.id(), 4)), 60).await.unwrap();
    let vb = b.rt().publish_version(g, &db, VersionKind::Session, Some((head4.id(), 4)), 90).await.unwrap();
    a.rt().probe_all().await;
    b.rt().probe_all().await;
    wait_for("fourche visible", 30, || a.rt().is_fork(&g) && b.rt().is_fork(&g)).await;
    assert_eq!(a.state_of(&g), HearthStateKind::Fork);
    let fork = a.node.st.hearth_detail(&card.id).await.unwrap().fork;
    assert_eq!(fork.len(), 2);
    assert!(a.node.st.hearth_play(&card.id).await.unwrap_err().contains("Deux versions"));
    // Alice garde la sienne : Bob reçoit la restauration et n'a plus qu'un monde.
    a.node.st.hearth_resolve_fork(&card.id, &va.id().to_hex()).await.unwrap();
    wait_for("fourche tranchée chez Alice", 10, || !a.rt().is_fork(&g)).await;
    a.rt().probe_all().await;
    wait_for("fourche tranchée chez Bob", 40, || !b.rt().is_fork(&g)).await;
    assert_eq!(b.rt().head(&g).unwrap().header.collection, va.header.collection);
    assert_ne!(vb.header.collection, va.header.collection);
    wait_for("monde gardé chez Bob", 40, || b.rt().head(&g).is_some_and(|h| b.rt().world_complete(&h))).await;
    // Historique : la version gardée est la tête, l'ancienne reste restaurable.
    let hist = b.node.st.hearth_detail(&card.id).await.unwrap().versions;
    assert!(hist.iter().any(|v| v.is_head && v.kind == "restore"));
    assert!(hist.iter().any(|v| v.id == vb.id().to_hex() && !v.is_head));

    // Le monde a fait le tour : mêmes octets chez ceux qui l'ont matérialisé (Alice l'a servi, Bob l'a joué).
    let wa = read_tree(&a.node.st.hearth.server_dir(&g).join("world"));
    assert!(wa.iter().any(|(n, _)| n == "level.dat"));

    for p in [&a, &b, &c] {
        p.stop().await;
    }
}

/// Personne d'autre en ligne : on peut quand même lancer le serveur (les amis le rejoindront), sans second clic.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn on_peut_heberger_sans_personne_d_autre_en_ligne() {
    let (a, b) = (Peer::start("Alice").await, Peer::start("Bob").await);
    befriend(&a.node, &b.node).await;
    wait_for("ami chez Bob", 20, || b.node.st.friends.is_friend(&a.node.key()).unwrap()).await;
    let card = a.node.st.hearth_create("Duo", &a.instance, None, vec![b.node.key()]).await.unwrap();
    let g = GroupId::from_hex(&card.id).unwrap();
    a.node.st.hearth_eula_accept();
    b.stop().await;
    // Bob est hors ligne : la carte le dit (bloquée), mais un seul clic sur Jouer démarre quand même le serveur et le jeu.
    assert_eq!(a.state_of(&g), HearthStateKind::Blocked);
    assert!(a.launcher.launched.lock().unwrap().is_empty());
    a.node.st.hearth_play(&card.id).await.unwrap();
    assert!(!a.launcher.launched.lock().unwrap().is_empty(), "le serveur et le jeu ont démarré");
    a.stop().await;
}

/// Simple Voice Chat dans les mods de l'instance (§ 6.7) : la carte porte `voice_unsupported`, qui suit l'ajout et le
/// retrait du jar (le serveur du groupe ne charge pas le chat vocal).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn carte_signale_le_chat_vocal_non_pris_en_charge() {
    let a = Peer::start("Alice").await;
    let b = Peer::start("Bob").await;
    befriend(&a.node, &b.node).await;
    wait_for("ami chez Bob", 20, || b.node.st.friends.is_friend(&a.node.key()).unwrap()).await;
    let card = a.node.st.hearth_create("Duo", &a.instance, None, vec![b.node.key()]).await.unwrap();
    b.stop().await;
    let g = GroupId::from_hex(&card.id).unwrap();
    assert!(!a.rt().card(&g).unwrap().voice_unsupported);
    let mods = a.node.st.instances.game_dir(&a.instance).unwrap().join("mods");
    std::fs::create_dir_all(&mods).unwrap();
    std::fs::write(mods.join("lithium-fabric-0.13.jar"), b"x").unwrap();
    assert!(!a.rt().card(&g).unwrap().voice_unsupported, "un autre mod ne déclenche rien");
    std::fs::write(mods.join("voicechat-fabric-1.21.1-2.5.26.jar"), b"x").unwrap();
    // La date du dossier change à chaque ajout : le cache ne masque pas le nouveau jar.
    assert!(a.rt().card(&g).unwrap().voice_unsupported);
    std::fs::remove_file(mods.join("voicechat-fabric-1.21.1-2.5.26.jar")).unwrap();
    assert!(!a.rt().card(&g).unwrap().voice_unsupported);
    a.stop().await;
}

/// Point de reprise interrompu pendant l'attente de `save-on` (arrêt du serveur au même moment) : la version de reprise
/// est déjà enregistrée, la `Session` finale doit en être l'enfant. Sinon V et S seraient deux enfants du même parent :
/// deux têtes, donc une fausse fourche « Deux versions » après une partie normale.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn point_de_reprise_interrompu_pendant_save_on_ne_fourche_pas() {
    let (peers, g, id) = group_of(&["Alice", "Bob"]).await;
    let a = &peers[0];
    a.node.st.hearth_play(&id).await.unwrap();
    assert_eq!(a.state_of(&g), HearthStateKind::Hosting);
    let host = a.rt().active(&g).expect("serveur d'Alice");
    a.runner.hang_end.store(true, std::sync::atomic::Ordering::SeqCst);
    let before = a.runner.calls.lock().unwrap().iter().filter(|c| **c == "save-on").count();
    let rt = a.rt().clone();
    let h = host.clone();
    let task = tokio::spawn(async move { rt.checkpoint(&h).await.map(|v| v.expect("le monde a changé : une version est publiée")) });
    // `save-on` est demandé : la version de reprise est déjà publiée, et la tâche attend la confirmation.
    wait_for("point de reprise en attente de save-on", 30, || {
        a.runner.calls.lock().unwrap().iter().filter(|c| **c == "save-on").count() > before
    })
    .await;
    let v = a.rt().head(&g).expect("la version de reprise est la tête");
    assert_eq!(v.header.kind, VersionKind::Checkpoint);
    // L'arrêt annule la tâche comme le fait `finish_hosting` (`t.abort()`), puis publie la Session.
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    a.runner.hang_end.store(false, std::sync::atomic::Ordering::SeqCst);
    let s = a.rt().finish_hosting(g).await.unwrap().expect("la session est publiée");
    assert_eq!(s.header.kind, VersionKind::Session);
    assert_eq!(s.header.parent, Some(v.id()), "la session prolonge le point de reprise");
    assert_eq!(s.header.number, v.header.number + 1);
    assert_eq!(a.rt().head(&g).expect("une seule tête").id(), s.id());
    assert!(!a.rt().is_fork(&g), "aucune fourche");
    for p in &peers {
        p.stop().await;
    }
}

/// Crée un groupe (Alice crée, les autres acceptent l'invitation et relient leur instance), sans monde de départ.
async fn group_of(names: &[&'static str]) -> (Vec<Peer>, GroupId, String) {
    let mut peers = Vec::new();
    for n in names {
        peers.push(Peer::start(n).await);
    }
    for i in 0..peers.len() {
        for j in (i + 1)..peers.len() {
            befriend(&peers[i].node, &peers[j].node).await;
        }
    }
    for p in &peers {
        let others: Vec<&Peer> = peers.iter().filter(|o| o.key() != p.key()).collect();
        wait_for("amis de tout le monde", 20, || others.iter().all(|o| p.node.st.friends.is_friend(&o.node.key()).unwrap())).await;
    }
    let others: Vec<String> = peers[1..].iter().map(|p| p.node.key()).collect();
    let card = peers[0].node.st.hearth_create("Les copains", &peers[0].instance, None, others).await.unwrap();
    let g = GroupId::from_hex(&card.id).unwrap();
    for p in &peers[1..] {
        wait_for("invitation de groupe", 30, || p.node.st.hearth_invites().len() == 1).await;
        let inv = p.node.st.hearth_invites().remove(0);
        p.node.st.hearth_invite_accept(&inv.group_id).unwrap();
        p.node.st.hearth_set_instance(&inv.group_id, &p.instance).await.unwrap();
    }
    for p in &peers {
        p.node.st.hearth_eula_accept();
        p.rt().probe_all().await;
    }
    (peers, g, card.id)
}

/// Deux membres lancent « Jouer » en même temps : la prise du bail en deux phases n'en laisse héberger qu'un seul, l'autre
/// cède (il rejoint, ou il est invité à réessayer). Un seul serveur démarre.
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn deux_candidats_simultanes_un_seul_heberge() {
    let (peers, g, id) = group_of(&["Alice", "Bob", "Chloe"]).await;
    let (a, b, c) = (&peers[0], &peers[1], &peers[2]);
    let (ra, rb) = tokio::join!(a.node.st.hearth_play(&id), b.node.st.hearth_play(&id));
    // Celui qui a cédé peut avoir reçu une erreur (« réessaie »), jamais celle d'un second serveur.
    for r in [&ra, &rb] {
        if let Err(e) = r {
            assert!(
                e.contains("démarre déjà") || e.contains("pas assez joignable") || e.contains("réessaie"),
                "refus inattendu d'un candidat : {e}"
            );
        }
    }
    let started = |p: &Peer| p.runner.calls.lock().unwrap().iter().filter(|c| **c == "start").count();
    assert_eq!(started(a) + started(b), 1, "un seul serveur démarre");
    let (host, loser) = if started(a) == 1 { (a, b) } else { (b, a) };
    assert_eq!(host.state_of(&g), HearthStateKind::Hosting);
    // Le perdant n'héberge pas ; s'il a reçu une erreur, un second clic le fait rejoindre l'hôte.
    assert_ne!(loser.state_of(&g), HearthStateKind::Hosting);
    if loser.launcher.launched.lock().unwrap().is_empty() {
        loser.rt().probe_all().await;
        loser.node.st.hearth_play(&id).await.unwrap();
    }
    assert_eq!(started(loser), 0, "le perdant ne démarre aucun serveur");
    assert_eq!(loser.launcher.launched.lock().unwrap().len(), 1, "le perdant rejoint la partie de l'hôte");
    // Le témoin voit une seule partie, celle de l'hôte.
    c.rt().probe_all().await;
    assert_eq!(c.state_of(&g), HearthStateKind::Hosted);
    assert_eq!(c.rt().card(&g).unwrap().host.as_ref().unwrap().public_key, host.node.key());
    for p in &peers {
        p.stop().await;
    }
}

/// Auto-exclusion après veille, avec des horloges simulées : l'hôte qui se réveille ne renouvelle pas son bail ; une ronde
/// `Hello` décide. Rien ne contredit : il reprend avec un nouveau `lease_id`. Un membre a une version plus récente : il
/// s'arrête (`save-all flush`, `stop`), publie sa session comme branche et relâche son bail.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auto_exclusion_apres_veille() {
    let (peers, g, id) = group_of(&["Alice", "Bob"]).await;
    let (a, b) = (&peers[0], &peers[1]);
    let clock = Arc::new(ManualClock::default());
    a.rt().set_clock(clock.clone());
    a.node.st.hearth_play(&id).await.unwrap();
    assert_eq!(a.state_of(&g), HearthStateKind::Hosting);
    let host = a.rt().active(&g).expect("serveur d'Alice");
    let first = lock(&host.lease).clone();
    assert!(first.verify(), "le bail du serveur est signé");

    // Renouvellement normal : trente secondes sur les deux horloges.
    clock.advance(Duration::from_secs(30));
    assert_eq!(a.rt().renew_tick(&host).await, RenewTick::Renewed);
    assert_eq!(lock(&host.lease).seq, first.seq + 1);
    assert_eq!(lock(&host.lease).lease_id, first.lease_id);

    // Veille d'un quart d'heure (l'horloge monotone ne l'a pas comptée) : pas de renouvellement ; personne ne contredit,
    // reprise avec un nouveau lease_id.
    clock.advance_wall(Duration::from_secs(900));
    assert_eq!(a.rt().renew_tick(&host).await, RenewTick::Resumed);
    let resumed = lock(&host.lease).clone();
    assert_ne!(resumed.lease_id, first.lease_id);
    assert_eq!(resumed.seq, 1);
    assert!(resumed.verify());
    wait_for("nouvelle partie chez Bob", 20, || b.rt().lease_of_holder(&g, &a.key()).is_some_and(|l| l.lease_id == resumed.lease_id)).await;
    assert_eq!(b.state_of(&g), HearthStateKind::Hosted);

    // Deuxième veille : pendant ce temps Bob a publié une version plus récente. L'hôte s'arrête et publie sa session
    // comme branche, sans bail.
    let dir = b.dir.path().join("monde-de-bob");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("level.dat"), "monde de Bob").unwrap();
    b.rt().publish_version(g, &dir, VersionKind::Session, None, 60).await.unwrap();
    clock.advance(Duration::from_secs(30));
    clock.advance_wall(Duration::from_secs(900));
    assert_eq!(a.rt().renew_tick(&host).await, RenewTick::Fenced);
    wait_for("serveur d'Alice arrêté", 30, || a.rt().active(&g).is_none()).await;
    let calls = a.runner.calls.lock().unwrap().clone();
    assert_eq!(calls, ["start", "save-off", "save-on", "stop"], "save-all flush, puis stop");
    wait_for("bail relâché chez Bob", 20, || b.rt().lease_of_holder(&g, &a.key()).is_none()).await;
    a.rt().probe_all().await;
    wait_for("fourche visible chez Alice", 30, || a.rt().is_fork(&g)).await;
    for p in &peers {
        p.stop().await;
    }
}

/// Alice héberge ; `joined` : Steve entre puis sort (le serveur compte 0 joueur mais le monde a été joué). Rend le
/// groupe, Bob ayant publié une version plus récente et son bail `Ready` étant prêt à être reçu par Alice.
async fn alice_heberge_et_bob_a_un_bail_plus_fort(joined: bool) -> (Vec<Peer>, GroupId, Lease) {
    let (peers, g, id) = group_of(&["Alice", "Bob"]).await;
    let (a, b) = (&peers[0], &peers[1]);
    a.node.st.hearth_play(&id).await.unwrap();
    assert_eq!(a.state_of(&g), HearthStateKind::Hosting);
    assert_eq!(a.node.be.shared_instance().as_deref(), Some(a.instance.as_str()), "le monde est partagé");
    if joined {
        let on_line = a.runner.specs.lock().unwrap()[0].on_line.clone();
        on_line("[12:00:00] [Server thread/INFO]: Steve joined the game");
        on_line("[12:09:00] [Server thread/INFO]: Steve left the game");
        std::fs::write(a.runner.specs.lock().unwrap()[0].dir.join("world").join("neuf-minutes.txt"), "9 minutes de jeu").unwrap();
    }
    let host = a.rt().active(&g).expect("serveur d'Alice");
    assert_eq!(host.players(), 0);
    // Bob a publié une version plus récente que la base d'Alice : son bail l'emporte au départage.
    let dir = b.dir.path().join("monde-de-bob");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("level.dat"), "monde de Bob").unwrap();
    let v = b.rt().publish_version(g, &dir, VersionKind::Session, None, 60).await.unwrap();
    wait_for("version de Bob chez Alice", 30, || a.rt().version(&g, &v.id()).is_some()).await;
    let lease = Lease::new(g, b.key(), Some((v.id(), v.header.number))).signed(&*b.node.st.identity);
    (peers, g, lease)
}

/// Un bail concurrent qui l'emporte alors que mon serveur est vide, après une vraie partie : la session est publiée
/// comme branche (`Suspend`), le serveur est arrêté après `save-all flush`, et le partage est fermé (`host_stop_sharing`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bail_concurrent_serveur_vide_publie_et_ferme_le_partage() {
    let (peers, g, lease) = alice_heberge_et_bob_a_un_bail_plus_fort(true).await;
    let (a, b) = (&peers[0], &peers[1]);
    let before = a.rt().heads_of(&g).unwrap().len();
    // Le bail adverse est renouvelé toutes les quelques secondes : plusieurs réceptions, une seule fin de partie.
    for _ in 0..3 {
        a.rt().on_lease(b.key(), lease.clone());
    }
    wait_for("serveur d'Alice arrêté", 30, || a.rt().active(&g).is_none()).await;
    wait_for("partage fermé", 30, || a.node.be.shared_instance().is_none()).await;
    assert_ne!(a.state_of(&g), HearthStateKind::Hosting);
    assert_eq!(a.runner.calls.lock().unwrap().clone(), ["start", "save-off", "save-on", "stop"], "save-all flush, puis un seul stop");
    wait_for("bail relâché chez Bob", 20, || b.rt().lease_of_holder(&g, &a.key()).is_none()).await;
    // La session d'Alice est publiée : une branche de plus, visible comme fourche chez Alice et chez Bob.
    a.rt().probe_all().await;
    b.rt().probe_all().await;
    wait_for("session d'Alice publiée (fourche)", 30, || a.rt().is_fork(&g) && a.rt().heads_of(&g).unwrap().len() > before).await;
    wait_for("fourche chez Bob", 30, || b.rt().is_fork(&g)).await;
    for p in &peers {
        p.stop().await;
    }
}

/// Personne n'est jamais entré : rien à publier. Le serveur s'arrête quand même et le partage est fermé.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bail_concurrent_serveur_jamais_joue_ne_publie_rien_mais_ferme_le_partage() {
    let (peers, g, lease) = alice_heberge_et_bob_a_un_bail_plus_fort(false).await;
    let (a, b) = (&peers[0], &peers[1]);
    let before = a.rt().heads_of(&g).unwrap().len();
    a.rt().on_lease(b.key(), lease);
    wait_for("serveur d'Alice arrêté", 30, || a.rt().active(&g).is_none()).await;
    wait_for("partage fermé", 30, || a.node.be.shared_instance().is_none()).await;
    assert_eq!(a.runner.calls.lock().unwrap().clone(), ["start", "stop"], "pas de publication : pas de save-all");
    assert_eq!(a.rt().heads_of(&g).unwrap().len(), before, "aucune version publiée");
    wait_for("bail relâché chez Bob", 20, || b.rt().lease_of_holder(&g, &a.key()).is_none()).await;
    for p in &peers {
        p.stop().await;
    }
}

/// Un joueur est connecté : un bail concurrent ne coupe pas le serveur.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bail_concurrent_serveur_occupe_continue() {
    let (peers, g, lease) = alice_heberge_et_bob_a_un_bail_plus_fort(true).await;
    let (a, b) = (&peers[0], &peers[1]);
    let on_line = a.runner.specs.lock().unwrap()[0].on_line.clone();
    on_line("[12:10:00] [Server thread/INFO]: Steve joined the game");
    assert_eq!(a.rt().active(&g).unwrap().players(), 1);
    a.rt().on_lease(b.key(), lease);
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(a.rt().active(&g).is_some(), "le serveur occupé reste");
    assert_eq!(a.state_of(&g), HearthStateKind::Hosting);
    assert!(!a.runner.calls.lock().unwrap().contains(&"stop"));
    assert!(a.node.be.shared_instance().is_some());
    for p in &peers {
        p.stop().await;
    }
}

/// Interrupteur de sécurité : coupé, les commandes refusent avec un message clair, la lecture d'état reste possible, et le
/// protocole `cubelink/hearth/1` ne reconnaît plus ni amis ni membres.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interrupteur_coupe_refuse_les_commandes_et_les_pairs() {
    let (peers, g, id) = group_of(&["Alice", "Bob"]).await;
    let (a, b) = (&peers[0], &peers[1]);
    assert!(a.rt().is_friend(&b.key()));
    assert!(a.rt().group_state(&g).is_some());
    a.rt().set_enabled(false);
    assert!(!a.node.st.hearth_enabled());
    let e = a.node.st.hearth_play(&id).await.unwrap_err();
    assert!(e.contains("désactivés"), "{e}");
    assert!(a.node.st.hearth_create("X", &a.instance, None, vec![]).await.unwrap_err().contains("désactivés"));
    assert!(a.node.st.hearth_leave(&id, false).await.unwrap_err().to_string().contains("désactivés"));
    assert_eq!(a.node.st.hearth_list().len(), 1, "la lecture d'état reste possible");
    assert!(!a.rt().is_friend(&b.key()), "le protocole refuse les pairs");
    assert!(a.rt().group_state(&g).is_none());
    a.rt().set_enabled(true);
    assert!(a.rt().group_state(&g).is_some());
    for p in &peers {
        p.stop().await;
    }
}

/// Oublier un groupe pendant qu'on l'héberge : le serveur est arrêté, l'accès fermé, les données supprimées.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn oublier_un_groupe_arrete_l_hote_et_supprime_les_donnees() {
    let (peers, g, id) = group_of(&["Alice", "Bob"]).await;
    let a = &peers[0];
    a.node.st.hearth_play(&id).await.unwrap();
    assert_eq!(a.state_of(&g), HearthStateKind::Hosting);
    let dir = a.rt().server_dir(&g);
    assert!(dir.is_dir());
    a.rt().forget_group_wait(&g).await.unwrap();
    assert!(a.runner.calls.lock().unwrap().contains(&"stop"), "le serveur actif est arrêté");
    assert!(a.rt().active(&g).is_none());
    assert!(a.rt().state(&g).is_none());
    assert!(!dir.exists(), "les données du groupe sont supprimées");
    assert!(!a.node.st.access.serving_group(), "l'accès du groupe est fermé");
    for p in &peers {
        p.stop().await;
    }
}

/// Dernier membre : « Quitter » est refusé avec le code `LastMember`, AVANT d'arrêter mon serveur, et le monde reste sur le
/// disque (#57 : l'écran ne doit plus pouvoir effacer la seule copie sur une erreur).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn quitter_en_dernier_membre_refuse_et_garde_le_monde() {
    let (peers, g, id) = group_of(&["Alice", "Bob"]).await;
    let (a, b) = (&peers[0], &peers[1]);
    // Bob part : Alice devient la seule membre.
    b.node.st.hearth_leave(&id, false).await.unwrap();
    wait_for("Alice est seule", 30, || a.rt().state(&g).is_some_and(|s| s.members.len() == 1)).await;
    a.node.st.hearth_play(&id).await.unwrap();
    assert_eq!(a.state_of(&g), HearthStateKind::Hosting);
    let dir = a.rt().server_dir(&g);
    assert!(dir.is_dir());
    assert_eq!(a.node.st.hearth_leave(&id, false).await.unwrap_err(), LeaveError::LastMember);
    assert!(a.rt().state(&g).is_some(), "le groupe existe toujours");
    assert!(dir.is_dir(), "le dossier du monde existe toujours");
    assert!(a.rt().active(&g).is_some(), "mon serveur n'a pas été arrêté");
    assert!(!a.runner.calls.lock().unwrap().contains(&"stop"));
    a.stop().await;
}

/// Personne n'est joignable et rien n'attend dans la file : « Quitter » rejette `Unreachable` (et non un message libre),
/// sans rien supprimer, et reste rejouable ; seul `hearth_forget` (choix explicite de l'utilisateur) efface ensuite le
/// monde de ce PC.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn quitter_sans_personne_de_joignable_ne_supprime_rien() {
    let (peers, g, id) = group_of(&["Alice", "Bob"]).await;
    let (a, b) = (&peers[0], &peers[1]);
    let dir = a.rt().server_dir(&g);
    std::fs::create_dir_all(&dir).unwrap();
    b.stop().await;
    assert_eq!(a.node.st.hearth_leave(&id, false).await.unwrap_err(), LeaveError::Unreachable);
    assert!(a.rt().state(&g).is_some() && dir.is_dir(), "rien n'est supprimé");
    assert_eq!(a.node.st.hearth_leave(&id, false).await.unwrap_err(), LeaveError::Unreachable, "rejouable");
    assert!(a.rt().state(&g).is_some() && dir.is_dir());
    a.node.st.hearth_forget(&id).await.unwrap();
    assert!(a.rt().state(&g).is_none());
    assert!(!dir.exists(), "forget supprime le monde local");
    a.stop().await;
}

/// #56 : Alice joue puis quitte tout de suite. Sa `Session` est en file au moment de signer `Leave` : elle doit être remise à
/// Bob AVANT que le groupe soit oublié (avant le correctif, `forget_now` la jetait de la file). En loopback la remise est vite faite : ce scénario garde le chemin
/// nominal ; le refus quand la remise est impossible est couvert par le test suivant, qui échouait avant le correctif.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn quitter_juste_apres_une_partie_la_session_arrive_chez_bob() {
    let (peers, g, id) = group_of(&["Alice", "Bob"]).await;
    let (a, b) = (&peers[0], &peers[1]);
    a.node.st.hearth_play(&id).await.unwrap();
    let dir = a.rt().server_dir(&g);
    a.close_game();
    a.node.st.hearth_leave(&id, false).await.unwrap();
    assert!(a.rt().state(&g).is_none(), "Alice a quitté");
    wait_for("son monde local est supprimé", 30, || !dir.exists()).await;
    wait_for("la Session d'Alice est chez Bob", 30, || {
        b.rt().head(&g).is_some_and(|h| h.header.author == a.key() && h.header.kind == VersionKind::Session && b.rt().world_complete(&h))
    })
    .await;
    for p in &peers {
        p.stop().await;
    }
}

/// #56 : la dernière partie n'a pu être remise à personne (Bob est hors ligne) : « Quitter » rejette `OutboxPending` sans rien
/// supprimer (la version reste en file, le serveur est arrêté), vite et en restant rejouable. `force` saute l'attente et
/// retombe sur `Unreachable` (#57), puis seul `hearth_forget` efface le monde.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn quitter_avec_une_partie_non_envoyee_refuse_puis_force() {
    let (peers, g, id) = group_of(&["Alice", "Bob"]).await;
    let (a, b) = (&peers[0], &peers[1]);
    a.node.st.hearth_play(&id).await.unwrap();
    let dir = a.rt().server_dir(&g);
    a.close_game();
    b.stop().await;
    let t = std::time::Instant::now();
    assert_eq!(a.node.st.hearth_leave(&id, false).await.unwrap_err(), LeaveError::OutboxPending);
    assert!(t.elapsed() < std::time::Duration::from_secs(30), "personne de joignable : pas d'attente de 5 minutes ({:?})", t.elapsed());
    assert!(a.rt().state(&g).is_some() && dir.is_dir(), "rien n'est supprimé");
    assert!(a.rt().pending_outbox() > 0, "la Session reste en file");
    assert!(a.rt().active(&g).is_none(), "le serveur est arrêté");
    assert_eq!(a.node.st.hearth_leave(&id, false).await.unwrap_err(), LeaveError::OutboxPending, "rejouable");
    assert_eq!(a.node.st.hearth_leave(&id, true).await.unwrap_err(), LeaveError::Unreachable, "force : personne n'a reçu le départ");
    assert!(a.rt().state(&g).is_some() && dir.is_dir(), "toujours rien de supprimé");
    a.node.st.hearth_forget(&id).await.unwrap();
    assert!(a.rt().state(&g).is_none());
    assert!(!dir.exists());
    a.stop().await;
}

/// `force` : Bob est en ligne mais on choisit de ne pas attendre la file d'envoi : le départ aboutit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn quitter_force_ne_attend_pas_la_file() {
    let (peers, g, id) = group_of(&["Alice", "Bob"]).await;
    let (a, _b) = (&peers[0], &peers[1]);
    a.node.st.hearth_play(&id).await.unwrap();
    a.close_game();
    a.node.st.hearth_leave(&id, true).await.unwrap();
    assert!(a.rt().state(&g).is_none());
    for p in &peers {
        p.stop().await;
    }
}

/// Invitations de groupe reçues (`on_invite`) : un ami qui n'est pas membre d'un groupe connu ne change pas sa clé ; au
/// plus 3 invitations par ami et par jour, une par groupe, la plus ancienne évincée au-delà de 16.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invitations_de_groupe_limitees_et_emetteur_membre_exige() {
    use p2p_lib::hearth::model::{PackPin, SignedOp};
    use p2p_lib::hearth::store::GroupKey;
    let (peers, g, _id) = group_of(&["Alice", "Bob"]).await;
    let a = &peers[0];
    let pack = PackPin {
        manifest_hash: "ab".repeat(32),
        minecraft: "1.21.1".into(),
        loader: core_lib::types::LoaderSpec { loader: core_lib::types::Loader::NeoForge, version: "21.1.77".into() },
    };
    let me: BTreeSet<[u8; 32]> = [a.key()].into();
    let sender = |_n: u8| {
        let dir = tempfile::tempdir().unwrap();
        let id = p2p_lib::identity::Identity::load_or_create(dir.path()).unwrap();
        (dir, id)
    };
    let invite_op = |id: &p2p_lib::identity::Identity, gid: GroupId| SignedOp::create(id, gid, "Autre", pack.clone(), &me).unwrap();

    // Groupe connu : l'émetteur doit être membre dans NOTRE état.
    let (_d, stranger) = sender(0);
    let known = a.rt().ops_after(&g, 0, 100);
    let r = a.rt().on_invite(stranger.public_key().to_bytes(), known, GroupKey::random());
    assert!(r.is_err(), "un non-membre ne renouvelle pas la clé d'un groupe connu");

    // 3 par ami et par jour ; une seule par groupe.
    let (_d1, s1) = sender(1);
    let k1 = s1.public_key().to_bytes();
    let same = GroupId::random();
    a.rt().on_invite(k1, vec![invite_op(&s1, same)], GroupKey::random()).unwrap();
    a.rt().on_invite(k1, vec![invite_op(&s1, same)], GroupKey::random()).unwrap();
    assert_eq!(a.rt().invites().len(), 1, "une invitation par groupe");
    a.rt().on_invite(k1, vec![invite_op(&s1, GroupId::random())], GroupKey::random()).unwrap();
    let e = a.rt().on_invite(k1, vec![invite_op(&s1, GroupId::random())], GroupKey::random()).unwrap_err();
    assert!(e.contains("trop"), "{e}");
    assert_eq!(a.rt().invites().len(), 2);

    // File pleine (16) : la plus ancienne est évincée.
    let mut dirs = Vec::new();
    let mut first = None;
    for i in 0..6 {
        let (d, s) = sender(10 + i);
        for _ in 0..3 {
            let gid = GroupId::random();
            first.get_or_insert(gid.to_hex());
            a.rt().on_invite(s.public_key().to_bytes(), vec![invite_op(&s, gid)], GroupKey::random()).unwrap();
        }
        dirs.push(d);
    }
    let list = a.rt().invites();
    assert_eq!(list.len(), 16);
    assert!(!list.iter().any(|i| Some(&i.group_id) == first.as_ref()), "la plus ancienne est évincée");
    assert!(!list.iter().any(|i| i.group_id == same.to_hex()), "les premières invitations aussi");
    for p in &peers {
        p.stop().await;
    }
}

/// Écrit un petit monde dans `dir` (chaque `txt` donne un monde distinct).
fn small_world(dir: &Path, txt: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("level.dat"), txt).unwrap();
}

/// Eve publie une version dont personne d'autre n'a le contenu, puis disparaît : « Continuer sans Eve » repart de la
/// dernière version dont le monde est complet (la n° 1), pas de la tête d'Eve. Après le vote, « Jouer » fonctionne.
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn continuer_sans_repart_de_la_derniere_version_complete() {
    let (peers, g, id) = group_of(&["Alice", "Bob", "Eve"]).await;
    let (a, b, e) = (&peers[0], &peers[1], &peers[2]);
    // n° 1 d'Alice, complète chez tout le monde.
    let d1 = a.dir.path().join("monde-1");
    small_world(&d1, "monde n° 1");
    let v1 = a.rt().publish_version(g, &d1, VersionKind::Session, None, 60).await.unwrap();
    wait_for("n° 1 complète chez Bob et Eve", 40, || {
        [b, e].iter().all(|p| p.rt().head(&g).is_some_and(|h| h.id() == v1.id() && p.rt().world_complete(&h)))
    })
    .await;
    // n° 2 d'Eve : l'en-tête part chez les autres, mais son contenu n'existe nulle part ailleurs (Eve est ensuite éteinte).
    let v2 = e.rt().record_version(g, VersionKind::Session, Some((v1.id(), 1)), 90, [7; 32], 4096).unwrap();
    wait_for("en-tête n° 2 chez Alice et Bob", 40, || [a, b].iter().all(|p| p.rt().head(&g).is_some_and(|h| h.id() == v2.id()))).await;
    e.stop().await;
    for p in [a, b] {
        p.rt().probe_all().await;
        p.rt().test_set_last_seen_days_ago(&g, &e.key(), 8);
    }
    for p in [a, b] {
        assert!(!p.rt().world_complete(&v2), "personne n'a le monde de la n° 2");
        let blocker = p.node.st.hearth.detail(&g).unwrap().blocker.expect("Eve bloque : sa version n'a pas de contenu");
        assert_eq!(blocker.member.public_key, e.node.key());
        assert_eq!(p.rt().complete_head(&g).unwrap().id(), v1.id(), "la tête complète est la n° 1");
    }

    // Bob propose, Alice vote : époque 1, repartie de la n° 1.
    b.node.st.hearth_continue_without(&id, &e.node.key()).await.unwrap();
    wait_for("proposition chez Alice", 20, || !a.node.st.hearth.detail(&g).unwrap().proposals.is_empty()).await;
    let prop = a.node.st.hearth.detail(&g).unwrap().proposals.remove(0);
    assert!(prop.can_vote && prop.kind == "continue_without");
    a.node.st.hearth_vote(&id, &prop.id).await.unwrap();
    wait_for("époque 1 chez Bob", 20, || b.rt().state(&g).is_some_and(|s| s.epoch == 1)).await;
    for p in [a, b] {
        assert_eq!(p.rt().state(&g).unwrap().epoch, 1);
        let head = p.rt().head(&g).unwrap();
        assert_eq!(head.id(), v1.id(), "la nouvelle époque repart de la n° 1, pas de la n° 2 sans contenu");
        assert!(p.rt().world_complete(&head));
        // La base est d'une époque précédente : elle reste annoncée comme version complète (et non « aucune »).
        assert_eq!(p.rt().complete_heads_of(&g), vec![v1.id()], "la base de la nouvelle époque est annoncée");
        assert_eq!(p.rt().complete_head(&g).map(|v| v.id()), Some(v1.id()));
        let d = p.node.st.hearth.detail(&g).unwrap();
        assert!(d.blocker.is_none(), "plus personne ne bloque");
        assert!(d.versions.iter().any(|v| v.id == v2.id().to_hex() && v.side_branch), "la n° 2 reste restaurable, en branche");
    }

    // « Jouer » fonctionne : Bob héberge à partir du monde de la n° 1.
    b.node.st.hearth_play(&id).await.unwrap();
    assert_eq!(b.state_of(&g), HearthStateKind::Hosting);
    assert!(read_tree(&b.rt().server_dir(&g).join("world")).iter().any(|(n, c)| n == "level.dat" && c == b"monde n\xc2\xb0 1"));
    b.close_game();
    wait_for("serveur de Bob arrêté", 30, || b.state_of(&g) != HearthStateKind::Hosting).await;
    for p in &peers {
        p.stop().await;
    }
}

/// Eve a publié la dernière version et son monde est déjà complet chez Alice : son absence ne bloque rien, pas de
/// bannière « Continuer sans Eve », et la commande est refusée.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pas_de_banniere_quand_le_monde_de_l_absent_est_deja_complet() {
    let (peers, g, id) = group_of(&["Alice", "Eve"]).await;
    let (a, e) = (&peers[0], &peers[1]);
    let d1 = e.dir.path().join("monde-eve");
    small_world(&d1, "monde d'Eve");
    let v1 = e.rt().publish_version(g, &d1, VersionKind::Session, None, 60).await.unwrap();
    wait_for("monde d'Eve complet chez Alice", 40, || a.rt().head(&g).is_some_and(|h| h.id() == v1.id() && a.rt().world_complete(&h))).await;
    e.stop().await;
    a.rt().probe_all().await;
    a.rt().test_set_last_seen_days_ago(&g, &e.key(), 8);
    let d = a.node.st.hearth.detail(&g).unwrap();
    assert!(d.blocker.is_none(), "Alice a le monde : Eve ne bloque rien");
    assert_eq!(d.members.iter().find(|m| m.public_key == e.node.key()).unwrap().unseen_days, Some(8));
    let err = a.node.st.hearth_continue_without(&id, &e.node.key()).await.unwrap_err();
    assert!(err.contains("personne ne bloque"), "{err}");
    for p in &peers {
        p.stop().await;
    }
}

/// Alice publie une n° 2 dont personne n'a le contenu (son PC s'éteint avant l'envoi) : l'en-tête arrive chez Bob et Carol.
/// Ils n'annoncent que la n° 1 (la seule complète), Jouer dit « head_unavailable » avec le repli sur la n° 1 au lieu d'échouer
/// sur « aucun membre joignable », et « Jouer depuis la n° 1 » démarre le serveur sur le monde de la n° 1 (fourche à la
/// publication, visible dans la page du serveur).
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn en_tete_sans_contenu_jouer_propose_la_version_precedente() {
    let (peers, g, id) = group_of(&["Alice", "Bob", "Carol"]).await;
    let (a, b, c) = (&peers[0], &peers[1], &peers[2]);
    let d1 = a.dir.path().join("monde-1");
    small_world(&d1, "monde n° 1");
    let v1 = a.rt().publish_version(g, &d1, VersionKind::Session, None, 60).await.unwrap();
    wait_for("n° 1 complète chez Bob et Carol", 40, || {
        [b, c].iter().all(|p| p.rt().head(&g).is_some_and(|h| h.id() == v1.id() && p.rt().world_complete(&h)))
    })
    .await;
    // n° 2 d'Alice : l'en-tête part chez les autres, le contenu n'existe nulle part ailleurs ; Alice s'éteint.
    let v2 = a.rt().record_version(g, VersionKind::Session, Some((v1.id(), 1)), 90, [9; 32], 4096).unwrap();
    wait_for("en-tête n° 2 chez Bob et Carol", 40, || [b, c].iter().all(|p| p.rt().head(&g).is_some_and(|h| h.id() == v2.id()))).await;
    a.stop().await;
    for p in [b, c] {
        assert!(!p.rt().world_complete(&v2), "personne n'a le monde de la n° 2");
        assert_eq!(p.rt().complete_heads_of(&g), vec![v1.id()], "seule la n° 1 est annoncée");
    }
    // Chacun voit l'autre à la n° 1, pas à la n° 2 dont il n'a que l'en-tête.
    for (me, other) in [(b, c), (c, b)] {
        me.rt().probe_all().await;
        let d = me.node.st.hearth.detail(&g).unwrap();
        let m = d.members.iter().find(|m| m.public_key == other.node.key()).unwrap();
        assert_eq!(m.version, Some(1), "{} annonce la n° 1 (complète), pas la n° 2", other.name);
        let moi = d.members.iter().find(|m| m.is_me).unwrap();
        assert_eq!((moi.version, moi.sync.as_str()), (Some(1), "behind"), "mon monde le plus avancé est la n° 1");
    }

    // Jouer : erreur typée avec le repli, jamais « aucun membre joignable ».
    b.node.st.hearth_eula_accept();
    let refus = b.node.st.hearth_play(&id).await.unwrap_err();
    assert_eq!(refus, "head_unavailable|2|1|Alice", "{refus}");
    assert_ne!(b.state_of(&g), HearthStateKind::Hosting);
    assert!(b.runner.calls.lock().unwrap().is_empty(), "aucun serveur démarré, aucun bail annoncé pour rien");

    // Jouer depuis la n° 1 : Bob héberge le monde de la n° 1.
    b.node.st.hearth_play_from_previous(&id).await.unwrap();
    assert_eq!(b.state_of(&g), HearthStateKind::Hosting);
    assert!(read_tree(&b.rt().server_dir(&g).join("world")).iter().any(|(n, c)| n == "level.dat" && c == b"monde n\xc2\xb0 1"));
    let lease = lock(&b.rt().active(&g).expect("serveur de Bob").lease).clone();
    assert_eq!(lease.base_version, Some(v1.id()), "le bail part de la n° 1");

    // Fin de partie : la session de Bob est une nouvelle version fille de la n° 1, face à la n° 2 d'Alice : fourche visible.
    b.close_game();
    wait_for("serveur de Bob arrêté", 30, || b.state_of(&g) != HearthStateKind::Hosting).await;
    assert!(b.rt().is_fork(&g), "deux versions du monde : la n° 2 d'Alice et la reprise de Bob");
    let d = b.node.st.hearth.detail(&g).unwrap();
    assert_eq!(d.fork.len(), 2, "la page du serveur propose de choisir entre les deux branches");
    for p in &peers {
        p.stop().await;
    }
}

// ---------------------------------------------------------------------------------------------------------
// #61 : partie mal terminée, le jeu non publié n'est jamais écrasé ni publié tout seul
// ---------------------------------------------------------------------------------------------------------

const SENTINELLE: &[u8] = b"8 minutes de jeu que personne n'a publiees";

/// Alice part de la n° 1, joue (la sentinelle est écrite dans `world/`), puis son Cubelink s'arrête sans publier (plantage).
/// Rend les pairs, le groupe, l'identifiant et la n° 1.
async fn alice_plante() -> (Vec<Peer>, GroupId, String, p2p_lib::hearth::version::SignedVersion) {
    let (mut peers, g, id) = group_of(&["Alice", "Bob"]).await;
    let d1 = peers[0].dir.path().join("monde-1");
    small_world(&d1, "monde n° 1");
    let v1 = peers[0].rt().publish_version(g, &d1, VersionKind::Session, None, 60).await.unwrap();
    peers[1].stop().await; // Bob est hors ligne : un seul clic sur Jouer suffit à héberger.
    let a = &peers[0];
    a.node.st.hearth_play(&id).await.unwrap();
    assert_eq!(a.state_of(&g), HearthStateKind::Hosting);
    assert!(a.rt().has_session_marker(&g), "le marqueur de session est écrit au démarrage du serveur");
    std::fs::write(a.rt().server_dir(&g).join("world/region-jouee.mca"), SENTINELLE).unwrap();
    // Plantage : le nœud disparaît sans `finish_hosting`. Ce qui retient le runtime est lâché pour que le magasin de blobs
    // libère son verrou : la surveillance du jeu (le serveur reste ouvert quand le jeu se ferme) et les spécifications
    // gardées par le faux lanceur (le rappel des lignes de log).
    a.rt().set_keep_background(&g, true);
    a.close_game();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    a.runner.specs.lock().unwrap().clear();
    peers[0].restart().await;
    let a = &peers[0];
    a.rt().set_keep_background(&g, false);
    assert!(a.rt().active(&g).is_none());
    assert!(a.rt().has_session_marker(&g), "le marqueur survit au plantage");
    (peers, g, id, v1)
}

fn orphan_files(a: &Peer, g: &GroupId) -> Vec<Vec<(String, Vec<u8>)>> {
    let dir = a.rt().server_dir(g).join("orphans");
    let mut v: Vec<_> = std::fs::read_dir(&dir).map(|d| d.flatten().map(|e| read_tree(&e.path().join("world"))).collect()).unwrap_or_default();
    v.sort();
    v
}

/// Le monde non publié est mis de côté, pas écrasé, quand la tête a avancé pendant ce temps (le cas du ticket : n° 12 chez
/// Bob, `materialized` en retard). Rien n'est publié ni mis en file d'envoi. L'hôte publie ensuite la partie : une
/// `Session` enfant de la version dont elle est partie, donc une fourche explicite.
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn partie_mal_terminee_le_monde_est_mis_de_cote_puis_publie() {
    let (peers, g, id, v1) = alice_plante().await;
    let a = &peers[0];
    a.node.st.hearth_eula_accept();
    // La tête avance pendant que le PC d'Alice est éteint (version d'un autre membre, simulée ici).
    let d2 = a.dir.path().join("monde-2");
    small_world(&d2, "monde n° 2");
    let v2 = a.rt().publish_version(g, &d2, VersionKind::Session, Some((v1.id(), 1)), 30).await.unwrap();
    let (heads_before, outbox_before) = (a.rt().heads_of(&g).unwrap().len(), a.rt().pending_outbox());
    assert_eq!(a.rt().head(&g).unwrap().id(), v2.id());

    a.node.st.hearth_play(&id).await.unwrap();
    assert_eq!(a.state_of(&g), HearthStateKind::Hosting);
    let world = a.rt().server_dir(&g).join("world");
    let served = read_tree(&world);
    assert!(served.iter().any(|(n, c)| n == "level.dat" && c == "monde n° 2".as_bytes()), "la tête est matérialisée");
    assert!(!served.iter().any(|(n, _)| n == "region-jouee.mca"), "le jeu non publié n'est plus dans world/");

    let kept = orphan_files(a, &g);
    assert_eq!(kept.len(), 1, "une fin de partie est gardée de côté");
    assert!(kept[0].iter().any(|(n, c)| n == "region-jouee.mca" && c == SENTINELLE), "la sentinelle est intacte");
    let d = a.node.st.hearth.detail(&g).unwrap();
    assert_eq!((a.rt().card(&g).unwrap().orphans, d.orphans.len()), (1, 1));
    assert!(d.orphans[0].size_bytes >= SENTINELLE.len() as u64);
    assert_eq!(d.orphans[0].base_number, Some(1), "la partie était partie de la n° 1");
    assert_eq!((a.rt().heads_of(&g).unwrap().len(), a.rt().pending_outbox()), (heads_before, outbox_before), "rien n'est publié ni annoncé");

    // Fin propre de la nouvelle partie : le marqueur disparaît, la fin de partie reste gardée.
    a.close_game();
    wait_for("serveur arrêté", 30, || a.state_of(&g) != HearthStateKind::Hosting).await;
    // L'état quitte `Hosting` dès le début de la fin de partie : on attend la publication, qui retire le marqueur.
    wait_for("marqueur retiré", 30, || !a.rt().has_session_marker(&g)).await;
    assert!(!a.rt().has_session_marker(&g), "la fin propre retire le marqueur");
    assert_eq!(orphan_files(a, &g).len(), 1);
    let head_after = a.rt().head(&g).unwrap();
    assert_eq!((head_after.header.kind, head_after.header.parent), (VersionKind::Session, Some(v2.id())));

    // « Quitter » ne supprime pas la fin de partie sans qu'on le décide.
    assert_eq!(a.node.st.hearth_leave(&id, false).await.unwrap_err(), LeaveError::OrphanPending);
    assert!(a.rt().state(&g).is_some() && !orphan_files(a, &g).is_empty());

    // Publier : une Session enfant de la n° 1, donc une fourche avec la chaîne de la n° 2.
    let oid = a.node.st.hearth.detail(&g).unwrap().orphans[0].id.clone();
    a.node.st.hearth_orphan_publish(&id, &oid).await.unwrap();
    assert!(orphan_files(a, &g).is_empty() && a.rt().card(&g).unwrap().orphans == 0);
    let heads = a.rt().heads_of(&g).unwrap();
    assert_eq!(heads.len(), 2, "fourche explicite");
    let mine = heads.iter().find(|h| h.header.parent == Some(v1.id())).expect("enfant de la n° 1");
    assert_eq!(mine.header.kind, VersionKind::Session);
    assert!(a.rt().world_complete(mine));
    assert!(a.rt().pending_outbox() > outbox_before, "la version publiée part en file d'envoi");
    assert!(a.node.st.hearth_orphan_publish(&id, &oid).await.is_err(), "déjà publiée");
    for p in &peers {
        p.stop().await;
    }
}

/// Même cas quand la tête n'a PAS avancé (`materialized == tête`) : avant, le monde non publié était repris tel quel puis
/// publié automatiquement. Maintenant il est mis de côté et la tête est rematérialisée ; « Supprimer » le retire.
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn partie_mal_terminee_tete_inchangee_n_est_pas_republiee_toute_seule() {
    let (peers, g, id, v1) = alice_plante().await;
    let a = &peers[0];
    a.node.st.hearth_eula_accept();
    assert_eq!(a.rt().materialized(&g), Some(v1.id()), "le cas du ticket : materialized == tête");
    let (versions_before, outbox_before) = (a.rt().heads_of(&g).unwrap().len(), a.rt().pending_outbox());

    a.node.st.hearth_play(&id).await.unwrap();
    let served = read_tree(&a.rt().server_dir(&g).join("world"));
    assert!(!served.iter().any(|(n, _)| n == "region-jouee.mca"), "la tête est remise à neuf");
    assert!(served.iter().any(|(n, c)| n == "level.dat" && c == "monde n° 1".as_bytes()));
    assert!(orphan_files(a, &g)[0].iter().any(|(n, c)| n == "region-jouee.mca" && c == SENTINELLE));
    assert_eq!(a.rt().head(&g).unwrap().id(), v1.id(), "aucune Session n'a été publiée");
    assert_eq!((a.rt().heads_of(&g).unwrap().len(), a.rt().pending_outbox()), (versions_before, outbox_before));

    let oid = a.node.st.hearth.detail(&g).unwrap().orphans[0].id.clone();
    assert!(a.node.st.hearth_orphan_discard(&id, "../../etc").await.is_err(), "l'identifiant ne sort jamais du dossier");
    a.node.st.hearth_orphan_discard(&id, &oid).await.unwrap();
    assert!(orphan_files(a, &g).is_empty());
    assert_eq!(a.node.st.hearth.detail(&g).unwrap().orphans.len(), 0);
    for p in &peers {
        p.stop().await;
    }
}

/// Si la mise de côté échoue, `prepare_world` rejette sans rien supprimer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mise_de_cote_impossible_ne_supprime_rien() {
    let (peers, g, id, _v1) = alice_plante().await;
    let a = &peers[0];
    a.node.st.hearth_eula_accept();
    // `orphans` est un fichier : le dossier de la fin de partie ne peut pas être créé.
    std::fs::write(a.rt().server_dir(&g).join("orphans"), b"x").unwrap();
    let e = a.node.st.hearth_play(&id).await.unwrap_err();
    assert!(e.contains("mise de côté") || e.contains("de côté"), "{e}");
    assert_eq!(std::fs::read(a.rt().server_dir(&g).join("world/region-jouee.mca")).unwrap(), SENTINELLE, "le monde est intact");
    assert!(a.rt().has_session_marker(&g));
    assert_eq!(a.node.st.hearth.detail(&g).unwrap().orphans.len(), 0);
    for p in &peers {
        p.stop().await;
    }
}

const JOINED: &str = "[12:00:00] [Server thread/INFO]: Steve joined the game";
const LEFT: &str = "[12:05:00] [Server thread/INFO]: Steve left the game";

fn timings(checkpoint_ms: u64, idle_stop_ms: u64) -> Timings {
    Timings { checkpoint: Duration::from_millis(checkpoint_ms), idle_stop: Duration::from_millis(idle_stop_ms), idle_poll: Duration::from_millis(100) }
}

fn on_line_of(a: &Peer) -> core_lib::hearth::server::LineSink {
    a.runner.specs.lock().unwrap()[0].on_line.clone()
}

fn world_file(a: &Peer, name: &str, contents: &str) {
    std::fs::write(a.runner.specs.lock().unwrap()[0].dir.join("world").join(name), contents).unwrap();
}

/// Serveur sans joueur : aucun point de reprise n'est publié (ticket #94), mais un appel direct reste possible.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn serveur_vide_ne_publie_aucun_point_de_reprise() {
    let (peers, g, id) = group_of(&["Alice", "Bob"]).await;
    let a = &peers[0];
    a.rt().set_timings(timings(150, 3_600_000));
    a.node.st.hearth_play(&id).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(a.rt().head(&g).is_none(), "aucun joueur : aucune version");
    assert!(!a.runner.calls.lock().unwrap().contains(&"save-off"), "le serveur n'est même pas mis en pause");
    // Un appel direct force le point (le monde est nouveau : une version).
    let host = a.rt().active(&g).unwrap();
    let v = a.rt().checkpoint(&host).await.unwrap().expect("monde nouveau : une version");
    assert_eq!(v.header.kind, VersionKind::Checkpoint);
    for p in &peers {
        p.stop().await;
    }
}

/// Un joueur entré puis sorti entre deux points compte ; un monde identique à la tête ne produit pas de version, et
/// `save-on` est quand même envoyé ; sans nouveau joueur, plus de point.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn point_de_reprise_seulement_si_joueur_et_monde_change() {
    let (peers, g, id) = group_of(&["Alice", "Bob"]).await;
    let a = &peers[0];
    a.rt().set_timings(timings(1500, 3_600_000));
    a.node.st.hearth_play(&id).await.unwrap();
    // Entré puis sorti avant le premier point : il compte.
    let on_line = on_line_of(a);
    on_line(JOINED);
    world_file(a, "region-1.mca", "un");
    on_line(LEFT);
    wait_for("premier point de reprise", 30, || a.rt().head(&g).is_some()).await;
    let v1 = a.rt().head(&g).unwrap();
    assert_eq!(v1.header.kind, VersionKind::Checkpoint);
    assert_eq!(v1.header.number, 1);
    // Personne depuis : plusieurs tours sans rien publier, même si le monde change.
    world_file(a, "region-2.mca", "deux");
    tokio::time::sleep(Duration::from_millis(4000)).await;
    assert_eq!(a.rt().head(&g).unwrap().id(), v1.id(), "aucun joueur depuis le dernier point : rien");
    // Un joueur revient mais le monde ne bouge pas par rapport à la tête : rien non plus, et save-on est bien envoyé.
    let host = a.rt().active(&g).unwrap();
    a.rt().checkpoint(&host).await.unwrap().expect("region-2 est nouveau");
    let v2 = a.rt().head(&g).unwrap();
    assert_eq!(v2.header.number, 2);
    let (offs, ons) = {
        let c = a.runner.calls.lock().unwrap();
        (c.iter().filter(|c| **c == "save-off").count(), c.iter().filter(|c| **c == "save-on").count())
    };
    assert_eq!(offs, ons, "chaque save-off est suivi d'un save-on");
    on_line(JOINED);
    tokio::time::sleep(Duration::from_millis(4000)).await;
    assert_eq!(a.rt().head(&g).unwrap().id(), v2.id(), "monde identique à la tête : aucune version");
    let ons_after = a.runner.calls.lock().unwrap().iter().filter(|c| **c == "save-on").count();
    assert!(ons_after > ons, "le point a bien eu lieu (save-on), sans publier");
    assert!(a.runner.calls.lock().unwrap().iter().filter(|c| **c == "save-off").count() == ons_after);
    // Joueur resté connecté et monde changé : un point de plus.
    world_file(a, "region-3.mca", "trois");
    wait_for("point de reprise avec joueur", 30, || a.rt().head(&g).is_some_and(|h| h.header.number == 3)).await;
    for p in &peers {
        p.stop().await;
    }
}

/// Serveur vide trop longtemps (même avec « garder ouvert ») : il s'arrête, la session est publiée, le partage fermé et le
/// bail relâché. Un joueur connecté empêche l'arrêt.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn serveur_vide_s_arrete_apres_l_inactivite() {
    let (peers, g, id) = group_of(&["Alice", "Bob"]).await;
    let (a, b) = (&peers[0], &peers[1]);
    a.rt().set_timings(timings(3_600_000, 2000));
    a.rt().set_keep_background(&g, true);
    a.node.st.hearth_play(&id).await.unwrap();
    assert_eq!(a.state_of(&g), HearthStateKind::Hosting);
    on_line_of(a)(JOINED);
    world_file(a, "region-jouee.mca", "jouée");
    tokio::time::sleep(Duration::from_millis(3500)).await;
    assert!(a.rt().active(&g).is_some(), "un joueur est connecté : le serveur reste");
    on_line_of(a)(LEFT);
    wait_for("serveur arrêté après inactivité", 30, || a.rt().active(&g).is_none()).await;
    wait_for("partage fermé", 30, || a.node.be.shared_instance().is_none()).await;
    assert_ne!(a.state_of(&g), HearthStateKind::Hosting);
    let calls = a.runner.calls.lock().unwrap().clone();
    assert_eq!(calls.iter().filter(|c| **c == "stop").count(), 1, "un seul arrêt : {calls:?}");
    let s = a.rt().head(&g).expect("la session est publiée");
    assert_eq!(s.header.kind, VersionKind::Session);
    assert!(!a.rt().has_session_marker(&g), "la session publiée efface le marqueur");
    wait_for("bail relâché chez Bob", 20, || b.rt().lease_of_holder(&g, &a.key()).is_none()).await;
    for p in &peers {
        p.stop().await;
    }
}

/// Personne n'est jamais entré : arrêt sans rien publier, marqueur effacé, partage fermé.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn serveur_jamais_rejoint_s_arrete_sans_version() {
    let (peers, g, id) = group_of(&["Alice", "Bob"]).await;
    let (a, b) = (&peers[0], &peers[1]);
    a.rt().set_timings(timings(3_600_000, 1500));
    a.node.st.hearth_play(&id).await.unwrap();
    assert!(a.rt().has_session_marker(&g));
    wait_for("serveur arrêté", 30, || a.rt().active(&g).is_none()).await;
    wait_for("partage fermé", 30, || a.node.be.shared_instance().is_none()).await;
    assert_eq!(a.runner.calls.lock().unwrap().clone(), ["start", "stop"], "rien à publier : pas de save-all");
    assert!(a.rt().head(&g).is_none(), "aucune version publiée");
    assert!(!a.rt().has_session_marker(&g));
    assert_ne!(a.state_of(&g), HearthStateKind::Hosting);
    wait_for("bail relâché chez Bob", 20, || b.rt().lease_of_holder(&g, &a.key()).is_none()).await;
    for p in &peers {
        p.stop().await;
    }
}

/// Alice et Bob, la n° 1 complète chez les deux, Alice héberge (Bob en ligne). `log` : ce que le serveur d'Alice écrit.
async fn alice_heberge_avec_bob(log: &[&str]) -> (Vec<Peer>, GroupId, String, p2p_lib::hearth::version::SignedVersion) {
    let (peers, g, id) = group_of(&["Alice", "Bob"]).await;
    let (a, b) = (&peers[0], &peers[1]);
    let d1 = a.dir.path().join("monde-1");
    small_world(&d1, "monde n° 1");
    let v1 = a.rt().publish_version(g, &d1, VersionKind::Session, None, 60).await.unwrap();
    wait_for("n° 1 complète chez Bob", 40, || b.rt().head(&g).is_some_and(|h| h.id() == v1.id() && b.rt().world_complete(&h))).await;
    *a.runner.log.lock().unwrap() = log.iter().map(|l| (*l).to_string()).collect();
    a.rt().probe_all().await;
    a.node.st.hearth_play(&id).await.unwrap();
    assert_eq!(a.state_of(&g), HearthStateKind::Hosting);
    wait_for("Bob voit la partie d'Alice", 20, || b.state_of(&g) == HearthStateKind::Hosted).await;
    (peers, g, id, v1)
}

/// Le serveur meurt seul avec un code d'erreur : rien n'est publié (la tête et la file d'envoi ne bougent pas), le monde est
/// mis de côté chez l'hôte, le bail est relâché avec `Crashed` et Bob voit le mod probable (ticket n° 98). L'hôte peut
/// ensuite publier cette partie comme une branche, qui ne devient pas la tête toute seule.
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn plantage_du_serveur_ne_publie_rien_et_previent_les_autres() {
    let (peers, g, id, v1) = alice_heberge_avec_bob(&[
        "[12:00:00] [main/INFO]: Loading Minecraft 1.21.1 with Fabric Loader 0.16.0",
        "[12:03:10] [Server thread/ERROR]: Mod 'Sodium' (sodium) 0.5.8 a planté pendant le tick",
    ])
    .await;
    let (a, b) = (&peers[0], &peers[1]);
    std::fs::write(a.rt().server_dir(&g).join("world/region-jouee.mca"), SENTINELLE).unwrap();
    let (heads_before, outbox_before) = (a.rt().heads_of(&g).unwrap().len(), a.rt().pending_outbox());

    a.runner.die(1);
    // L'état quitte `Hosting` dès le début de la fin de partie : on attend que le monde planté soit mis de côté.
    wait_for("monde planté mis de côté", 30, || a.rt().card(&g).unwrap().orphans == 1).await;
    wait_for("plantage retenu", 30, || a.rt().card(&g).unwrap().crash.is_some()).await;
    assert_eq!(a.rt().head(&g).unwrap().id(), v1.id(), "la tête n'a pas bougé");
    assert_eq!((a.rt().heads_of(&g).unwrap().len(), a.rt().pending_outbox()), (heads_before, outbox_before), "rien n'est publié");
    assert!(a.rt().active(&g).is_none());

    // Le monde planté est gardé de côté chez l'hôte, intact.
    assert!(!a.rt().has_session_marker(&g));
    assert_eq!(orphan_files(a, &g).len(), 1);
    assert!(orphan_files(a, &g)[0].iter().any(|(n, c)| n == "region-jouee.mca" && c == SENTINELLE));
    let ca = a.rt().card(&g).unwrap();
    assert_eq!((ca.state, ca.orphans), (HearthStateKind::Idle, 1));
    assert_eq!(ca.crash.as_ref().unwrap().mod_hint.as_deref(), Some("Sodium (sodium)"));

    // Bob : le bail est libéré, le plantage est annoncé avec le mod probable.
    wait_for("plantage chez Bob", 20, || b.rt().card(&g).unwrap().crash.is_some()).await;
    let cb = b.rt().card(&g).unwrap();
    let crash = cb.crash.unwrap();
    assert_eq!(cb.state, HearthStateKind::Idle);
    assert_eq!((crash.host.public_key, crash.mod_hint.as_deref()), (a.node.key(), Some("Sodium (sodium)")));
    assert_eq!(b.rt().head(&g).unwrap().id(), v1.id());

    // Le monde mis de côté ne se publie que sur le choix de l'hôte : une Session enfant de la n° 1 (la tête n'a pas avancé
    // entre-temps, donc elle la prolonge ; si le groupe avait avancé, ce serait une fourche, voir `partie_mal_terminee_*`).
    let oid = a.node.st.hearth.detail(&g).unwrap().orphans[0].id.clone();
    a.node.st.hearth_orphan_publish(&id, &oid).await.unwrap();
    assert_eq!(orphan_files(a, &g).len(), 0);
    let head = a.rt().head(&g).unwrap();
    assert_eq!((head.header.kind, head.header.parent, head.header.number), (VersionKind::Session, Some(v1.id()), 2));
    for p in &peers {
        p.stop().await;
    }
}

/// Un plantage sans mod identifiable dit « cause inconnue » ; le prochain serveur efface l'annonce, et une fin propre
/// ensuite publie comme avant.
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn plantage_sans_mod_connu_puis_nouvelle_partie_publiee() {
    let (peers, g, id, v1) = alice_heberge_avec_bob(&["[12:00:00] [Server thread/ERROR]: Exception in server tick loop"]).await;
    let (a, b) = (&peers[0], &peers[1]);
    a.runner.die(-1);
    wait_for("serveur d'Alice arrêté", 30, || a.state_of(&g) != HearthStateKind::Hosting).await;
    wait_for("plantage chez Bob", 20, || b.rt().card(&g).unwrap().crash.is_some()).await;
    assert_eq!(b.rt().card(&g).unwrap().crash.unwrap().mod_hint, None);
    assert_eq!(a.rt().head(&g).unwrap().id(), v1.id());

    // Nouvelle partie chez Alice : l'annonce disparaît, la fin propre publie une Session.
    a.rt().probe_all().await;
    a.node.st.hearth_play(&id).await.unwrap();
    assert_eq!(a.state_of(&g), HearthStateKind::Hosting);
    assert!(a.rt().card(&g).unwrap().crash.is_none());
    wait_for("Bob voit la partie", 20, || b.state_of(&g) == HearthStateKind::Hosted).await;
    assert!(b.rt().card(&g).unwrap().crash.is_none());
    a.close_game();
    wait_for("serveur d'Alice arrêté", 30, || a.state_of(&g) != HearthStateKind::Hosting).await;
    wait_for("monde publié", 30, || !a.rt().has_session_marker(&g)).await;
    assert!(a.rt().card(&g).unwrap().crash.is_none());
    let head = a.rt().head(&g).unwrap();
    assert_eq!((head.header.kind, head.header.parent), (VersionKind::Session, Some(v1.id())));
    for p in &peers {
        p.stop().await;
    }
}

/// Code 0 sans arrêt demandé (un `/stop` tapé dans la console du serveur) : fin propre, le monde est publié et le bail
/// relâché avec `Stopped`, comme avant.
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn serveur_arrete_proprement_de_lui_meme_publie_la_session() {
    let (peers, g, _id, v1) = alice_heberge_avec_bob(&[]).await;
    let (a, b) = (&peers[0], &peers[1]);
    a.runner.die(0);
    wait_for("serveur d'Alice arrêté", 30, || a.state_of(&g) != HearthStateKind::Hosting).await;
    wait_for("monde publié", 30, || !a.rt().has_session_marker(&g)).await;
    let head = a.rt().head(&g).unwrap();
    assert_eq!((head.header.kind, head.header.parent), (VersionKind::Session, Some(v1.id())));
    assert_eq!(a.rt().card(&g).unwrap().orphans, 0);
    wait_for("version 2 chez Bob", 40, || b.rt().head(&g).is_some_and(|h| h.header.number == 2 && b.rt().world_complete(&h))).await;
    assert!(b.rt().card(&g).unwrap().crash.is_none());
    for p in &peers {
        p.stop().await;
    }
}

// ---------------------------------------------------------------------------------------------------------
// #96 : un membre qui ne fait que rejoindre libère aussi les vieilles versions
// ---------------------------------------------------------------------------------------------------------

/// Un monde d'environ `len` octets, distinct selon `seed`.
fn big_world(dir: &Path, seed: u8, len: usize) {
    std::fs::create_dir_all(dir).unwrap();
    let data: Vec<u8> = (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed).wrapping_add((i >> 8) as u8)).collect();
    std::fs::write(dir.join("region.mca"), data).unwrap();
}

/// Alice publie quatre versions, Bob (qui ne fait que rejoindre) les reçoit toutes. Sa synchro libère toute seule les
/// vieilles (date de réception ici, pas `created_at`), la page du serveur mesure les blobs, « Restaurer » n'est proposé
/// que pour les mondes présents.
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn un_membre_qui_ne_fait_que_rejoindre_libere_les_vieilles_versions() {
    let (peers, g, _id) = group_of(&["Alice", "Bob"]).await;
    let (a, b) = (&peers[0], &peers[1]);
    let mut vs = Vec::new();
    for n in 1..=3u8 {
        let d = a.dir.path().join(format!("monde-{n}"));
        big_world(&d, n, 60_000);
        let parent = vs.last().map(|v: &p2p_lib::hearth::version::SignedVersion| (v.id(), v.header.number));
        let v = a.rt().publish_version(g, &d, VersionKind::Session, parent, 60).await.unwrap();
        wait_for("monde complet chez Bob", 40, || b.rt().head(&g).is_some_and(|h| h.id() == v.id() && b.rt().world_complete(&h))).await;
        vs.push(v);
    }
    let (v1, v2, v3) = (&vs[0], &vs[1], &vs[2]);
    let detail = || b.node.st.hearth.detail(&g).unwrap();
    let d = detail();
    assert!(d.versions.iter().all(|v| v.available), "Bob a les trois mondes");
    // La place mesurée, ce sont les blobs (3 x 60 Ko), pas les quelques Ko d'en-têtes du dossier du groupe.
    assert!(d.settings.disk_bytes >= 180_000, "les blobs comptent : {}", d.settings.disk_bytes);
    let full = d.settings.disk_bytes;

    // Récent : rien n'est libéré, même 13 jours plus tard.
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    b.rt().gc_versions_at(g, now + 13 * 86_400).await;
    assert!(vs.iter().all(|v| b.rt().world_complete(v)), "moins de 14 jours depuis la réception : tout reste");

    // La n° 1 est arrivée chez Bob il y a 20 jours : la prochaine synchro (après la n° 4 d'Alice) la libère toute seule.
    let file = b.rt().server_dir(&g).parent().unwrap().join("versions").join(format!("{}.json", v1.id()));
    let old = std::time::SystemTime::now() - Duration::from_secs(20 * 86_400);
    std::fs::File::options().write(true).open(&file).unwrap().set_modified(old).unwrap();
    let d4 = a.dir.path().join("monde-4");
    big_world(&d4, 4, 60_000);
    let v4 = a.rt().publish_version(g, &d4, VersionKind::Session, Some((v3.id(), 3)), 60).await.unwrap();
    wait_for("n° 4 chez Bob", 40, || b.rt().head(&g).is_some_and(|h| h.id() == v4.id() && b.rt().world_complete(&h))).await;
    wait_for("n° 1 libérée par la synchro", 40, || !b.rt().world_complete(v1)).await;
    assert!(b.rt().world_complete(v2) && b.rt().world_complete(v3) && b.rt().world_complete(&v4), "récentes, parent et tête restent");
    let d = detail();
    let avail = |n: u64| d.versions.iter().find(|v| v.number == n).unwrap().available;
    assert_eq!((avail(1), avail(2), avail(3), avail(4)), (false, true, true, true));
    assert!(d.versions.len() == 4, "l'historique garde la ligne de la n° 1");

    // 15 jours plus tard : seules la tête (n° 4) et son parent (n° 3) restent ; la place mesurée baisse.
    b.rt().gc_versions_at(g, now + 15 * 86_400).await;
    assert!(!b.rt().world_complete(v2) && b.rt().world_complete(v3) && b.rt().world_complete(&v4));
    let d = detail();
    assert!(d.versions.iter().filter(|v| v.available).count() == 2);
    assert!(d.settings.disk_bytes < full, "n° 1 et n° 2 libérées : {} contre {full}", d.settings.disk_bytes);
    assert!(d.settings.disk_bytes >= 120_000, "les blobs des deux versions gardées comptent encore");

    for p in &peers {
        p.stop().await;
    }
}

/// Bob était éteint pendant les n° 2 et n° 3 : à son retour il reprend la tête (n° 3) mais pas le monde de la n° 2, dont il
/// n'a que l'en-tête. Alice éteinte, restaurer la n° 2 dit en clair que le contenu n'est plus sur ce PC et que personne ne
/// l'a en ligne, pas l'erreur de transfert ; la page du serveur ne propose pas « Restaurer » (`available` faux) (#96).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restaurer_un_monde_absent_sans_personne_en_ligne_le_dit_en_clair() {
    let (mut peers, g, id) = group_of(&["Alice", "Bob"]).await;
    let d1 = peers[0].dir.path().join("monde-1");
    big_world(&d1, 1, 10_000);
    let v1 = peers[0].rt().publish_version(g, &d1, VersionKind::Session, None, 60).await.unwrap();
    wait_for("n° 1 complète chez Bob", 40, || peers[1].rt().head(&g).is_some_and(|h| h.id() == v1.id() && peers[1].rt().world_complete(&h))).await;
    peers[1].stop().await;
    let mut parent = (v1.id(), 1);
    let mut vs = Vec::new();
    for n in 2..=3u8 {
        let d = peers[0].dir.path().join(format!("monde-{n}"));
        big_world(&d, n, 10_000);
        let v = peers[0].rt().publish_version(g, &d, VersionKind::Session, Some(parent), 60).await.unwrap();
        parent = (v.id(), u64::from(n));
        vs.push(v);
    }
    let v3 = vs.pop().unwrap();
    let v2 = vs.pop().unwrap();
    peers[1].restart().await;
    peers[1].rt().probe_all().await;
    let b = &peers[1];
    wait_for("n° 3 complète chez Bob", 60, || b.rt().head(&g).is_some_and(|h| h.id() == v3.id() && b.rt().world_complete(&h))).await;
    assert!(!b.rt().world_complete(&v2), "seule la tête est rapatriée : Bob n'a que l'en-tête de la n° 2");
    peers[0].stop().await;
    b.rt().probe_all().await;
    let d = b.node.st.hearth.detail(&g).unwrap();
    assert!(d.versions.iter().find(|v| v.number == 2).is_some_and(|v| !v.available), "la n° 2 n'a pas de contenu ici");
    assert!(d.versions.iter().find(|v| v.number == 1).is_some_and(|v| v.available));
    let err = b.node.st.hearth_restore(&id, &v2.id().to_hex()).await.unwrap_err();
    assert!(err.contains("n'est plus sur ce PC"), "{err}");
    b.stop().await;
}
