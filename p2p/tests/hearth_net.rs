//! Hearth, réseau et stockage pour de vrai : trois endpoints iroh en loopback (relais désactivé), un vrai dossier de
//! monde de plusieurs Mo, de vrais fichiers de groupe sur le disque.
//!
//! Scénario : non-membre refusé -> création de groupe -> invitation -> rattrapage des opérations -> publication d'un
//! monde -> rapatriement et matérialisation (octets identiques) -> deuxième version où un seul fichier change et
//! seul ce blob voyage -> bail annoncé, expiré, relâché -> file d'envoi (entrée gardée, puis livrée) -> invitation
//! d'une longue chaîne (préfixe + rattrapage) -> rapatriement avec bascule de pair.
//!
//! `cargo test -p cubelink-p2p --test hearth_net`

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use core_lib::types::{Loader, LoaderSpec};
use iroh::endpoint::presets;
use iroh::protocol::Router;
use iroh::{Endpoint, EndpointAddr, RelayMode, SecretKey};
use p2p_lib::hearth::lease::{Claim, ClaimReply, ConflictKind, Lease, LeaseId, LeaseTable, ReleaseReason};
use p2p_lib::hearth::model::{GroupId, GroupOp, GroupState, HearthSigner, OpHash, PackPin, Proposal, Sig, SignedOp};
use p2p_lib::hearth::outbox::{MemberReplica, Outbox, Replica};
use p2p_lib::hearth::proto::{HearthConn, HearthDelegate, HearthHandler, NetError, HEARTH_ALPN, HEARTH_BLOBS_ALPN};
use p2p_lib::hearth::store::{Group, GroupKey, GroupStore};
use p2p_lib::hearth::sync::{FetchOptions, GroupMembers, SyncError, WorldStore};
use p2p_lib::hearth::version::{SignedVersion, VersionHeader, VersionId, VersionKind};
use p2p_lib::identity::Identity;
use p2p_lib::presence::{FriendLookup, PeerKey};

// ---------------------------------------------------------------------------------------------------------
// Un nœud : identité, groupes sur disque, protocole et blobs
// ---------------------------------------------------------------------------------------------------------

struct Delegate {
    store: GroupStore,
    groups: Mutex<HashMap<GroupId, Group>>,
    friends: Mutex<Vec<PeerKey>>,
    leases: Mutex<LeaseTable>,
    invites: Mutex<Vec<(PeerKey, GroupId)>>,
    complete: Arc<Mutex<HashSet<VersionId>>>,
    addrs: Mutex<HashMap<PeerKey, EndpointAddr>>,
    refuse_offers: AtomicBool,
    world: OnceLock<WorldStore>,
    endpoint: OnceLock<Endpoint>,
}

impl GroupMembers for Delegate {
    fn is_member(&self, group: &GroupId, key: &PeerKey) -> bool {
        self.groups.lock().unwrap().get(group).is_some_and(|g| g.state().is_member(key))
    }
}

impl FriendLookup for Delegate {
    fn friend_keys(&self) -> Vec<PeerKey> {
        self.friends.lock().unwrap().clone()
    }
    fn is_friend(&self, key: &PeerKey) -> bool {
        self.friends.lock().unwrap().contains(key)
    }
}

