# Hearth : serveurs de groupe P2P

> Nom interne : **Hearth**. Nom affiché : « serveur de groupe ». Étude : voir le document « Étude : serveurs de groupe P2P » et la maquette « Cubelink · Serveurs de groupe » (liens dans la PR). Statut : en cours d'implémentation, par phases (fin de document).

## 1. Objectif

Un groupe d'amis partage un monde Minecraft moddé. N'importe quel membre peut le lancer quand il veut, sans attendre un « hôte » fixe :

- un seul bouton **Jouer** : Cubelink rejoint la partie en cours, ou la démarre sur le PC du joueur ;
- un seul serveur à la fois (bail d'hôte) : pas de désynchronisation ;
- le monde est versionné, signé et répliqué chez tous les membres ;
- pas de jeu hors ligne sur un serveur de groupe ;
- un membre absent ne bloque jamais le groupe : points de reprise, file d'envoi locale, puis « continuer sans lui » à la majorité.

Hors périmètre : fusion automatique de deux mondes. Une fourche se tranche par un choix du groupe.

## 2. Règle de jonction (mondes solo et serveurs de groupe)

Avant Hearth, tout ami accepté pouvait ouvrir le tunnel et lire le manifeste d'un hôte dès que son jeu tournait, et `HostAuto` invitait d'office tous les amis en ligne.

Nouvelle règle :

| Cas | Qui peut ouvrir le tunnel et lire le manifeste |
|---|---|
| Monde solo partagé | Un ami **invité** pour cette partie, ou dont la **demande à rejoindre** a été acceptée |
| Serveur de groupe | Tout **membre** du groupe hébergé |
| Tout autre pair | Personne |

- L'accès est une liste de clés tenue par l'hôte pour la partie en cours (`SessionAccess`, `src-tauri/src/authz.rs`). Elle est vidée par `host_stop_sharing`.
- `friend_invite` ajoute la clé invitée. L'invitation automatique de tous les amis en ligne n'a lieu que si le réglage `open_to_all_friends` est activé (désactivé par défaut).
- **Demande à rejoindre** : ALPN `cubelink/joinreq/1` (`p2p/src/join_request.rs`). L'invité envoie une demande ; l'hôte reçoit l'événement `friend://join_request` et répond par `friend_join_request_answer`. Le flux reste ouvert au plus 120 s. Après un refus, la même clé ne peut pas redemander avant 10 min. Une demande d'un non-ami, ou quand rien n'est partagé, est refusée sans réponse.

## 3. Modèle de données

### 3.1 Groupe (`p2p/src/hearth/model.rs`)

```
GroupId      = 16 octets aléatoires (hex dans les noms de fichiers et le JSON)
PackPin      = { manifest_hash, minecraft, loader: LoaderSpec }
Governance   = Creator | Majority
GroupState   = { id, name, pack: PackPin, creator: PeerKey, members: BTreeSet<PeerKey>,
                 governance, epoch: u64, base_version: Option<VersionId>, op_seq: u64 }
```

L'état d'un groupe se reconstruit en rejouant une **chaîne d'opérations signées** (`GroupOp`), numérotées `op_seq = 1, 2, …`, chacune liée à la précédente par son empreinte :

| Opération | Signée par | Effet |
|---|---|---|
| `Create { name, pack, members }` | le créateur | crée le groupe, `governance = Creator`, `epoch = 0` |
| `AddMember { key }` | gouvernance | ajoute un ami du signataire |
| `RemoveMember { key }` | gouvernance | retire un membre ; la clé de groupe est renouvelée |
| `ContinueWithout { absent, base_version }` | majorité des membres hors `absent` | `epoch += 1`, nouvelle base, bail de l'absent annulé ; si `absent == creator`, `governance = Majority` |
| `PackUpdate { pack }` | gouvernance | change le modpack épinglé (après sauvegarde) |
| `Leave` | le membre qui part | le retire |

- « gouvernance » = le créateur tant que `governance == Creator`, sinon la majorité stricte des membres.
- Une opération à plusieurs signatures est d'abord une **proposition** (`Proposal`) qui accumule les signatures de façon asynchrone (chacun vote à son prochain lancement) ; elle devient une opération quand le seuil est atteint.
- Signature ed25519 de la clé d'identité Cubelink, avec séparation de domaine `b"cubelink/hearth/op/1"`. Encodage canonique : JSON aux clés triées (`serde_json` sur des `BTreeMap`/structs à ordre fixe).
- La **clé de groupe** (32 octets) chiffre les points de reprise confiés aux gardiens. Elle n'est jamais dans une opération : elle est transmise à chaque membre sur la connexion QUIC authentifiée, et stockée localement.

### 3.2 Versions du monde (`p2p/src/hearth/version.rs`)

```
VersionHeader = { group_id, number: u64, parent: Option<VersionId>, epoch, author: PeerKey,
                  pack_hash, created_at, played_secs, kind: Session | Checkpoint | Restore,
                  collection: BlobHash, total_size }
VersionId     = BLAKE3(encodage canonique de l'en-tête)
SignedVersion = { header, sig }
```

- `collection` : une collection iroh-blobs (`HashSeq`) qui liste `chemin relatif → blob BLAKE3` des fichiers du monde. Deux versions partagent tous les blobs inchangés : seuls les fichiers modifiés voyagent.
- Fichiers exclus du monde : `session.lock`, `*.tmp`. Les chemins sont validés comme ceux du manifeste (pas de `..`, pas d'absolu, pas de nom réservé Windows).
- **Tête** : parmi les versions valides de l'époque courante, celles qui n'ont pas d'enfant. Une seule tête = état normal. Plusieurs têtes = **fourche**, montrée au groupe.
- Une version de l'absent postérieure à `base_version` d'un `ContinueWithout` reste stockée, comme branche restaurable, mais n'est jamais tête toute seule.
- `Restore { from }` : une nouvelle version dont le contenu est celui d'une ancienne ; rien n'est supprimé.

### 3.3 Bail d'hôte (`p2p/src/hearth/lease.rs`)

```
Lease = { group_id, holder: PeerKey, base_version: VersionId, seq: u64, ttl_secs: 90 }
```

- L'hôte renvoie son bail toutes les 30 s aux membres en ligne. Chaque récepteur le garde 90 s **à partir de sa réception** (pas d'horloge partagée).
- Décision au clic sur Jouer (`decide`, pure) :
  1. un bail valide d'un autre membre est connu → **Rejoindre**(holder) ;
  2. un membre en ligne a une version plus récente que la mienne → **Synchroniser** d'abord ;
  3. aucun membre ni source joignable → **Bloqué** (jeu hors ligne interdit) ;
  4. sinon → **Héberger** : j'annonce mon bail, j'attends 2 s une contre-annonce ; en cas de conflit, le plus petit couple `(base_version.number, clé)` garde la main et l'autre rejoint.
- Le bail n'est pas une exclusion parfaite (coupure réseau partielle) : la chaîne de versions rend toute fourche visible.

## 4. Protocole réseau `cubelink/hearth/1` (`p2p/src/hearth/proto.rs`)

Même discipline que `invite.rs` et `presence.rs` : l'émetteur est `conn.remote_id()`, messages JSON bornés (64 Kio), un flux bidirectionnel par requête, délai 10 s, non-membre refusé sans réponse (sauf `GroupInvite`, accepté d'un ami).

