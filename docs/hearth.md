# Hearth : serveurs de groupe P2P

> Nom interne : **Hearth**. Nom affiché : « serveur de groupe ». Étude : voir le document « Étude : serveurs de groupe P2P » et la maquette « Cubelink · Serveurs de groupe » (liens dans la PR). Statut : en cours d'implémentation, par phases (§ 10). Les cas limites étudiés sont dans `docs/hearth-et-si.md` (§ 11) et leurs décisions de conception sont regroupées au § 12.

## 1. Objectif

Un groupe d'amis partage un monde Minecraft moddé. N'importe quel membre peut le lancer quand il veut, sans attendre un « hôte » fixe :

- un seul bouton **Jouer** : Cubelink rejoint la partie en cours, ou la démarre sur le PC du joueur ;
- un seul serveur à la fois (bail d'hôte à deux phases, avec les autres membres comme témoins) : pas de désynchronisation. Le bail est « sûr » quand une majorité de membres actifs l'a acquitté, « au mieux » sinon (§ 3.4) ;
- le monde est versionné, signé et répliqué chez tous les membres ;
- pas de jeu sans Internet sur un serveur de groupe. Un membre qui est seul en ligne (Internet joignable, aucun autre membre joignable) peut héberger, après une confirmation explicite : c'est un assouplissement assumé de la règle « pas de jeu hors ligne », qui peut produire une fourche visible si un autre membre joue seul de son côté. La vraie réponse est un membre « toujours allumé » (hébergement en arrière-plan) ou les gardiens de la phase 6 ;
- un membre absent ne bloque jamais le groupe : points de reprise, file d'envoi locale, puis « continuer sans lui » à la majorité des membres actifs.

Hors périmètre : fusion automatique de deux mondes. Une fourche se tranche par un choix du groupe.

### Principes transversaux

- **Rien n'est supprimé en silence.** Une branche écartée, une fin de partie plantée, un point de reprise abandonné restent récupérables jusqu'à une action explicite de l'utilisateur (§ 5, rétention).
- **La chaîne signée fait foi.** L'état d'un groupe et le graphe de ses versions se reconstruisent toujours depuis les opérations et les en-têtes stockés (idempotent), jamais en les corrigeant en place.
- **Toutes les boucles vivent en Rust** (tokio) : bail, sondes, points de reprise, outbox. Le front ne fait qu'afficher `hearth://state`. Aucun `setInterval` ne porte une règle (WebView2 ralentit les minuteurs d'une fenêtre cachée).
- **Le format est gelé avant d'être publié** (§ 3.1) : champ `format` et espace `ext` dans tout objet signé, dès la première pré-version.
- **Un message qui ne s'explique pas n'existe pas** : toute erreur affichable passe par la couche de messages (§ 9).

## 2. Règle de jonction (mondes solo et serveurs de groupe)

Avant Hearth, tout ami accepté pouvait ouvrir le tunnel et lire le manifeste d'un hôte dès que son jeu tournait, et `HostAuto` invitait d'office tous les amis en ligne.

Nouvelle règle :

| Cas | Qui peut ouvrir le tunnel et lire le manifeste |
|---|---|
| Monde solo partagé | Un ami **invité** pour cette partie, ou dont la **demande à rejoindre** a été acceptée |
| Serveur de groupe | Tout **membre** du groupe hébergé |
| Tout autre pair | Personne |