impl HearthDelegate for Delegate {
    fn is_friend(&self, key: &PeerKey) -> bool {
        FriendLookup::is_friend(self, key)
    }
    fn group_state(&self, group: &GroupId) -> Option<GroupState> {
        self.groups.lock().unwrap().get(group).map(|g| g.state().clone())
    }
    fn ops_after(&self, group: &GroupId, after: u64, limit: usize) -> Vec<SignedOp> {
        self.groups.lock().unwrap().get(group).map(|g| g.ops_after(after, limit)).unwrap_or_default()
    }
    fn versions_after(&self, group: &GroupId, after_number: u64, limit: usize) -> Vec<SignedVersion> {
        self.groups.lock().unwrap().get(group).map(|g| g.versions_after(after_number, limit)).unwrap_or_default()
    }
    fn heads(&self, group: &GroupId) -> Vec<VersionId> {
        self.groups.lock().unwrap().get(group).map(Group::heads).unwrap_or_default()
    }
    fn lease(&self, group: &GroupId) -> Option<Lease> {
        // Un bail signé ne se modifie pas : il est relayé tel quel.
        self.leases.lock().unwrap().holder(group, Instant::now())
    }
    fn have_version(&self, _group: &GroupId, version: &VersionId) -> bool {
        self.complete.lock().unwrap().contains(version)
    }
    fn on_lease(&self, from: PeerKey, lease: Lease) {
        let groups = self.groups.lock().unwrap();
        let g = groups.get(&lease.group_id).expect("groupe connu : le protocole l'a vérifié");
        self.leases.lock().unwrap().observe(g.state(), g.graph(), &from, lease, Instant::now()).expect("bail valide");
    }
    fn on_claim(&self, from: PeerKey, claim: Claim) -> Result<ClaimReply, String> {
        let groups = self.groups.lock().unwrap();
        let g = groups.get(&claim.group_id).expect("groupe connu : le protocole l'a vérifié");
        self.leases.lock().unwrap().claim(g.state(), g.graph(), &from, &claim, Instant::now()).map_err(|e| e.to_string())
    }
    fn on_lease_release(
        &self,
        from: PeerKey,
        group: &GroupId,
        lease_id: LeaseId,
        last_seq: u64,
        _reason: ReleaseReason,
        _version: Option<VersionId>,
    ) {
        self.leases.lock().unwrap().release_lease(group, &from, &lease_id, last_seq, Instant::now());
    }
    fn on_invite(&self, from: PeerKey, ops: Vec<SignedOp>, key: GroupKey) -> Result<(), String> {
        let mut groups = self.groups.lock().unwrap();
        let id = match &ops[0].op {
            GroupOp::Create { id, .. } => *id,
            _ => return Err("pas une création".into()),
        };
        if let Some(g) = groups.get_mut(&id) {
            g.append_ops(&ops).map_err(|e| e.to_string())?;
            g.set_key(key).map_err(|e| e.to_string())?;
        } else {
            let g = self.store.import(&ops, key).map_err(|e| e.to_string())?;
            groups.insert(id, g);
        }
        self.invites.lock().unwrap().push((from, id));
        Ok(())
    }
    fn on_proposal(&self, from: PeerKey, group: &GroupId, proposal: Proposal) -> Result<(), String> {
        let mut groups = self.groups.lock().unwrap();
        groups.get_mut(group).ok_or("groupe inconnu")?.put_proposal(proposal, &from).map(|_| ()).map_err(|e| e.to_string())
    }
    fn on_vote(&self, from: PeerKey, group: &GroupId, proposal_id: OpHash, sig: Sig) -> Result<(), String> {
        let mut groups = self.groups.lock().unwrap();
        groups.get_mut(group).ok_or("groupe inconnu")?.add_vote(&proposal_id, from, sig).map(|_| ()).map_err(|e| e.to_string())
    }
    fn on_version_offer(&self, from: PeerKey, version: SignedVersion) -> Result<(), String> {
        if self.refuse_offers.load(Ordering::SeqCst) {
            return Err("pas maintenant".into());
        }
        let group = version.header.group_id;
        let report = {
            let mut groups = self.groups.lock().unwrap();
            groups.get_mut(&group).ok_or("groupe inconnu")?.add_versions(vec![version.clone()]).map_err(|e| e.to_string())?
        };
        if !report.rejected.is_empty() {
            return Err(report.rejected[0].1.clone());
        }
        let (world, endpoint) = (self.world.get().unwrap().clone(), self.endpoint.get().unwrap().clone());
        let addr = self.addrs.lock().unwrap().get(&from).cloned().ok_or("adresse inconnue")?;
        let id = version.id();
        let me = self.complete.clone();
        tokio::spawn(async move {
            let h = &version.header;
            let r = world.fetch(&endpoint, &h.group_id, &[addr], &h.collection, h.total_size, &FetchOptions::default()).await;
            if r.is_ok() {
                me.lock().unwrap().insert(id);
            }
        });
        Ok(())
    }
}

struct Node {
    _dir: tempfile::TempDir,
    ident: Arc<Identity>,
    key: PeerKey,
    ep: Endpoint,
    delegate: Arc<Delegate>,
    world: WorldStore,
    _router: Router,
}

impl Node {
    fn addr(&self) -> EndpointAddr {
        self.ep.addr()
    }
    fn befriend(&self, other: &Node) {
        self.delegate.friends.lock().unwrap().push(other.key);
        self.delegate.addrs.lock().unwrap().insert(other.key, other.addr());
    }
    fn group_dir(&self, id: &GroupId) -> PathBuf {
        self.delegate.store.group_dir(id)
    }
    fn with_group<R>(&self, id: &GroupId, f: impl FnOnce(&mut Group) -> R) -> R {
        f(self.delegate.groups.lock().unwrap().get_mut(id).expect("groupe connu"))
    }
    fn mark_complete(&self, v: VersionId) {
        self.delegate.complete.lock().unwrap().insert(v);
    }
}

