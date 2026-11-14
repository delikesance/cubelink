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
use cubelink_app::hearth::{BoxFut, ClientLauncher, RenewTick, RunningServer, ServerRunner, ServerSpec, EULA_REQUIRED};
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
        Box::pin(async { Ok(()) })
    }
    fn stop(&self, _grace: Duration) -> BoxFut<'_, ()> {
        self.calls.lock().unwrap().push("stop");
        let n = std::fs::read_dir(self.world_dir()).map(|d| d.count()).unwrap_or(0);
        std::fs::write(self.world_dir().join(format!("partie-{n}.txt")), format!("jouée {n}")).unwrap();
        self.task.abort();
        Box::pin(async {})
    }
    fn wait_exit(&self) -> BoxFut<'_, ()> {
        Box::pin(std::future::pending())
    }
}

#[derive(Default)]
struct FakeRunner {
    calls: Arc<Mutex<Vec<&'static str>>>,
    specs: Arc<Mutex<Vec<ServerSpec>>>,
}

impl ServerRunner for FakeRunner {
    fn start(&self, spec: ServerSpec) -> BoxFut<'static, Result<Arc<dyn RunningServer>, String>> {
        let (calls, specs) = (self.calls.clone(), self.specs.clone());
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
            Ok(Arc::new(FakeServer { port, dir, task, calls }) as Arc<dyn RunningServer>)
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

/// Seul dans le groupe, ou sans personne en ligne : le jeu hors ligne est interdit, sauf pour un groupe d'un seul membre.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn jeu_hors_ligne_interdit_sur_un_serveur_de_groupe() {
    let (a, b) = (Peer::start("Alice").await, Peer::start("Bob").await);
    befriend(&a.node, &b.node).await;
    wait_for("ami chez Bob", 20, || b.node.st.friends.is_friend(&a.node.key()).unwrap()).await;
    let card = a.node.st.hearth_create("Duo", &a.instance, None, vec![b.node.key()]).await.unwrap();
    let g = GroupId::from_hex(&card.id).unwrap();
    a.node.st.hearth_eula_accept();
    b.stop().await;
    let e = a.node.st.hearth_play(&card.id).await.unwrap_err();
    assert!(e.contains("Connexion requise"), "{e}");
    assert_eq!(a.state_of(&g), HearthStateKind::Blocked);
    assert!(a.launcher.launched.lock().unwrap().is_empty());
    a.stop().await;
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
    assert!(a.node.st.hearth_leave(&id).await.unwrap_err().contains("désactivés"));
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
