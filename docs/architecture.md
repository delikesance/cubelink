# Launcher Minecraft P2P : notes d'architecture

> Synthèse des décisions prises lors de la phase de cadrage (2026-11-09), mise à jour le 2026-11-10. Statut : le MVP, le social P2P et le parcours de catalogue sont implémentés (voir « État d'implémentation » en fin de document) ; la refonte du design est en cours dans l'artifact « Cubelink Design » et n'est pas encore appliquée au code.

## 1. Vision

Un launcher Minecraft avec :
- accès aux catalogues **Modrinth** et **CurseForge** et aux modpacks (pour construire sa propre instance) ;
- installation d'un modpack puis lancement du jeu ;
- possibilité de **rejoindre le monde d'un ami** : le launcher installe automatiquement le bon loader et les mods exacts de l'hôte ;
- cache pour ne jamais retélécharger deux fois le même fichier ;
- le tout en **pair à pair**, sans serveur central à maintenir.

Verdict de faisabilité : faisable, intéressant, avec une vraie place sur le marché (le différenciateur est « clique sur un ami, tu as ses mods et tu es dans sa partie »). Le marché des launchers est saturé (Prism, Modrinth App, CurseForge App), donc un simple launcher de plus ne suffirait pas.

## 2. Décision clé : source de vérité = les fichiers de l'hôte

Quand on rejoint un ami, on **télécharge les mods depuis l'hôte** (pas depuis CurseForge/Modrinth). Avantages :
- versions exactement identiques ;
- permet de tester un mod créé par l'hôte, absent de tout catalogue ;
- aucune dépendance à l'API CurseForge pour le join.

Les catalogues ne servent qu'à **construire sa propre instance** avant de jouer.

### Fonctionnement du join
1. L'hôte publie un **manifeste** : version de Minecraft, loader + version exacte, et pour chaque fichier nom, taille, hash (SHA-256).
2. L'invité compare avec son **cache local** (stockage adressé par hash). Seuls les fichiers manquants sont demandés à l'hôte, par chunks vérifiés par hash.
3. Le launcher crée un **profil invité isolé** : installe le bon loader, place les mods en hardlink depuis le cache, adapte la version de Java, lance le jeu. L'instance perso n'est pas touchée.
4. Le manifeste inclut aussi configs, resource packs et shaders (certains mods exigent des configs identiques).

### Risques à gérer
- **Sécurité** : rejoindre un ami = exécuter son code. Prévoir : écran de confirmation listant ce qui sera installé ; niveau de confiance par ami (« toujours accepter » ou « demander ») ; marquage des mods dont le hash est inconnu de Modrinth et de CurseForge (lookup par hash / empreinte) comme « custom, non vérifié ».
- **Licences** : transfert privé entre amis, sans hébergement central, risque faible, mais pas une garantie juridique. Ne pas passer par l'API CurseForge pour ce transfert.
- **Cas à part** : mods client-only vs serveur ; compatibilité de versions MC/loader.

## 3. Architecture système

### Besoin de serveur central ?
**Non pour les comptes, la liste d'amis et la présence.** Seul un **relais** (sans état) est nécessaire.

- **Identité** : paire de clés ed25519 générée au premier lancement ; la clé publique = ID du nœud. L'auth Microsoft se fait directement client ↔ Microsoft.
- **Liste d'amis** : stockée en local (SQLite).
- **Présence** : le launcher contacte directement les clés de ses amis ; s'ils répondent, ils sont en ligne.  La réponse porte aussi « un jeu tourne chez moi » (un octet facultatif après `PONG`, ignoré par les anciennes versions) : on ne propose « Demander à rejoindre » qu'à un ami en jeu.
- **Découverte d'IP** : relais d'attache indiqué dans le code ami ; la découverte publique d'Iroh (pkarr/DNS n0) est prévue mais désactivée par défaut (voir §4 et §7).
- **Relais** : nécessaire car le hole punching échoue dans ~10-20 % des cas (NAT symétrique, réseaux stricts). Il établit la connexion puis transporte le trafic en dernier recours. Binaire `iroh-relay`, ne stocke rien.

### Schéma
```
[Launcher A] <-- QUIC direct (hole punching) --> [Launcher B]
      |                                              |
      +---- relais (fallback uniquement) ------------+
                     ^
          1 petit serveur iroh-relay
          (ou relais publics au début)
```

Chaque launcher contient : client Tauri, nœud P2P (Iroh), store de blobs adressé par hash, tunnel QUIC <-> localhost pour Minecraft (l'hôte ouvre son monde en LAN), base SQLite locale (amis, cache).

### Hébergement
- **Vercel : non** pour le relais (serverless, pas de connexions longues). OK pour un site vitrine.
- **Mises à jour du launcher** : GitHub Releases (gratuit).
- **Petit hébergement (PaaS 256-512 Mo ou VPS)** : largement suffisant pour ~100 joueurs simultanés. Un relais ne fait que transférer des paquets ; le jeu consomme ~50-200 kbit/s par joueur.
- **Piège** : le transfert de mods via le relais coûte de la bande passante. Règle : transfert de gros fichiers **en direct uniquement**, ou débit plafonné sur le relais.
- Un **VPS** (type Hetzner) est préférable à un PaaS pour le coût du trafic.

### Trajectoire de coût
1. **0 €** : relais publics gratuits d'Iroh (best effort, pas pour la production) + GitHub Releases.
2. **3-5 €/mois** : son propre relais sur petit VPS (dizaines à centaines d'utilisateurs).
3. **Plus tard, si besoin** : API légère (SQLite/Postgres) pour des fonctions optionnelles (synchro d'amis multi-appareils, découverte publique). Pas requis pour le MVP.

### Limites connues
- Pas d'**invitation hors ligne** (pas de boîte aux lettres sans serveur).
- Rien ne **prouve l'identité Minecraft** d'un pair sur le réseau P2P (le serveur de l'hôte vérifie toutefois la session Mojang à la connexion).
- Rejoindre des **inconnus** (IP exposée, griefing, code arbitraire) : commencer par amis uniquement.
- L'état actuel d'Iroh (version, relais publics, stabilité de `iroh-blobs`) est à **valider par un prototype** avant de s'y engager.

## 4. Ajouter un ami

**Flux :**
1. B clique sur « Mon code ami » : il obtient une chaîne courte (`mcl1-…`) ou un lien `launcher://add/…`. Le code contient sa clé publique ed25519, un `epoch` (régénération) et des indices de connexion : l'URL du relais d'attache et, **seulement si le relais est désactivé**, jusqu'à 8 adresses IP directes (`code_hints`, `src-tauri/src/iroh_backend.rs`). Aucun secret ; par défaut pas d'IP : il peut être posté sur Discord. Le CRC-32 ne détecte que les fautes de frappe, il n'apporte aucune sécurité.
2. A colle le code dans « Ajouter un ami ».
3. Le launcher de A contacte B par sa clé publique, via le relais d'attache du code puis hole punching (Iroh/QUIC). A n'a jamais besoin de connaître l'IP. La découverte publique (pkarr/DNS n0) existe dans `p2p/src/relay.rs` mais est **désactivée en dur** (`address_lookup: false`, `iroh_backend.rs`) : un pair n'est joignable que par les indices de son code.
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
- Autorisation (`src-tauri/src/authz.rs`) : seuls les amis **acceptés** ouvrent le tunnel de jeu ; les fichiers de mods demandent en plus « envoyer des mods à mes amis ».
- L'ami voit techniquement l'IP (comme toute connexion P2P directe) ; option « relais uniquement » pour la masquer, au prix de la latence.
- Le pseudo Minecraft affiché est **déclaratif** (non vérifié) ; seule la clé fait foi.
- La comparaison du code de vérification n'est pas imposée par le protocole : si l'utilisateur accepte sans comparer, un homme du milieu passe.

**Stockage local par ami :** clé publique, pseudo MC, niveau de confiance (installer sans demander ou non), date d'ajout.

## 5. Stack recommandée
- **Tauri** (Rust + front web) : binaire léger, évite Electron.
- **Iroh** (ou libp2p) : QUIC, hole punching, relais, blobs adressés par hash.
- **SQLite** : amis, index du cache.
- Cache : stockage adressé par hash, hardlinks vers les instances.

## 6. Plan par phases
1. **MVP** : lancer le jeu, auth Microsoft, installer des mods Modrinth, cache par hash.
2. CurseForge + import de modpacks (`.mrpack`, zip CurseForge).
3. Ajout d'ami par code + tunnel P2P + sync par manifeste (le vrai produit).
4. Cas avancés : jars custom, configs, découverte publique.

**Prochaine étape suggérée : spike de 1-2 jours** : deux machines, un tunnel Iroh vers un monde en LAN, et un transfert de fichier par hash, pour valider le hole punching avant de construire le reste.

## 7. Points ouverts
- Politique de limitation du relais (débit, taille).
- **Reconnexion après redémarrage** : traitée. `FriendsDb` passe en schéma v2 (migration automatique de la v1) avec une table `friend_addrs` ; les adresses d'amis sont rechargées au démarrage et rafraîchies à chaque connexion reçue, et l'adresse du demandeur est conservée à l'acceptation (test `src-tauri/tests/friend_reconnect.rs`). **Limite restante** : si les DEUX nœuds changent d'adresse en même temps, il faut le relais (ou une découverte publique) pour se retrouver.
- Découverte publique optionnelle (pkarr/DNS n0) : décision de confidentialité à prendre (n0 voit la clé, le relais et l'IP).
- Fonction « amis ayant joué au modpack » : nouvelle donnée réseau, à cadrer (opt-in, amis acceptés seulement, protocole versionné et signé).
- Code de vérification : la comparaison est maintenant imposée dans l'interface (case à cocher avant « Accepter »), mais pas par le protocole ; code de 64 bits à allonger ?
- Cache anti-rejeu en mémoire (vidé au redémarrage) ; sauvegarde de `identity.key`.

Points tranchés depuis le cadrage : format du code ami (`mcl1-`, voir `p2p/src/friend_code.rs`), messages d'ajout (`friend_proto.rs`), protocole de transfert (`p2p/src/transfer/`), manifeste (`core/src/manifest.rs`). Hash : SHA-256 dans le manifeste ; BLAKE3 pour iroh-blobs.

## 8. État d'implémentation (2026-11-10)

Workspace Rust `core` (logique), `p2p` (réseau), `host` (la Scène, voir § 9), `src-tauri` (commandes et intégration) ; front React/TypeScript dans `src/`. `spike/p2p-tunnel` est exclu du workspace (prototype jetable).

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
| Manifeste hôte, transfert par hash, reprise | fait | `core/src/manifest.rs`, `p2p/src/transfer/`, `manifest_channel.rs` |
| Profil invité, mods inconnus | fait | `core/src/guest.rs`, `unknown_mods.rs` |
| Tunnel de jeu | fait | `p2p/src/tunnel.rs` |
| Relais (réglages, indices du code) | fait | `p2p/src/relay.rs` |
| Installeur Windows | NSIS + CLI Tauri (`target/release/bundle/nsis/`) | |

Tests : `npm test` (clippy `-D warnings`, `cargo test --workspace`, `tsc`, vitest) ; `npm run e2e` (Playwright, scénarios `e2e/01` à `11`).

Refonte d'interface « clay doux » (fond `#131116`, accent cyan unique `#4FC3E8`) : tokens dans `src/styles/tokens.css`, composants dans `src/ui/`, écrans refaits dans `src/screens/`. Corrigés dans le même lot : fenêtres de terminal (`CREATE_NO_WINDOW`), démarrage non bloquant du backend P2P, miniatures d'instance au lieu du base64.

Limites connues : polices Inter et Space Grotesk non embarquées (repli système) ; pas de progression chiffrée pendant la préparation du jeu ; « amis ayant joué au modpack » non implémenté ; « Vider le cache » sans effet réel ; Échap depuis Mentions légales revient à l'accueil.

## 9. La Scène (jeu intégré à la fenêtre du launcher)

Le jeu garde son processus (une JVM par modpack : Java, loader et natives diffèrent) ; sa fenêtre native est adoptée dans celle du launcher dès sa création, avant d'être affichée. Changer de modpack remplace le processus : un rideau (deux panneaux aux couleurs du launcher, barre de chargement) couvre l'intervalle. Le joueur la coupe par le réglage « Jeu intégré » (lu au lancement : un jeu déjà ouvert n'est jamais arraché à sa fenêtre).

### Découpe du code

- Crate `host` (`cubelink-host`) :
  - modules purs, testables sans fenêtre : `flow` (machine à états launcher / rideau / jeu), `guard` (quand renoncer), `health` (contrat avec le launcher : `HostEvent`, `FallbackReason`), `progress` (étapes du démarrage lues dans la sortie du jeu), `curtain`, `foreground`, `system` ;
  - `scene.rs` : `GameHost` et l'état du lancement, communs aux deux systèmes ; `win/` et `x11/` fournissent chacun un `Backend`.
- App : `src-tauri/src/stage/` (`mod.rs` : réglage, `staged`, replis ; `platform.rs` : ce qui dépend du système ; `commands.rs` : commandes `stage_*`, voir `docs/contrats.md`) et `src-tauri/src/stage_log.rs` (journal `logs/stage.log`).

### Par système

- **Windows** : la fenêtre du jeu reste une fenêtre de premier niveau (le jeu n'accepte clavier et souris que s'il est au premier plan, ce qu'une fenêtre fille n'obtient jamais), mais sans cadre, possédée par le launcher et calée sur la zone de jeu.
- **Linux, X11 ou XWayland** : deux modes d'adoption.
  - Reparentage dans une sous-fenêtre X11 du launcher (le focus clavier est possible) ; le rideau est une fenêtre ARGB `override-redirect`, traversée par la souris.
  - Mode transitoire (`WM_TRANSIENT_FOR`, sans décoration, `SKIP_TASKBAR`, calage en coordonnées écran, garage hors écran, panneau découpé par XShape). C'est le chemin par défaut dès qu'un gestionnaire de fenêtres tourne, ou si le reparentage est refusé. Il est vérifié sous Xvfb avec un gestionnaire simulé et avec `openbox` (`host/tests/x11_transient_*.rs`), pas sous Mutter, KWin ni XWayland : ne pas le tenir pour acquis ailleurs.
- **Wayland ou XWayland** : Wayland natif ne permet pas d'adopter la fenêtre d'un autre processus. Au démarrage, avant la création de la fenêtre, `lib.rs` pose `GDK_BACKEND=x11` (le launcher tourne donc sous XWayland quand la session est Wayland), seulement si un serveur X11 est joignable (`DISPLAY`), si `CUBELINK_NO_STAGE` est absent, si la Scène est permise (hors mode test) et si « Jeu intégré » est actif (réglage lu sur le disque). Sinon GTK garde le Wayland natif et le jeu garde sa fenêtre ; le réglage ne s'applique donc qu'au prochain démarrage de Cubelink. Un jeu de la Scène est lancé en XWayland (`RunOptions::force_x11`).
- **Wayland pur (sans `DISPLAY`) et macOS** : la Scène est inerte, le jeu garde sa fenêtre.

### Filets de sécurité

- Quand l'hôte ne peut pas tenir le jeu (surveillance ou rattachement refusé, aucune fenêtre dans le délai de garde), il renonce pour ce lancement : `HostEvent::Fallback`, la fenêtre est rendue au jeu, le lancement n'est plus `staged` et l'événement `stage://fallback` est émis.
- `MAX_FALLBACK_STREAK` (2) replis de suite coupent la Scène pour la session (le réglage enregistré ne bouge pas) jusqu'à `Stage::retry` (commande `stage_retry`) ou au redémarrage du processus.
- Un rideau fermé a toujours une suite : `abort_switch` (lancement qui n'a pas lieu après un changement de modpack) et `run_finished` (tâche de jeu arrêtée) le rouvrent sur le launcher.
- Coupe-circuit de développement : `CUBELINK_NO_STAGE=1` (la zone n'est même pas créée) ; `CUBELINK_STAGE_TEST` permet la Scène en mode test.
- Journal `logs/stage.log` (ouvert sous Windows et Linux, jamais en mode test), tournant en `stage.log.1` (environ 2 Mo au plus sur le disque).
