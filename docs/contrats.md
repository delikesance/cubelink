# Contrats : commandes et événements Tauri

Propriété : team lead. Les types Rust sont dans `core/src/types.rs` ; côté front, les mêmes structures en JSON (noms de champs identiques, énumérations en minuscules).
Toute commande renvoie `Result<T, String>` (message d'erreur affichable, en français). Les agents n'ajoutent pas de commande sans l'avoir demandée au team lead.

## Commandes (invoke)
| Domaine | Commande | Entrée | Sortie |
|---|---|---|---|
| auth | `auth_status` | — | `Profile \| null` |
| auth | `auth_begin_login` | — | `{ user_code, verification_uri, expires_in }` |
| auth | `auth_poll_login` | — | `Profile` (attend la validation) |
| auth | `auth_logout` | — | — |
| auth | `auth_offline_login` | `{ name }` | `Profile` (drapeau de test uniquement) |
| instances | `instances_list` | — | `Instance[]` |
| instances | `instances_create` | `{ name, minecraft, loader: LoaderSpec, ram_mb }` | `Instance` |
| instances | `instances_import_mrpack` | `{ path }` | `Instance` |
| instances | `instances_launch` | `{ id }` | — (progression par événement) |
| instances | `minecraft_versions` | — | `string[]` |
| instances | `instances_kill` | `{ id }` | — (arrête le jeu en cours) |
| instances | `instances_delete` | `{ id }` | — (refusé si l'instance est en lancement ou en jeu) |
| instances | `instances_update` | `{ id, name?, ram_mb?, java_path?, jvm_args? }` | `Instance` |
| instances | `instances_open_folder` | `{ id }` | — |
| instances | `instances_logs` | `{ id }` | `string` (fin de `latest.log`, ou `latest.log.old`) |
| art | `instances_art_image` | `{ instance_id, file }` | `string` (data URL) |
| art | `instances_art_set` | `{ instance_id, path, role }` | `Instance` (image personnalisée) |
| art | `instances_art_lookup` | `{ instance_id }` | `Instance` (cherche et applique l'art du modpack) |
| browse | `browse_search` | `BrowseQuery` | `BrowsePage` |
| browse | `browse_versions` | `{ source, id, kind, minecraft?, loader? }` | `BrowseVersion[]` |
| browse | `browse_install` | `{ input: BrowseInstallInput }` | `BrowseInstallResult` (progression par `browse://progress`) |
| mods | `mods_list` | `{ instance_id }` | `ModEntry[]` |
| mods | `mods_toggle` | `{ instance_id, file, enabled }` | — |
| mods | `mods_remove` | `{ instance_id, file }` | — |
| mods | `mods_add` | `{ instance_id, path }` | `ModEntry` (chemin absolu requis) |
| friends | `friend_my_code` | — | `FriendCode` |
| friends | `friend_regenerate_code` | — | `FriendCode` |
| friends | `friend_list` | — | `{ friend: Friend, presence: Presence }[]` |
| friends | `friend_send_request` | `{ code }` | `VerifyCode` |
| friends | `friend_requests` | `{ offset, limit, query? }` | `{ items: FriendRequest[], total: number }` |
| friends | `friend_accept` / `friend_refuse` | `{ public_key }` | — |
| friends | `friend_refuse_all` | — | `number` (nombre refusé) |
| friends | `friend_invite` | `{ public_key }` | — : hôte, invite un ami dans son monde ouvert ; lui ouvre l'accès (tunnel, manifeste, fichiers) pour la partie en cours (`SessionAccess`) |
| friends | `friend_invites` / `friend_invite_dismiss` | — / `{ id }` | `Invite[]` / — : invitations reçues |
| friends | `friend_join_request` | `{ public_key }` | `"accepted" \| "refused" \| "expired"` : invité, demande à rejoindre la partie d'un ami (ALPN `cubelink/joinreq/1`) ; attend sa décision (120 s au plus). `refused` aussi si rien n'est partagé ou si l'ami a refusé il y a moins de 10 min. Erreur : ami injoignable, une demande déjà en attente, ne nous a pas pour ami |
| friends | `friend_join_requests` | — | `JoinRequest[]` : hôte, demandes en attente de décision |
| friends | `friend_join_request_answer` | `{ public_key, accept }` | — : hôte, accepte (ouvre l'accès à la partie en cours) ou refuse. Erreur « plus valable » si la demande a expiré, été retirée, ou si rien n'est partagé |
| join | `join_plan` | `{ friend_public_key }` | `{ manifest: Manifest, manifest_hash: string, cached: number, to_receive: number, to_receive_bytes: number, unknown_mods: string[], unknown_unchecked: string[], unknown_scripts: string[], unknown_trusted: string[] }` ; `manifest_hash` (hash canonique du manifeste montré) est à renvoyer à `join_start`. Demandent l'accord (case de confiance) : `unknown_mods` (mods inconnus), `unknown_unchecked` (jars non vérifiés : `sha1` absent, réseau, limite de débit ; traités comme inconnus) et `unknown_scripts` (scripts `kubejs/**/*.js`). Chaque liste ne contient que les fichiers pas encore acceptés : ceux acceptés lors d'un `join_start` précédent (même sha256, mémorisés dans `p2p/trusted_mods.json`) passent dans `unknown_trusted`. « Inconnu » = ni Modrinth (sha1) ni CurseForge (empreinte murmur2 `cf_fingerprint` du manifeste, avec la clé de Cubelink) ne reconnaissent le jar ; `sha1` et `cf_fingerprint` sont des revendications de l'hôte, revérifiées à la réception |
| join | `join_start` | `{ friend_public_key, manifest_hash: string, accept_unknown?: boolean, ram_mb?: number }` | — (progression par événement). Refuse si le manifeste de l'hôte n'a plus le `manifest_hash` du dernier `join_plan` (« La partie de ton ami a changé… » : recharger le plan), ou si ce plan avait des fichiers à accepter et que `accept_unknown` n'est pas `true`. Avec `accept_unknown`, ces fichiers sont mémorisés comme acceptés. `game_join_friend` passe `accept_unknown: false` : il échoue s'il y a des fichiers à accepter |
| join | `join_pause` / `join_resume` / `join_cancel` | — | — |
| settings | `settings_get` / `settings_set` | `Settings` | `Settings` |

## Événements (listen)
| Événement | Charge utile |
|---|---|
| `auth://state` | `Profile \| null` |
| `launch://state` | `{ id, state: "preparing" \| "running" \| "exited" \| "failed", message? }` |
| `transfer://progress` | `TransferProgress` (champ optionnel `error` en phase `failed`) |
| `friend://presence` | `{ public_key, presence }` |
| `tunnel://state` | `{ state: "ok" \| "reconnecting" \| "lost" }` : liaison du tunnel invité. `reconnecting` : connexion à l'hôte coupée, reconnexion automatique en cours (10 s max, le port local `127.0.0.1:<port>` reste le même : Minecraft peut se reconnecter sans relancer le jeu) ; `lost` : hôte parti ou refus, le tunnel est inutilisable. Émis seulement pendant un tunnel ouvert (`join_open_tunnel` / `join_play`). |
| `friend://request` | `FriendRequest` |
| `friend://invite` / `friend://invites` | `Invite` (une invitation reçue) / `Invite[]` (file complète à chaque changement) |
| `friend://join_request` | `JoinRequest` `{ public_key, name, received_at, expires_at }` : un ami demande à rejoindre ma partie (hôte) |
| `friend://join_requests` | `JoinRequest[]` : demandes en attente, à chaque changement (décidée, expirée, retirée, partage arrêté) |
| `launch://log` | lignes de log du jeu en cours |
| `browse://progress` | progression d'une installation depuis le catalogue (`done`, `total`, libellé) |
| `import://warning` | avertissement pendant l'import d'un modpack |
| `transfer://unknown_mods` | liste des mods inconnus des catalogues (Modrinth et CurseForge, recalculés sur les octets reçus), pendant un join (sans ceux déjà acceptés lors d'un « Rejoindre » précédent) |
| `transfer://error_details` | échec d'une étape d'installation invitée, émis juste avant la phase `failed` : `{ step, message, details, log_file }`. `message` = `TransferProgress.error` (« Étape : cause » quand la cause est générique) ; `details` = texte multi-ligne copiable (étape, détail, erreur, debug) pour un bloc « Détails » ; `log_file` = `join-errors.log` dans `<data>/logs/` (toujours écrit, horodaté UTC) |

## Règles
- Le front n'accède ni au disque ni au réseau : tout passe par ces commandes.
- Les tests de l'interface utilisent un faux service (`src/services/fake.ts`, propriété de la tâche U1) qui respecte exactement ces signatures.
- Le profil hors ligne (`auth_offline_login`) n'existe que si l'application est lancée avec `CUBELINK_TEST_MODE=1`.
- Les commandes Tauri sont déclarées avec `#[tauri::command(rename_all = "snake_case")]` : le front envoie les arguments en snake_case, comme dans le tableau ci-dessus (Tauri 2 attend du camelCase par défaut).
- `Settings` : `{ language, default_ram_mb, java_path | null, trust_default, allow_relay, close_on_launch, relay_only, share_mods, discovery, lan_auto, open_to_all_friends }` (voir `core/src/types.rs`). `open_to_all_friends` (bool, défaut `false`, `serde(default)`, `settings_set` : optionnel) : invite automatiquement tous les amis en ligne quand le monde s'ouvre ; désactivé, seuls les amis invités (`friend_invite`) ou dont la demande est acceptée entrent (docs/hearth.md § 2). `lan_auto` (bool, défaut `true`, `serde(default)`, `settings_set` : optionnel) charge au lancement des instances perso l'agent Java `core/resources/cubelink-lan-agent.jar` (ressource Tauri ; `CUBELINK_LAN_AGENT=<jar>` le remplace ; jamais chargé en mode test sans cette variable) qui ouvre le monde solo au réseau local ; diagnostic : lignes `[Cubelink] LAN: ...` dans `latest.log`. Reconstruction du jar : `core/resources/lan-agent/build.sh` (JDK >= 17). `discovery` (bool, défaut `true`, `serde(default)` : anciens fichiers compatibles) active la découverte d'adresses publique n0 (pkarr + DNS) ; lu au démarrage du backend, sans effet quand le relais est désactivé (mode test hors réseau).
- Les schémas exacts de `BrowseQuery`, `BrowsePage`, `BrowseVersion`, `BrowseInstallInput`, `BrowseInstallResult` et `ModEntry` sont dans `core/src/browse.rs` et `core/src/mods.rs` ; le front les reprend dans `src/services/types.ts`.
- Le contrat du `invoke_handler` est dans `src-tauri/src/lib.rs` (deux listes identiques : mode normal et mode test, la seconde ajoute `test_state`).
- Fenêtre : `decorations: false` et permissions `core:window:allow-minimize` / `allow-close` (tâche U13).

## Ajouts (intégration)
- `host_start_sharing` `{ instance_id, world_port }` → — ; `host_stop_sharing` → — : partage d'un monde LAN avec les amis. Aucun écran ne les appelle : le partage est automatique (au passage de l'instance à `running`, manifeste partagé sans port ; port détecté dans la sortie du jeu `Started serving on N` / `Local game hosted on port N` ; arrêt à la fin du jeu). `world_port` accepte 0 (fichiers seulement) ; le dernier appel, manuel ou automatique, l'emporte.
- `join_play` `{ friend_public_key, instance_id }` → — (MULTI) : invité, « Rejoindre et jouer » en un geste : ouvre le tunnel vers le monde de l'ami, lance l'instance invitée avec connexion automatique (`--quickPlayMultiplayer 127.0.0.1:<port>`, Minecraft ≥ 1.20 ; `--server`/`--port` avant), referme le tunnel quand le jeu s'arrête. Événements de lancement habituels (`launch://state`). Erreurs françaises distinctes : ami hors ligne / monde pas encore ouvert.
- Règle de jonction (docs/hearth.md § 2) : l'hôte n'ouvre le tunnel, le manifeste et les fichiers qu'aux amis invités pour la partie en cours (`friend_invite`) ou dont la demande à rejoindre a été acceptée ; l'accès (`SessionAccess`) est vidé par `host_stop_sharing`, qui annule aussi les demandes en attente (le demandeur reçoit `expired`).
- `host_state` → `HostState | null` ; événement `host://state` `{ instance_id, sharing, world_port | null }` (MULTI) : état du partage hôte, émis à chaque changement dans `HostAuto` (partage prêt, monde ouvert au LAN = port détecté, arrêt avec `sharing:false`). Sert l'indice de l'accueil (« Tes amis peuvent te rejoindre » / « Ouvre ton monde au réseau local »).
- `test_state` → état de test (profil de données, URLs surchargées, port du tunnel) : enregistrée UNIQUEMENT si `CUBELINK_TEST_MODE=1`.
- Variables de test (ignorées sans `CUBELINK_TEST_MODE=1`, URLs limitées à 127.0.0.1/localhost) : `CUBELINK_AUTH_{MS,XBOX,XSTS,MC}_URL`, `CUBELINK_MOJANG_META_URL`, `CUBELINK_FABRIC_META_URL`, `CUBELINK_ADOPTIUM_URL`, `CUBELINK_MODRINTH_API_URL`, `CUBELINK_JAVA_PATH`, `CUBELINK_DATA_DIR`, `CUBELINK_RELAY_MODE`, `CUBELINK_SKIP_JAVA_PROBE`.
- `TransferProgress.error` (optionnel) en phase `failed`.
- `join_open_tunnel` `{ friend_public_key }` → `u16` ; `join_close_tunnel` → — (FIX-TAURI) : invité, ouvre un tunnel local vers le monde LAN de l'ami (ami connu, hôte en partage) et renvoie le port de `127.0.0.1` à saisir dans Minecraft (« Connexion directe »). Un seul tunnel à la fois (un nouvel appel ferme le précédent). `test_state.tunnel_port` = port du monde côté hôte (inchangé) ; `test_state.guest_tunnel_port` = port du tunnel côté invité (0 si fermé).
- `instances_launch` est asynchrone côté Rust (même signature JS : `{ id }` → —).
- `settings_set` refuse `relay_only = true` avec `allow_relay = false` (erreur française).
- Transfert d'invité figé : sans octet reçu pendant 15 s, le fichier est relancé avec l'adresse de l'hôte re-résolue (adresses IP directes périmées retirées si un relais est connu) ; sans octet pendant 60 s (hors pause) : `transfer://progress` phase `failed` avec `error` « le transfert est figé : aucune donnée reçue depuis N s… ». Test : `CUBELINK_TRANSFER_IDLE_SECS` (1 à 3600) fixe les 60 s ; le délai de relance vaut alors au plus la moitié.
- Mode test, `CUBELINK_RELAY_MODE=disabled` : le code ami contient les adresses directes (sans effet hors mode test).
- Signal « plus lentement » : `TransferProgress.via_relay` (déjà émis, issu de l'observateur de chemin p2p) ; p2p fournit `relay::Advice::SlowViaRelay` / `SLOW_MESSAGE_FR` (débit < 2 Mo/s pendant un moment) mais le schéma `TransferProgress` n'a pas de champ pour le porter : le front affiche l'avertissement quand `via_relay` est vrai et `speed_bps` bas. Aucun changement de schéma.

## Hearth (serveurs de groupe)

Voir `docs/hearth.md`. Implémentation : `src-tauri/src/hearth.rs` (runtime : sondes, synchro de fond, file d'envoi, bail, serveur, points de reprise, passation), `src-tauri/src/commands_hearth.rs` (commandes), `p2p/src/hearth/*` (modèle, protocole, blobs) et `core/src/hearth/*` (serveur dédié). Les types JSON sont dans `core/src/types.rs` (`Hearth*`) et miroités dans `src/services/types.ts`. Les identifiants de groupe (`id`, `group_id`) sont 32 caractères hexadécimaux ; les clés de membres sont les mêmes clés publiques que celles des amis. Toutes les commandes prennent leurs arguments en `snake_case` ; les erreurs sont des chaînes en français (`Result<T, String>`).

### Commandes

| Commande | Arguments | Retour | Rôle |
|---|---|---|---|
| `hearth_list` | | `HearthCard[]` | Cartes de l'accueil. |
| `hearth_detail` | `id` | `HearthDetail` | Page du serveur : `card`, `members`, `versions` (récentes d'abord), `proposals`, `blocker`, `fork`, `settings`. |
| `hearth_worlds` | `instance_id` | `HearthWorld[]` | Mondes (`saves/`) de l'instance, proposés à la création. |
| `hearth_create` | `name`, `instance_id`, `world: string \| null`, `members: string[]` | `HearthCard` | Crée le groupe depuis une de mes instances (`world` : sauvegarde copiée chez le groupe et publiée comme version 1 ; `null` : nouveau monde, créé au premier lancement). `members` : au moins un ami accepté. Les invitations partent en arrière-plan (renvoyées tant qu'un membre n'a pas répondu : file d'envoi). |
| `hearth_invites` | | `HearthInvite[]` | Invitations de groupe reçues, en attente. |
| `hearth_invite_accept` | `id` (= `group_id`) | | Rejoint le groupe : la carte apparaît, la synchro du monde démarre. |
| `hearth_invite_dismiss` | `id` | | Refuse l'invitation (elle disparaît). L'écran « Plus tard » ne l'appelle pas. |
| `hearth_play` | `id` | | Le bouton unique : rejoint la partie en cours (`hosted`), ou démarre le serveur sur ce PC puis le jeu (`idle`). Rejette avec `eula_required` tant que le contrat Minecraft n'est pas accepté. États qui refusent : `blocked`, `unclean` (voir `hearth_unclean_choice`), `fork`, `syncing`, `pack_missing`. Avant de décider, `hearth_play` fait une ronde `Hello` à tous les membres. Si la prise du bail n'obtient pas la majorité des membres (bail « au mieux »), il rejette une première fois avec un message qui commence par « Le groupe n'est pas assez joignable pour une prise du serveur sûre » ; un second `hearth_play` dans les 2 minutes vaut confirmation. Si un autre membre prend le bail en même temps, `hearth_play` le rejoint, ou rejette avec « … démarre déjà la partie : réessaie dans un instant. ». |
| `hearth_stop` | `id` | | Arrête mon serveur (enregistre une version, relâche le bail). |
| `hearth_unclean_choice` | `id`, `choice: "wait" \| "checkpoint" \| "last"` | | Après une partie mal terminée : `wait` ne fait rien ; `checkpoint` reprend au dernier point de reprise connu, `last` à la dernière version ; le bail de l'ancien hôte est révoqué et le serveur démarre ici (peut rejeter avec `eula_required`). |
| `hearth_set_instance` | `id`, `instance_id` | | Relie le groupe à une instance locale dont le modpack est celui du groupe (carte `pack_missing`). |
| `hearth_add_member` | `id`, `public_key` | | Propose d'ajouter un ami ; devient une décision du groupe (majorité). |
| `hearth_remove_member` | `id`, `public_key` | | Propose de retirer un membre. |
| `hearth_leave` | `id` | | Quitte le groupe : arrête mon serveur s'il tourne, ferme l'accès, libère les collections du groupe dans le magasin de blobs puis supprime le monde local (même chose quand je suis retiré du groupe). |
| `hearth_vote` | `id`, `proposal_id` | | Approuve une décision en attente. |
| `hearth_continue_without` | `id`, `public_key` | | Propose de continuer sans un membre absent (bannière de la page Membres). |
| `hearth_pack_update` | `id`, `instance_id` | | Propose au groupe le modpack de cette instance (les mods sont figés : sans vote, le modpack ne change pas). L'instance liée au groupe ne change qu'une fois `PackUpdate` appliqué à la chaîne (tout de suite si ma voix suffit, sinon au vote qui atteint le quota). |
| `hearth_restore` | `id`, `version_id` | | Restaure une ancienne version : crée une nouvelle version `restore`, rien n'est effacé. |
| `hearth_resolve_fork` | `id`, `version_id` | | Garde une des deux versions d'une fourche ; l'autre reste dans l'historique. |
| `hearth_set_background` | `id`, `on` | | Garder le serveur ouvert quand mon jeu se ferme. |
| `hearth_eula` | | `{ accepted, url }` | État du contrat d'utilisation de Minecraft (`https://aka.ms/MinecraftEULA`), accepté une fois par PC. |
| `hearth_eula_accept` | | | Enregistre l'acceptation (`hearth/meta.json`). |
| `hearth_enabled` | | `bool` | Interrupteur « Serveurs de groupe (expérimental) » (`hearth/meta.json`, `false` par défaut ; docs/hearth.md § 12.1 n° 63). |
| `hearth_set_enabled` | `enabled` | | Active ou coupe Hearth, et émet `hearth://state`. Coupé : toutes les commandes `hearth_*` refusent (« Les serveurs de groupe sont désactivés… ») sauf `hearth_list`, `hearth_detail`, `hearth_invites`, `hearth_eula`, `hearth_stop`, `hearth_enabled` et celle-ci ; le protocole `cubelink/hearth/1` refuse tous les pairs ; un serveur déjà lancé reste arrêtable. |

Erreur spéciale : `eula_required` (chaîne exacte, `HEARTH_EULA_REQUIRED` côté front), renvoyée par `hearth_play` et `hearth_unclean_choice` avant toute préparation. Le front ouvre alors la boîte du contrat puis relance l'action.

### Événements

- `hearth://state` : `HearthCard[]`, liste complète à chaque changement visible (état, sondes, synchro, étapes de lancement, votes).
- `hearth://invites` : `HearthInvite[]`, liste complète des invitations en attente.
- `hearth://handover` : `{ group_id, name, from_name }`. L'hôte vient d'arrêter le serveur pendant que je jouais (je peux reprendre avec `hearth_play`).
- `hearth://host_prompt` : `{ group_id, name, players }`. Mon jeu s'est fermé mais `players` joueurs sont encore sur mon serveur (et le réglage « garder ouvert » est désactivé) : le serveur reste ouvert jusqu'à `hearth_stop`.

### Types

- `HearthCard` : `{ id, name, minecraft, loader, state, host, players, progress, step, step_index, step_count, unclean, members_online, members_total, is_creator, can_host, instance_id, version }`. `state` : `idle` (personne ne joue), `hosted` (`host` héberge, `players` joueurs), `syncing` (`progress` en %), `blocked` (aucun membre joignable : jeu hors ligne interdit), `unclean` (`unclean = { holder, checkpoint_at }`), `fork` (deux versions), `launching` (`step`, `step_index` sur `step_count`), `hosting` (ce PC héberge), `pack_missing` (modpack du groupe absent ici). `players` est une approximation : joueurs vus dans le journal du serveur quand ce PC héberge, sinon nombre de membres en ligne quand un autre membre héberge.
- `HearthMemberLite` : `{ public_key, name, online }`. `HearthInvite` : `{ group_id, name, from, members, minecraft, loader, received_at }`. `HearthWorld` : `{ name, size_bytes }`.
- `HearthDetail` : `{ card, members: HearthMember[], versions: HearthVersion[], proposals: HearthProposal[], blocker, fork: HearthBranch[], settings: HearthSettings }`.
  - `HearthMember` : `{ public_key, name, online, is_me, is_creator, sync: "ok" | "behind" | "unknown", version, unseen_days }`.
  - `HearthVersion` : `{ id, number, kind: "session" | "checkpoint" | "restore", author, author_name, created_at, played_secs, total_size, is_head, side_branch }`.
  - `HearthProposal` : `{ id, kind: "continue_without" | "set_governance" | "add_member" | "remove_member" | "pack_update" | "leave", subject, approvals, needed, i_voted, can_vote }`.
  - `HearthBlocker` : `{ member, days }` : le membre qui avait la version la plus récente n'est pas revenu depuis `days` jours (7 au moins) ; la bannière propose `hearth_continue_without`.
  - `HearthBranch` : `{ version_id, number, author_name, created_at, played_secs, total_size }`. `HearthSettings` : `{ keep_background, disk_bytes, outbox_pending, instance_name }`.

### Écarts avec docs/hearth.md et réseau

- Protocole `cubelink/hearth/1` (JSON, 64 Kio, un flux bidirectionnel par requête) et `cubelink/hearth/blobs/1` (magasin iroh-blobs dédié `hearth/blobs`, lecture réservée aux membres du groupe) sont acceptés par le Router iroh ; les membres ont accès au tunnel du serveur via `SessionAccess` (le tunnel de jeu d'un groupe ne vérifie pas la présence d'une invitation : `P2pBackend::hearth_open_tunnel`).
- File d'envoi : une seule par groupe (`MemberReplica`), traitée à chaque sonde (60 s) et à chaque `Hello`. Le serveur dédié vit dans `hearth/<groupe>/server/` ; le monde publié est `server/world`.
- Fourche : le modèle signé ne sait pas résoudre une fourche avec un `Restore` à un seul parent ; l'application regroupe les têtes par collection (`effective_heads`) et `hearth_resolve_fork` ajoute des `Restore { from: gardée }` sur les têtes écartées.
- Modpack absent : l'acquisition automatique du modpack d'un groupe n'est pas faite ; la carte passe en `pack_missing` et `hearth_set_instance` relie une instance locale correspondante (empreinte `manifest_hash` du manifeste sans le nom).