| Requête | Réponse | Usage |
|---|---|---|
| `Hello { group_id, op_seq, heads }` | `Status { op_seq, heads, lease? }` | état d'un membre (sonde périodique, 60 s) |
| `GetOps { group_id, after }` | `Ops [...]` | rattraper la chaîne d'opérations |
| `GetVersions { group_id, after_number }` | `Versions [...]` | rattraper les en-têtes |
| `Lease { lease }` | `Ack` | annonce ou renouvellement du bail |
| `LeaseRelease { group_id, version? }` | `Ack` | fin de partie, avec la version publiée |
| `GroupInvite { ops, group_key }` | `Ack` | invitation au groupe (d'un ami seulement) |
| `Propose { proposal }` / `Vote { proposal_id, sig }` | `Ack` | gouvernance asynchrone |

Les blobs du monde passent par l'ALPN iroh-blobs déjà servi par le `Router`, avec un contrôle d'accès : un blob d'une collection Hearth n'est servi qu'aux membres du groupe.

## 5. Stockage local

Sous le dossier de données de l'app : `hearth/<group_id>/`

- `ops.jsonl` (chaîne d'opérations), `versions/<id>.json` (en-têtes signés), `proposals/`, `group.key` ;
- `world/` : le monde matérialisé de la tête (liens durs ou copies depuis le magasin de blobs) ;
- `outbox/` : versions et points de reprise à pousser, persistés (survivent au redémarrage) ;
- `server/` : le serveur dédié installé pour ce groupe.

Les blobs vivent dans le magasin iroh-blobs existant. Le GC garde : la tête, sa parente, les versions de moins de 14 jours, les sauvegardes `PackUpdate`, et tout ce qui est dans `outbox/`.

## 6. Serveur dédié (`core/src/hearth/server.rs`)

L'hôte lance un **serveur dédié** en arrière-plan, et tous les joueurs (hôte compris) s'y connectent par le tunnel. Raisons : pas d'« Ouvrir au LAN », le serveur survit au départ du joueur hôte, données des joueurs toutes dans `playerdata/`, et sauvegarde pilotable.

- Installation par loader : vanilla (jar serveur Mojang), Fabric (lanceur serveur `meta.fabricmc.net`), Quilt (installeur `install-server`), Forge et NeoForge (installeur `--installServer`).
- Mods : ceux du modpack épinglé, moins les mods « client seulement » (métadonnées Modrinth `server_side == "unsupported"`, liste connue). Un échec au démarrage affiche le mod en cause.
- `eula.txt` n'est écrit qu'après l'acceptation de l'EULA Minecraft par l'utilisateur (une fois, dans l'app).
- `server.properties` : `online-mode=true`, `server-ip=127.0.0.1`, port local libre, `level-name=world`.
- Contrôle par l'entrée standard : `save-off`, `save-all flush`, `save-on`, `stop`. Prêt quand la sortie contient `Done (`.
- Mémoire : choisie selon la RAM libre ; avertissement sous 12 Go au total.

## 7. Cycle d'une partie

1. **Jouer** → `decide` (§ 3.3).
2. **Héberger** : matérialiser la tête dans `world/`, démarrer le serveur, annoncer le bail, ouvrir l'accès du tunnel aux membres, lancer le client en `--quickPlayMultiplayer` vers le port du tunnel local.
3. **Points de reprise** toutes les 10 min : `save-off`, `save-all flush`, import des fichiers modifiés dans le magasin de blobs, nouvelle version `Checkpoint`, `save-on`. La version part dans `outbox/`.
4. **Fin** (`stop` ou passation) : version `Session`, `LeaseRelease`, `outbox/`.
5. **Rejoindre** : tunnel vers le holder, client en `--quickPlayMultiplayer`. À la fin, la nouvelle tête arrive par la synchro.

### Envoi des versions (outbox)

- Chaque entrée de `outbox/` est poussée à tout membre joignable (il récupère les blobs manquants lui-même).
- Aucun membre joignable : l'entrée reste en file, et part dès qu'un membre (ou plus tard un gardien) apparaît, tant que Cubelink tourne (mode arrière-plan, autostart).
- **Gardiens** : quand le réseau de relais Cubelink existera (fil P2P, phase « relais réseau »), une entrée sans membre joignable est aussi confiée, chiffrée par la clé de groupe (XChaCha20-Poly1305), à 2 ou 3 gardiens (3 Go par groupe, 14 jours). Interface prévue : trait `Replica` dans `p2p/src/hearth/outbox.rs`.

## 8. Absences et fourches

- **Partie mal terminée** (bail expiré sans `LeaseRelease`) : les autres voient « La partie d'Alice ne s'est pas terminée proprement ». S'ils ont son dernier point de reprise : « Jouer depuis 23 h 00 ». Sinon : « Attendre » ou « Reprendre depuis la dernière version connue » (crée une fourche, temps de jeu mis de côté affiché).
- **Continuer sans un membre** : proposable par tout membre quand le bloqueur n'a pas été vu depuis 7 jours ; opération `ContinueWithout` à la majorité des autres ; repart de la version la plus récente trouvée chez les membres restants (et les gardiens) ; si l'absent revient, sa version devient une branche restaurable et il reste membre.
- **Fourche** : écran « Deux versions existent » ; le choix du groupe crée une version `Restore` de la branche gardée.

## 9. Commandes et événements Tauri

Voir `docs/contrats.md`, section « Hearth ». Préfixe `hearth_`, événement `hearth://state` (payload `HearthCard[]`) à chaque changement d'état visible.

## 10. Phases

| Phase | Contenu | État |
|---|---|---|
| 1 | Règle de jonction : `SessionAccess`, demande à rejoindre, fin de l'invitation automatique | en cours |
| 2 | Groupes : modèle signé, opérations, propositions et votes, protocole, stockage | en cours |
| 3 | Versions du monde : collections iroh-blobs, chaîne, têtes et fourches, synchro de fond, matérialisation | en cours |
| 4 | Bail, serveur dédié, bouton Jouer, passation | en cours |
| 5 | Points de reprise, outbox, partie mal terminée, continuer sans un membre | en cours |
| 6 | Tête et bail publiés dans la DHT, gardiens chiffrés | après la DHT et le réseau de relais du fil P2P |
