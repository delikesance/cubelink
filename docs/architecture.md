# Launcher Minecraft P2P : notes d'architecture

> Synthèse des décisions prises lors de la phase de cadrage (2026-11-09). Statut : conception, rien n'est implémenté.

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
- **Sécurité** : rejoindre un ami = exécuter son code. Prévoir : écran de confirmation listant ce qui sera installé ; niveau de confiance par ami (« toujours accepter » ou « demander ») ; marquage des mods dont le hash est inconnu de Modrinth (lookup par hash) comme « custom, non vérifié ».
- **Licences** : transfert privé entre amis, sans hébergement central, risque faible, mais pas une garantie juridique. Ne pas passer par l'API CurseForge pour ce transfert.
- **Cas à part** : mods client-only vs serveur ; compatibilité de versions MC/loader.

## 3. Architecture système

### Besoin de serveur central ?
**Non pour les comptes, la liste d'amis et la présence.** Seul un **relais** (sans état) est nécessaire.

- **Identité** : paire de clés ed25519 générée au premier lancement ; la clé publique = ID du nœud. L'auth Microsoft se fait directement client ↔ Microsoft.
- **Liste d'amis** : stockée en local (SQLite).
- **Présence** : le launcher contacte directement les clés de ses amis ; s'ils répondent, ils sont en ligne.
- **Découverte d'IP** : DNS/DHT d'Iroh.
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
1. B clique sur « Mon code ami » : il obtient une chaîne courte (ex. `mcl1-...`), un lien `launcher://add/...` ou un QR code. Le code contient sa clé publique + indications de connexion (relais d'attache, adresses). Aucun secret, pas d'IP en clair : il peut être posté sur Discord.
2. A colle le code dans « Ajouter un ami ».
3. Le launcher de A contacte B par sa clé publique (découverte DNS/DHT ou relais d'attache, puis hole punching). A n'a jamais besoin de connaître l'IP.
4. A envoie une demande d'ami signée (pseudo Minecraft + clé publique).
5. B accepte ; les deux enregistrent l'autre localement.

**Problème : B doit être en ligne.** Deux solutions :
- **Code à double sens** (retenu pour le MVP) : A donne aussi son code à B ; dès que les deux se contactent, c'est validé automatiquement (consentement mutuel).
- Demande en attente : le launcher de A réessaie périodiquement tant que B est hors ligne.

**Sécurité :**
- Le code ne contient que du public : un inconnu peut seulement envoyer une demande, que l'on refuse. Le code peut être **régénéré** pour invalider l'ancien.
- Toute communication est authentifiée par les clés : impossible de se faire passer pour B.
- L'ami voit techniquement l'IP (comme toute connexion P2P directe) ; option « relais uniquement » pour la masquer, au prix de la latence.
- Le pseudo Minecraft affiché est **déclaratif** (non vérifié).

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
- Format exact du manifeste et protocole de transfert (chunks, reprise).
- Format précis du code ami et des messages d'ajout.
- Choix du hash : SHA-256 (manifeste) vs BLAKE3 (natif Iroh-blobs).
- Politique de limitation du relais (débit, taille).
