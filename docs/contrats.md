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
| friends | `friend_my_code` | — | `FriendCode` |
| friends | `friend_regenerate_code` | — | `FriendCode` |
| friends | `friend_list` | — | `{ friend: Friend, presence: Presence }[]` |
| friends | `friend_send_request` | `{ code }` | `VerifyCode` |
| friends | `friend_requests` | `{ offset, limit, query? }` | `{ items: FriendRequest[], total: number }` |
| friends | `friend_accept` / `friend_refuse` | `{ public_key }` | — |
| friends | `friend_refuse_all` | — | `number` (nombre refusé) |
| join | `join_plan` | `{ friend_public_key }` | `{ manifest: Manifest, cached: number, to_receive: number, to_receive_bytes: number, unknown_mods: string[] }` |
| join | `join_start` | `{ friend_public_key }` | — (progression par événement) |
| join | `join_pause` / `join_resume` / `join_cancel` | — | — |
| settings | `settings_get` / `settings_set` | `Settings` | `Settings` |

## Événements (listen)
| Événement | Charge utile |
|---|---|
| `auth://state` | `Profile \| null` |
| `launch://state` | `{ id, state: "preparing" \| "running" \| "exited" \| "failed", message? }` |
| `transfer://progress` | `TransferProgress` (champ optionnel `error` en phase `failed`) |
| `friend://presence` | `{ public_key, presence }` |
| `friend://request` | `FriendRequest` |

## Règles
- Le front n'accède ni au disque ni au réseau : tout passe par ces commandes.
- Les tests de l'interface utilisent un faux service (`src/services/fake.ts`, propriété de la tâche U1) qui respecte exactement ces signatures.
- Le profil hors ligne (`auth_offline_login`) n'existe que si l'application est lancée avec `CUBELINK_TEST_MODE=1`.
- Les commandes Tauri sont déclarées avec `#[tauri::command(rename_all = "snake_case")]` : le front envoie les arguments en snake_case, comme dans le tableau ci-dessus (Tauri 2 attend du camelCase par défaut).
- `Settings` : `{ language, default_ram_mb, java_path | null, trust_default, allow_relay, close_on_launch }` (voir `core/src/types.rs`).
- Fenêtre : `decorations: false` et permissions `core:window:allow-minimize` / `allow-close` (tâche U13).

## Ajouts (intégration)
- `host_start_sharing` `{ instance_id, world_port }` → — ; `host_stop_sharing` → — : partage d'un monde LAN avec les amis (aucun écran ne l'appelle encore : le design n'en prévoit pas).
- `test_state` → état de test (profil de données, URLs surchargées, port du tunnel) : enregistrée UNIQUEMENT si `CUBELINK_TEST_MODE=1`.
- Variables de test (ignorées sans `CUBELINK_TEST_MODE=1`, URLs limitées à 127.0.0.1/localhost) : `CUBELINK_AUTH_{MS,XBOX,XSTS,MC}_URL`, `CUBELINK_MOJANG_META_URL`, `CUBELINK_FABRIC_META_URL`, `CUBELINK_ADOPTIUM_URL`, `CUBELINK_MODRINTH_API_URL`, `CUBELINK_JAVA_PATH`, `CUBELINK_DATA_DIR`, `CUBELINK_RELAY_MODE`, `CUBELINK_SKIP_JAVA_PROBE`.
- `TransferProgress.error` (optionnel) en phase `failed`.
- `join_open_tunnel` `{ friend_public_key }` → `u16` ; `join_close_tunnel` → — (FIX-TAURI) : invité, ouvre un tunnel local vers le monde LAN de l'ami (ami connu, hôte en partage) et renvoie le port de `127.0.0.1` à saisir dans Minecraft (« Connexion directe »). Un seul tunnel à la fois (un nouvel appel ferme le précédent). `test_state.tunnel_port` = port du monde côté hôte (inchangé) ; `test_state.guest_tunnel_port` = port du tunnel côté invité (0 si fermé).
- `instances_launch` est asynchrone côté Rust (même signature JS : `{ id }` → —).
- `settings_set` refuse `relay_only = true` avec `allow_relay = false` (erreur française).
- Transfert d'invité figé : sans octet reçu pendant 15 s, le fichier est relancé avec l'adresse de l'hôte re-résolue (adresses IP directes périmées retirées si un relais est connu) ; sans octet pendant 60 s (hors pause) : `transfer://progress` phase `failed` avec `error` « le transfert est figé : aucune donnée reçue depuis N s… ». Test : `CUBELINK_TRANSFER_IDLE_SECS` (1 à 3600) fixe les 60 s ; le délai de relance vaut alors au plus la moitié.
- Mode test, `CUBELINK_RELAY_MODE=disabled` : le code ami contient les adresses directes (sans effet hors mode test).
- Signal « plus lentement » : `TransferProgress.via_relay` (déjà émis, issu de l'observateur de chemin p2p) ; p2p fournit `relay::Advice::SlowViaRelay` / `SLOW_MESSAGE_FR` (débit < 2 Mo/s pendant un moment) mais le schéma `TransferProgress` n'a pas de champ pour le porter : le front affiche l'avertissement quand `via_relay` est vrai et `speed_bps` bas. Aucun changement de schéma.
