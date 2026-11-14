# Contrats : commandes et événements Tauri

Les commandes et leurs types sont générés depuis Rust (tauri-specta) : `src/services/bindings.ts` (fonctions `commands.*` et types) et le tableau ci-dessous. Après tout changement de commande ou de type échangé : `npm run bindings`, puis committer les deux fichiers. Le test `bindings::tests::liaisons_a_jour` (lancé par `cargo test`, donc par la CI) échoue s'ils ne correspondent plus au code.
Une commande qui échoue renvoie `Err(String)` (message d'erreur affichable, en français) : la promesse rejette avec ce message. Les types Rust sont dans `core/src/types.rs` et dans les modules des commandes ; le front les reçoit en JSON (noms de champs identiques, énumérations en minuscules).

## Commandes (invoke)
Arguments : clés JSON envoyées à Tauri (snake_case) et leur type TypeScript. Les commandes du mode test (`test_state`, `test_cut_guest_connection`) n'y figurent pas.

<!-- commandes : début (généré par src-tauri/src/bindings.rs, ne pas éditer) -->
| Commande | Arguments | Retour | Rôle |
|---|---|---|---|
| `auth_status` | — | `Profile \| null` | Profil du compte actif, `None` sans session. |
| `auth_accounts` | — | `AccountsState_Serialize` | Comptes enregistrés et identifiant du compte actif. |
| `auth_skin_head` | `uuid: string` | `string \| null` | Tête du skin (PNG 8×8 en data URL) d'un compte Microsoft, `None` si elle est indisponible (hors ligne, pas de skin). Mise en cache pour la session : un seul appel réseau par compte. |
| `skin_head_by_name` | `name: string` | `string \| null` | Tête du skin (data URL PNG 8×8) du joueur au pseudo `name`, pour la liste d'amis. `None` si le pseudo est inconnu de Mojang (compte hors ligne) ou le réseau indisponible. Mise en cache pour la session. |
| `auth_switch` | `id: string` | `Profile` | Rend le compte actif et renvoie son profil. |
| `auth_remove` | `id: string` | — | Retire un compte enregistré. |
| `auth_begin_login` | — | `DeviceCodeInfo` | Démarre la connexion Microsoft (code d'appareil à saisir sur la page de vérification). |
| `auth_poll_login` | — | `Profile` | Attend la validation du code d'appareil et renvoie le profil connecté. |
| `auth_logout` | — | — | Déconnecte le compte actif. |
| `auth_offline_login` | `name: string` | `Profile` | Crée ou reprend un compte hors ligne au pseudo donné. |
| `instances_list` | — | `Instance_Serialize[]` | Instances installées. |
| `instances_create` | `name: string`, `minecraft: string`, `loader: LoaderSpec`, `ram_mb: number` | `Instance_Serialize` | Crée une instance vide. |
| `instances_import_mrpack` | `path: string`, `name: string \| null`, `minecraft: string \| null`, `loader: LoaderSpec \| null`, `ram_mb: number \| null` | `Instance_Serialize` | Importe un .mrpack, un zip CurseForge ou un zip brut (les champs facultatifs ne servent qu'au zip brut). Avertissements par `import://warning`. |
| `instances_launch` | `id: string` | — | Lance l'instance. Progression par `launch://state`, sortie du jeu par `launch://log`. |
| `minecraft_versions` | — | `string[]` | Versions de Minecraft disponibles. |
| `instances_kill` | `id: string` | — | Arrête le jeu de l'instance. |
| `instances_delete` | `id: string` | — | Supprime l'instance (refusé si elle est en lancement ou en jeu). |
| `instances_update` | `id: string`, `name: string \| null`, `ram_mb: number \| null`, `java_path: string \| null`, `jvm_args: string \| null` | `Instance_Serialize` | Modifie les champs donnés d'une instance (les autres restent). |
| `instances_open_folder` | `id: string` | — | Ouvre le dossier de l'instance dans le gestionnaire de fichiers. |
| `instances_logs` | `id: string` | `string` | Fin de `latest.log` (ou `latest.log.old`) de l'instance. |
| `instances_art_image` | `instance_id: string`, `file: string`, `max_px: number \| null` | `string` | Data URL d'un fichier d'illustration (`icon` ou une capture), réduit à `max_px` de côté (512 par défaut). |
| `instances_art_set` | `instance_id: string`, `path: string`, `role: string` | `Instance_Serialize` | Remplace l'icône ou la bannière (`role` : "icon" ou "banner") par une image choisie par le joueur. |
| `instances_art_lookup` | `instance_id: string`, `auto: boolean \| null` | `Instance_Serialize` | Cherche l'illustration du modpack et l'applique. `auto` : recherche silencieuse de l'accueil (nom identique seulement). |
| `instances_description` | `instance_id: string` | `string \| null` | Description longue du modpack, en HTML déjà nettoyé ; `None` sans projet d'origine. |
| `net_test` | — | `NetTest_Serialize` | Test de connexion réseau (Paramètres → « Tester ma connexion »). |
| `friend_my_code` | — | `FriendCode` | Mon code ami (`mcl1-…`). |
| `friend_regenerate_code` | — | `FriendCode` | Génère un nouveau code ami (l'ancien ne marche plus). |
| `friend_list` | — | `FriendWithPresence_Serialize[]` | Amis, avec leur présence et la dernière fois vus en ligne. |
| `friend_send_request` | `code: string` | `VerifyCode` | Envoie une demande d'ami à partir de son code ; renvoie le code de vérification à comparer. |
| `friend_requests` | `offset: number`, `limit: number`, `query: string \| null` | `RequestsPage` | Demandes d'amis reçues, par page, filtrées par nom. |
| `friend_accept` | `public_key: string` | — | Accepte une demande d'ami. |
| `friend_refuse` | `public_key: string` | — | Refuse une demande d'ami. |
| `friend_refuse_all` | — | `number` | Refuse toutes les demandes ; renvoie leur nombre. |
| `join_plan` | `friend_public_key: string` | `JoinPlan_Serialize` | Plan du « Rejoindre » d'un ami : manifeste, fichiers à recevoir, fichiers à accepter et hash du manifeste montré. |
| `join_start` | `friend_public_key: string`, `ram_mb: number \| null`, `manifest_hash: string`, `accept_unknown: boolean \| null` | — | Démarre le transfert du plan montré (`manifest_hash`) ; `accept_unknown` exigé s'il y a des fichiers à accepter. Progression par `transfer://progress`. |
| `join_pause` | — | — | Met le transfert en pause. |
| `join_resume` | — | — | Reprend le transfert. |
| `join_cancel` | — | — | Annule le transfert. |
| `join_open_tunnel` | `friend_public_key: string` | `number` | Ouvre un tunnel local vers le monde LAN de l'ami ; renvoie le port de `127.0.0.1`. Un seul tunnel à la fois. |
| `join_close_tunnel` | — | — | Ferme le tunnel invité. |
| `join_play` | `friend_public_key: string`, `instance_id: string` | — | Ouvre le tunnel, lance l'instance invitée avec connexion automatique, referme le tunnel à la fin du jeu. `instance_id` vide : l'instance du dernier transfert. |
| `settings_get` | — | `Settings_Serialize` | Réglages. |
| `settings_set` | `settings: SettingsInput_Deserialize` | `Settings_Serialize` | Le front envoie les réglages dans `{ settings }`. |
| `browse_search` | `query: BrowseQuery_Deserialize` | `BrowsePage_Serialize` | Recherche dans le catalogue (Modrinth ou CurseForge). |
| `browse_versions` | `source: string`, `id: string`, `kind: string`, `minecraft: string \| null`, `loader: string \| null` | `BrowseVersion[]` | Versions d'un projet du catalogue, filtrées par version de Minecraft et loader. |
| `browse_install` | `input: BrowseInstallInput_Deserialize` | `BrowseInstallResult_Serialize` | Installe un projet du catalogue (nouvelle instance pour un modpack). Progression par `browse://progress`. |
| `mods_list` | `instance_id: string` | `ModEntry[]` | Mods d'une instance (activés ou non). |
| `mods_icon` | `instance_id: string`, `file: string` | `string \| null` | Icône d'un mod lue dans son jar (data URL 64 px), `None` s'il n'en a pas. |
| `mods_check` | `instance_id: string` | `ModCheck_Serialize` | Vérifie la compatibilité des mods installés (dépendances, incompatibilités, version, loader, doublons). |
| `browse_check` | `source: string`, `id: string`, `kind: string`, `instance_id: string`, `version_id: string \| null` | `ContentCheck_Serialize` | Compatibilité d'un mod du catalogue avec une instance (mods seulement ; autres types : résultat vide). |
| `mods_fix_plan` | `instance_id: string` | `FixPlan_Serialize` | Calcule les corrections possibles des problèmes de mods, sans rien modifier. |
| `mods_fix_apply` | `instance_id: string`, `plan_id: string`, `action_ids: string[]` | `FixResult_Serialize` | Applique les actions choisies du plan ; les fichiers remplacés sont sauvegardés (annulable). Progression par `mods-fix://progress`. |
| `mods_fix_undo` | `instance_id: string`, `backup_id: string` | `ModCheck_Serialize` | Annule une correction à partir de sa sauvegarde. |
| `mods_toggle` | `instance_id: string`, `file: string`, `enabled: boolean` | — | Active ou désactive un mod (`.disabled`). |
| `mods_remove` | `instance_id: string`, `file: string` | — | Supprime un mod de l'instance. |
| `mods_add` | `instance_id: string`, `path: string` | `ModEntry` | Ajoute un jar à l'instance (chemin absolu). |
| `host_start_sharing` | `instance_id: string`, `world_port: number` | — | Partage le monde LAN avec les amis (`world_port` 0 : fichiers seulement). Normalement automatique. |
| `host_stop_sharing` | — | — | Arrête le partage hôte. |
| `host_state` | — | `HostState_Serialize \| null` | État du partage hôte, `None` si rien n'est partagé. |
| `friend_invite` | `public_key: string` | — | Invite un ami dans mon monde ouvert (refusé si aucun monde n'est ouvert). |
| `friend_invites` | — | `Invite_Serialize[]` | Invitations reçues en attente. |
| `friend_invite_dismiss` | `id: string` | — | Écarte une invitation reçue. |
| `game_switch` | `instance_id: string` | — | Ferme le jeu en cours puis lance l'instance. |
| `game_current` | — | `string \| null` | Instance du jeu en cours (en préparation ou lancé), `None` sinon. |
| `game_close` | — | — | Ferme le jeu en cours sans rien relancer (dans la zone intégrée : fermeture propre, le monde est enregistré). |
| `game_join_friend` | `public_key: string`, `instance_id: string \| null` | — | Ferme le jeu si besoin puis rejoint l'ami (plan, transfert, jeu). Échoue s'il y a des fichiers à accepter. |
| `stage_view` | `game: boolean` | — | Bascule entre le jeu et le launcher (bouton de la barre de titre), avec l'animation de rideau. |
| `stage_state` | — | `StageState` | État de la zone de jeu intégrée (rattachée, jeu visible, panneau ouvert). |
| `stage_panel` | `open: boolean` | — | Ouvre (`true`) ou ferme (`false`) le panneau « en jeu » (Echap, bouton Fermer). |
| `stage_layout` | `x: number`, `y: number`, `w: number`, `h: number`, `panel_w: number` | — | Zone de jeu mesuree par l'interface (element reserve du DOM), en pixels physiques dans la zone cliente de la fenetre. |
| `stage_supported` | — | `boolean` | La zone de jeu integree existe sur cette plateforme (l'interface n'affiche le reglage « Jeu integre » que dans ce cas). |
| `update_state` | — | `UpdateState_Serialize` | État de la mise à jour automatique du launcher. |
| `update_check` | — | `UpdateState_Serialize` | Vérifie s'il existe une mise à jour ; si oui, le téléchargement démarre en arrière-plan. |
| `update_install` | — | — | Redémarre et installe la mise à jour téléchargée (refusé si une partie est en cours). |
<!-- commandes : fin -->

### Précisions
- `join_plan` : `{ manifest: Manifest, manifest_hash: string, cached: number, to_receive: number, to_receive_bytes: number, unknown_mods: string[], unknown_unchecked: string[], unknown_scripts: string[], unknown_trusted: string[] }` ; `manifest_hash` (hash canonique du manifeste montré) est à renvoyer à `join_start`. Demandent l'accord (case de confiance) : `unknown_mods` (mods inconnus), `unknown_unchecked` (jars non vérifiés : `sha1` absent, réseau, limite de débit ; traités comme inconnus) et `unknown_scripts` (scripts `kubejs/**/*.js`). Chaque liste ne contient que les fichiers pas encore acceptés : ceux acceptés lors d'un `join_start` précédent (même sha256, mémorisés dans `p2p/trusted_mods.json`) passent dans `unknown_trusted`. « Inconnu » = ni Modrinth (sha1) ni CurseForge (empreinte murmur2 `cf_fingerprint` du manifeste, avec la clé de Cubelink) ne reconnaissent le jar ; `sha1` et `cf_fingerprint` sont des revendications de l'hôte, revérifiées à la réception
- `join_start` : — (progression par événement). Refuse si le manifeste de l'hôte n'a plus le `manifest_hash` du dernier `join_plan` (« La partie de ton ami a changé… » : recharger le plan), ou si ce plan avait des fichiers à accepter et que `accept_unknown` n'est pas `true`. Avec `accept_unknown`, ces fichiers sont mémorisés comme acceptés. `game_join_friend` passe `accept_unknown: false` : il échoue s'il y a des fichiers à accepter
- `instances_launch` est asynchrone côté Rust : il rend la main tout de suite, la suite arrive par `launch://state`.

## Événements (listen)
Les types nommés (`Profile`, `TransferProgress`, `HostState`, `StageState`…) sont dans `src/services/bindings.ts` ; les autres charges utiles sont construites en JSON côté Rust et décrites dans `src/services/types.ts`.

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
| `auth://accounts` | `AccountsState` : liste des comptes et compte actif, à chaque changement |
| `host://state` | `HostState` (`{ instance_id, sharing, world_port \| null }`) : partage hôte prêt, monde ouvert au LAN, arrêt (`sharing: false`) |
| `friend://invite` | `Invite` : invitation reçue d'un ami |
| `friend://invites` | `Invite[]` : liste complète des invitations à chaque changement |
| `mods-fix://progress` | `{ done, total, label }` : progression de `mods_fix_apply` |
| `update://state` | `UpdateState` : état de la mise à jour automatique du launcher |
| `stage://state` | `StageState` (`{ attached, game, panel }`) : zone de jeu intégrée (jeu rattaché, jeu visible, panneau « en jeu » ouvert) |
| `stage://fallback` | `{ reason, session_off }` : le jeu n'a pas pu être affiché dans Cubelink et garde sa fenêtre ; `reason` va au journal `stage.log`, jamais au joueur ; `session_off` : repli jusqu'au redémarrage de Cubelink |
| `stage://fullscreen` | `boolean` : le jeu demande (F11) ou quitte le plein écran ; le launcher passe la fenêtre en plein écran tant que le jeu est rattaché et visible |

## Règles
- Le front n'accède ni au disque ni au réseau : tout passe par ces commandes.
- Le front appelle les commandes par `Services` (`src/services/types.ts`) : `src/services/tauri.ts` les relie aux fonctions générées, les tests de l'interface utilisent un faux service (`src/services/fake.ts`) avec les mêmes signatures. Aucun écran n'appelle `invoke` directement, la zone de jeu (`stage_*`) comprise.
- Le profil hors ligne (`auth_offline_login`) n'existe que si l'application est lancée avec `CUBELINK_TEST_MODE=1`.
- Les commandes Tauri sont déclarées avec `#[tauri::command(rename_all = "snake_case")]` et `#[specta::specta(rename_all = "snake_case")]` : le front envoie les arguments en snake_case, comme dans le tableau ci-dessus (Tauri 2 attend du camelCase par défaut). tauri-specta écrit les clés en camelCase quoi qu'on lui dise : `bindings.rs` les remet en snake_case à la génération.
- `Settings` (voir `core/src/types.rs`). `lan_auto` (bool, défaut `true`, `serde(default)`, `settings_set` : optionnel) charge au lancement des instances perso l'agent Java `core/resources/cubelink-lan-agent.jar` (ressource Tauri ; `CUBELINK_LAN_AGENT=<jar>` le remplace ; jamais chargé en mode test sans cette variable) qui ouvre le monde solo au réseau local ; diagnostic : lignes `[Cubelink] LAN: ...` dans `latest.log`. Reconstruction du jar : `core/resources/lan-agent/build.sh` (JDK >= 17). `discovery` (bool, défaut `true`, `serde(default)` : anciens fichiers compatibles) active la découverte d'adresses publique n0 (pkarr + DNS) ; lu au démarrage du backend, sans effet quand le relais est désactivé (mode test hors réseau).
- Les schémas exacts des types échangés sont dans `src/services/bindings.ts` (générés). Une structure que serde lit et écrit différemment y a deux variantes : `X_Serialize` (ce que Rust renvoie) et `X_Deserialize` (ce qu'il accepte) ; `src/services/types.ts` les nomme pour le front.
- La liste des commandes est dans `src-tauri/src/lib.rs` (macro `commands!`, source unique du gestionnaire d'invocation et des liaisons) : `specta_builder` pour le mode normal (avec la mise à jour), `test_builder` pour le mode test (avec `test_state` et `test_cut_guest_connection`, sans la mise à jour).
- `settings_set` reçoit `{ settings: SettingsInput }` : les champs de `Settings`, les plus récents facultatifs (absents : valeur actuelle gardée).
- Fenêtre : `decorations: false` et permissions `core:window:allow-minimize` / `allow-close` (tâche U13).

## Ajouts (intégration)
- `host_start_sharing` `{ instance_id, world_port }` → — ; `host_stop_sharing` → — : partage d'un monde LAN avec les amis. Aucun écran ne les appelle : le partage est automatique (au passage de l'instance à `running`, manifeste partagé sans port ; port détecté dans la sortie du jeu `Started serving on N` / `Local game hosted on port N` ; arrêt à la fin du jeu). `world_port` accepte 0 (fichiers seulement) ; le dernier appel, manuel ou automatique, l'emporte.
- `join_play` `{ friend_public_key, instance_id }` → — (MULTI) : invité, « Rejoindre et jouer » en un geste : ouvre le tunnel vers le monde de l'ami, lance l'instance invitée avec connexion automatique (`--quickPlayMultiplayer 127.0.0.1:<port>`, Minecraft ≥ 1.20 ; `--server`/`--port` avant), referme le tunnel quand le jeu s'arrête. Événements de lancement habituels (`launch://state`). Erreurs françaises distinctes : ami hors ligne / monde pas encore ouvert.
- `host_state` → `HostState | null` ; événement `host://state` `{ instance_id, sharing, world_port | null }` (MULTI) : état du partage hôte, émis à chaque changement dans `HostAuto` (partage prêt, monde ouvert au LAN = port détecté, arrêt avec `sharing:false`). Sert l'indice de l'accueil (« Tes amis peuvent te rejoindre » / « Ouvre ton monde au réseau local »).
- `test_state` → état de test (profil de données, URLs surchargées, port du tunnel) : enregistrée UNIQUEMENT si `CUBELINK_TEST_MODE=1`.
- Variables de test (ignorées sans `CUBELINK_TEST_MODE=1`, URLs limitées à 127.0.0.1/localhost) : `CUBELINK_AUTH_{MS,XBOX,XSTS,MC}_URL`, `CUBELINK_MOJANG_META_URL`, `CUBELINK_FABRIC_META_URL`, `CUBELINK_ADOPTIUM_URL`, `CUBELINK_MODRINTH_API_URL`, `CUBELINK_JAVA_PATH`, `CUBELINK_DATA_DIR`, `CUBELINK_RELAY_MODE`, `CUBELINK_SKIP_JAVA_PROBE`.
- `TransferProgress.error` (optionnel) en phase `failed`.
- `join_open_tunnel` `{ friend_public_key }` → `u16` ; `join_close_tunnel` → — (FIX-TAURI) : invité, ouvre un tunnel local vers le monde LAN de l'ami (ami connu, hôte en partage) et renvoie le port de `127.0.0.1` à saisir dans Minecraft (« Connexion directe »). Un seul tunnel à la fois (un nouvel appel ferme le précédent). `test_state.tunnel_port` = port du monde côté hôte (inchangé) ; `test_state.guest_tunnel_port` = port du tunnel côté invité (0 si fermé).
- `settings_set` refuse `relay_only = true` avec `allow_relay = false` (erreur française).
- Transfert d'invité figé : sans octet reçu pendant 15 s, le fichier est relancé avec l'adresse de l'hôte re-résolue (adresses IP directes périmées retirées si un relais est connu) ; sans octet pendant 60 s (hors pause) : `transfer://progress` phase `failed` avec `error` « le transfert est figé : aucune donnée reçue depuis N s… ». Test : `CUBELINK_TRANSFER_IDLE_SECS` (1 à 3600) fixe les 60 s ; le délai de relance vaut alors au plus la moitié.
- Mode test, `CUBELINK_RELAY_MODE=disabled` : le code ami contient les adresses directes (sans effet hors mode test).
- Signal « plus lentement » : `TransferProgress.via_relay` (déjà émis, issu de l'observateur de chemin p2p) ; p2p fournit `relay::Advice::SlowViaRelay` / `SLOW_MESSAGE_FR` (débit < 2 Mo/s pendant un moment) mais le schéma `TransferProgress` n'a pas de champ pour le porter : le front affiche l'avertissement quand `via_relay` est vrai et `speed_bps` bas. Aucun changement de schéma.