- L'accès est une liste de clés tenue par l'hôte pour la partie en cours (`SessionAccess`, `src-tauri/src/authz.rs`). Elle est vidée par `host_stop_sharing`.
- **L'accès est indexé par cible** : `SessionTarget::{Solo, Group(GroupId)}`. Le premier message de chaque flux du tunnel désigne la cible ; `HostTunnel` tient une table cible vers port local ; l'autorisation porte sur le couple (clé, cible). Un PC qui sert un groupe et un monde solo en même temps ne mélange donc pas les accès.
- **Révocation immédiate** : à l'application d'un `RemoveMember` ou d'un `Leave` (§ 3.2), `SessionAccess::revoke(clé)` retire la clé et `tunnel.rs` ferme toutes ses connexions actives avec le code `CLOSE_DENIED` (registre des flux par clé). Le serveur reçoit en plus `kick` puis `whitelist remove` (§ 6.4).
- `friend_invite` ajoute la clé invitée. L'invitation automatique de tous les amis en ligne n'a lieu que si le réglage `open_to_all_friends` est activé (désactivé par défaut).
- **Demande à rejoindre** : ALPN `cubelink/joinreq/1` (`p2p/src/join_request.rs`). L'invité envoie une demande ; l'hôte reçoit l'événement `friend://join_request` et répond par `friend_join_request_answer`. Le flux reste ouvert au plus 120 s. Après un refus, la même clé ne peut pas redemander avant 10 min. Une demande d'un non-ami, ou quand rien n'est partagé, est refusée sans réponse.
- **Membres non amis** : un membre ajouté par un autre n'est pas forcément mon ami. Avant d'accepter une invitation de groupe, l'écran dit : « X n'est pas ton ami ; il verra ton adresse IP pendant les parties, ou, si tu préfères, ses parties passeront par un relais (plus lent) ». Le chemin réservé au relais (second point d'accès, donc autre clé liée au compte par un `DeviceCert`) est prévu en phase 6 ; d'ici là, les sondes vers les non-amis passent de 60 s à 5 min.
- **Blobs** : le service des blobs d'un groupe se fait sur un ALPN dédié (§ 4) et autorise **par hash**, pas par connexion (§ 5.3).

## 3. Modèle de données

### 3.1 Format, identité et appareils

**Format.** Tout objet signé (opération, en-tête de version, bail, adresse, battement) porte `format: u16` (valeur 1) et `ext: BTreeMap<String, Value>` signé, pour les champs optionnels futurs (lu s'il est connu, conservé tel quel sinon). Les identifiants (`VersionId`, empreinte d'opération) sont le BLAKE3 de l'encodage canonique incluant `ext`, donc stables d'une version de Cubelink à l'autre. Hors `ext`, les champs inconnus restent refusés. `Hello` annonce `app_version` et `proto_max` ; une requête d'un format trop récent reçoit une erreur explicite `Unsupported { need }`, jamais un silence. L'interface dit alors « Bob doit mettre Cubelink à jour pour rejoindre la partie » ou « Mets Cubelink à jour : le groupe utilise une version plus récente », et `decide` renvoie `NeedsUpdate` au lieu d'héberger depuis une vieille tête. L'opération `MinFormat { n }` (gouvernance) relève le format du groupe quand tous les membres actifs sont à jour. Les fichiers locaux portent `hearth/<id>/FORMAT` ; une migration au démarrage fait d'abord une sauvegarde `*.bak`.

**Encodage et signature.** JSON canonique aux clés triées (`serde_json` sur des `BTreeMap` ou structs à ordre fixe). Signature ed25519 avec séparation de domaine, un domaine par objet (`cubelink/hearth/op/1`, `…/lease/1`, `…/device/1`, `…/join/1`, `…/addr/1`, `…/seen/1`, `…/mcprofile/1`).

**Identité.** Deux niveaux de clés :

```
AccountKey = ed25519, identité du membre. Chiffrée au repos (DPAPI portée utilisateur sous Windows,
             trousseau sous macOS et Linux, crate keyring), migration transparente depuis identity.key.
             Ne signe que DeviceCert, RevokeDevice et ReplaceKey.
DeviceKey  = ed25519 par installation : transport iroh, baux, versions, adresses, battements.
DeviceCert = { account, device, label, created_at, sig_account }   domaine cubelink/hearth/device/1
device_id  = 16 octets aléatoires par installation, stockés hors du profil itinérant
             (%LOCALAPPDATA%), présents dans Hello, Status et tous les objets signés
```

- Un **membre** est un compte. Votes, majorités, dernière activité et « absent » se comptent par compte ; l'activité d'un appareil compte pour son compte. Un bail est tenu par un appareil, attribué au compte, et **un seul bail est actif par compte** (un second bail du même compte fait rejoindre le premier).
- **Étape intermédiaire** : tant que la migration vers les clés d'appareil n'est pas livrée, la clé de compte est aussi l'unique clé d'appareil (`DeviceCert` implicite, `device` = `account`). Les types distinguent déjà les deux rôles pour que le format n'ait pas à changer ensuite. Les codes amis et `friends_db` sont aujourd'hui indexés par `EndpointId` : leur migration (compte vers appareils, `DeviceCert` échangés par la présence) est à planifier **avant de figer le format de la phase 2**.
- **Instance unique** : `tauri-plugin-single-instance` plus un verrou de fichier exclusif (`fs4::try_lock_exclusive`) sur `hearth/.lock`. Un second processus redonne le focus au premier et s'arrête.
- **Même identité sur un autre PC** : un message signé de ma clé avec un `device_id` inconnu de moi déclenche « Ton identité Cubelink est utilisée sur un autre PC » et désactive Héberger tant que l'utilisateur n'a pas choisi l'appareil actif.
- **Export et import** de l'identité dans Réglages : fichier chiffré par phrase de passe (Argon2id + XChaCha20-Poly1305). L'import crée un nouveau `DeviceCert`, il ne clone pas la clé d'appareil. Un rappel « Sauvegarder mon identité » apparaît à la création du premier groupe.
- **Perte ou vol** : `ReplaceMember` (§ 3.2) par la majorité des autres membres actifs, avec un code de vérification à 6 chiffres hors bande (`verify_code.rs`), sans exiger la signature de l'ancienne clé. `ReplaceKey`, signé par l'ancienne clé, ne sert qu'au transfert volontaire (« Lier un nouvel appareil » : code à 8 caractères et connexion QUIC entre les deux appareils).

### 3.2 Groupe (`p2p/src/hearth/model.rs`)

```
GroupId      = 16 octets aléatoires (hex dans les noms de fichiers et le JSON)
PackPin      = { pack_collection: BlobHash, pack_hash, minecraft, loader: LoaderSpec }     (§ 6.2)
GroupSettings= { ops: Creator | Members(BTreeSet<AccountKey>) | None, op_level (4 ou 2),
                 spawn_protection, allow_flight, pvp, view_distance, simulation_distance,
                 enable_command_block, idle_stop_minutes (défaut 5), extra_exclude, … }
MemberInfo   = { since_op, until_op?, dormant: bool, mc_profile? }
GroupState   = { id, name, pack: PackPin, pack_history: Vec<(op_seq, base_version, pack_hash)>,
                 creator: AccountKey, members: BTreeMap<AccountKey, MemberInfo>,
                 governance, settings: GroupSettings, epoch: u64, base_version: Option<VersionId>,
                 key_epoch: u64, key_commit, min_format: u16, op_seq: u64, op_hash }
```

L'état d'un groupe se reconstruit en rejouant une **chaîne linéaire d'opérations signées** (`GroupOp`), numérotées `op_seq = 1, 2, …`, chacune liée à la précédente par son empreinte. `member_since` et `member_until` (dans `MemberInfo`) permettent de savoir qui était membre à un `op_seq` donné.

| Opération | Signée par | Effet |
|---|---|---|
| `Create { name, pack, members, settings, seed?, level_type? }` | le créateur | crée le groupe, `governance = Creator`, `epoch = 0`. La graine et le type de monde d'un monde neuf sont figés ici |
| `AddMember { key, name, invitee_sig }` | gouvernance | ajoute un ami du signataire. `invitee_sig` est la signature, par la clé ajoutée, de `("cubelink/hearth/join/1", group_id, name)` : la clé existe et consent |
| `RemoveMember { key, key_epoch, key_commit }` | gouvernance | retire un membre ; le signataire qui finalise génère la nouvelle clé de groupe (§ 3.5) |
| `Leave` | le membre qui part | le retire ; suivi d'un `RotateKey` |
| `RotateKey { key_epoch, key_commit }` | un membre actif | renouvelle la clé après un `Leave` (le prochain hôte, ou le premier membre actif qui applique le départ) |
| `ContinueWithout { absents, notice_secs }` | majorité des membres actifs hors `absents` | `epoch += 1`, base effective (§ 3.3), baux des absents annulés, absents marqués dormants ; si le créateur est dormant depuis 30 jours, `governance = Majority` |
| `Wake` | le dormant lui-même | le remet actif (envoyé automatiquement à son retour, une fois la chaîne reçue) |
| `PackUpdate { pack, base_version, major }` | gouvernance (majorité si `major` ou jar inconnu, § 6.2) | change le modpack épinglé ; ajoute une entrée à `pack_history` |
| `SetGovernance { Majority }` | le créateur seul, ou la majorité stricte des autres membres actifs si le créateur n'a signé ni opération ni version depuis 30 jours | passe en majorité sans toucher à l'époque ni à la base |
| `SetSettings { settings }` | gouvernance | change les réglages du groupe, dont les opérateurs du serveur (§ 6.4) |
| `MinFormat { n }` | gouvernance | relève le format minimal du groupe |
| `ReplaceMember { old, new }` | `new` (preuve de possession) et la majorité des **autres** membres actifs | transfère appartenance, droits de créateur et lien Minecraft ; l'ancienne clé ne vote pas |
| `Veto { op_id }` | majorité des autres membres actifs | voir ci-dessous |

**Gouvernance et quorums.**

- « gouvernance » = le créateur tant que `governance == Creator`, sinon la majorité stricte des **membres actifs**. Un membre actif est un membre non dormant ; tous les quorums de tous les types d'opération comptent `members − dormants`.
- Sous `Creator`, les opérations à fort impact (retrait d'au moins 2 membres en 24 h, `PackUpdate` avec jar inconnu) attendent 24 h. Pendant ce délai, une majorité des autres membres actifs peut signer `Veto { op_id }` : l'opération est abandonnée et la gouvernance passe en `Majority`.
- Option à la création : « Décisions : moi seul / à la majorité ». Défaut : majorité à partir de 3 membres.
- Le créateur qui quitte voit l'effet avant de confirmer (« Le groupe décidera à la majorité »).

**Propositions.**

- Une opération à plusieurs signatures est d'abord une **proposition** qui accumule les signatures de façon asynchrone. Chaque vote signe le **contenu** : `{group_id, epoch, type, arguments, proposal_nonce}`, pas `(op_seq, prev)`. À la finalisation, la proposition se rebase sur la tête de chaîne courante, et les votes restent valides tant que `validate_op` et `check_authorization` passent sur l'état courant. Sinon : `StaleProposal` et un « Revoter » qui préremplit la même opération et prévient les votants.
- Une proposition expire après **14 jours** (« 2 votes sur 3, expire dans 5 jours »). Un invité qui n'a pas signé son consentement sous 14 jours fait expirer la proposition d'ajout.
- Un membre ne signe qu'**un vote par cible contradictoire** (par exemple deux `RemoveMember` croisés) ; le vote est refusé localement, avec « tu as déjà voté pour… ».
- Limites : 1 proposition ouverte par membre et par cible (une nouvelle remplace l'ancienne), 10 stockées par groupe (les plus anciennes sont évincées), au plus 5 `Propose` par heure et par clé émettrice (refus sans réponse au-delà).
- `AddMember` : la signature de consentement de l'invité est recueillie **avant** la proposition (flux d'invitation). Il n'y a donc ni état `pending` ni opération `Accept` : l'invité n'entre dans le quorum qu'une fois l'opération appliquée. Un `Propose AddMember` n'est accepté que si le proposant a cette clé en ami accepté (sinon refus sans réponse). L'écran de vote affiche le nom de **ma** liste d'amis ; à défaut « inconnu de toi, ami de <proposant> », les 8 premiers caractères de la clé et un code à 6 chiffres comparable avec la personne.
- `GroupInvite` : au plus 3 par jour et par ami, 1 invitation en attente par `group_id`, `ops` limité à 1 000 opérations (taille vérifiée avant la désérialisation), rejouée seulement quand l'utilisateur ouvre l'écran, jamais à la réception.

**Concurrence sur la chaîne.** Le modèle reste **linéaire** (pas de DAG : un tri par hash réordonnerait l'historique et changerait après coup la validité de versions déjà acceptées).

1. Deux opérations valides au même `op_seq` : garder celle qui a le plus de signatures éligibles, puis le plus petit hash. La perdante est archivée dans `ops_conflicts.jsonl`. Une perdante à un seul signataire (`Leave`, `Wake`…) est re-signée automatiquement sur la nouvelle tête ; une perdante à plusieurs signatures redevient une proposition ouverte, et l'écran explique le résultat.
2. Après une réorganisation, l'état du groupe **et** le graphe des versions sont reconstruits depuis zéro.
3. **Équivocation** : deux opérations au même `op_seq` signées par la même clé seule forment une preuve. Elle est stockée, montrée au groupe (« X a signé deux décisions contradictoires ») et rend recevable un `RemoveMember { X }`, même contre le créateur.
4. `MAX_OPS` (10 000) s'applique dans `apply` comme dans `replay` (erreur `TooManyOps`), et la réception accepte au plus 1 opération par minute et par signataire unique.

**Dormance.** `ContinueWithout` accepte plusieurs absents. Un membre visé par un `ContinueWithout` en vigueur est **dormant** : il sort du dénominateur de tous les quorums. Il redevient actif en signant `Wake` (envoyé à son retour), et son retour (`Present`) annule la bascule de gouvernance si elle n'a pas encore eu lieu. Après 30 jours de dormance, la carte propose « Retirer Hugo du groupe » (`RemoveMember` au quorum des actifs). Un groupe dont la moitié des clés est morte peut donc décider (test `proptest`).

**Préavis de `ContinueWithout`.** Une proposition est diffusée (`Propose`, plus tard DHT et gardiens) avec un `notice_secs` signé de 7 jours.

- Elle ne devient une opération que si chaque votant atteste localement n'avoir reçu aucun message signé de l'absent depuis l'émission, et que le délai est écoulé sur son horloge locale. Chaque vote porte `last_seen_absent` (affiché ; refusé localement si moins de 7 jours).
- **Éligibilité** : chaque membre émet un battement signé `Seen { key, device_id, counter }`, relayé dans `Status` ; chacun garde le maximum reçu par membre (heure locale de réception). Une proposition n'est éligible que si le dernier battement connu de l'absent, chez tous les membres joignables, date de plus de 7 jours. Elle est annulée de plein droit si un battement ou une version de l'absent postérieurs à sa création circulent, ou si l'absent envoie `Present { group_id, proposal_id, sig }` (relayé et stocké par qui le reçoit).
- Le vote signe « absents + époque », pas une base figée : la **base effective** est calculée à la finalisation (§ 3.3).
- **Groupe de 2** : après le préavis de 7 jours sans veto, l'absent est dormant. Le membre actif peut jouer (mode « seul », § 3.4) mais **ne peut ni inviter ni retirer** tant que la gouvernance `Creator` d'un créateur dormant n'a pas basculé (30 jours de dormance confirmée). À la création d'un groupe de 2 : « À deux, il faut que l'autre ait Cubelink ouvert pour jouer ».
- Deux `ContinueWithout` croisés : une opération dont un signataire est l'`absent` d'une opération concurrente perd (règle 1 ci-dessus), et l'écran dit « Tom et toi avez chacun décidé de continuer sans l'autre : la décision de Tom a été retenue, ta partie est gardée comme branche restaurable ».
- `ContinueWithout` est refusé quand le proposant n'a pas vu l'absent depuis 7 jours **mais aussi** quand l'absent a signé une opération ou une version dans les 7 derniers jours de la chaîne locale.

### 3.3 Versions du monde (`p2p/src/hearth/version.rs`)

```
VersionHeader = { format, group_id, number: u64, parent: Option<VersionId>, supersedes: Vec<VersionId>,
                  epoch, op_seq, author: AccountKey, device_id, pack_hash, created_at, played_secs,
                  kind: Session | Checkpoint | Restore { from, paths? },
                  players: Vec<String>, changed_files: Summary,
                  lease_id, witnesses: Vec<AccountKey>,
                  collection: BlobHash, total_size, ext }
VersionId     = BLAKE3(encodage canonique de l'en-tête)
SignedVersion = { header, sig }
```

- `number` = `parent.number + 1` (ou `max(numéros des têtes absorbées) + 1` pour une résolution). **C'est une profondeur, pas une récence** : il ne sert qu'à l'affichage et à un départage final déterministe. « Plus récent » se juge par ascendance (`is_ancestor`).
- `op_seq` : dernière opération connue de l'auteur à la création. `players` : pseudos Minecraft vus dans le journal du serveur pendant la session. `changed_files` : résumé des fichiers modifiés. `lease_id` et `witnesses` : **provenance affichée** (« jouée avec Bob en ligne » ou « jouée sans témoin »), jamais une condition de validité.
- `Restore { from, paths }` : une nouvelle version dont le contenu est celui d'une ancienne ; `paths` vide = restauration complète, sinon restauration partielle (fichiers région, ou les trois fichiers d'un joueur, § 8). Rien n'est supprimé.
- Version locale `Orphan` : état de `world/` après une fin non propre (§ 7.5). Stockée localement (`versions/` avec le drapeau `local_only`), protégée du GC, **jamais annoncée, jamais signée ni tête**. La publier crée une nouvelle `Session`, enfant de la base.

**Collection.** La collection d'une version est un **arbre de Merkle par dossier**, avec deux sous-arbres racines : `world/` et `server_state/` (§ 6.5). Un nœud de dossier est une `HashSeq` iroh-blobs dont le premier élément est un blob de noms canonique (`[{name, kind: dir|file|chunked, size}]`, noms triés et en NFC) et les suivants les enfants. Un fichier de 1 Mio ou moins est un blob. Un fichier plus gros est une `HashSeq` de **blocs fixes de 64 Kio** (le format région ne décale jamais les secteurs ; aucun analyseur de région). Un point de reprise ne crée que les nœuds situés sur les chemins modifiés. Deux versions partagent tous les blobs inchangés. Le tag persistant posé sur la racine doit protéger tout l'arbre : si la collecte d'iroh-blobs ne suit pas les `HashSeq` imbriquées, poser un tag par nœud (`hearth/<g>/<vid>/<hash du nœud>`). **Avant de figer le format**, mesurer le surcoût du `FsStore` par blob (une entrée redb par blob) avec un monde de 15 Go (200 000 fichiers, 240 000 blocs).

**Validation des chemins** (`validate_world_path`, propre au monde, sans la liste de préfixes du manifeste) : mêmes noms interdits que `check_segment` (noms réservés Windows, caractères interdits, point ou espace final), pas de `..` ni de chemin absolu, séparateurs `/` seulement, normalisation **NFC**, clé de collision = minuscules Unicode complètes du chemin NFC. Plafonds : 200 000 fichiers, 2 Gio par fichier, 200 Go au total. Pas de filtre d'extension (rien ne s'exécute depuis `world/`, mais le dossier ne s'ouvre jamais en double-clic automatique). Liens symboliques, points de jonction et fichiers hors de la liste : ignorés (`symlink_metadata`).

- **À la création** : un chemin invalide est exclu et signalé dans le champ `skipped` de l'événement ; en cas de collision de casse ou d'Unicode, le fichier au mtime le plus récent est gardé et l'autre est signalé. Un saut de taille de plus de 2 fois est signalé avant publication, avec les plus gros nouveaux dossiers.
- **À la réception** : refus strict d'une collection invalide ou en collision. `total_size` plus grand que 3 fois la tête précédente + 2 Gio est refusé sans confirmation (« La partie d'Alice fait 32 Go contre 6 Go avant ») ; l'espace libre est vérifié avant de tirer (reste à télécharger + 5 Go) ; si les octets reçus dépassent le total annoncé, la version est marquée `oversize` ; à la fin, la somme des tailles de la collection doit égaler `total_size`.

**Table d'exclusion unique** (partagée par la création, le staging et l'import) : `session.lock`, `*.tmp`, `*.mca.tmp`, `*.dat_new`, `playerdata/*-<chiffres>.dat`, `level<chiffres>.dat`, `*.sqlite-wal`, `*.sqlite-shm`, `world/data/DistantHorizons*.sqlite*`, `world/bluemap/**`, `world/dynmap/**`, `**/*.mca.bak`, `*.cl-tmp`. Les `*_old` sont **gardés** (le jeu s'en sert pour réparer, et ils se dédupliquent). Extensible par groupe (`extra_exclude`) et par le pack.

**Têtes et époque.**

- **Tête** : parmi les versions valides de l'époque courante, celles qui n'ont pas d'enfant **et** qui ne sont citées dans le `supersedes` d'aucune version valide connue. Une seule tête = état normal ; plusieurs = **fourche** (§ 8).
- `check_lineage` : le parent est connu ; chaque version de `supersedes` est connue et de la même époque ; un descendant de la base effective (ci-dessous) est accepté même s'il porte l'ancienne époque.
- **Base effective d'un `ContinueWithout`** : calculée de façon déterministe à la finalisation. C'est la tête la plus avancée parmi les descendants de la base votée dont **aucun n'a l'un des absents pour auteur**. `heads()` et `check_lineage` traitent ces descendants (signés avec l'ancienne époque, non réétiquetables) comme appartenant à la nouvelle époque. Les versions des absents postérieures à la base restent stockées comme branches restaurables, jamais tête toutes seules. Test pur : versions v41 à v44 jouées pendant le vote.
- Avant de signer un vote de base, `Proposal::vote` exige que la base soit présente localement (en-tête et collection complète) et soit une feuille de l'époque courante ; l'écran affiche date, auteur, `played_secs` et le temps de jeu perdu par rapport à la tête locale (confirmation explicite s'il est positif). Une base restée inconnue 7 jours après l'opération permet à tout membre de proposer un nouveau `ContinueWithout` sur la même époque.

**Règles d'insertion** (`insert`, et à la réception) :

1. **Appartenance ancrée** : l'auteur doit avoir été membre à `op_seq` (`member_since ≤ op_seq ≤ member_until`). Une version d'un ex-membre dont l'`op_seq` est postérieur à son retrait n'est acceptée que si elle est ancêtre d'une version d'un membre courant. Retirer un membre n'invalide donc pas son historique.
2. **Pack** : `pack_hash` doit correspondre à `pack_history` pour la position de la version (une version dont le parent descend de la `base_version` d'un `PackUpdate` porte le nouveau pack, toute version antérieure l'ancien) ; sinon `PackMismatch`.
3. **Quotas à la réception**, selon l'heure locale de réception et non `created_at` : au plus 30 versions par heure et par auteur (le surplus est mis en attente) ; plafond par auteur (5 000), au-delà on supprime les plus anciennes versions **non protégées** de cet auteur au lieu de refuser.
4. **Restore** : n'est affichée et ne devient tête qu'une fois la source présente localement et la collection vérifiée égale ; sinon elle reste en attente. Restaurer une version qui n'est ni la tête ni son parent passe par une proposition (majorité des actifs, ou le créateur sous `Creator`). Résoudre une fourche est autorisé au titulaire du bail (§ 8).
5. **Attente** : une version bien signée rejetée pour une raison d'attente (`FutureEpoch`, `NotMember`, `ParentUnknown`) va dans `pending/` (1 000 en-têtes, 7 jours), déclenche `GetOps` puis `GetVersions { ids: [parent] }`, et est rejouée à chaque opération appliquée et chaque insertion. `insert` est idempotent.
6. **Contrôle de contenu** léger, en tâche de fond, à la création et à la réception : `level.dat` se décompresse et contient `Data` ; chaque `.mca` a une taille multiple de 4 096 (ou nulle) et chaque entrée de sa table mène à un en-tête de chunk dont longueur et compression (1 à 4) sont valides ; les `playerdata/*.dat` se décompressent. Résultat dans `versions/<id>.check.json`. Une version qui régresse (valide chez le parent, invalide ici) est signalée avec « Restaurer v<parent> » et protégée du GC avec son parent sain.

### 3.4 Bail d'hôte (`p2p/src/hearth/lease.rs`)

```
SignedLease = { format, group_id, epoch, holder: AccountKey, device_id, lease_id (16 octets), seq,
                base_version, base_number, phase: Starting | Ready, ttl_secs: 90, sig }
              domaine b"cubelink/hearth/lease/1"      MAX_LEASE_TTL_SECS = 90
```

**Validité** (`validate`) : l'émetteur direct est le titulaire (`holder` et `device_id` = `conn.remote_id()` en direct) ; le titulaire est membre actif ; `epoch` égale l'époque courante (une époque inférieure est rejetée) ; `base_version` est la tête locale ou un descendant connu ; `base_number` n'est qu'un champ redondant et le bail est **rejeté s'il contredit l'en-tête local de `base_version`** (le numéro et l'époque viennent toujours de l'en-tête local) ; une base inconnue est d'abord synchronisée.

**Durée et renouvellement.**

- Le titulaire renvoie son bail toutes les 30 s aux membres en ligne, en `phase: Starting` dès qu'il a choisi d'héberger (le démarrage d'un gros pack prend 2 à 6 min), puis `Ready`. Chaque récepteur le garde `ttl` à partir de sa réception (pas d'horloge partagée).
- Un bail **relayé** (dans `Status`, avec son âge local `age_secs`) est accepté pour `ttl − age_secs − 5 s`, l'âge étant borné à 90 s.
- Un bail sans version publiée est ignoré après 12 h.
- **Identifiant de bail** : `lease_id` est un nonce par partie. `LeaseRelease` porte `lease_id` et `last_seq` ; le récepteur garde une **pierre tombale** `lease_id vers last_seq` 10 min et ignore tout `Lease` de ce `lease_id`. Hors pierre tombale, tout `seq` inférieur ou égal au dernier vu est ignoré. Un récepteur qui a vu le bail remplacé refuse un renouvellement du même `lease_id` (`Observed::Superseded`).
- **« Vivant »** : le titulaire ne renouvelle que si le processus serveur existe **et**, soit la phase est `Starting` avec une ligne de sortie depuis moins de 60 s et moins de 15 min écoulées, soit la phase est `Ready` et le port accepte des connexions. Si le serveur meurt : `LeaseRelease { reason: Crashed }` immédiat avec le dernier point de reprise.
- **Preuve de service** : après 2 échecs de connexion au tunnel en 3 minutes, le bail de ce titulaire est ignoré 15 minutes localement ; l'interface propose « Héberger quand même » (fourche visible).

**Fin de partie.** `LeaseRelease { group_id, lease_id, last_seq, version?, reason: Stopped | Handover | Shutdown | Suspend | Crashed { mod_hint }, replicated_to?, successor? }`. Pour `Stopped` et `Handover`, `replicated_to` liste les membres qui ont confirmé la collection complète (`Stored`, § 4) ; sans eux, l'hôte voit « Envoi du monde à Bob… ne pas éteindre » et l'arrêt de Windows est retenu pendant l'envoi. Pour `Shutdown` et `Suspend`, il n'y a pas le temps d'envoyer : `replicated_to` est omis, et la version est publiée au démarrage suivant.

**Prise du bail en deux phases.**

1. `LeaseClaim { lease_id, holder, device_id, base_version, epoch }` est envoyé en parallèle à tous les membres joignables (5 s pour établir une connexion, relais compris). Chaque témoin répond `ClaimAck` ou `ClaimConflict { bail ou claim déjà vu }`. Il garde le **premier** claim comme verrou jusqu'au premier renouvellement de ce bail en phase `Starting` (à défaut, 90 s après le claim). Un témoin qui reçoit deux claims applique le départage ci-dessous et répond `ClaimConflict` au perdant.
2. **Quorum** : si les acquittements (dormants exclus, le candidat compte pour lui-même) couvrent la **majorité stricte des membres actifs**, le bail est **sûr**. Sinon il est **au mieux** : l'interface le dit, une confirmation explicite est demandée (« si un autre membre joue de son côté, il y aura deux versions à départager »), et une fourche reste possible mais visible. Un témoin muet compte comme injoignable. Un candidat seul ne dépasse jamais « au mieux ».
3. **Conflit découvert après le démarrage** : si un titulaire déjà démarré apprend un bail concurrent gagnant, et qu'aucun joueur distant n'est connecté, il fait `stop` sans publier de version et rejoint ; sinon il continue et sa fin de session est marquée fourche explicite.

**Départage** de deux candidats (jamais par le seul `number`) :

1. si la base de l'un descend de celle de l'autre dans le graphe **local**, la descendante gagne ;
2. si les bases sont sur deux branches, c'est une fourche : gagne le candidat dont la base est la tête commune vue par la majorité des témoins qui ont répondu, puis la plus petite clé ; l'interface marque la fourche ;
3. à base égale, la plus petite clé.

Un perdant dont la base n'est pas un ancêtre-ou-égal de la base gagnante passe d'abord par Synchroniser. Tests purs sur ces cas.

**`decide`** reçoit un `ProbeRound { réponses, injoignables, durée }` complet, jamais le cache seul. La ronde est une `Hello` en parallèle à tous les membres (4 s par membre, connexions chaudes réutilisées) ; l'interface affiche « Recherche de la partie… ». Le cache sert à précharger les connexions (Hello à tous à l'ouverture de Cubelink et au réveil). Ordre des règles :

1. **Prérequis bloquants** : compte Microsoft requis (profil hors ligne ou jeton non rafraîchissable), `NeedsUpdate`, espace disque insuffisant (§ 7.4), serveur à préparer (`Prepare`, § 6.2).
2. un bail valide d'un autre membre (ou d'un autre appareil de **mon** compte) est connu : **Rejoindre**(titulaire), avec les états `Unreachable { holder }` (« La partie d'Alice continue mais ton PC ne la joint pas ») et « Alice démarre le serveur (2:10) » ;
3. un bail expiré sans `LeaseRelease` : `Unclean`, après interrogation des membres en ligne (si l'un voit un bail valide, rejoindre) et deux sondes `Hello` directes échouées à 10 s d'écart ;
4. un membre a une tête que je ne connais pas : **Synchroniser** (en-têtes puis blobs), seulement si au moins un pair en ligne a la version **complète** ; sinon « Attendre Alice » ou « Jouer depuis la dernière version disponible » (fourche explicite) ;
5. après synchro, plusieurs têtes : `Fork { heads }`, jamais un Héberger silencieux ; une tête marquée `crash_on_start` par au moins un membre : « La dernière partie fait planter le serveur » ;
6. je ne peux pas héberger (capacité, § 6.6) et aucun bail n'existe : `NeedHost` (« demande à Bob ») ;
7. **Mode réseau** : `Offline` (pas d'Internet) bloque ; `Filtered` (UDP bloqué, relais en 443 seulement) affiche un message dédié et force le relais ; `Alone` (Internet, aucun membre joignable) autorise **Héberger après confirmation**, sauf si un `Status` de moins de 24 h montrait une version plus récente indisponible ;
8. sinon **Héberger** : prise du bail en deux phases.

**Auto-exclusion (fencing).** Avant chaque renouvellement, comparer l'écart d'horloge murale (`SystemTime`) et l'écart monotone depuis le dernier renouvellement acquitté. S'il dépasse 60 s, ou sur `PBT_APMRESUMEAUTOMATIC`, ou si plus de `ttl` s se sont écoulées : **ne pas renouveler**, lancer une ronde `Hello`. Si un autre bail ou une version plus récente existe, faire `save-all flush` puis `stop` et publier la session comme branche (fourche visible) sans bail ; sinon continuer avec un nouveau `lease_id`. Sur `PBT_APMSUSPEND`, tenter `save-all` et `LeaseRelease { reason: Suspend }` en 2 s au mieux. Tant que le serveur tourne, `SetThreadExecutionState(ES_SYSTEM_REQUIRED)` évite la veille par inactivité (réglage `hearth_block_sleep`). Tout écart de plus de 60 s entre deux renouvellements est journalisé.

**Passation.** `Handover { lease_id, to, version }` : le titulaire sortant fait un dernier point de reprise et l'envoie. Le successeur tire la collection et répond `HandoverReady` une fois complète. Le sortant arrête son serveur et envoie `LeaseRelease { reason: Handover, successor, version }`. Le successeur annonce un bail avec `prev_lease_id`. Les témoins traitent ce bail relâché comme **réservé au successeur 60 s** (refus des `LeaseClaim` des autres) ; passé ce délai, la réservation tombe. Le choix du successeur privilégie un membre connecté `host_ready` avec assez de RAM (§ 6.6).

**Un seul bail par appareil.** Héberger un second groupe exige d'arrêter ou de passer la main (« Tu héberges déjà “Les Copains” »).

**DHT (phase 6).** La DHT ne sert qu'à la **découverte** (adresses des membres, indice de tête : `head_id`, `number`, `holder?` avec un âge en minutes). Un bail lu en DHT veut seulement dire « sonder ce titulaire en direct » : il ne vaut jamais « bail valide », et son absence vaut « inconnu », pas « libre ». Republication toutes les 30 min et au changement de tête, jamais à chaque renouvellement. Enregistrements : un par membre, signé par une paire ed25519 dérivée `HKDF(group_key, "cubelink/hearth/dht/1" ‖ member_key ‖ key_epoch ‖ jour UTC)`, contenu chiffré (XChaCha20-Poly1305, clé dérivée de même façon), signé à l'intérieur par l'identité du membre, ne contenant que `{tête, device du titulaire, relay_url}` et **jamais d'adresse IP directe**. Interroger les cibles de tous les membres par au moins 2 chemins d'amorçage distincts. Une tête reçue par DHT n'est crue qu'après vérification de la signature de la version, puis synchro directe.

**Limite connue.** Le bail n'est pas une exclusion parfaite : en cas de partition, deux candidats peuvent joindre deux ensembles disjoints de témoins. Seule l'intersection de quorums protège (d'où « sûr » et « au mieux »). La chaîne de versions rend toute fourche visible.

### 3.5 Clé de groupe

La **clé de groupe** (32 octets) chiffre les points de reprise confiés aux gardiens et dérive les identifiants DHT. Elle n'est jamais dans une opération. Elle est transmise à chaque membre sur la connexion QUIC authentifiée et stockée localement (`group.keys` : `key_epoch` vers clé, les anciennes clés ne sont jamais supprimées).

- Le signataire qui finalise un `RemoveMember` génère la nouvelle clé ; l'opération porte `key_commit = BLAKE3("cubelink/hearth/key/1" ‖ clé)` et `key_epoch` (compteur), qui ne révèlent rien. Après un `Leave`, le premier membre actif qui l'applique signe `RotateKey`.
- Tout membre qui détient la clé dont l'empreinte correspond la transmet par `GetGroupKey { key_epoch }`, servi aux membres courants seulement. Un membre qui n'a pas la clé courante n'écrit **rien** chez les gardiens (l'outbox attend), et l'interface affiche « clé du groupe en attente ». Une clé perdue se redemande à un membre.
- `GroupInvite` n'envoie la clé qu'une fois l'`AddMember` présent dans la chaîne envoyée ; l'invité vérifie qu'il est membre après `replay(ops)` avant d'écrire `group.keys`.
- Un membre retiré garde une copie du monde jusqu'à la date du retrait : l'interface le dit (« Eve garde une copie du monde jusqu'à aujourd'hui ; elle ne recevra plus rien »).

## 4. Protocole réseau `cubelink/hearth/1` (`p2p/src/hearth/proto.rs`)

Même discipline que `invite.rs` et `presence.rs` : l'émetteur est `conn.remote_id()` (clé d'appareil, vérifiée par son `DeviceCert`), messages JSON bornés (64 Kio), un flux bidirectionnel par requête, délai 10 s, non-membre refusé sans réponse (sauf `GroupInvite`, accepté d'un ami). Un message de format trop récent reçoit `Unsupported { need }`. Trois ALPN : `cubelink/hearth/1` (messages), `cubelink/hearth-blobs/1` (blobs du monde, § 5.3), `cubelink/hearth-fwd/1` (relais applicatif de membre, phase 6).

| Requête | Réponse | Usage |
|---|---|---|
| `Hello { group_id, op_seq, op_hash, heads, device_id, app_version, proto_max, addrs }` | `Status` | ronde de sondage (au clic sur Jouer, à l'ouverture, au réveil) et sonde périodique (60 s ; 5 min vers les non-amis et sur batterie avec économiseur) |
| `GetOps { group_id, after }` | `Ops [...]` | rattraper la chaîne d'opérations |
| `GetVersions { group_id, ids }` (ou `after_number` comme raccourci) | `Versions [...]` | rattraper les en-têtes en remontant les parents (10 000 par requête paginée) |
| `GetProposals { group_id }` | `Proposals [...]` | découvrir les propositions ouvertes |
| `LeaseClaim { … }` | `ClaimAck` ou `ClaimConflict { … }` | première phase du bail |
| `Lease { signed_lease }` | `Ack` | annonce ou renouvellement |
| `LeaseRelease { … }` | `Ack` | fin de partie (§ 3.4) |
| `Handover { lease_id, to, version }` | `HandoverReady` | passation |
| `Announce { group_id, version }` | `Ack` | annoncer une version (en-tête signé) |
| `Have { group_id, version_id }` | bool | la collection est-elle complète chez toi ? |
| `Stored { group_id, version_id }` | `Ack` | confirmation durable : collection tirée, tag posé, fsync fait |
| `GetGroupKey { group_id, key_epoch }` | `Key` | clé de groupe, aux membres courants |
| `GetServerMods { group_id }` | liste | mods annoncés par le serveur du titulaire (contrôle contre le pack, § 6.2) |
| `HostRequest { group_id }` | `Ack` / refus | demander à un membre en ligne capable d'héberger |
| `Present { group_id, proposal_id, sig }` | `Ack` | veto de l'absent |
| `GroupInvite { ops, group_key }` | `Ack` | invitation au groupe (d'un ami seulement) |
| `Propose { proposal }` / `Vote { proposal_id, sig }` | `Ack` | gouvernance asynchrone |

`Status` répond : `{ op_seq, op_hash, heads, complete_heads, lease? (signé, avec age_secs), open_proposals, seen counters, ready: { pack, server, eula, head_complete }, capacity { total_ram, server_ram_ok, can_host }, rtt et type de chemin par pair, crash_on_start, signed addrs, McProfileClaim, app_version }`. Seules des versions **complètes localement** apparaissent dans `complete_heads` ; `heads` est le simple état du graphe. Le **carnet d'adresses de groupe** est un ensemble de `SignedAddr { key, device_id, addr, issued_seq, sig }`, réémis à chaque changement d'adresse, repris dans `GroupInvite`, `AddMember`, `Hello` et `Status`, stocké dans `peers.json` et injecté dans l'endpoint ; les membres non amis sont sondés par l'ALPN Hearth (le service de présence refuse les non-amis). La synchronisation des en-têtes se fait par identifiants (têtes inconnues, remontée des parents) avec un résumé horaire (BLAKE3 de la liste triée des ids par époque) pour détecter tout écart.

**Ordonnanceur de débit.** Un seul ordonnanceur dans le p2p. Pendant une session active (hôte ou joueur) : priorité absolue au tunnel ; blobs Hearth plafonnés par seau à jetons à **25 %** du débit montant mesuré (minimum 256 Kio/s) ; outbox des autres groupes et service de mods en pause. Hors session, le fond est plafonné à 50 % du débit mesuré. Les joueurs **tirent** les blobs, l'hôte ne les pousse pas. `PathObserver` mesure `rtt` et le type de chemin ; le ping de chaque joueur est affiché, et la passation est proposée si un joueur dépasse 200 ms via relais alors qu'un autre membre a des chemins directs vers tous.

**Réseau limité.** Sur connexion limitée ou itinérance (`GetConnectionCost`), seuls opérations, en-têtes et baux se synchronisent ; les blobs sont différés (« Synchronisation en pause (connexion limitée) », bouton « Synchroniser quand même »). Un Jouer explicite télécharge toujours, taille annoncée avant.

**Relais** (phase 6, `p2p/src/relay.rs`). Un nœud ne s'annonce relais que s'il réussit un auto-test (connexion entrante d'un pair tiers sur l'adresse annoncée : IPv6 globale ou port mappé UPnP, PCP ou NAT-PMP), idéalement sur 443/TCP. Tant que le pool est vide, repli explicite (relais public ou relais auto-hébergé d'un membre, réglage `RelayChoice`), montré comme tel ; l'absence de chemin est une raison distincte (`Reach::NoPath`). `cubelink/hearth-fwd/1` : un membre M qui joint les deux côtés colle deux flux QUIC (A, M, B), entre membres d'un même groupe, 2 sessions au plus. L'état « Partie en cours chez Alice, injoignable depuis ton réseau » (lecture du net-report iroh) est distinct de Bloqué, dès maintenant.

## 5. Stockage local

### 5.1 Arborescence

Sous le dossier local de l'application (pour le paquet MSIX : `ApplicationData.LocalFolder`, plus court et non virtualisé ; jamais sous OneDrive, refusé avec un message) : `hearth/<8 premiers hex du group_id>/` (table complète dans un fichier d'index, pour raccourcir les chemins Windows ; tout accès passe par un utilitaire de chemins longs `\\?\`).

- `FORMAT`, `.lock` (verrou d'instance, à la racine `hearth/`) ;
- `ops.jsonl` (chaîne d'opérations ; `ops_conflicts.jsonl`), `proposals/` (avec `created_at` et expiration), `group.keys`, `peers.json`, `pack_accepted.json` (acceptations de pack par `manifest_hash`) ;
- `versions/<id>.json` (en-têtes signés) avec `received_at` local, `<id>.check.json` ; `pending/` (§ 3.3) ;
- `w/` : le monde matérialisé de la tête, `world.index.json` (chemin vers `(blake3, taille, mtime)` de la version matérialisée), `materialize.json` (`{ cible, état: en_cours|fait }`), `.cubelink-session` (marqueur, § 7.5) ;
- `staging/<n>/` : copies des fichiers modifiés d'un point de reprise ;
- `outbox/<id>.json` : un fichier par version à pousser, avec la liste des membres ayant confirmé ;
- `server/<pack_hash>/` : le serveur dédié installé, avec `installed.json`, `stats.json`, journaux par session `logs/<version_id>.log.gz` (10 dernières) ;
- `server_state/` (§ 6.5) ;
- `events.jsonl` (rotation à 5 Mo, 3 fichiers, § 9) ;
- `blobs/` : le magasin iroh-blobs **propre à Hearth** (§ 5.3).

### 5.2 Écritures durables

Tout fichier de métadonnées (`group.keys`, `versions/*`, `outbox/*`, `proposals/*`, journal) s'écrit par fichier temporaire, `fsync`, renommage, `fsync` du dossier. `ops.jsonl` : `fsync` après chaque ajout ; au chargement, une dernière ligne tronquée est ignorée et retirée (et redemandée par `GetOps` à un membre) ; une ligne invalide au milieu est une erreur et la chaîne repart de `GetOps` depuis 0. Toutes les opérations de fichiers du monde, du staging et de l'outbox passent par `retry_io` : erreurs Windows 32, 33 et 5 relancées jusqu'à 10 fois, recul exponentiel de 50 ms à 2 s, ouverture en lecture avec partage `READ|WRITE|DELETE`. Un fichier qui reste verrouillé **abandonne ce point de reprise** (réessayé au cycle suivant) : on ne reprend jamais le blob de la version précédente pour un fichier verrouillé, ce qui mélangerait deux états.

**Publication atomique d'une version**, dans cet ordre : (1) importer les blobs et créer la collection ; (2) poser le tag persistant ; (3) écrire l'en-tête (temporaire, `fsync`, renommage, `fsync` du dossier) ; (4) écrire l'entrée d'outbox de la même façon. Au démarrage, chaque version locale est vérifiée (collection complète) ; une version incomplète est marquée `incomplete`, jamais annoncée, à réparer.

### 5.3 Magasin de blobs et accès

Hearth a son propre `FsStore` (`hearth/blobs/`), son propre `GcConfig` et son propre ALPN : il ne partage ni le GC ni le gestionnaire du transfert de mods (un magasin corrompu ne touche plus les mods). Les jars du pack restent dans le magasin des mods (pas de double stockage).

- **Tags** : chaque version a un tag persistant `hearth/<g>/<version_id>` sur sa collection. Un `TempTag` n'est **pas** persistant : il ne sert qu'entre l'import et la pose du tag. Un téléchargement en cours pose un tag `hearth/<g>/pending/<vid>` sur la `HashSeq` dès le début du transfert et le supprime quand la version est complète et taguée (les blobs partiels survivent ainsi au GC et au redémarrage).
- **Autorisation par hash** : un `ProtocolHandler` iroh-blobs avec hook de requête (`EventSender`, mode intercept sur `GetRequest`) vérifie, pour chaque hash demandé, qu'il appartient à une collection du groupe G dont le pair est membre actif. Un index `blob_hash vers {group_id}` est tenu à l'import. Refus explicite sinon. Test : un membre de G1 ne récupère pas un blob propre à G2, ni un blob postérieur à son retrait.
- **Interdiction des liens durs** vers le magasin : le serveur réécrit les `.mca`, `level.dat` et `playerdata` en place, ce qui détruirait le blob (§ 7.1). Import toujours en `ImportMode::Copy`, export en `ExportMode::Copy` ; les magasins de fichiers passent en lecture seule.

### 5.4 Rétention (`p2p/src/hearth/store.rs`) : une seule politique

1. **Ensemble protégé** : jamais supprimé par l'âge ni par le quota, seulement par une action explicite de l'utilisateur. Il contient : les têtes de l'époque courante, la base de chaque `ContinueWithout`, la pointe de chaque branche de côté ou d'absent, les sources de `Restore`, les sauvegardes de `PackUpdate`, les entrées d'outbox non confirmées, la dernière version saine avant une régression (§ 3.3 règle 6) et les `Orphan` locales.
2. **Paliers** pour le reste : tout ce qui a moins de 48 h ; puis seulement le dernier point de reprise de chaque session et toutes les `Session` pendant 14 jours ; puis une version par jour jusqu'à 60 jours et une par semaine jusqu'à 6 mois.
3. **Budget par groupe** : le plus petit de 10 Go et de 10 % du disque libre. Il n'élague que **hors** de l'ensemble protégé ; si l'ensemble protégé dépasse seul le budget, on prévient sans rien supprimer. Écran « Stockage » par groupe (taille, « Libérer de l'espace »).
4. **L'âge** se calcule sur `received_at` (heure locale de réception), jamais sur `created_at`.

L'outbox garde au plus **un** `Checkpoint` par lignée (le plus récent, remplacé par le suivant non confirmé) plus la `Session` finale.

## 6. Serveur dédié (`core/src/hearth/server.rs`)

L'hôte lance un **serveur dédié** en arrière-plan, et tous les joueurs (hôte compris) s'y connectent par le tunnel. Raisons : pas d'« Ouvrir au LAN », le serveur survit au départ du joueur hôte, données des joueurs toutes dans `playerdata/`, et sauvegarde pilotable.

### 6.1 Installation, lancement, supervision

- Installation par loader : vanilla (jar serveur Mojang), Fabric (lanceur serveur `meta.fabricmc.net`), Quilt (installeur `install-server`), Forge et NeoForge (installeur `--installServer`). Le serveur est matérialisé depuis la **même collection de pack** que le client (un serveur qui diffère selon l'hôte est exclu), dans `server/<pack_hash>/` avec un marqueur `installed.json`. Un ancien `server/<pack_hash>` n'est supprimé qu'une fois le nouveau prêt.
- **Lancement direct de Java**, jamais `run.bat` (qui casse le suivi par PID) : `java @user_jvm_args.txt @libraries/.../win_args.txt nogui` pour Forge et NeoForge, le jar pour vanilla, Fabric et Quilt. Le répertoire de travail est le dossier du serveur, les chemins des fichiers d'arguments sont relatifs (Java lit ces fichiers dans la page de code ANSI). Avant l'installation, le chemin absolu est vérifié contre la page de code ANSI (`WideCharToMultiByte` avec `WC_NO_BEST_FIT_CHARS`) ; sinon nom court 8.3 (`GetShortPathNameW`) ou racine Hearth choisie par l'utilisateur. Test en CI Windows avec un dossier nommé « Дима é ».
- `eula.txt` n'est écrit qu'après l'acceptation de l'EULA Minecraft, demandée **à l'acceptation de l'invitation** (une fois, dans l'app), jamais en plein jeu.
- **Superviseur `cubelink-hearthd`** (binaire annexe, `core/src/bin/`). Java tourne sous ce petit superviseur, qui possède l'entrée standard du serveur, survit à Cubelink et expose un tube nommé local (socket Unix sous Linux), accessible à l'utilisateur seul (ACL). Cubelink y envoie toutes les commandes (`save-off`, `save-on`, `save-all flush`, `list`, `say`, `tellraw`, `kick`, `whitelist reload`, `stop`). **RCON reste désactivé** (`enable-rcon=false`). Si Cubelink ne revient pas dans les **2 minutes** (ou tout de suite si aucun joueur n'est connecté), le superviseur fait `save-on`, `save-all flush`, `stop`, puis écrit un marqueur d'arrêt propre. Au redémarrage, Cubelink réadopte le superviseur par son PID et sa ligne de commande (`server/hearth-pid.json`). Un Job Object `KILL_ON_JOB_CLOSE` est **proscrit** : il équivaut à un crash (pas de `stop`, parfois `save-off` encore actif). Fermer l'entrée standard ne suffit pas non plus : la console du serveur s'arrête sur EOF sans arrêter le serveur.
- Le serveur lit sa sortie dans une tâche dédiée (jamais de tube plein). Prêt quand la sortie contient `Done (`, avec un délai de **15 minutes** ; la durée réelle est mesurée (`server/stats.json`) pour afficher une estimation.
- **Priorité** : sur le PID Java, `SetProcessInformation(ProcessPowerThrottling)` avec `StateMask = 0` désactive EcoQoS, et priorité `ABOVE_NORMAL`.
- Windows : le serveur est lancé sans console (`CREATE_NO_WINDOW`). Sous Linux : inhibiteur logind (`delay`) et gestion de `SIGTERM`.
- Le chemin sous MSIX est plus long (§ 5.1) ; ne jamais afficher ni ouvrir un chemin avant de l'avoir résolu (`GetFinalPathNameByHandleW`).

### 6.2 Pack, mods et instance

**`HearthPack` canonique et versionné**, publié comme collection iroh-blobs (même contrôle d'accès « membre » que le monde) :

```
HearthPack = { format: 1, minecraft, loader, java_major,
               files: [{ path, size, blake3, sha1?, side: both | client | server }],
               persist?: [chemins], exclude?: [motifs] }
```

Il ne contient que les fichiers qui comptent pour le jeu (mods, config, kubejs, defaultconfigs). `PackPin = { pack_collection, pack_hash = BLAKE3(HearthPack canonique), minecraft, loader }`. L'empreinte n'est **jamais recalculée depuis une instance locale** : on compare au fichier reçu (le `manifest_hash` actuel n'est pas stable entre pairs). Le champ `side` remplace toute liste `server_excluded` séparée.

- **Classement des jars** (`core/src/mod_meta.rs`, `mods.rs`), première réponse sûre : (1) métadonnées du jar (Fabric `environment`, Quilt `minecraft.environment`, `clientSideOnly` des `mods.toml`, `displayTest="IGNORE_ALL_VERSION"` comme indice) ; (2) `env.server` du mrpack, conservé dans `Provenance.server` ; (3) API Modrinth `version_files` par sha1, et API CurseForge `gameVersionTypeId` si une clé est présente ; (4) liste intégrée. En cas de plantage au démarrage (`invalid dist DEDICATED_SERVER`, cible Mixin `net.minecraft.client.`), analyse de `logs/latest.log` et `crash-reports/`, jar identifié, exclu, une seule relance, puis exclusion enregistrée par une `PackUpdate` mineure pour que tous les hôtes se comportent pareil.
- **Mods serveur seulement** : à l'import, les fichiers `client: unsupported` sont conservés sous `server_only/` avec `side: server`, installés seulement dans `server/`. Pour CurseForge, quand `serverPackFileId` existe, le server pack complète le HearthPack à la création.
- **Instance de groupe gérée** `InstanceKind::Hearth { group_id }` (sur le modèle de `guest.rs`), matérialisée depuis la collection du pack. Bascules, mises à jour et ajouts de mods y sont désactivés. Avant chaque lancement du client, vérification des fichiers contre le HearthPack (cache taille + mtime, rehachage si la date a changé) et réparation sans demander.
- **Surcouche `client_extras/`**, hors pin : `resourcepacks/`, `shaderpacks/`, `options.txt` et jars classés client seulement. Un mod qui ajoute des registres ou n'est pas classé client est refusé avec un message ; rien n'y va vers `server/`.
- **Avant chaque démarrage** : vérifier les hashs de `server/mods` contre le pack (moins les fichiers `side: client`) ; refuser de démarrer si `version.header.pack_hash` ne correspond pas au pin sans `PackUpdate` ; comparer `Data.DataVersion` de `level.dat` à celle du jar serveur (refus si supérieure) ; vérifier la liste signée de mods vulnérables connus (famille BleedingPipe, etc.) : avertissement bloquant et proposition d'ajouter PipeBlocker ou la version corrigée. Après le démarrage, chercher « Missing registry », « Unidentified mapping », « Detected missing registry entries » : s'il y en a, `stop` **sans** point de reprise ni `Session`, et afficher le mod en cause. Côté joueur, la liste de mods annoncée par le serveur (handshake Forge/NeoForge, ou `GetServerMods` pour Fabric) est comparée au pack, et la connexion est refusée si elle diffère.
- **`PackUpdate { pack, base_version, major }`** :
  - jamais installée toute seule : chaque membre voit « Nouveau modpack proposé par X » avec le **diff calculé par lui-même** sur la collection publiée (mods ajoutés ou retirés, source de chaque jar via `unknown_mods.rs`, scripts `kubejs/`, `defaultconfigs/`, `config/` modifiés). Acceptation explicite, mémorisée par `manifest_hash` dans `pack_accepted.json` ; sans elle, Jouer devient « Vérifier le nouveau modpack ». Un pack dont la collection est inaccessible ne s'applique pas. Le même écran s'applique au pack d'un `GroupInvite` ;
  - un jar inconnu passe par `trusted_mods.json`, membre par membre, jamais hérité du groupe ;
  - seuil de majorité décidé **à l'application**, sur le diff vérifié : majorité stricte si le diff ajoute un jar inconnu ou un script, **ou** si la mise à jour est « majeure » (`minecraft` ou loader change, ou un mod présent dans les registres du monde est retiré : espaces de noms de `level.dat`/`Data.WorldGenSettings` et dossiers `dimensions/<ns>`), même sous `Creator` ;
  - avant une mise à jour majeure, la liste des dimensions modées qui contiennent des joueurs (`playerdata/*.dat`, `Dimension`) est affichée et confirmée ;
  - applicable seulement **sans bail valide connu** et depuis une **tête unique** (sinon « terminez la partie de Bob d'abord ») ; les versions de l'ancien pack postérieures à la base deviennent des branches de côté, « jouées avec l'ancien pack », jamais tête ; un `Restore` dont la source a un autre `pack_hash` que le pin n'est permis que dans une proposition composée `PackUpdate { pack_de_from } + Restore` ;
  - sauvegarde automatique avant application (rétention `PackUpdate`, § 5.4).
- **Config du pack** : `defaultconfigs/` et `serverconfig/` font partie du pack épinglé et sont **recopiés en écrasant** dans `world/serverconfig/` à chaque démarrage. À l'arrêt, `server/config`, `kubejs` et `defaultconfigs` sont comparés au pack (hors chemins de la liste `persist`) : s'il y a un écart, Cubelink propose au groupe une `PackUpdate` mineure (« Alice a changé 2 réglages du pack : les garder ? »), sinon le prochain démarrage les remet depuis le pin. Refus de démarrer si une clé sensible connue est assouplie (CC:Tweaked `http.rules`, OpenComputers `internet.filteringRules`, `webserver-bindaddress` de Dynmap ou BlueMap). Un changement de `world/datapacks/` entre deux versions apparaît dans le journal de modifications.

### 6.3 Propriétés du serveur et réglages

`server.properties` est **régénéré à chaque démarrage** depuis `GroupSettings`. Valeurs par défaut : `spawn-protection=0`, `allow-flight=true`, `max-tick-time=-1`, `view-distance=10`, `simulation-distance=8` ; `difficulty` et `hardcore` lus dans `level.dat` à la création ; `pvp` et `enable-command-block` réglables. Pour un monde neuf, `level-seed` et `level-type` (preset du pack) viennent de l'opération `Create`. **Clés forcées**, non modifiables : `online-mode=true`, `server-ip=127.0.0.1`, `server-port`, `level-name=w`, `white-list=true`, `enforce-whitelist=true`, `enable-rcon=false`, `enable-query=false`, `prevent-proxy-connections=false`.

Réglage d'appareil `hearth_can_host` (Auto, Jamais). Réglage de groupe `idle_stop_minutes` (défaut 5, de 1 à 30).

### 6.4 Identité Minecraft, liste blanche, opérateurs

- Chaque membre publie `McProfileClaim { group_id, mc_uuid, mc_name, claim_seq, sig }`, signé par sa clé Cubelink (domaine `cubelink/hearth/mcprofile/1`), créé au premier Jouer depuis le profil Microsoft actif et diffusé par `Hello` et `Status`. Un changement de compte est un nouveau claim, qui propose de renommer `playerdata`, `advancements` et `stats` de l'ancien identifiant vers le nouveau.
- À chaque démarrage, l'hôte écrit `whitelist.json` (UUID liés aux membres actifs) et `ops.json` (niveau 4 ou 2) depuis l'état du groupe (`GroupSettings.ops`, modifié par `SetSettings`) ; `ops.json`, `whitelist.json` et `banned-*.json` ne sont **jamais versionnés**. Un `/op` tapé en jeu est perdu au prochain démarrage : l'interface le dit. Après un changement de membres : `whitelist reload`.
- **Tunnel** : le côté hôte lit le paquet `Login Start` (non chiffré : nom, et UUID depuis la 1.19) de chaque flux TCP et ferme le flux si le nom ou l'UUID ne correspond pas au `McProfileClaim` de la clé QUIC qui le porte : un tunnel ne sert qu'à son propriétaire.
- **Adresse source** : chaque connexion vers le serveur part d'une adresse de boucle locale propre au membre (`127.1.X.Y` dérivée de sa clé, `bind` avant `connect` ; repli `127.0.0.1` sous macOS), ce qui rend `ban-ip` et la traçabilité utilisables. La correspondance adresse vers membre est journalisée.
- Compte hors ligne ou jeton non rafraîchissable : `decide` renvoie « Compte Microsoft requis » avant tout démarrage de serveur. Avant le lancement, le jeton est rafraîchi (`auth/refresh.rs`) et la session Mojang vérifiée par un GET du profil ; si elle est en panne : « Service de connexion Minecraft indisponible ».
- **Côté client**, adresse locale stable par groupe : `127.0.0.1` et un port dérivé du `group_id` (essayer d'abord ce port, puis 25565), plus une entrée nommée du nom du groupe dans `servers.dat` de l'instance de groupe. Le tunnel garde son écouteur ouvert et transmet le port, sans le relâcher avant usage (plus de course `free_port`). Les mods de carte gardent ainsi cartes et waypoints.

### 6.5 État du serveur hors de `world/` (`server_state/`)

Un seul mécanisme : le préfixe `server_state/` de la collection (§ 3.3). Il reprend la liste `persist` (relative à la racine du serveur), par défaut `schematics/uploaded/**`, `config/ftbquests/quests/**`, `config/luckperms/**`, `local/**`, complétée par `HearthPack.persist`. Ces fichiers sont restaurés avant le démarrage et réimportés à chaque point de reprise. Les fichiers de base de données (`*.mv.db`, `*.sqlite`, `*.db`) ne sont capturés **qu'à l'arrêt**, jamais à chaud. Les dossiers régénérables (bluemap, dynmap) sont exclus. `serverconfig/` n'en fait **pas** partie (config du pack, § 6.2). Les chemins de `persist` sont exclus de la comparaison de config du § 6.2. À la fin d'une première partie, les fichiers de `server/` modifiés pendant la session (hors `logs/`, `crash-reports/`, `libraries/`, `mods/`) et non couverts sont signalés pour proposer de les ajouter à `persist`.

### 6.6 Capacité, préparation, hébergement en arrière-plan

- **Préparation en fond** dès qu'un `GroupInvite` est accepté ou qu'une `PackUpdate` est reçue : collection du pack par les blobs (les fichiers CurseForge non redistribuables viennent des membres), Java requis (`java_install`), installation du serveur, EULA, tête complète. `Status.ready = { pack, server, eula, head_complete }` ; `decide` gagne l'état `Prepare` (progression affichée) avant Héberger. Toute demande d'action en cours de partie (EULA, RAM) passe par une notification Windows et un message en jeu, jamais par une boîte modale cachée. La passation affiche en jeu et dans la carte « Bob reprend la partie : démarrage du serveur 2/3 ».
- **RAM serveur** : `max(pack_min, 4 Go + 150 Mo × nb_mods / 100)`, ajustée ensuite par la mesure du tas après `Done (` (enregistrée dans les stats du groupe). On réserve `client_xmx + serveur_xmx + 3 Go` au système et on réduit au besoin le `Xmx` du client hôte ; la RAM tient compte des autres serveurs. `Status.capacity = { total_ram, server_ram_ok, can_host }` ; `hearth_can_host = Auto` vaut `Jamais` sous le seuil calculé.
- Sous le seuil, `decide` n'héberge pas : `NeedHost` (« Ton PC ne peut pas héberger ce modpack : demande à Bob, qui est en ligne et peut héberger »), bouton qui envoie `HostRequest` à un membre en ligne capable ayant activé « Héberger pour le groupe en fond » (il lance sur son PC sans y jouer). Sinon, blocage explicite avec les chiffres.

### 6.7 Exposition réseau, voix, plantages

- Avant le premier démarrage, surcharges de config connues : `bind-address` / `webserver-bindaddress = 127.0.0.1` pour Dynmap, BlueMap et Squaremap ; Simple Voice Chat désactivé tant que le tunnel n'a pas de transport UDP, avec `config/voicechat/voicechat-server.properties` forcé (`bind_address=127.0.0.1`, port fixe, `voice_host` vide). Interface : « Chat vocal non pris en charge ».
- Après `Done (`, lister les sockets en écoute du PID Java (`GetExtendedTcpTable` et `GetExtendedUdpTable` sous Windows, `/proc/net` sous Linux). Une écoute non loopback est signalée avec le mod probable (« BlueMap ouvre le port 8100 sur ton réseau ») et le serveur est arrêté faute de confirmation. **Jamais de demande UPnP** pour le serveur.
- Tourner sous des droits réduits n'est pas faisable simplement sous Windows : risque connu, documenté ici (un mod serveur vulnérable donne l'exécution de code à l'hôte).
- **Plantage au démarrage** : si le serveur s'arrête en erreur deux fois avant `Done (` ou dans les 2 minutes sur la même tête, ou dans les 3 minutes après le démarrage d'une version, celle-ci est marquée localement `crash_on_start` et diffusée dans `Status`. `decide` affiche « La dernière partie fait planter le serveur » avec « Restaurer la partie de 21 h » (`Restore` de la parente) et « Réessayer », et l'auteur de la tête.
- **Joueurs connectés** suivis par les lignes `joined the game` / `left the game` et `list` toutes les 60 s ; `stop` après `idle_stop_minutes` sans joueur. Une sortie précédée de `Stopping the server` est un arrêt propre, même si `/stop` vient d'un joueur.
- UDP (phase 6) : datagrammes QUIC (`send_datagram`) sur la connexion du tunnel, un mapping par port déclaré, écoute UDP locale côté joueur.

## 7. Cycle d'une partie

### 7.1 Jouer

`decide` (§ 3.4) après la ronde de sondage. Hearth reste désactivé par défaut (`hearth_enabled`, § 12.3) tant que les phases 4 et 5 et les gardes de plantage ne sont pas vérifiées en réel.

### 7.2 Héberger

1. Vérifications : prérequis du `decide`, espace libre (§ 7.4), compte Microsoft et session Mojang, serveur préparé.
2. **Prise du bail** en deux phases (§ 3.4). Le bail est annoncé dès ce choix, en `Starting`.
3. **Matérialisation de la tête** dans `w/`, sous la machine à états par groupe `Idle | Syncing | Hosting | Joining` (un `tokio::sync::Mutex` par groupe) : pas de matérialisation pendant `Hosting`, et Jouer attend la fin de `Syncing`. Seule une collection **complète** (`has_complete` vérifié sur tous les blobs) est matérialisée, depuis n'importe quelle source qui l'a. Copie ou clonage de blocs (reflink sous Linux/btrfs/xfs et macOS/APFS, `FSCTL_DUPLICATE_EXTENTS_TO_FILE` sous ReFS et Dev Drive, sinon copie), **jamais un lien dur**. Méthode **incrémentale**, fichier par fichier : écriture en `.<nom>.cl-tmp` puis renommage, suppression des fichiers retirés, journal `materialize.json` (« en cours » relancé au démarrage, « fait » en dernier), index `world.index.json` (BLAKE3, taille, mtime). La toute première matérialisation se fait dans `w.tmp-<id>/` puis un renommage de dossier ; au démarrage, tout `w.tmp-*` est supprimé. Avant de démarrer le serveur, si l'index ne correspond pas à la version attendue, les fichiers dont le tampon diffère sont re-vérifiés par hachage. **Jamais de matérialisation par-dessus un `w/` qui porte un marqueur de session** ou dont l'index diffère de la version matérialisée, sans l'avoir d'abord importé (§ 7.5).
4. Écriture du marqueur `.cubelink-session` (version de base, PID du serveur, heure), démarrage du serveur sous `cubelink-hearthd` (§ 6.1) et **lancement du client en parallèle** (`--quickPlayMultiplayer` vers l'adresse locale stable du tunnel, § 6.4). Les autres membres voient « Alice démarre le serveur (2:10) » et lancent leur client.
5. Ouverture de l'accès du tunnel aux membres (cible `Group(GroupId)`), bail en `Ready` à `Done (`.

### 7.3 Pendant la partie

- **Renouvellement** toutes les 30 s (règles de vie du § 3.4). Les boucles tournent en Rust.
- **Réplication continue** : chaque joueur connecté tire en fond les blobs de chaque `Checkpoint` (débit plafonné, § 4), pour que le dernier delta soit petit.
- **Serveur vide** : `stop` après `idle_stop_minutes` sans joueur, avec version `Session` et `LeaseRelease`.
- **Éviction** : si un `RemoveMember` ou `Leave` s'applique pendant la partie, voir § 2 (connexions fermées, `kick`, `whitelist remove`).

### 7.4 Points de reprise (toutes les 10 min)

Une seule séquence (`core/src/hearth/server.rs`, `world.rs`) :

1. Vérifier l'espace libre : au moins 2 fois la taille des fichiers modifiés pour le staging. Sinon `tellraw` et carte « Disque presque plein, la partie continue mais les points de reprise sont suspendus ».
2. `save-off`, `save-all flush`, attendre `Saved the game` **120 s au plus** ; sinon abandon du point de reprise.
3. Copier dans `staging/<n>/` **les seuls fichiers modifiés** (reflink si possible), en notant (taille, mtime, identifiant de fichier) avant et après chaque copie ; jusqu'à 3 essais par fichier dont le tampon a bougé, sinon abandon.
4. `save-on`, **garanti par une garde** (`Drop` ou `finally`) sur tous les chemins : panne, panique, délai dépassé, ENOSPC. La phase `save-off` se limite à la copie vers le staging.
5. **Hors `save-off`** : hachage BLAKE3 et import (`ImportMode::Copy` depuis le staging), validation légère (chaque `.dat` commence par l'en-tête gzip et se décompresse), nouvelle version `Checkpoint`, entrée d'outbox.

Règles : une seule empreinte, le BLAKE3 du blob (`FileStamp { size, mtime_ms, blake3 }`, rempli par le résultat de l'import) ; aucune version si aucun fichier n'a changé (même collection) ou si aucun joueur n'était connecté depuis le point précédent. Le flush est chronométré : s'il dépasse 5 s, l'intervalle passe à 20 puis 30 minutes, sans jamais s'allonger davantage ; l'autosave du jeu (5 min, non bloquant) tourne normalement entre deux. Annonce en jeu avant le flush (`tellraw @a`, désactivable). Un point de reprise abandonné est journalisé et retenté au cycle suivant. **Disque** : sondé toutes les 30 s ; sous 3 Go : avertissement dans le jeu (`say`) et points de reprise suspendus ; sous 1,5 Go : `save-all flush`, `stop`, puis publication de la `Session` depuis `w/` (import par hachage sans copie si le staging est impossible). Avant d'héberger : monde + 2 fois la taille moyenne d'un point de reprise + 2 Go libres, sinon Bloqué avec « Libère X Go ».

### 7.5 Fin de partie, plantage, arrêt de la machine

- **Arrêt propre** (`stop` par l'utilisateur, par le serveur vide, ou passation) : version `Session` (import depuis `w/`), `LeaseRelease` avec `replicated_to`, outbox. Le marqueur `.cubelink-session` n'est supprimé qu'**après** le `stop` propre et la publication de la `Session`.
- **Fin non propre** (plantage du serveur ou du PC, Cubelink tué) : au démarrage de Cubelink, si le marqueur est présent et qu'un serveur orphelin vit encore (PID et ligne de commande), on se rattache au superviseur ; sinon `w/` est contrôlé (`level.dat` se décompresse, en-têtes `.mca` cohérents : `décalage × 4096 + longueur ≤ taille`) puis **importé comme version `Orphan` locale** (protégée du GC, jamais annoncée ni tête). Le contrôle de contenu (§ 3.3, règle 6) tourne dessus, puis l'hôte choisit « Publier la fin de partie (N min) » (nouvelle `Session`) ou « La mettre de côté ». **Rien n'est publié automatiquement** : cela propagerait un chunk qui fait planter le serveur. Pendant ce temps, les autres joueurs jouent depuis le dernier point de reprise, ce qui crée une fourche explicite. Le rapport de plantage (`crash-reports/`) et le mod probable sont joints.
- **Arrêt de Windows** (déconnexion, extinction, redémarrage, Windows Update) : une **fenêtre cachée de premier niveau** qui nous appartient (pas `HWND_MESSAGE`, qui ne reçoit pas les diffusions ; Tao/Tauri n'expose pas ce message) traite `WM_QUERYENDSESSION`. Pendant qu'un serveur tourne : `ShutdownBlockReasonCreate("Cubelink enregistre la partie du groupe…")`, renvoyer **FALSE** (renvoyer TRUE ne garantit aucun délai) ; faire seulement `tellraw @a` (« Alice éteint son PC : la partie s'arrête, rien n'est perdu »), `stop`, `LeaseRelease { reason: Shutdown }` et écrire le marqueur d'arrêt propre ; puis `ShutdownBlockReasonDestroy`. L'utilisateur garde « Arrêter quand même ». **Hacher et publier plusieurs Go en quelques secondes est irréaliste** : la version `Session` est publiée au démarrage suivant (marqueur d'arrêt propre : publication directe). Le même gestionnaire traite `ENDSESSION_CLOSEAPP` (fermeture demandée par le Store ou le Restart Manager pour une mise à jour MSIX) ; test réel : paquet installé avec `Add-AppxPackage`, puis mise à jour pendant qu'un serveur tourne. Le hook d'arrêt propre de Minecraft sert de dernier filet.
- **Veille** : voir § 3.4 (auto-exclusion).
- **Mise à jour de Cubelink** : `update_install`, `app.restart()` et la fermeture de la fenêtre refusent ou diffèrent tant qu'un bail Hearth est détenu : « Tu héberges la partie de “Les Copains” (2 joueurs) : passe la main ou arrête la partie d'abord », avec « Passer la main » et « Arrêter et mettre à jour ». Le vérificateur en fond ne propose jamais le redémarrage pendant un bail.
- **Infobulle** de l'icône : « Ton PC héberge “Les Copains” : 3 joueurs ».

### 7.6 Rejoindre

Tunnel vers le titulaire, client en `--quickPlayMultiplayer` vers l'adresse locale stable du groupe (le tunnel est redirigé vers le nouveau titulaire sans changer de port). L'agent Java de `cubelink-lan-agent.jar` gagne un mode « Hearth » : sur `DisconnectedScreen`, il affiche « Cubelink cherche la suite de la partie… », lit `hearth/<id>/client_state.json` (mis à jour par Cubelink) et se reconnecte quand l'état passe à `ready` (retour au menu sur `ended`). Détection rapide : si la connexion QUIC vers le titulaire est fermée et que 2 sondes `Hello` à 5 s d'écart échouent, la carte passe à « Alice ne répond plus » en environ 15 s, sans attendre les 90 s du bail ; le choix « Jouer depuis 23 h 00 » reste soumis à l'expiration du bail. À la fin, la nouvelle tête arrive par la synchro.

### 7.7 Envoi des versions (outbox)

- Chaque entrée de `outbox/` est annoncée (`Announce`) à tout membre joignable, qui tire les blobs lui-même, puis confirme par `Stored` (collection tirée, tag posé, `fsync`). **Une entrée ne disparaît qu'après un `Stored` d'au moins `min(2, nombre d'autres membres)` membres** (ou de tous si le groupe est plus petit).
- Aucun membre joignable : l'entrée reste en file et part dès qu'un membre (ou plus tard un gardien) apparaît, tant que Cubelink tourne. **Cubelink doit donc tourner en fond** : icône de zone de notification (tray Tauri), plugin autostart (en MSIX : extension `windows.startupTask`), activé par défaut et désactivable dans Réglages. Dès qu'un groupe existe, la croix de la fenêtre masque l'app au lieu de quitter ; « Quitter » (menu de l'icône) passe par la séquence d'arrêt propre. Un écran unique explique : « Laisse Cubelink tourner en fond pour que tes amis puissent jouer sans toi ». L'état réel de la tâche de démarrage est lu (`StartupTask.GetAsync().State` en MSIX, clé Run sinon) et la carte affiche « Démarrage automatique désactivé dans Windows ». L'écran Bloqué liste les membres (« vu il y a 3 h ») : « Il faut qu'au moins un membre du groupe ait Cubelink ouvert (même sans jouer) ».
- **Sur batterie avec économiseur** : sondes toutes les 5 min, sans téléchargement de blobs.
- **Gardiens** (phase 6), quand le réseau de relais Cubelink existera, une entrée sans membre joignable est aussi confiée à 2 ou 3 gardiens (interface prévue : trait `Replica` dans `p2p/src/hearth/outbox.rs`) :
  - **blobs** chiffrés un par un, de façon **convergente** (clé et nonce dérivés de la clé de groupe, de `key_epoch` et du hash : `k_b = BLAKE3-keyed(group_key, "blob" ‖ key_epoch ‖ hash)`, AAD = `group_id ‖ hash`), pour garder la déduplication entre versions ; la convergence ne joue qu'à l'intérieur d'une même époque de clé, et un ex-membre ne peut tester que des blobs antérieurs à son retrait ; le gardien ne stocke que le delta depuis la dernière version confirmée (`Stored`) par tous ;
  - **en-tête et index** de version chiffrés par XChaCha20-Poly1305, nonce aléatoire de 24 octets, bourrage au bloc suivant (1 Mio) ; emplacement opaque `slot = BLAKE3("cubelink/guardian/slot/1" ‖ group_key ‖ key_epoch ‖ version_id)` ; toutes les anciennes clés sont conservées (`group.keys`) ; une entrée antérieure à la tête connue est ignorée à la récupération ;
  - **choix** : d'abord les nœuds connus (amis ayant activé « Garder pour mes amis »), puis des inconnus, jamais plus d'un gardien par /24 IPv4 ou /48 IPv6 ; **vérification** quotidienne par défi (BLAKE3 d'une plage aléatoire du chiffré, gardée localement), gardien fautif remplacé ; **fraîcheur** : un `ContinueWithout` interroge **tous** les gardiens connus et tous les membres et prend la plus grande version signée (l'interface dit combien de sources ont répondu) ;
  - **quotas côté gardien** : opt-in, plafond global choisi par l'utilisateur (5 Go par défaut), au plus 1 Go par clé émettrice, TTL de 14 jours non prolongeable par un inconnu.

## 8. Absences et fourches

- **Partie mal terminée** (bail expiré sans `LeaseRelease`) : après vérification (§ 3.4 règle 3), les autres voient « La partie d'Alice ne s'est pas terminée proprement ». S'ils ont son dernier point de reprise : « Jouer depuis 23 h 00 ». Sinon : « Attendre » ou « Reprendre depuis la dernière version connue » (crée une fourche, temps de jeu mis de côté affiché). Si l'hôte retrouve un état `Orphan` (§ 7.5), il peut le publier plus tard : c'est alors une nouvelle branche.
- **Continuer sans un ou plusieurs membres** : voir § 3.2 (préavis de 7 jours signé, éligibilité par battements, veto `Present`, dormance, base effective, groupe de 2). La partie repart de la base effective : la tête la plus avancée parmi les descendants de la base votée dont aucun n'a l'absent pour auteur, y compris les versions jouées pendant le vote. Si l'absent revient, sa version devient une branche restaurable et il reste membre ; il envoie `Wake` à son retour.
- **Fourche** : plusieurs têtes (après un clic sur « Jouer depuis… », un bail « au mieux », une session qui finit en conflit). Écran « Deux versions existent », avec par branche « Bob · mardi de 21 h à 23 h · 2 h de jeu · joué par Bob » (`players`, `changed_files`).
  - **Règle automatique** : si une branche a moins de **5 minutes** de `played_secs` cumulés depuis l'ancêtre commun, l'autre est gardée sans demander (la petite reste restaurable).
  - **Sinon** : `decide` renvoie `Fork`, **seul le titulaire du bail** publie la résolution, et les autres voient « Bob choisit quelle partie garder ». La résolution est un `Restore` dont le **parent** est la branche gardée et dont **`supersedes`** liste les autres têtes, avec `number = max(numéros) + 1` ; `heads()` exclut les versions citées, et deux résolutions concurrentes sont détectées comme une nouvelle fourche.
  - **Jamais de suppression** : la branche écartée reste dans l'historique avec « Restaurer ».
  - **Données de joueur** : chaque joueur peut choisir « Garder mes données de joueur de l'autre branche ». La résolution reprend alors la collection de la branche gardée et remplace seulement `playerdata/<uuid>.dat`, `advancements/<uuid>.json` et `stats/<uuid>.json` de ce joueur (`Restore { paths }`) ; la position est remise au spawn ou au lit si le chunk n'existe pas dans la branche gardée ; avertissement sur les objets liés au monde de l'autre branche (cartes `map_N`, stockages).
  - **Restauration partielle** (`RestorePartial` : `Restore { from, paths }`) : « Restaurer cette zone », listant les fichiers région changés entre deux versions avec leurs coordonnées de blocs. Chaque version affiche un journal de modifications (fichiers région modifiés, auteur, joueurs connectés), qui rend visible un griefing.
- **Quitter le groupe** : refusé tant qu'on détient le bail (« Passe la main d'abord »). L'outbox est vidée (« Envoi de ta dernière partie… ») avant de signer `Leave`, avec « Quitter quand même (ta partie de mardi sera perdue) ». Le départ renouvelle la clé de groupe (§ 3.5). Puis : « Supprimer les 2,3 Go de ce groupe de ce PC » ou « Garder une copie en monde solo » (copie indépendante, le monde du groupe reste intact). Les versions de l'ancien membre restent valides (§ 3.3 règle 1). Réglages propose aussi « Avant de désinstaller : Quitter mes groupes » (la désinstallation MSIX efface le paquet et ne laisse exécuter aucun code).
- **Créateur indisponible** : `SetGovernance` (§ 3.2) ; une `PackUpdate` ou une invitation n'a pas besoin de `ContinueWithout`.
- **Création d'un groupe depuis un monde solo** : `Data.Player` de `level.dat` (NBT gzip, `fastnbt`) est copié dans `playerdata/<uuid Minecraft du créateur>.dat` (capabilities Forge et attachments NeoForge comprises) puis retiré, **avant la version n° 1** ; `advancements/` et `stats/` par UUID sont conservés ; si le fichier de joueur existe déjà (monde ouvert au LAN), le plus récent selon `LastPlayed`/mtime est gardé. Le dossier d'origine est renommé et `LevelName` de son `level.dat` réécrit en « Survie (copie avant le groupe Les Copains) », avec un marqueur `cubelink-hearth.json` qui fait refuser ce monde au partage solo : « Ce monde est devenu un serveur de groupe ». « Exporter en monde solo » reste disponible.

## 9. Commandes et événements Tauri

Voir `docs/contrats.md`, section « Hearth ». Préfixe `hearth_`, événement `hearth://state` (payload `HearthCard[]`) à chaque changement d'état visible. Toute commande ou tout événement ajouté par les décisions du § 12 (par exemple `hearth_diagnostic(group_id)`, `hearth_export_solo`, les décisions `Fork`, `Prepare`, `NeedHost`, `NeedsUpdate`, `Unreachable`, et les notifications de vote) est documenté dans `docs/contrats.md` dans le même commit.

- **Couche de messages** : les erreurs Rust restent techniques (journaux) ; `hearth_ui::explain(err, ctx) -> UiMessage { title, body, action }` (dans `src-tauri`) produit un message en langage courant avec une action par variante (par exemple `Blocked` : « Personne du groupe n'est en ligne. Il faut qu'un ami ait Cubelink ouvert (même sans jouer) » ; `ParentUnknown` : « Il manque une partie de l'historique, Cubelink la cherche chez les autres membres »). Toute clé est remplacée par le nom de l'ami (`FriendsDb`), l'empreinte courte seulement dans « Détails ». Glossaire : bail = « partie en cours chez X », époque = jamais affichée, version = « partie du mardi 21 h ». Un test Vitest vérifie qu'aucun texte de `HearthCard` ne contient d'hexadécimal de 12 caractères ou plus, ni les mots « bail », « époque » ou « opération ».
- **Notifications** : une proposition qui demande mon vote déclenche un toast Windows (`tauri-plugin-notification` : « Léa propose de continuer sans Hugo, il manque ton vote »), une pastille sur l'icône de la zone de notification et un `tellraw` si je suis en jeu ; un rappel après 24 h, une seule fois. Le statut annonce `open_proposals` et `GetProposals` les récupère à chaque `Hello`.
- **Journal et diagnostic** : `events.jsonl` par groupe (rotation à 5 Mo, 3 fichiers) : baux observés, accordés ou expirés, `decide` avec ses entrées, versions créées, reçues ou refusées (avec la raison), opérations rejetées et votes, passations, `WM_QUERYENDSESSION`, mises en veille, arrêts serveur avec code de sortie, correspondance adresse de boucle vers membre. Les journaux serveur sont gardés par session ; les rapports de plantage sont attachés à la version. `hearth_diagnostic(group_id)` produit un zip : événements, `ops.jsonl`, en-têtes de versions, baux, outbox, versions de Cubelink des membres, derniers journaux serveur. **Avant l'export** : lignes de chat retirées (`<pseudo> message`, `[Server]`, messages privés), adresses IP et `EndpointAddr` remplacés par des empreintes courtes, contenu montré à l'utilisateur. Jamais `group.key`, `identity.key`, le monde ni les jetons. L'écran « Historique du groupe » s'alimente de ce journal.

## 10. Phases

| Phase | Contenu | État |
|---|---|---|
| 1 | Règle de jonction : `SessionAccess`, demande à rejoindre, fin de l'invitation automatique | codée, testée (tests de crate) |
| 2 | Groupes : modèle signé, opérations, propositions et votes, protocole, stockage | codée, testée (tests de crate) ; branchée à l'app (`cubelink/hearth/1`, créer, inviter, voter), exercée par `hearth_flow` |
| 3 | Versions du monde : collections iroh-blobs, chaîne, têtes et fourches, synchro de fond, matérialisation | codée, testée (tests de crate) ; branchée à l'app (blobs dédiés, synchro de fond, fourche résolue côté app), exercée par `hearth_flow` |
| 4 | Bail, serveur dédié, bouton Jouer, passation | codée avec les écrans (accueil, création, page du serveur, moments) ; cycle testé avec un faux serveur (`src-tauri/tests/hearth_flow.rs`) ; **non vérifiée** avec un vrai serveur Minecraft, un vrai jeu et deux PC |
| 5 | Points de reprise, outbox, partie mal terminée, continuer sans un membre | codée avec les écrans ; testée avec un faux serveur (outbox, bail expiré, vote à la majorité, fourche) ; points de reprise toutes les 10 min et bannière des 7 jours non vérifiés en réel |
| 6 | Tête et bail publiés dans la DHT, gardiens chiffrés | après la DHT et le réseau de relais du fil P2P |

### 10.1 Écarts entre cette spécification et le code (état actuel)

Le code est plus simple que les § 2 à 4 ; ce qui suit est ce qui est réellement fait, avec ses tests.

- **Accès par cible** (`authz.rs`) : `SessionTarget::{Solo, Group(GroupId)}` ; `AccessSets` tient les invités du monde solo, un ensemble de membres par groupe et `serving` (le seul groupe servi). Le tunnel n'a qu'un port : la cible servie est donc `serving`, pas un premier message de flux. Un groupe servi n'accepte que ses membres, un monde solo que ses invités encore amis ; `friend_invite` est refusé tant qu'un groupe est servi.
- **Révocation** : `tunnel.rs` revérifie l'autorisation toutes les 500 ms et avant chaque nouveau flux d'une connexion établie, puis la ferme avec `CLOSE_DENIED` (pas de registre de flux). Test `un_tunnel_deja_ouvert_est_coupe_a_la_fin_de_partie`. Il n'y a pas de `kick` ni de `whitelist remove` côté serveur : la coupure du tunnel en tient lieu.
- **Propositions** (`store.rs`) : le premier signataire est l'auteur ; au plus 10 propositions ouvertes par groupe (la plus ancienne de l'auteur qui en a le plus est évincée), une par (auteur, cible), 5 propositions par heure et par auteur, expiration à 14 jours.
- **`ContinueWithout { absent, base_version, absent_since, at }`** : refusé sans 7 jours d'absence (`at >= absent_since + 7 j`), sans base connue et feuille de l'époque. Il change d'époque mais **ne change plus la gouvernance** : `SetGovernance { at }` (créateur, ou majorité stricte des autres si le créateur est dormant depuis 30 jours) le fait, et `maybe_take_over` le propose automatiquement. Pas de `Seen`, `Present`, `Wake` ni de dormance persistée.
- **Bail** : non signé (authentifié par la connexion QUIC), `MAX_LEASE_TTL_SECS = 90`, `phase: Starting | Ready` annoncée avant `start_server` (`begin_starting`, `wait_ready` côté invité). Classement : phase (Ready avant Starting), descendance dans le graphe local, plus petite clé. `base_number` n'est qu'indicatif et rejeté s'il contredit l'en-tête local. Preuve de service : 2 échecs en 3 min, titulaire ignoré 15 min. Un membre qui répond sans bail efface le bail périmé qu'on lui connaît (`probe_inner`). Pas de `lease_id`, de pierre tombale, de `LeaseClaim` ni de test « vivant ».
- **Fin de partie** : `Leave` est diffusé à tous les membres avant la finalisation ; pas de rotation de clé de groupe. `checkpoint` avance `tip` dès la publication, même si `flush_end` échoue. La résolution d'une fourche est durable (`VersionGraph::effective_heads`). Fermer Cubelink arrête les serveurs dédiés (`hearth_stop_all`, 150 s au plus). Un hôte qui perd l'arbitrage sans joueur connecté s'arrête (`arbitrate_now`).
- **Réception d'un monde** (`sync.rs`) : refus au-delà de 3 fois la version précédente plus 2 Gio (`Oversize`, confirmé par un second « Jouer »), et sans 5 Gio libres de réserve (`NoSpace`).

## 11. Et si… ?

Les cas limites étudiés pour Hearth (pannes, concurrence, sécurité, modpacks, vie du groupe) sont réunis dans [`docs/hearth-et-si.md`](hearth-et-si.md) : **127 cas**, regroupés en cinq thèmes (réseau et bail, données et sauvegardes, sécurité et confiance, Minecraft et modpacks, vie du groupe et expérience). Chaque cas est une question concrète (« Et si deux amis cliquent sur Jouer à la même seconde ? »), la réponse de Cubelink, et une étiquette d'impact. L'étiquette « conception modifiée » signale les cas dont la solution a changé cette spécification ; leurs décisions sont regroupées au § 12.

## 12. Décisions de conception par phase

Chaque ligne est un changement de conception issu de l'analyse des cas du § 11. « **Maintenant** » veut dire : implémentable dans les phases 1 à 5, sans la DHT ni les gardiens. « **Phase 6 ou plus tard** » dépend de la DHT, du réseau de relais ou des gardiens. Les numéros sont ceux de la liste de travail. Les identifiants d'origine (colonne « Cas ») renvoient aux entrées de l'analyse.

### 12.1 Maintenant (phases 1 à 5)

| N° | Décision | Module | Cas |
|---|---|---|---|
| 1 | Format gelé : `format` et `ext` signés dans tout objet, `Hello.app_version` et `proto_max`, erreur `Unsupported { need }`, `MinFormat`, fichier `FORMAT` local et migration avec `.bak` (§ 3.1). À figer avant la première pré-version qui active Hearth | `p2p/src/hearth/{proto,version,model,lease}.rs` | UX-24, CRIT-08 |
| 2 | Types `AccountKey` et `DeviceKey` distincts, `DeviceCert`, `device_id`, un seul bail actif par compte ; en attendant la migration, clé de compte = clé d'appareil (§ 3.1) | `p2p/src/identity.rs, p2p/src/hearth/model.rs` | NET-14, NET-15, SEC-19, SEC-18, UX-21 |
| 3 | Instance unique (plugin et verrou `hearth/.lock`), alerte « identité utilisée sur un autre PC » et Héberger désactivé | `src-tauri/src/lib.rs, p2p/src/identity.rs` | NET-14 |
| 4 | Clé de compte chiffrée au repos (DPAPI ou trousseau, migration transparente), export et import par phrase de passe (Argon2id + XChaCha20-Poly1305) | `p2p/src/identity.rs, src-tauri` | SEC-18, SEC-19 |
| 5 | Chaîne d'opérations linéaire : votes signés sur le contenu, un vote par cible contradictoire, réconciliation (plus de signatures éligibles, puis plus petit hash, re-signature automatique), reconstruction complète, preuve d'équivocation, `MAX_OPS` dans `apply`, 1 opération par minute et par signataire | `p2p/src/hearth/model.rs, p2p/src/hearth/sync.rs` | NET-21, SEC-02, UX-13, UX-10 |
| 6 | Nouvelles opérations `Wake`, `SetGovernance`, `SetSettings`, `MinFormat`, `RotateKey`, `ReplaceMember`, `Veto`, `AddMember { name, invitee_sig }`, `ContinueWithout { absents, notice_secs }` ; pas d'état `pending` | `p2p/src/hearth/model.rs` | SEC-20, UX-11, UX-14, SEC-18, SEC-12, MC-18, UX-24 |
| 7 | Dormance : quorums sur les membres actifs, `Wake`, proposition de retrait après 30 jours, test `proptest` d'un groupe à moitié mort | `p2p/src/hearth/model.rs` | SEC-05, UX-11, UX-12 |
| 8 | `ContinueWithout` : préavis de 7 jours signé, message `Present`, `last_seen_absent`, battements `Seen`, éligibilité, groupe de 2, bascule du créateur après 30 jours de dormance | `p2p/src/hearth/model.rs, p2p/src/hearth/proto.rs` | SEC-03, NET-22, UX-12, UX-14 |
| 9 | Base effective d'un `ContinueWithout` (tête la plus avancée hors auteurs absents), époque des descendants, vote sur « absents + époque », contrôles de base avant signature | `p2p/src/hearth/version.rs, p2p/src/hearth/model.rs` | NET-20, SEC-04 |
| 10 | Propositions : expiration à 14 jours, `open_proposals` et `GetProposals`, quotas anti-spam, notifications Windows et en jeu | `p2p/src/hearth/proto.rs, p2p/src/hearth/store.rs, src-tauri` | UX-10, SEC-21 |
| 11 | Ajout de membre avec consentement signé de l'invité, écran de vote (nom de ma liste d'amis, code à 6 chiffres), proposant ami accepté | `p2p/src/hearth/model.rs, src/` | SEC-20 |
| 12 | Clé de groupe : `key_epoch`, `key_commit`, `GetGroupKey`, `RotateKey` après un `Leave`, invitation envoyée après l'`AddMember`, `group.keys` conservé | `p2p/src/hearth/model.rs, p2p/src/hearth/proto.rs, p2p/src/hearth/store.rs` | SEC-12, UX-15 |
| 13 | En-tête de version : `supersedes`, `op_seq`, `players`, `changed_files`, `lease_id` et `witnesses` (provenance seulement), `Restore { paths }`, version locale `Orphan` | `p2p/src/hearth/version.rs` | CRIT-01, SEC-07, SEC-08, UX-16, SEC-09, MC-25 |
| 14 | Règles d'insertion : appartenance ancrée sur `op_seq`, `pack_history`, quotas à la réception (30 par heure et par auteur, plafond par auteur), `Restore` conditionnée | `p2p/src/hearth/version.rs` | SEC-06, SEC-07, SEC-08, MC-06, SEC-09 |
| 15 | File `pending/` (1 000, 7 jours), synchro des en-têtes par identifiants, résumé horaire par époque | `p2p/src/hearth/sync.rs, p2p/src/hearth/proto.rs` | CRIT-03, D19 |
| 16 | Contrôle de contenu `check.json` avec signalement des régressions, garde de taille (`total_size`, espace libre, `oversize`) | `p2p/src/hearth/sync.rs` | D25, CRIT-04 |
| 17 | `validate_world_path` (NFC, collisions, plafonds), chemins longs, table d'exclusion unique | `p2p/src/hearth/version.rs, core/src/hearth/world.rs` | D14, D15, D16, CRIT-06, D04, MC-17 |
| 18 | Collection en arbre de Merkle par dossier et blocs fixes de 64 Kio ; mesure du coût du `FsStore` sur un monde de 15 Go avant de figer le format | `p2p/src/hearth/version.rs, core/src/hearth/world.rs` | D17, MC-17 |
| 19 | Matérialisation : copie ou reflink (jamais de lien dur), incrémentale avec journal et index BLAKE3 unique, machine à états par groupe, première matérialisation dans un dossier temporaire | `core/src/hearth/world.rs, p2p/src/hearth/sync.rs` | D01, MC-11, UX-18, D11, NET-11, CRIT-05 |
| 20 | `FsStore` propre à Hearth, ALPN `cubelink/hearth-blobs/1`, tags persistants et tag `pending`, autorisation par hash | `p2p/src/hearth/store.rs, p2p/src/transfer/send.rs` | D02, D18, SEC-22 |
| 21 | Rétention unique : ensemble protégé, paliers, budget par groupe, âge sur `received_at`, écran « Stockage » | `p2p/src/hearth/store.rs` | D03, UX-19, SEC-09 |
| 22 | Écritures durables : `fsync` et renommage, journal d'opérations tronqué, publication atomique d'une version, `retry_io`, OneDrive refusé | `p2p/src/hearth/store.rs, p2p/src/hearth/version.rs, core/src/hearth/world.rs` | D06, D13, D21 |
| 23 | `SignedLease` (époque, `device_id`, `lease_id`, `seq`, phase), TTL 90 s, `validate`, bail relayé avec `age_secs` | `p2p/src/hearth/lease.rs, p2p/src/hearth/proto.rs` | NET-04, SEC-10 |
| 24 | Bail en deux phases (`LeaseClaim`, `ClaimAck`, `ClaimConflict`), quorum « sûr » ou « au mieux », verrou témoin jusqu'au premier renouvellement en phase `Starting` | `p2p/src/hearth/lease.rs, p2p/src/hearth/proto.rs` | NET-02, MC-09 |
| 25 | Départage par ascendance, puis tête commune des témoins, puis plus petite clé ; récence par ascendance et écran de fourche | `p2p/src/hearth/lease.rs, p2p/src/hearth/version.rs` | NET-01, CRIT-02 |
| 26 | `decide` avec `ProbeRound`, modes `Offline`, `Filtered`, `Alone` (confirmation), états `Prepare`, `NeedHost`, `NeedsUpdate`, `Unreachable`, `Fork` | `p2p/src/hearth/lease.rs, p2p/src/hearth/sync.rs` | NET-03, NET-19, UX-08, UX-12, MC-08 |
| 27 | Pierres tombales et `lease_id`, définition de « vivant », `LeaseRelease` enrichi (raison, `replicated_to`, successeur), preuve de service | `p2p/src/hearth/lease.rs` | NET-08, NET-09, SEC-10, UX-25 |
| 28 | Auto-exclusion après veille : écarts d'horloge, ronde de sondage, `PBT_APMSUSPEND`, `PBT_APMRESUMEAUTOMATIC`, `ES_SYSTEM_REQUIRED` | `p2p/src/hearth/lease.rs, src-tauri/src (alimentation)` | NET-07, UX-04, UX-03 |
| 29 | Passation `Handover`, `HandoverReady`, réservation du bail au successeur pendant 60 s | `p2p/src/hearth/lease.rs, p2p/src/hearth/proto.rs` | NET-24 |
| 30 | `complete_heads`, `Have`, `Announce`, `Stored`, réplication continue pendant la partie, outbox libérée après `Stored` de 2 membres | `p2p/src/hearth/sync.rs, p2p/src/hearth/outbox.rs, p2p/src/hearth/proto.rs` | D07, UX-05, NET-10, D08 |
| 31 | Un seul `Checkpoint` par lignée dans l'outbox, plus la `Session` finale | `p2p/src/hearth/outbox.rs` | D09 |
| 32 | Ordonnanceur de débit (tunnel prioritaire, 25 % en session, 50 % hors session), `PathObserver`, ping affiché | `p2p/src/hearth/outbox.rs, p2p/src/tunnel.rs` | NET-13, MC-17, UX-22 |
| 33 | Carnet d'adresses de groupe `SignedAddr` | `p2p/src/hearth/proto.rs, p2p/src/hearth/store.rs` | NET-16 |
| 34 | `SessionAccess` indexé par cible, fermeture des connexions d'un membre retiré | `src-tauri/src/authz.rs, p2p/src/tunnel.rs` | NET-23, SEC-11 |
| 35 | Lecture du net-report avant d'héberger, état « injoignable depuis ton réseau » | `p2p/src/hearth/lease.rs, src-tauri/src/iroh_backend.rs` | NET-05 |
| 36 | Port local stable par groupe, reconnexion automatique par l'agent Java, détection de panne en 15 s | `cubelink-lan-agent.jar, src-tauri/src/stage.rs, p2p/src/tunnel.rs` | UX-07, MC-24 |
| 37 | Superviseur `cubelink-hearthd` (tube nommé, `stop` propre sans Cubelink), RCON désactivé, pas de Job Object, réadoption au redémarrage | `core/src/hearth/server.rs, core/src/bin/cubelink-hearthd.rs` | UX-02, MC-20, D23 |
| 38 | Arrêt de Windows : fenêtre cachée, `FALSE` après `ShutdownBlockReasonCreate`, `stop` et fin de bail, `ENDSESSION_CLOSEAPP` | `src-tauri/src/lib.rs, core/src/hearth/server.rs` | UX-03, MC-21, CRIT-09, D23 |
| 39 | Fin non propre : marqueur de session, version `Orphan` locale, « Publier la fin de partie » ou « La mettre de côté », aucune publication automatique | `core/src/hearth/world.rs, p2p/src/hearth/version.rs` | D10, MC-23, UX-02 |
| 40 | Séquence unique de point de reprise (120 s, staging, 3 essais, `save-on` gardé, hachage hors `save-off`), intervalle adaptatif | `core/src/hearth/server.rs, core/src/hearth/world.rs` | NET-12, D04, D05, MC-15, MC-16, UX-09 |
| 41 | Seuils de disque (avant, 3 Go, 1,5 Go) et gestion d'`ENOSPC` | `core/src/hearth/server.rs` | D12, UX-19 |
| 42 | Lancement direct de Java, `server.properties` régénéré et forcé, adresse source par membre, chemins non ANSI, adresse locale stable côté client | `core/src/hearth/server.rs, core/src/launch_args.rs` | MC-19, MC-20, MC-24, CRIT-12, CRIT-13 |
| 43 | `McProfileClaim`, liste blanche et opérateurs générés depuis le groupe, contrôle de `Login Start` dans le tunnel | `p2p/src/hearth/model.rs, core/src/hearth/server.rs, p2p/src/tunnel.rs` | SEC-13, MC-18 |
| 44 | « Compte Microsoft requis », rafraîchissement du jeton et vérification de la session Mojang avant Jouer | `src-tauri/src/commands_game.rs` | SEC-14 |
| 45 | `HearthPack` canonique, `pack_collection`, instance de groupe gérée, `client_extras/`, classement client et serveur, `server_only/` | `core/src/hearth/pack.rs, core/src/mrpack.rs, core/src/mod_meta.rs` | MC-01, MC-02, MC-03, MC-04, MC-05 |
| 46 | `PackUpdate` : écran de diff calculé localement, majorité sur le diff vérifié, mise à jour majeure, sans bail et depuis une tête unique, proposition composée avec `Restore` | `p2p/src/hearth/model.rs, core/src/hearth/pack.rs, src/` | SEC-01, MC-06, MC-07 |
| 47 | Contrôles avant démarrage : hashs de mods, `DataVersion`, registres manquants, liste signée de mods vulnérables, `GetServerMods` | `core/src/hearth/server.rs, core/src/mod_check.rs` | D22, MC-06, SEC-17, SEC-07 |
| 48 | Config du pack recopiée en écrasant, clés sensibles refusées, `PackUpdate` mineure proposée, préfixe `server_state/` unique | `core/src/hearth/server.rs, core/src/hearth/world.rs` | SEC-15, MC-14, MC-13, D24 |
| 49 | Plantage au démarrage : `crash_on_start`, raison `Crashed`, restauration proposée | `core/src/hearth/server.rs, p2p/src/hearth/lease.rs` | SEC-17, UX-25, MC-23 |
| 50 | Préparation en fond, `host_ready`, capacité RAM, `can_host`, `HostRequest`, un bail par appareil | `core/src/hearth/server.rs, p2p/src/hearth/lease.rs, p2p/src/hearth/proto.rs` | MC-08, UX-06, MC-10, UX-20 |
| 51 | Arrêt du serveur vide après N minutes, `/stop` d'un joueur traité comme arrêt propre | `core/src/hearth/server.rs` | MC-22 |
| 52 | Exposition réseau : adresses d'écoute forcées en boucle locale, liste des sockets après `Done (`, jamais d'UPnP | `core/src/hearth/server.rs` | SEC-16 |
| 53 | Création depuis un monde solo : `Data.Player` copié, dossier d'origine renommé et marqué | `core/src/hearth/world.rs, src/` | MC-12, CRIT-14 |
| 54 | Fourche : règle automatique à 5 minutes, résolution par le titulaire du bail, `supersedes`, données de joueur, restauration partielle, journal de modifications | `p2p/src/hearth/version.rs, core/src/hearth/world.rs, src/` | UX-16, CRIT-01, MC-25, SEC-09 |
| 55 | Quitter le groupe (outbox vidée, bail refusé), départ de l'application, créateur indisponible | `src-tauri, p2p/src/hearth/model.rs` | UX-15, UX-21, UX-14 |
| 56 | Fond : icône de zone de notification, autostart, état de la tâche de démarrage, toutes les boucles en Rust, EcoQoS désactivé | `src-tauri/src/lib.rs, src-tauri/src (nouveau background.rs)` | UX-01, CRIT-10 |
| 57 | `update_install` et fermeture refusés ou différés pendant un bail | `src-tauri/src/commands_update.rs, src-tauri/src/state.rs` | UX-02, MC-20 |
| 58 | Dossier `hearth/` dans le dossier local du paquet, test réel d'une mise à jour pendant un serveur actif | `src-tauri (paquet MSIX), docs/release.md` | CRIT-09 |
| 59 | Couche `hearth_ui::explain`, glossaire, test Vitest des textes de carte | `src-tauri/src/error.rs, src/ (cartes Hearth)` | UX-17 |
| 60 | Journal `events.jsonl`, journaux serveur par session, diagnostic filtré (chat retiré, IP en empreintes) | `p2p/src/hearth/store.rs, src-tauri (hearth_diagnostic)` | UX-23 |
| 61 | Connexion limitée et batterie : synchro réduite, sondes à 5 minutes | `p2p/src/hearth/sync.rs, src-tauri (état réseau Windows)` | UX-22 |
| 62 | Membres non amis : avertissement avant d'accepter, sondes à 5 minutes (le chemin réservé au relais arrive plus tard) | `src-tauri/src/authz.rs, p2p/src/hearth/proto.rs, src/` | SEC-23 |
| 63 | Réglage `hearth_enabled` désactivé par défaut, critères de sortie par phase, « Exporter en monde solo » | `src-tauri (réglage), docs/release.md` | CRIT-08 |
| 64 | Banc `hearth_cluster`, trait `Clock` injecté, scénario `E2E_HEARTH`, chemins de données paramétrables | `p2p/tests/hearth_cluster.rs, e2e/, p2p/src/hearth` | CRIT-07 |

### 12.2 Phase 6 ou plus tard

| N° | Décision | Module | Cas |
|---|---|---|---|
| 65 | DHT réduite à la découverte (jamais un verrou), republication toutes les 30 minutes | `p2p/src/hearth/lease.rs, futur module DHT` | NET-17 |
| 66 | Enregistrements DHT par membre, clé dérivée par jour et par époque de clé, contenu chiffré, aucune IP directe, 2 chemins d'amorçage | `p2p/src/hearth/dht.rs` | NET-18, SEC-25 |
| 67 | Gardiens : chiffrement convergent des blobs, en-têtes à nonce aléatoire et bourrage, emplacement opaque, anciennes clés conservées, delta, défis quotidiens, fraîcheur du `ContinueWithout`, choix et quotas | `p2p/src/hearth/outbox.rs (trait `Replica`), p2p/src/hearth/guardian.rs` | D20, SEC-24 |
| 68 | Relais éligibles par auto-test, écoute sur 443, repli explicite et raison `NoPath` | `p2p/src/relay.rs` | NET-06 |
| 69 | Relais applicatif de membre `cubelink/hearth-fwd/1` | `p2p/src/tunnel.rs, p2p/src/hearth/lease.rs` | NET-05 |
| 70 | Chemin réservé au relais pour les non-amis (second point d'accès lié par `DeviceCert`) | `src-tauri/src/authz.rs, p2p/src/hearth/proto.rs` | SEC-23 |
| 71 | UDP par datagrammes QUIC pour la voix | `p2p/src/tunnel.rs, core/src/hearth/server.rs` | CRIT-11 |
| 72 | Multi-appareils complet, migration des amitiés vers les clés d'appareil, « Lier un nouvel appareil » | `p2p/src/identity.rs, src-tauri, friends` | NET-15, UX-21 |

### 12.3 Garde-fous de livraison et de test

- **Drapeau `hearth_enabled`** : désactivé par défaut, activable seulement par une variable d'environnement ou un réglage caché, tant que les phases 4 et 5 et les gardes de plantage (marqueur de session, `Orphan`, superviseur, arrêt de Windows) ne sont pas livrées et vérifiées en réel. Les groupes créés pendant cette période sont marqués expérimentaux, avec « Exporter en monde solo » toujours disponible.
- **Critères de sortie par phase** (à compléter par le test réel exigé, sans toucher au tableau du § 10) : phase 1 (accès par cible, révocation) ; phase 2 (chaîne linéaire, réconciliation, quorums, format gelé) ; phase 3 (collection, matérialisation incrémentale, `pending/`, contrôle de contenu) ; phase 4 (bail en deux phases, serveur sous superviseur, bouton Jouer, passation) ; phase 5 (point de reprise unique, outbox avec `Stored`, `Orphan`, `ContinueWithout`).
- **Gel du format** : `format` et `ext` doivent être en place avant la première pré-version qui active Hearth ; un identifiant calculé sur le JSON canonique devient un format à migrer pour toujours.
- **Banc de test** : `p2p/tests/hearth_cluster.rs` fait tourner N endpoints iroh dans un processus, avec un relais local de test et une injection de pannes sur le transport Hearth (latence, perte, partition, réordonnancement, gel d'un nœud pour simuler la veille) ; un trait `Clock` est injecté dans le bail, l'outbox et les votes. Propriétés vérifiées avec graine fixe : jamais deux serveurs dont les témoins se recoupent ; une version complète finit chez au moins 2 membres ; même état d'opérations partout après réconciliation. Scénario réel `E2E_HEARTH` : deux instances Cubelink sur la même machine, un dossier de données par variable d'environnement (`identity.key`, `hearth/` et le verrou d'instance sont donc paramétrables) et un vrai serveur vanilla pour le bail, la passation, le point de reprise et la partie mal terminée.
- **Tests purs exigés** : départage (ascendance, tête commune, clé), fourche à 2 puis 3 têtes et résolution à une seule tête, versions reçues dans l'ordre inverse, base effective (v41 à v44 jouées pendant le vote), matérialisation puis modification de `w/` sans altérer le blob source, groupe dont la moitié des clés est morte.

### 12.4 Contradictions tranchées

Les solutions proposées par l'analyse se contredisaient parfois. Quand c'est arrivé, la plus sûre a été retenue :


- **Départage du bail** : plus petit numéro de base (spécification d'origine, NET-01, SEC-10, UX-04) contre plus grand numéro. Aucun des deux : `number` est une profondeur. Retenu : ascendance, puis tête commune des témoins, puis plus petite clé (§ 3.4).
- **Témoins** : « chaque membre joignable répond en 5 s » contre majorité. Retenu : majorité des membres actifs (seule une intersection de quorums protège d'une partition), « sûr » ou « au mieux ». Le verrou témoin dure jusqu'au premier renouvellement en phase `Starting`, avec un plafond de 90 s si le titulaire disparaît (précision ajoutée).
- **Version sans témoin** : « jamais tête » (SEC-07, SEC-06, SEC-08) contre hébergement d'un membre seul (NET-19, UX-12). Retenu : le témoin est une provenance affichée, jamais une condition ; l'exigence de bail témoigné dans les quotas et dans l'ancrage d'appartenance est retirée.
- **Chaîne d'opérations** : DAG trié par hash (NET-21) contre chaîne linéaire avec départage (SEC-02, UX-13) contre votes liés à `(op_seq, prev)` (UX-10). Retenu : chaîne linéaire, votes sur le contenu, réconciliation déterministe, « un vote par cible contradictoire ».
- **Quotas de propositions** : « une par `(op_seq, prev)` » (SEC-21) devient « une par cible », puisque les votes ne lient plus `prev`.
- **Rétention** : cinq politiques incompatibles (48 h/30 j/6 mois, 8 semaines, 14 j/60 j, 10 Go, 10 % du disque). Retenu : une seule (§ 5.4), âge sur `received_at`.
- **Point de reprise** : délais de 60 s, 120 s ou 5 min, lecture sous `save-off` ou staging, et reprise du blob précédent pour un fichier verrouillé (MC-15 point 4, erroné). Retenu : § 7.4, 120 s, staging sous `save-off`, hachage hors `save-off`, abandon du point de reprise si un fichier reste verrouillé.
- **Serveur et mort de Cubelink** : Job Object qui tue (UX-02), reprise par RCON (D23, MC-20) et `enable-rcon=false` (SEC-13). Retenu : superviseur `cubelink-hearthd` à tube nommé, RCON désactivé.
- **Arrêt de Windows** : renvoyer TRUE puis publier une `Session` en 5 à 30 s (UX-03, MC-21, D23) contre blocage. Retenu : renvoyer FALSE après `ShutdownBlockReasonCreate`, `stop` et fin de bail seulement, `Session` publiée au démarrage suivant.
- **Après un plantage** : publication automatique (UX-02), rien (MC-23), version `Orphan` (D10). Retenu : `Orphan` locale, choix de l'hôte, `VersionKind::Session { unclean }` abandonné.
- **Identité** : `ReplaceKey` signé par l'ancienne clé (UX-21) contre remplacement à la majorité (SEC-18, SEC-19), un appareil actif contre plusieurs. Retenu : clé de compte plus clés d'appareil ; `ReplaceMember` à la majorité après perte ; `ReplaceKey` seulement pour un transfert volontaire.
- **Magasin de blobs** : magasin existant partagé (D02 d'origine) contre magasin propre ; `TempTag` « persistant » (D18). Retenu : `FsStore` propre, ALPN dédié, tags persistants.
- **Découpage des régions** : blocs fixes de 64 Kio (D17) contre découpage par chunk (MC-17). Retenu : blocs fixes, dans un arbre de Merkle par dossier.
- **Matérialisation** : dossier temporaire renommé (NET-11) contre incrémentale en place (D11, MC-11). Retenu : incrémentale ; dossier temporaire seulement la première fois.
- **Veille** : arrêter le serveur au réveil (UX-04) contre ronde de sondage (NET-07). Retenu : ronde de sondage, puis arrêt et branche seulement si quelqu'un a repris la main.
- **Chiffrement chez les gardiens** : convergent (D20) contre nonce aléatoire avec bourrage (SEC-24). Retenu : les deux, chacun à sa place (blobs convergents par époque de clé, en-têtes aléatoires).
- **Données hors `world/`** : versionner `ops.json`, `whitelist.json` et `banned-*.json` (D24) contre les régénérer (MC-13, MC-18, SEC-13). Retenu : régénérés ; `server_state/` unique pour le reste.
- **Invitation** : membre `pending` puis `Accept` (UX-11) contre `invitee_sig` (SEC-20). Retenu : `invitee_sig` seul.
- **Gouvernance à deux** : ne jamais transférer la gouvernance (SEC-03) contre un seul membre qui invite (UX-12). Retenu : dormance après préavis ; basculement après 30 jours de dormance confirmée ; entre-temps le membre actif joue mais n'invite ni ne retire.
- **Délai pour `SetGovernance`** : 14 jours d'inactivité (UX-14) contre 30 jours de dormance. Retenu : 30 jours, le plus sûr contre une prise de pouvoir (précision ajoutée).
- **Quotas de versions** : fondés sur `created_at` et `lease_seq` (SEC-06). Retenu : à la réception, sur l'heure locale.
- **Relais pour non-amis** : un réglage par pair n'existe pas dans iroh. Retenu : avertissement dès maintenant, second point d'accès en phase 6.
- **Adresse de boucle** : `127.X.Y.Z` et port 25565 fixes (MC-24) contre macOS et collisions. Retenu : `127.0.0.1` et port dérivé du groupe côté client ; l'adresse source par membre (CRIT-12) reste séparée.
- **Diagnostic** : contenu brut (UX-23) contre confidentialité. Retenu : chat retiré, IP en empreintes, contenu montré avant export.
- **Drapeau `adds_unknown`** (SEC-01) : remplacé par un diff calculé par chaque membre sur le pack publié.
- **« Vivant »** : port qui accepte (NET-09) contre démarrage de 2 à 6 minutes (MC-09). Retenu : définition en deux phases (§ 3.4).
- **Noms de messages** : `HaveCollection`, `HaveVersion`, `Have`, `Stored` désignaient la même chose. Retenu : `Have` (interrogation) et `Stored` (confirmation durable).
- **Réveil d'un dormant** : `Rejoin`, `Present` ou `Wake` (SEC-05, UX-11). Retenu : `Wake` (opération) ; `Present` reste le veto d'une proposition.
- **Clé de profil Minecraft** : opération `SetProfile` (MC-18) contre `McProfileClaim` diffusé (SEC-13) ; opérateurs par `SetOperators` ou `GroupSettings.ops`. Retenu : `McProfileClaim` hors chaîne, réglages et opérateurs par `SetSettings`.
- **Config serveur du pack** : écrasement à chaque démarrage (SEC-15) contre fusion à trois voies (MC-14). Retenu : écrasement, plus une `PackUpdate` mineure proposée à l'arrêt si des fichiers ont changé.
- **Arrêt d'un serveur vide** : 10 minutes (UX-09) contre 5 minutes (MC-22). Retenu : 5 minutes par défaut, réglable de 1 à 30.
- **Dérivation DHT** : par membre (NET-18) contre paire commune (SEC-25). Retenu : un enregistrement par membre, avec `key_epoch`, pour éviter les collisions de `seq`.
- **Départ d'un membre** : seule la clé de `RemoveMember` change (SEC-12) contre `Leave` aussi (UX-15). Retenu : `Leave` renouvelle aussi la clé, par `RotateKey`.
