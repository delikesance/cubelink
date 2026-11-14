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
| instances | `instances_update` | `{ id, name?, ram_mb?, java_path?, jvm_args?, wrapper? }` (`wrapper` : commande d'enrobage placée devant Java, Linux seulement ; chaîne vide = aucune) | `Instance` |
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
- `Settings` : `{ language, default_ram_mb, java_path | null, trust_default, allow_relay, close_on_launch, relay_only, share_mods, discovery, lan_auto }` (voir `core/src/types.rs`). `lan_auto` (bool, défaut `true`, `serde(default)`, `settings_set` : optionnel) charge au lancement des instances perso l'agent Java `core/resources/cubelink-lan-agent.jar` (ressource Tauri ; `CUBELINK_LAN_AGENT=<jar>` le remplace ; jamais chargé en mode test sans cette variable) qui ouvre le monde solo au réseau local ; diagnostic : lignes `[Cubelink] LAN: ...` dans `latest.log`. Reconstruction du jar : `core/resources/lan-agent/build.sh` (JDK >= 17). `discovery` (bool, défaut `true`, `serde(default)` : anciens fichiers compatibles) active la découverte d'adresses publique n0 (pkarr + DNS) ; lu au démarrage du backend, sans effet quand le relais est désactivé (mode test hors réseau).
- Les schémas exacts de `BrowseQuery`, `BrowsePage`, `BrowseVersion`, `BrowseInstallInput`, `BrowseInstallResult` et `ModEntry` sont dans `core/src/browse.rs` et `core/src/mods.rs` ; le front les reprend dans `src/services/types.ts`.
- Le contrat du `invoke_handler` est dans `src-tauri/src/lib.rs` (deux listes identiques : mode normal et mode test, la seconde ajoute `test_state`).
- Fenêtre : `decorations: false` et permissions `core:window:allow-minimize` / `allow-close` (tâche U13).

## Ajouts (intégration)
- `host_start_sharing` `{ instance_id, world_port }` → — ; `host_stop_sharing` → — : partage d'un monde LAN avec les amis. Aucun écran ne les appelle : le partage est automatique (au passage de l'instance à `running`, manifeste partagé sans port ; port détecté dans la sortie du jeu `Started serving on N` / `Local game hosted on port N` ; arrêt à la fin du jeu). `world_port` accepte 0 (fichiers seulement) ; le dernier appel, manuel ou automatique, l'emporte.
- `join_play` `{ friend_public_key, instance_id }` → — (MULTI) : invité, « Rejoindre et jouer » en un geste : ouvre le tunnel vers le monde de l'ami, lance l'instance invitée avec connexion automatique (`--quickPlayMultiplayer 127.0.0.1:<port>`, Minecraft ≥ 1.20 ; `--server`/`--port` avant), referme le tunnel quand le jeu s'arrête. Événements de lancement habituels (`launch://state`). Erreurs françaises distinctes : ami hors ligne / monde pas encore ouvert.
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