async fn node(serve_blobs: bool) -> Node {
    let dir = tempfile::tempdir().unwrap();
    let ident = Arc::new(Identity::load_or_create(dir.path()).unwrap());
    let mut alpns = vec![HEARTH_ALPN.to_vec()];
    if serve_blobs {
        alpns.push(HEARTH_BLOBS_ALPN.to_vec());
    }
    let ep = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .secret_key(SecretKey::from_bytes(&ident.secret_bytes()))
        .alpns(alpns)
        .bind()
        .await
        .unwrap();
    let delegate = Arc::new(Delegate {
        store: GroupStore::open(dir.path().join("hearth")).unwrap(),
        groups: Mutex::default(),
        friends: Mutex::default(),
        leases: Mutex::default(),
        invites: Mutex::default(),
        complete: Arc::default(),
        addrs: Mutex::default(),
        refuse_offers: AtomicBool::new(false),
        world: OnceLock::new(),
        endpoint: OnceLock::new(),
    });
    let world = WorldStore::open(dir.path().join("hearth-blobs"), delegate.clone()).await.unwrap();
    delegate.world.set(world.clone()).ok().unwrap();
    delegate.endpoint.set(ep.clone()).ok().unwrap();
    let mut rb = Router::builder(ep.clone()).accept(HEARTH_ALPN, HearthHandler::new(ep.clone(), delegate.clone()));
    if serve_blobs {
        rb = rb.accept(HEARTH_BLOBS_ALPN, world.blobs_handler());
    }
    let key = ident.public_key().to_bytes();
    Node { _dir: dir, ident, key, ep, delegate, world, _router: rb.spawn() }
}

// ---------------------------------------------------------------------------------------------------------
// Aides
// ---------------------------------------------------------------------------------------------------------

fn pack() -> PackPin {
    PackPin { manifest_hash: "ab".repeat(32), minecraft: "1.21.1".into(), loader: LoaderSpec { loader: Loader::Fabric, version: "0.16.0".into() } }
}

