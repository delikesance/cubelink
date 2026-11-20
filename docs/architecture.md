# Launcher Minecraft P2P : notes d'architecture

> Architecture de référence, état au 2026-11-20. Le cadrage initial (2026-11-09) est conservé en annexe (§9) ; le reste du document décrit l'état du code. Le MVP, le social P2P, le parcours de catalogue, la Scène (jeu dans la fenêtre du launcher), Hearth (serveurs de groupe, en cours par phases), le mode fond, la mise à jour automatique et la refonte d'interface « clay doux » sont en place (voir « État d'implémentation », §8).

## 1. Vision

Un launcher Minecraft avec :
- accès aux catalogues **Modrinth** et **CurseForge** et aux modpacks (pour construire sa propre instance) ;
- installation d'un modpack puis lancement du jeu ;
- possibilité de **rejoindre le monde d'un ami** : le launcher installe automatiquement le bon loader et les mods exacts de l'hôte ;
- cache pour ne jamais retélécharger deux fois le même fichier ;
- le tout en **pair à pair**, sans serveur central à maintenir.

Le différenciateur est « clique sur un ami, tu as ses mods et tu es dans sa partie » (verdict de faisabilité du cadrage : annexe, §9).

## 2. Décision clé : source de vérité = les fichiers de l'hôte

Quand on rejoint un ami, on **télécharge les mods depuis l'hôte** (pas depuis CurseForge/Modrinth). Avantages :
- versions exactement identiques ;
- permet de tester un mod créé par l'hôte, absent de tout catalogue ;
- aucune dépendance à l'API CurseForge pour le join.

Les catalogues ne servent qu'à **construire sa propre instance** avant de jouer.

### Fonctionnement du join
1. L'hôte publie un **manifeste** : version de Minecraft, loader + version exacte, et pour chaque fichier nom, taille, hash (SHA-256).
2. L'invité compare avec son **cache local** (stockage adressé par hash). Seuls les fichiers manquants sont demandés à l'hôte, par chunks vérifiés par hash.
3. Le launcher crée un **profil invité isolé** : installe le bon loader, place les mods en hardlink depuis le cache, adapte la version de Java (le Java des Paramètres n'est retenu que si sa version convient, sinon sélection ou installation automatique ; un Java propre à l'instance est imposé), lance le jeu. L'instance perso n'est pas touchée.
4. Le manifeste inclut aussi configs, resource packs et shaders (certains mods exigent des configs identiques).

### Risques à gérer
- **Sécurité** : rejoindre un ami = exécuter son code. Prévoir : écran de confirmation listant ce qui sera installé ; niveau de confiance par ami (« toujours accepter » ou « demander ») ; marquage des mods dont le hash est inconnu de Modrinth et de CurseForge (lookup par hash / empreinte) comme « custom, non vérifié ».
- **Licences** : transfert privé entre amis, sans hébergement central, risque faible, mais pas une garantie juridique. Ne pas passer par l'API CurseForge pour ce transfert.
- **Cas à part** : mods client-only vs serveur ; compatibilité de versions MC/loader.

## 3. Architecture système

### Besoin de serveur central ?
**Non pour les comptes, la liste d'amis et la présence.** Aucun serveur n'est utilisé pour relier deux amis (#55 : n0 retiré, ni relais ni découverte).

- **Identité** : paire de clés ed25519 générée au premier lancement ; la clé publique = ID du nœud. L'auth Microsoft se fait directement client ↔ Microsoft.
- **Liste d'amis** : stockée en local (SQLite).
- **Présence** : le launcher contacte directement les clés de ses amis ; s'ils répondent, ils sont en ligne.  La réponse porte aussi « un jeu tourne chez moi » (un octet facultatif après `PONG`, ignoré par les anciennes versions) : on ne propose « Demander à rejoindre » qu'à un ami en jeu.
- **Découverte d'IP** : les adresses IP du code ami (jusqu'à 8 : LAN, IPv6, adresse ouverte sur la box) ; aucune découverte publique (ni n0, ni DNS, ni pkarr) : un ami n'est joignable que par ces indices et par les adresses mémorisées. La découverte par DHT Mainline, sans tiers, est une piste future non implémentée (#136, voir §7).
- **Relais : aucun** (#55). L'endpoint est créé avec `RelayMode::Disabled` (`build_endpoint`, `p2p/src/relay.rs`) : ni relais publics n0, ni relais d'attache, ni réglage « relais » (les anciens `allow_relay` / `relay_only` / `discovery` des `settings.json` sont ignorés puis supprimés à la réécriture). Sans relais, iroh ne fait plus de hole punching coordonné ni de mappage de port : Cubelink ouvre lui-même le port UDP sur la box (UPnP, NAT-PMP, PCP, `p2p/src/portmap.rs`, toujours actif sauf en mode test) et annonce l'adresse obtenue.
- **Régression de joignabilité assumée** : deux amis derrière un NAT strict ou un CGNAT, sans IPv6 ni UPnP, peuvent ne pas se joindre ; il suffit que l'un des deux soit joignable (IPv6 publique ou port ouvert). En attendant la DHT Mainline (#136) et d'éventuels relais entre amis, c'est le prix de zéro tiers. L'écran Réglages → Réseau (« Tester ma connexion ») indique si l'appareil est probablement joignable (`net_test` : IPv6 globale, IPv4 publique, port ouvert ; un pare-feu peut encore filtrer une adresse publique, ce que le test ne voit pas). Le code ami classe ses 8 adresses : port ouvert sur la box, IPv6 globale, IPv4 publique, puis privées, lien local, loopback (`endpoint_addr_info`).

### Schéma
```
[Launcher A] <-- QUIC direct (IPv6, LAN ou port ouvert sur la box) --> [Launcher B]

          aucun relais, aucun annuaire, aucun tiers
```

Chaque launcher contient : client Tauri, nœud P2P (Iroh), store de blobs adressé par hash, tunnel QUIC <-> localhost pour Minecraft (l'hôte ouvre son monde en LAN), base SQLite locale (amis, cache).

### Hébergement
- **Aucune infrastructure tierce ni hébergée par le projet** : pas de serveur, pas de relais, pas d'annuaire.
- **Mises à jour du launcher** : GitHub Releases, lues par le client (voir §6.4). C'est un téléchargement, pas un service que le projet fait tourner.
- **Piste future, non décidée ni implémentée** : un réseau de relais Cubelink tenus par les amis eux-mêmes (relais entre amis, jamais un serveur du projet), et la découverte par la DHT Mainline sans tiers (#136, voir §7). Règle de conception si des relais existent : gros fichiers **en direct uniquement**, ou débit plafonné (le jeu consomme ~50-200 kbit/s par joueur ; un transfert de mods par un relais coûterait cher en bande passante).

### Trajectoire de coût
1. **0 €** (situation actuelle et visée) : connexions directes + GitHub Releases.
2. **Plus tard, si besoin, sans infrastructure propriétaire** : relais entre amis et DHT Mainline (#136). Aucun budget d'hébergement n'est prévu.

### Limites connues
- Pas d'**invitation hors ligne** (pas de boîte aux lettres sans serveur).
- Rien ne **prouve l'identité Minecraft** d'un pair sur le réseau P2P (le serveur de l'hôte vérifie toutefois la session Mojang à la connexion).
- Rejoindre des **inconnus** (IP exposée, griefing, code arbitraire) : commencer par amis uniquement.

## 4. Ajouter un ami

**Flux :**
1. B clique sur « Mon code ami » : il obtient une chaîne courte (`mcl1-…`) ou un lien `launcher://add/…`. Le code contient sa clé publique ed25519, un **jeton aléatoire révocable** de 16 octets (renouvelé par « Générer un nouveau code » : l'hôte refuse alors les nouvelles demandes portant l'ancien) et des indices de connexion : l'URL du relais d'attache et, **seulement si le relais est désactivé**, jusqu'à 8 adresses IP directes (`code_hints`, `src-tauri/src/iroh_backend.rs`). Pas un secret d'authentification (l'identité est prouvée par les signatures) ; par défaut pas d'IP : il peut être posté sur Discord. Le CRC-32 ne détecte que les fautes de frappe, il n'apporte aucune sécurité.
2. A colle le code dans « Ajouter un ami ».
3. Le launcher de A contacte B par sa clé publique, via le relais d'attache du code puis hole punching (Iroh/QUIC). A n'a jamais besoin de connaître l'IP. La découverte publique (pkarr/DNS n0) existe dans `p2p/src/relay.rs` mais est **désactivée en dur** (`address_lookup: false`, `iroh_backend.rs`) : un pair n'est joignable que par les indices de son code.
4. A et B échangent des messages signés sur l'ALPN `cubelink/friend/1` (`p2p/src/friend_proto.rs`) : demande (pseudo + jeton du code + engagement `H(clé|nonce)`, type 5 ; l'hôte la refuse par une fermeture applicative de code 2 si le jeton est absent ou périmé, avant toute admission), engagement de B, révélation des deux nonces, puis décision signée de B. Les engagements empêchent de « grinder » le code de vérification.
1. B clique sur « Mon code ami » : il obtient une chaîne courte (`mcl1-…`) ou un lien `launcher://add/…`. Le code contient sa clé publique ed25519, un `epoch` (régénération) et des indices de connexion : jusqu'à 8 adresses IP directes (`code_hints`, `src-tauri/src/iroh_backend.rs`), jamais de relais. Il révèle donc tes adresses IP à quiconque le reçoit : ne le poste pas en public (Discord, etc.), seulement à des personnes de confiance. Le CRC-32 ne détecte que les fautes de frappe, il n'apporte aucune sécurité.
2. A colle le code dans « Ajouter un ami ».
3. Le launcher de A contacte B par sa clé publique, directement, aux adresses IP du code (Iroh/QUIC). Aucun relais ni service de découverte n'est configuré sur l'endpoint (`build_endpoint`, `p2p/src/relay.rs` : preset `Minimal`, `RelayMode::Disabled`, rien n'est publié ni interrogé) : un pair n'est joignable que par les indices de son code et ses adresses mémorisées. Un relais présent dans un ancien code (celui d'n0) est ignoré.
4. A et B échangent des messages signés sur l'ALPN `cubelink/friend/1` (`p2p/src/friend_proto.rs`) : demande (pseudo + engagement `H(clé|nonce)`), engagement de B, révélation des deux nonces, puis décision signée de B. Les engagements empêchent de « grinder » le code de vérification.
5. Les deux côtés calculent le même **code de vérification** (64 bits, « 3F9A · C21B · 77D0 · 5E14 ») à comparer à l'oral : c'est la seule protection contre un homme du milieu.
6. B accepte ; A crée l'ami à la réception d'une acceptation signée valide, B le crée dans `accept`. Les amis sont créés avec le niveau de confiance « demander ».

**Problème : B doit être en ligne.** Deux solutions :
- **Code à double sens** (retenu pour le MVP) : A donne aussi son code à B ; dès que les deux se contactent, c'est validé automatiquement (consentement mutuel).
- Demande en attente : le launcher de A réessaie périodiquement tant que B est hors ligne.

**Sécurité :**
- Le code ne contient que du public : un inconnu peut seulement envoyer une demande, que l'on refuse. Le code peut être **régénéré** pour invalider l'ancien.
- Toute communication est authentifiée par les clés : messages signés (séparation de domaine), expéditeur = clé de la connexion QUIC, horodatage à ±300 s, cache anti-rejeu des engagements.
- Anti-spam : 3 demandes par pair et par heure, 64 demandes en attente, 32 poignées de main simultanées, 1000 demandes stockées, trames de 4 Kio, délais de 15 s, décision attendue 10 min, rejet silencieux.
- Autorisation (`src-tauri/src/authz.rs`) : seuls les amis **acceptés** invités pour la partie ouvrent le tunnel de jeu (un membre du groupe Hearth servi n'a pas besoin d'être ami, voir §7) ; les fichiers de mods demandent en plus « envoyer des mods à mes amis ».
- L'ami voit techniquement l'IP (comme toute connexion P2P directe) et son code ami la contient ; il n'y a plus d'option « relais uniquement » (plus de relais). Masquer l'IP passerait par d'éventuels relais entre amis (piste, non décidé).
- Le pseudo Minecraft affiché est **déclaratif** (non vérifié) ; seule la clé fait foi.
- La comparaison du code de vérification n'est pas imposée par le protocole : si l'utilisateur accepte sans comparer, un homme du milieu passe.

**Stockage local par ami :** clé publique, pseudo MC, niveau de confiance (installer sans demander ou non), date d'ajout.

## 5. Stack recommandée
- **Tauri** (Rust + front web) : binaire léger, évite Electron.
- **Iroh** : QUIC, blobs adressés par hash (`iroh-blobs`) ; ni relais ni découverte (§3).
- **SQLite** : amis, index du cache.
- Cache : stockage adressé par hash, hardlinks vers les instances.
  - Le compteur `refs` de l'index n'est pas la source de vérité (les suppressions de mod, de sauvegarde ou d'instance ne le décrémentent pas, une réinstallation le compte deux fois). Il est **recalculé** avant chaque nettoyage : un objet est vivant tant qu'un fichier d'une instance (sauvegardes `.cubelink` comprises, hors `saves/`, `logs/`, `crash-reports/`, `screenshots/`, `art/`) en est un lien dur (`core/src/store/live.rs`). Une copie ne le retient pas.
  - `store::gc::sweep` recalcule puis supprime les objets délies plus anciens que `min_age` (3 jours au démarrage, ~1 minute après le lancement ; 1 jour après la suppression d'une instance, jamais moins : un objet déposé par une installation ou un transfert, ou réutilisé depuis le cache (`StoreIndex::touch`), pas encore lié, est protégé par son âge). Sont aussi balayés les `.mrpack` de Parcourir, jamais liés. Le balayage est sauté si une instance se prépare ou tourne, ou si un autre est en cours (`AppState::store_gc`).
  - Fail-safe : si un dossier d'instance ne peut pas être lu, l'ensemble vivant est incomplet et rien n'est supprimé. Magasin et instances sur deux volumes : pas de lien dur, donc tout le cache est balayé après `min_age` (les instances gardent leur copie).

## 6. Fonctions ajoutées depuis le cadrage

Le plan par phases du cadrage est en annexe (§9) ; il est terminé.

### 6.1 La Scène (crate `host/`)
Crate `cubelink-host` : le jeu tourne **dans la fenêtre du launcher**. Le jeu garde son processus ; sa fenêtre est adoptée dès sa création. Windows (`host/src/win/`) : fenêtre de premier niveau sans cadre, possédée par le launcher et calée sur la zone de jeu. Linux X11 (`host/src/x11/`) : fenêtre reparentée dans une sous-fenêtre du launcher. Le tronc commun est `host/src/scene.rs` (`GameHost`).
- `host/src/curtain.rs` : rideau de chargement, aux couleurs du launcher, qui couvre le changement de modpack ; `progress.rs` : étapes réelles du démarrage lues dans la sortie du jeu.
- `host/src/flow.rs` : déroulement de la vue (launcher, rideau, jeu), machine à états pure ; `guard.rs` et `health.rs` : quand la Scène renonce (le jeu retrouve sa propre fenêtre) et ce qu'elle annonce au launcher ; `foreground.rs` : premier plan.
- Côté application : `src-tauri/src/stage/` (commandes et état de la zone de jeu) ; panneau des invitations « rejoins mon monde » superposé au jeu (`src-tauri/src/invites.rs`, `e2e/18-invites-overlay.spec.ts`).
- **Wayland** : Wayland natif et macOS ne permettent pas d'adopter la fenêtre d'un autre processus. Sous Linux, Cubelink démarre en X11 (XWayland sous Wayland, `GDK_BACKEND=x11`, `src-tauri/src/lib.rs`) seulement si le réglage « Jeu intégré » est activé ; coupé, le launcher garde le Wayland natif et le jeu sa propre fenêtre.

### 6.2 Hearth (serveurs de groupe)
Un groupe d'amis partage un monde moddé que n'importe quel membre lance : bail d'hôte à deux phases (un seul serveur à la fois), monde versionné et signé, répliqué chez les membres. Toutes les boucles (bail, sondes, points de reprise) vivent en Rust ; le front affiche seulement l'événement `hearth://state`. Statut : en cours d'implémentation, par phases. Conception : `docs/hearth.md` ; cas limites : `docs/hearth-et-si.md`. Code : `core/src/hearth/`, `src-tauri/src/hearth.rs`, `src-tauri/src/commands_hearth.rs` ; E2E : `e2e/19-hearth.spec.ts`.

### 6.3 Mode fond et cycle de vie
Fermer la fenêtre la masque dans la zone de notification au lieu de quitter, pour que les amis restent joignables ; « Quitter Cubelink » est dans le menu de l'icône. Briques : feature Tauri `tray-icon`, `tauri-plugin-autostart` (réglage `start_with_os`, lancement masqué avec `--background`), `tauri-plugin-single-instance` ; le tout dans `src-tauri/src/background.rs`. Sans icône disponible (Linux sans bibliothèque d'icône) et en mode test, fermer quitte comme avant.

### 6.4 Mises à jour et paquets
Mise à jour automatique par `tauri-plugin-updater` (`src-tauri/src/commands_update.rs`) : `plugins.updater` de `src-tauri/tauri.conf.json` lit `latest.json` sur GitHub Releases (généré par `scripts/make-latest-json.mjs`). Paquets : `.msi` (updater) et `.msix` du Microsoft Store (`scripts/make-msix.ps1`) sous Windows ; AppImage et `.rpm` (Fedora, `scripts/rpm-smoke.sh`) sous Linux ; paquet Nix (`packaging/nix/`). `tauri.conf.json` ne liste que `msi` et `appimage` : le `.rpm` et le `.msix` sont produits par `.github/workflows/release.yml` (et `canary.yml` pour les pré-versions Linux). Détail du flux : `docs/release.md`.

## 7. Points ouverts
- Politique de limitation d'un éventuel relais entre amis (débit, taille), si cette piste est retenue.
- **Reconnexion après redémarrage** : traitée. `FriendsDb` passe en schéma v2 (migration automatique de la v1) avec une table `friend_addrs` ; les adresses d'amis sont rechargées au démarrage et rafraîchies à chaque connexion reçue, et l'adresse du demandeur est conservée à l'acceptation (test `src-tauri/tests/friend_reconnect.rs`). **Adresses stables** : le port UDP est fixe (tiré au premier lancement dans `p2p/udp_port`, repli sur un port libre s'il est occupé), donc les adresses mémorisées par les amis et l'adresse ouverte sur la box ne se périment plus à chaque démarrage ; la fusion des adresses observées (`merge_ips`) garde l'union bornée à 8, la plus récente en tête, et ne remplace jamais des IP connues par une seule observation. Aucun relais n'est mémorisé ni contacté. **Limites restantes** : si les DEUX nœuds changent d'adresse IP en même temps (nouvelle box, autre réseau), plus rien ne permet de se retrouver (plus de relais d'attache ni de découverte publique n0) tant que l'un n'a pas donné un nouveau code ; l'écran du code ami le redemande de lui-même quand les adresses changent (`friend://code_changed`). Un membre de groupe Hearth qui n'est pas ami n'a pas d'IP mémorisée tant qu'il ne nous a pas contactés depuis le démarrage : il est joignable seulement s'il se connecte d'abord à nous ; sonde, rapatriement de monde et tunnel vers lui affichent « hors ligne » sinon (`addr_of` et `addr_for` dans `src-tauri/src/iroh_backend.rs`). Piste : la DHT Mainline (#136) ou des relais entre amis.
- **n0 retiré complètement** (#55) : ni découverte (pkarr/DNS), ni relais publics n0. Les réglages `discovery`, `allow_relay` et `relay_only` et le code associé ont été supprimés.
- **Découverte : DHT Mainline (#136)** : piste future, sans tiers (le réseau BitTorrent existant, pas de serveur du projet), **non implémentée**. Elle retrouverait un ami dont l'adresse IP a changé sans nouveau code. Tant qu'elle n'existe pas, voir la régression de joignabilité assumée (§3).
- **Limites connues de confidentialité** : le statut « en jeu » est visible de tous les amis acceptés (octet facultatif du `PONG`) ; un membre de groupe Hearth qui n'est pas ami peut ouvrir un tunnel vers le monde du groupe ; la poignée de main QUIC est ouverte à qui connaît la clé publique (`p2p/src/presence.rs`) ; sans relais, le code ami contient des IP ; les images des descriptions de mods sont chargées depuis des hôtes tiers (`img-src https:`, `core/src/art.rs`) ; les sondes de présence (30 s) ne ralentissent pas pour un ami hors ligne.
- Fonction « amis ayant joué au modpack » : nouvelle donnée réseau, à cadrer (opt-in, amis acceptés seulement, protocole versionné et signé).
- Code de vérification : la comparaison est maintenant imposée dans l'interface (case à cocher avant « Accepter »), mais pas par le protocole ; code de 64 bits à allonger ?
- Cache anti-rejeu en mémoire (vidé au redémarrage).
- **Sauvegarde de `identity.key`** : traitée. Paramètres > Compte exporte l'identité et la liste d'amis dans un fichier `.cubelink-id` chiffré par une phrase de passe (format v1 : `p2p/src/identity_backup.rs`), et le restaure par dépôt dans `p2p/import-pending/` puis redémarrage (anciens fichiers gardés en `.bak`, `hearth/` mis de côté si la clé change). **Restent à faire** : clés de groupe Hearth dans le conteneur, chiffrement de la clé au repos (DPAPI ou trousseau), alerte « même identité sur deux PC », restauration proposée au premier lancement et rappel après le premier ami.

## 8. État d'implémentation (2026-11-20)

Workspace Rust `core` (logique), `p2p` (réseau), `host` (la Scène, §6.1), `src-tauri` (commandes et intégration) ; front React/TypeScript dans `src/`. `spike/p2p-tunnel` est exclu du workspace (prototype jetable).

| Domaine | État | Où |
|---|---|---|
| Auth Microsoft/Xbox/Minecraft (code d'appareil) | fait | `core/src/auth/` |
| Manifestes Mojang, libraries, natives, Java, lancement | fait | `core/src/mojang/`, `java*.rs`, `launch_*.rs` |
| Loaders | fait | `core/src/loader/` |
| Cache par hash | fait | `core/src/store/` |
| Modrinth, import `.mrpack`, parcours du catalogue | fait | `core/src/providers/`, `mrpack.rs`, `browse.rs` |
| CurseForge | client présent (sans avoir revérifié sa couverture) | `core/src/providers/curseforge*.rs` |
| Mods d'une instance (liste, activer, retirer, ajouter) | fait | `core/src/mods.rs` |
| Art d'instance (image auto du modpack, image perso) | fait | `core/src/art.rs` |
| Identité, code ami, ajout à double sens, présence | fait | `p2p/src/identity.rs`, `friend_*.rs`, `presence.rs` |
| Sauvegarde et restauration de l'identité (`.cubelink-id`) | fait (export, import, redémarrage ; Windows non essayé en réel) | `p2p/src/identity_backup.rs`, `src-tauri/src/commands_identity.rs` |
| Manifeste hôte, transfert par hash, reprise | fait | `core/src/manifest.rs`, `p2p/src/transfer/`, `manifest_channel.rs` |
| Profil invité, mods inconnus | fait | `core/src/guest.rs`, `unknown_mods.rs` |
| Tunnel de jeu | fait | `p2p/src/tunnel.rs` |
| Relais (réglages, indices du code) | fait | `p2p/src/relay.rs` |
| Journal applicatif (`logs/cubelink.log`) | fait : tout ce que Cubelink, le coeur, le P2P et Iroh disent par `tracing` ; 1 Mo puis `cubelink.log.1` (~2 Mo au plus) ; chaque ligne passe par `sanitize` ; `CUBELINK_LOG=debug` (syntaxe `EnvFilter`) pour diagnostiquer ; les panics y vont aussi | `src-tauri/src/app_log.rs` |
| Installeur Windows | NSIS + CLI Tauri (`target/release/bundle/nsis/`) | |
| Endpoint sans relais ni découverte, indices du code (IP) | fait | `p2p/src/relay.rs` |
| La Scène (jeu dans la fenêtre du launcher) | fait (Windows et Linux X11) | `host/`, `src-tauri/src/stage/` |
| Invitations « rejoins mon monde », demandes à rejoindre | fait | `src-tauri/src/invites.rs`, `p2p/src/join_request.rs` |
| Comptes multiples, session conservée | fait | `core/src/auth/`, `e2e/17-accounts.spec.ts` |
| Hearth (serveurs de groupe) | en cours, par phases (`docs/hearth.md`) | `core/src/hearth/`, `src-tauri/src/hearth.rs` |
| Mode fond (icône, démarrage automatique, instance unique) | fait | `src-tauri/src/background.rs` |
| Mise à jour automatique | fait | `src-tauri/src/commands_update.rs`, `scripts/make-latest-json.mjs` |
| Paquets | `.msi` (updater) + `.msix` (Microsoft Store, `scripts/make-msix.ps1`), AppImage + `.rpm` (Linux, `scripts/rpm-smoke.sh`), Nix | `.github/workflows/release.yml`, `canary.yml`, `docs/release.md` |

Tests : `npm test` (clippy `-D warnings`, `cargo test --workspace`, `tsc`, vitest) ; `npm run e2e` (Playwright, Windows seulement : 20 specs, `01` à `19` plus `05b`). Les specs `12`, `14`, `15` et `16` jouent sur le vrai jeu ou le vrai réseau (`E2E_REAL*`) et ne tournent pas dans la CI simulée.

Refonte d'interface « clay doux » appliquée (fond `#131116`, accent cyan `#4FC3E8`) : tokens dans `src/styles/tokens.css` (polices Inter et Space Grotesk embarquées), composants dans `src/ui/`, écrans dans `src/screens/` ; le design Hearth la prolonge (`docs/hearth.md`).

Limites connues : polices Inter et Space Grotesk non embarquées (repli système) ; pas de progression chiffrée pendant la préparation du jeu ; « amis ayant joué au modpack » non implémenté ; Échap depuis Mentions légales revient à l'accueil.
Limites connues : « amis ayant joué au modpack » non implémenté ; « Vider le cache » (Réglages) n'est pas branché sur une commande (le bouton dit que le vidage n'est pas encore disponible) ; Échap depuis Mentions légales revient à l'accueil.

## 9. Annexe : cadrage initial (archive, 2026-11-09)

> Historique : ne reflète pas l'état actuel. Seules les parties qui ne sont plus vraies ou plus utiles au code sont ici ; les décisions encore en vigueur sont aux §2 à §4.

**Verdict de faisabilité** : faisable, intéressant, avec une vraie place sur le marché. Le marché des launchers est saturé (Prism, Modrinth App, CurseForge App), donc un simple launcher de plus ne suffirait pas ; le différenciateur est « clique sur un ami, tu as ses mods et tu es dans sa partie ».

**Plan par phases (terminé)**
1. MVP : lancer le jeu, auth Microsoft, installer des mods Modrinth, cache par hash.
2. CurseForge + import de modpacks (`.mrpack`, zip CurseForge).
3. Ajout d'ami par code + tunnel P2P + sync par manifeste.
4. Cas avancés : jars custom, configs, découverte publique (la découverte publique est abandonnée : #55, #136).

**Spike suggéré (fait)** : deux machines, un tunnel Iroh vers un monde en LAN et un transfert par hash, pour valider le hole punching (`spike/p2p-tunnel/`, exclu du workspace). Depuis, le hole punching coordonné a disparu avec les relais (#55). Iroh est en production : la réserve « état d'Iroh à valider par un prototype » est levée.

Points tranchés depuis le cadrage : format du code ami (`mcl1-`, voir `p2p/src/friend_code.rs`), messages d'ajout (`friend_proto.rs`), protocole de transfert (`p2p/src/transfer/`), manifeste (`core/src/manifest.rs`). Hash : SHA-256 dans le manifeste ; BLAKE3 pour iroh-blobs.