/// Octets pseudo-aléatoires reproductibles.
fn noise(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

const MIB: usize = 1024 * 1024;

fn write_world(dir: &Path) {
    std::fs::create_dir_all(dir.join("region")).unwrap();
    std::fs::create_dir_all(dir.join("playerdata")).unwrap();
    std::fs::write(dir.join("level.dat"), noise(1, 2_000)).unwrap();
    std::fs::write(dir.join("region/r.0.0.mca"), noise(2, 3 * MIB)).unwrap();
    std::fs::write(dir.join("region/r.0.1.mca"), noise(3, 4 * MIB)).unwrap();
    std::fs::write(dir.join("region/r.1.0.mca"), noise(4, 2 * MIB + 17)).unwrap();
    std::fs::write(dir.join("playerdata/0000-aaaa.dat"), noise(5, 900)).unwrap();
    // Jamais publiés.
    std::fs::write(dir.join("session.lock"), b"verrou").unwrap();
    std::fs::write(dir.join("region/r.0.0.mca.tmp"), noise(6, 1000)).unwrap();
}

/// Contenu d'un dossier (hors verrous et temporaires) : chemin relatif -> BLAKE3.
fn tree(dir: &Path) -> BTreeMap<String, String> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            let p = e.path();
            if p.is_dir() {
                walk(root, &p, out);
            } else {
                let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
                if rel.ends_with("session.lock") || rel.ends_with(".tmp") {
                    continue;
                }
                out.insert(rel, blake3::hash(&std::fs::read(&p).unwrap()).to_hex().to_string());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

fn version(
    signer: &Identity,
    group: GroupId,
    number: u64,
    parent: Option<VersionId>,
    collection: [u8; 32],
    total_size: u64,
    kind: VersionKind,
) -> SignedVersion {
    let header = VersionHeader {
        group_id: group,
        number,
        parent,
        epoch: 0,
        author: signer.peer_key(),
        pack_hash: pack().manifest_hash,
        created_at: 1_800_000_000 + number,
        played_secs: 600,
        kind,
        collection,
        total_size,
    };
    SignedVersion::sign(signer, header).unwrap()
}

/// Rattrape les opérations et les en-têtes de `node` auprès de `addr`.
async fn catch_up(node: &Node, group: &GroupId, addr: &EndpointAddr) {
    let conn = HearthConn::connect(&node.ep, addr.clone()).await.unwrap();
    let (op_seq, heads) = node.with_group(group, |g| (g.state().op_seq, g.heads()));
    let status = conn.hello(group, op_seq, &heads).await.unwrap();
    if status.op_seq > op_seq {
        let ops = conn.fetch_ops(group, op_seq).await.unwrap();
        node.with_group(group, |g| g.append_ops(&ops)).unwrap();
    }
    let versions = conn.fetch_versions(group, 0).await.unwrap();
    let report = node.with_group(group, |g| g.add_versions(versions)).unwrap();
    assert!(report.rejected.is_empty(), "{report:?}");
}

async fn eventually(what: &str, mut ok: impl FnMut() -> bool) {
    for _ in 0..200 {
        if ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("délai dépassé : {what}");
}

// ---------------------------------------------------------------------------------------------------------
// Le scénario
// ---------------------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hearth_over_real_endpoints() {
    tokio::time::timeout(Duration::from_secs(180), scenario()).await.expect("le scénario ne doit pas dépasser 3 minutes");
}

async fn scenario() {
    let (a, b, c) = (node(true).await, node(true).await, node(true).await);
    // E ne sert pas les blobs : sert à tester la bascule de pair.
    let e = node(false).await;
    a.befriend(&b);
    b.befriend(&a);
    a.befriend(&c);
    c.befriend(&a);
    c.befriend(&b);
    b.befriend(&c);
    let wait = FetchOptions::default();

    // 1) Création de A : groupe G1 = {A, B}. C n'en est pas membre.
    let g1 = GroupId::random();
    let key1 = GroupKey::random();
    let create = SignedOp::create(&*a.ident, g1, "Les copains", pack(), &BTreeSet::from([b.key])).unwrap();
    let group = a.delegate.store.create(&create, key1.clone()).unwrap();
    a.delegate.groups.lock().unwrap().insert(g1, group);
    assert!(a.group_dir(&g1).join("ops.jsonl").is_file());

    // 2) Un non-membre est refusé sans un octet de réponse, partout.
    {
        let conn = HearthConn::connect(&c.ep, a.addr()).await.unwrap();
        assert_eq!(conn.hello(&g1, 0, &[]).await.unwrap_err(), NetError::NoReply);
    }
    {
        let conn = HearthConn::connect(&c.ep, a.addr()).await.unwrap();
        assert_eq!(conn.get_ops(&g1, 0).await.unwrap_err(), NetError::NoReply);
    }
    {
        // Un groupe inconnu se comporte exactement comme un groupe où l'on n'est pas membre.
        let conn = HearthConn::connect(&c.ep, a.addr()).await.unwrap();
        assert_eq!(conn.hello(&GroupId::random(), 0, &[]).await.unwrap_err(), NetError::NoReply);
    }
    {
        let conn = HearthConn::connect(&c.ep, a.addr()).await.unwrap();
        let lease = Lease::new(g1, c.key, None);
        assert_eq!(conn.send_lease(&lease).await.unwrap_err(), NetError::NoReply);
    }
    // Un ami qui n'est pas ami de A ne peut pas inviter A.
    {
        let stranger = node(false).await;
        let conn = HearthConn::connect(&stranger.ep, a.addr()).await.unwrap();
        let g = GroupId::random();
        let op = SignedOp::create(&*stranger.ident, g, "Intrus", pack(), &BTreeSet::from([a.key])).unwrap();
        assert_eq!(conn.send_invite(&[op], &GroupKey::random()).await.unwrap_err(), NetError::NoReply);
        assert!(a.delegate.invites.lock().unwrap().is_empty());
    }

    // 3) Invitation de B par A (A et B sont amis) : la chaîne est rejouée, la clé de groupe arrive.
    {
        let conn = HearthConn::connect(&a.ep, b.addr()).await.unwrap();
        let ops = a.with_group(&g1, |g| g.ops().to_vec());
        conn.send_invite(&ops, &key1).await.unwrap();
    }
    assert_eq!(b.delegate.invites.lock().unwrap().as_slice(), &[(a.key, g1)]);
    assert_eq!(b.with_group(&g1, |g| g.state().clone()), a.with_group(&g1, |g| g.state().clone()));
    assert_eq!(b.with_group(&g1, |g| g.key().clone()), key1);
    // Relecture du disque de B : tout se revalide.
    let reopened = b.delegate.store.open_group(&g1).unwrap();
    assert_eq!(reopened.state().op_seq, 1);

    // 4) Rattrapage : A fait évoluer le groupe, B rattrape.
    {
        let mut newer = pack();
        newer.minecraft = "1.21.4".into();
        let op = a.with_group(&g1, |g| g.state().sign_single(GroupOp::PackUpdate { pack: newer }, &*a.ident)).unwrap();
        a.with_group(&g1, |g| g.append_op(&op)).unwrap();
        assert_eq!(b.with_group(&g1, |g| g.state().op_seq), 1);
        catch_up(&b, &g1, &a.addr()).await;
        assert_eq!(b.with_group(&g1, |g| g.state().clone()), a.with_group(&g1, |g| g.state().clone()));
    }

    // 5) Publication d'un vrai monde chez A, puis rapatriement par B.
    let world_a = tempfile::tempdir().unwrap();
    write_world(world_a.path());
    // Les fichiers « calmes » (plus de 2 s) seront mémorisés : la republication ne les relira pas.
    tokio::time::sleep(Duration::from_millis(2300)).await;
    let p1 = a.world.publish_dir(&g1, world_a.path()).await.unwrap();
    assert_eq!(p1.files, 5, "session.lock et *.tmp exclus");
    assert_eq!(p1.imported, 5);
    let expected_total = (2_000 + 3 * MIB + 4 * MIB + 2 * MIB + 17 + 900) as u64;
    assert_eq!(p1.total_size, expected_total);
    let v1 = version(&a.ident, g1, 1, None, p1.collection, p1.total_size, VersionKind::Session);
    a.with_group(&g1, |g| g.add_version(v1.clone())).unwrap();
    a.mark_complete(v1.id());

    catch_up(&b, &g1, &a.addr()).await;
    assert_eq!(b.with_group(&g1, |g| g.heads()), vec![v1.id()]);
    let r1 = b.world.fetch(&b.ep, &g1, &[a.addr()], &p1.collection, p1.total_size, &wait).await.unwrap();
    assert_eq!((r1.files, r1.missing_files), (5, 5));
    assert!(r1.downloaded_bytes >= expected_total, "tout a voyagé : {} octets", r1.downloaded_bytes);
    assert_eq!(r1.total_size, expected_total);
    b.mark_complete(v1.id());
    assert!(b.world.is_complete(&p1.collection).await);

    let dest_b = b._dir.path().join("hearth-world").join("world");
    assert_eq!(b.world.materialize(&p1.collection, &dest_b).await.unwrap(), 5);
    assert_eq!(tree(&dest_b), tree(world_a.path()), "octets identiques après matérialisation");
    assert_eq!(tree(&dest_b).len(), 5);
    assert!(!dest_b.join("session.lock").exists());

    // 6) Deuxième version : un seul fichier change, seul ce blob voyage.
    let changed = world_a.path().join("region/r.0.1.mca");
    let mut bytes = std::fs::read(&changed).unwrap();
    for b in bytes.iter_mut().skip(1_000_000).take(64) {
        *b ^= 0xFF;
    }
    std::fs::write(&changed, &bytes).unwrap();
    let p2 = a.world.publish_dir(&g1, world_a.path()).await.unwrap();
    assert_ne!(p2.collection, p1.collection);
    assert_eq!(p2.imported, 1, "les quatre autres fichiers n'ont pas été relus");
    assert_eq!(p2.total_size, p1.total_size);
    let v2 = version(&a.ident, g1, 2, Some(v1.id()), p2.collection, p2.total_size, VersionKind::Checkpoint);
    a.with_group(&g1, |g| g.add_version(v2.clone())).unwrap();
    a.mark_complete(v2.id());
    catch_up(&b, &g1, &a.addr()).await;
    assert_eq!(b.with_group(&g1, |g| g.heads()), vec![v2.id()]);
    let r2 = b.world.fetch(&b.ep, &g1, &[a.addr()], &p2.collection, p2.total_size, &wait).await.unwrap();
    assert_eq!(r2.missing_files, 1, "un seul fichier manquait");
    let changed_len = (4 * MIB) as u64;
    assert!(r2.downloaded_bytes >= changed_len, "le fichier modifié a voyagé : {}", r2.downloaded_bytes);
    assert!(
        r2.downloaded_bytes < changed_len + 64 * 1024,
        "seul le blob modifié voyage : {} octets reçus pour {} de monde",
        r2.downloaded_bytes,
        p2.total_size
    );
    b.mark_complete(v2.id());
    // Le monde matérialisé est remplacé en bloc, sans résidu.
    assert_eq!(b.world.materialize(&p2.collection, &dest_b).await.unwrap(), 5);
    assert_eq!(tree(&dest_b), tree(world_a.path()));
    let siblings: Vec<String> = std::fs::read_dir(dest_b.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(siblings, vec!["world".to_string()], "aucun dossier temporaire ne traîne");

    // 7) Bail signé : annoncé à B, valable 90 s depuis la réception, expiré ensuite ; relâché à la fin de partie
    // avec une pierre tombale. Prise en deux phases : claim acquitté, second claim en conflit.
    {
        let conn = HearthConn::connect(&a.ep, b.addr()).await.unwrap();
        // Première phase : B garde le premier claim comme verrou ; une autre partie du même titulaire est en conflit.
        let draft = Lease::new(g1, a.key, Some((v2.id(), 2)));
        let claim = Claim::of(&draft);
        assert_eq!(conn.claim_lease(&claim).await.unwrap(), ClaimReply::Ack);
        let rival = Claim::of(&Lease::new(g1, a.key, Some((v2.id(), 2))));
        let ClaimReply::Conflict(conf) = conn.claim_lease(&rival).await.unwrap() else { panic!("conflit attendu") };
        assert_eq!((conf.kind, conf.holder, conf.lease_id), (ConflictKind::Claim, a.key, claim.lease_id));
        // Un claim au nom d'un autre, ou d'une autre époque, est rejeté ; un non-membre n'a pas de réponse.
        let cb = HearthConn::connect(&b.ep, a.addr()).await.unwrap();
        assert!(matches!(cb.claim_lease(&claim).await, Err(NetError::Rejected(_))));
        let mut old_epoch = claim.clone();
        old_epoch.epoch = 3;
        assert!(matches!(conn.claim_lease(&old_epoch).await, Err(NetError::Rejected(_))));
        let cc = HearthConn::connect(&c.ep, a.addr()).await.unwrap();
        assert_eq!(cc.claim_lease(&claim).await.unwrap_err(), NetError::NoReply);
        // Le candidat qui renonce relâche son verrou (release avec seq 0) : le claim suivant est acquitté.
        conn.release_lease(&g1, claim.lease_id, 0, ReleaseReason::Handover, None).await.unwrap();
        let again = Claim::of(&Lease::new(g1, a.key, Some((v2.id(), 2))));
        assert_eq!(conn.claim_lease(&again).await.unwrap(), ClaimReply::Ack);

        // Deuxième phase : le bail signé de cette partie.
        let lease = Lease::new(g1, a.key, Some((v2.id(), 2))).with_lease_id(again.lease_id).signed(&*a.ident);
        conn.send_lease(&lease).await.unwrap();
        let now = Instant::now();
        {
            let table = b.delegate.leases.lock().unwrap();
            assert_eq!(table.holder(&g1, now).map(|l| l.holder), Some(a.key));
            assert!(table.locked_claim(&g1, now).is_none(), "le premier bail remplace le verrou");
            assert!(table.holder(&g1, now + Duration::from_secs(89)).is_some());
            assert!(table.holder(&g1, now + Duration::from_secs(91)).is_none(), "bail expiré");
            assert_eq!(table.unclean(&g1, now + Duration::from_secs(91)).len(), 1, "partie mal terminée");
        }
        // Le bail est visible dans l'état renvoyé par `Hello`, toujours signé.
        let status = HearthConn::connect(&a.ep, b.addr()).await.unwrap().hello(&g1, 3, &[]).await.unwrap();
        let seen = status.lease.expect("bail relayé");
        assert!(seen.verify());
        assert_eq!(seen.holder, a.key);
        // Un bail annoncé au nom d'un autre membre est rejeté ; un bail falsifié ou d'une autre époque aussi.
        let forged = Lease::new(g1, a.key, None).signed(&*a.ident);
        assert!(matches!(cb.send_lease(&forged).await, Err(NetError::Rejected(_))));
        let mut tampered = lease.renewed().signed(&*a.ident);
        tampered.seq += 5;
        assert!(matches!(conn.send_lease(&tampered).await, Err(NetError::Rejected(_))));
        let stale_epoch = lease.renewed().at_epoch(2).signed(&*a.ident);
        assert!(matches!(conn.send_lease(&stale_epoch).await, Err(NetError::Rejected(_))));
        let unsigned = Lease::new(g1, a.key, None);
        assert!(matches!(conn.send_lease(&unsigned).await, Err(NetError::Rejected(_))));
        // Fin de partie : pierre tombale, un renouvellement tardif de la même partie est ignoré.
        conn.release_lease(&g1, lease.lease_id, lease.seq, ReleaseReason::Stopped, Some(v2.id())).await.unwrap();
        assert!(b.delegate.leases.lock().unwrap().holder(&g1, Instant::now()).is_none());
        let late: Lease = lease.renewed().signed(&*a.ident);
        conn.send_lease(&late).await.unwrap();
        assert!(b.delegate.leases.lock().unwrap().holder(&g1, Instant::now()).is_none(), "bail relâché : pierre tombale");
    }

    // 8) Gouvernance asynchrone : B propose, vote ; A vote à son tour et finalise.
    {
        let d = node(false).await;
        let prop = b
            .with_group(&g1, |g| {
                let mut p = Proposal::new(g.state(), GroupOp::AddMember { key: d.key }).unwrap();
                p.vote(g.state(), &*b.ident).unwrap();
                p
            });
        let conn = HearthConn::connect(&b.ep, a.addr()).await.unwrap();
        conn.propose(&g1, &prop).await.unwrap();
        assert_eq!(a.with_group(&g1, |g| g.proposals().count()), 1);
        // Une signature qui n'est pas celle du votant, ou une proposition inconnue : rejetées.
        assert!(matches!(conn.vote(&g1, prop.id(), Sig([0; 64])).await, Err(NetError::Rejected(_))));
        assert!(matches!(conn.vote(&g1, [3; 32], prop.sign(&*b.ident)).await, Err(NetError::Rejected(_))));
        conn.vote(&g1, prop.id(), prop.sign(&*b.ident)).await.unwrap();
        // Sous gouvernance « créateur », la signature de A est nécessaire.
        let status = a.with_group(&g1, |g| g.add_vote(&prop.id(), a.key, prop.sign(&*a.ident))).unwrap();
        assert!(status.ready, "{status:?}");
        let op = a.with_group(&g1, |g| g.finalize(&prop.id())).unwrap();
        assert!(matches!(op.op, GroupOp::AddMember { .. }));
        assert_eq!(a.with_group(&g1, |g| g.proposals().count()), 0);
    }
    catch_up(&b, &g1, &a.addr()).await;

    // 9) File d'envoi : version 3 (point de reprise) en file chez A.
    std::fs::write(world_a.path().join("level.dat"), noise(77, 2_500)).unwrap();
    let p3 = a.world.publish_dir(&g1, world_a.path()).await.unwrap();
    let v3 = version(&a.ident, g1, 3, Some(v2.id()), p3.collection, p3.total_size, VersionKind::Checkpoint);
    a.with_group(&g1, |g| g.add_version(v3.clone())).unwrap();
    let outbox_dir = a.with_group(&g1, |g| g.outbox_dir());
    let outbox = Arc::new(Outbox::open(&outbox_dir).unwrap());
    assert!(outbox.enqueue(v3.clone()).unwrap());

    let member_b: Arc<dyn Replica> = Arc::new(MemberReplica::new(a.ep.clone(), b.addr(), BTreeSet::from([g1])).with_confirm_timeout(Duration::from_secs(60)));
    // a) Aucun membre joignable : l'entrée reste en file, sur le disque.
    let r = outbox.drain(&[]).await;
    assert_eq!((r.delivered.len(), r.pending), (0, 1));
    drop(outbox);
    let outbox = Arc::new(Outbox::open(&outbox_dir).unwrap());
    assert_eq!(outbox.len(), 1, "la file survit au redémarrage");
    // b) Le membre est joignable mais refuse : l'entrée reste, l'échec est noté.
    b.delegate.refuse_offers.store(true, Ordering::SeqCst);
    let r = outbox.drain(std::slice::from_ref(&member_b)).await;
    assert_eq!((r.delivered.len(), r.pending, r.failures.len()), (0, 1, 1));
    assert!(r.failures[0].2.contains("pas maintenant"), "{:?}", r.failures);
    assert_eq!(outbox.pending()[0].attempts, 1);
    // c) Il apparaît (boucle d'envoi) : il rapatrie la version, l'entrée est livrée puis retirée.
    b.delegate.refuse_offers.store(false, Ordering::SeqCst);
    let online = Arc::new(AtomicBool::new(false));
    let (flag, replica) = (online.clone(), member_b.clone());
    let task = outbox.spawn_drain_loop(
        Arc::new(move || if flag.load(Ordering::SeqCst) { vec![replica.clone()] } else { Vec::new() }),
        Duration::from_millis(200),
    );
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(outbox.len(), 1);
    online.store(true, Ordering::SeqCst);
    outbox.wake();
    eventually("l'entrée est livrée", || outbox.is_empty()).await;
    task.stop().await;
    assert!(b.delegate.complete.lock().unwrap().contains(&v3.id()), "B a tout le contenu de la version 3");
    assert!(b.world.is_complete(&p3.collection).await);
    assert_eq!(b.with_group(&g1, |g| g.heads()), vec![v3.id()]);
    let dest_b3 = b._dir.path().join("hearth-world").join("world3");
    b.world.materialize(&p3.collection, &dest_b3).await.unwrap();
    assert_eq!(tree(&dest_b3), tree(world_a.path()));

    // 10) Contrôle d'accès des blobs : par groupe et par racine.
    // C n'est membre d'aucun groupe de A : connexion refusée, rien ne sort.
    let err = c.world.fetch(&c.ep, &g1, &[a.addr()], &p1.collection, p1.total_size, &wait).await.unwrap_err();
    assert!(matches!(err, SyncError::Net(_)), "{err}");
    assert!(!c.world.is_complete(&p1.collection).await);
    // G2 = {A, C} avec son propre monde : C lit G2 mais pas G1, bien qu'il soit membre « de quelque chose » chez A.
    let g2 = GroupId::random();
    let create2 = SignedOp::create(&*a.ident, g2, "Autre groupe", pack(), &BTreeSet::from([c.key])).unwrap();
    let grp2 = a.delegate.store.create(&create2, GroupKey::random()).unwrap();
    a.delegate.groups.lock().unwrap().insert(g2, grp2);
    let tiny = tempfile::tempdir().unwrap();
    std::fs::write(tiny.path().join("level.dat"), noise(9, 3_000)).unwrap();
    let q = a.world.publish_dir(&g2, tiny.path()).await.unwrap();
    let rc = c.world.fetch(&c.ep, &g2, &[a.addr()], &q.collection, q.total_size, &wait).await.unwrap();
    assert_eq!(rc.files, 1);
    // C se fait passer pour le groupe G2 en demandant la collection de G1 : la requête est refusée (c'est la
    // racine demandée qui est contrôlée, pas le groupe annoncé par le client).
    let err = c.world.fetch(&c.ep, &g2, &[a.addr()], &p1.collection, p1.total_size, &wait).await.unwrap_err();
    assert!(matches!(err, SyncError::Net(_)), "{err}");
    assert!(!c.world.is_complete(&p1.collection).await);

    // 11) Longue chaîne : A ajoute C à G1 puis enchaîne assez d'opérations pour dépasser 64 Kio. L'invitation porte un
    // préfixe, C rattrape le reste auprès de A avant d'accepter.
    {
        let add_c = a.with_group(&g1, |g| g.state().sign_single(GroupOp::AddMember { key: c.key }, &*a.ident)).unwrap();
        a.with_group(&g1, |g| g.append_op(&add_c)).unwrap();
        for i in 0..160 {
            let mut p = pack();
            p.minecraft = format!("1.21.{}", 10 + i);
            let op = a.with_group(&g1, |g| g.state().sign_single(GroupOp::PackUpdate { pack: p }, &*a.ident)).unwrap();
            a.with_group(&g1, |g| g.append_op(&op)).unwrap();
        }
        let ops = a.with_group(&g1, |g| g.ops().to_vec());
        let size: usize = ops.iter().map(|o| serde_json::to_vec(o).unwrap().len()).sum();
        assert!(size > 64 * 1024, "la chaîne ne tient pas dans un message ({size} octets)");
        let conn = HearthConn::connect(&a.ep, c.addr()).await.unwrap();
        conn.send_invite(&ops, &key1).await.unwrap();
        assert_eq!(c.with_group(&g1, |g| g.state().op_seq), ops.len() as u64);
        assert_eq!(c.with_group(&g1, |g| g.state().clone()), a.with_group(&g1, |g| g.state().clone()));
        assert_eq!(c.with_group(&g1, |g| g.key().clone()), key1);
        // C est maintenant membre : il rattrape les en-têtes, et B apprend aussi les nouvelles opérations.
        catch_up(&c, &g1, &a.addr()).await;
        catch_up(&b, &g1, &a.addr()).await;
        assert_eq!(b.with_group(&g1, |g| g.state().op_seq), ops.len() as u64);
        assert_eq!(c.with_group(&g1, |g| g.heads()), vec![v3.id()]);
    }

    // 12) Rapatriement avec bascule de pair : E ne sert pas les blobs, B oui. C reçoit la version 3 de B.
    let r = c.world.fetch(&c.ep, &g1, &[e.addr(), b.addr()], &p3.collection, p3.total_size, &wait).await.unwrap();
    assert_eq!(r.missing_files, 5);
    let dest_c = c._dir.path().join("world");
    c.world.materialize(&p3.collection, &dest_c).await.unwrap();
    assert_eq!(tree(&dest_c), tree(world_a.path()));
    // Une taille signée différente de la réalité est refusée (monde qui ment).
    let liar = c.world.fetch(&c.ep, &g1, &[b.addr()], &p3.collection, p3.total_size + 1, &wait).await;
    assert!(matches!(liar, Err(SyncError::SizeMismatch { .. })), "{liar:?}");
    // Et un plafond de taille trop bas arrête tout avant le moindre transfert.
    let capped = FetchOptions { max_total: 1000, ..FetchOptions::default() };
    assert!(matches!(c.world.fetch(&c.ep, &g1, &[b.addr()], &p3.collection, p3.total_size, &capped).await, Err(SyncError::TooLarge(_))));
    // Une croissance démesurée par rapport à la version précédente et un disque trop plein arrêtent aussi tout avant le
    // moindre transfert (une grande taille signée ne coûte rien à l'émetteur).
    let grown = FetchOptions { previous_total: Some(0), ..FetchOptions::default() };
    let huge = 3 * 1024 * 1024 * 1024u64;
    assert!(matches!(c.world.fetch(&c.ep, &g1, &[b.addr()], &p3.collection, huge, &grown).await, Err(SyncError::Oversize { .. })));
    let full = FetchOptions { free_bytes: Some(1024), confirmed: true, ..FetchOptions::default() };
    assert!(matches!(c.world.fetch(&c.ep, &g1, &[b.addr()], &p3.collection, p3.total_size, &full).await, Err(SyncError::NoSpace { .. })));

    // 13) Libération : une collection relâchée n'est plus servie (C, qui ne l'a pas, ne peut plus la rapatrier).
    a.world.release(&g1, &p1.collection).await.unwrap();
    let err = c.world.fetch(&c.ep, &g1, &[a.addr()], &p1.collection, p1.total_size, &wait).await.unwrap_err();
    assert!(matches!(err, SyncError::Net(_)), "{err}");
}
