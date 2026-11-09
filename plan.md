# Plan : Launcher Minecraft P2P

## Context
Launcher Minecraft (Tauri/Rust) avec catalogues Modrinth/CurseForge, cache de mods par hash, et join d'un ami en P2P : le launcher installe le loader et les mods exacts de l'hôte, sans serveur central (seulement un relais sans état). Architecture décidée dans `docs/architecture.md`. Le dossier du projet est quasi vide (seul ce document existe), donc tout est à créer.

Après approbation, ce plan sera écrit dans `C:\Users\delikesance\Projects\minecraft-launcher\plan.md`.

## Phase 0 : Spike P2P (1-2 jours, à faire en premier)
Valider le risque principal avant de construire le reste.
- Deux machines, un nœud Iroh chacune.
- Tunnel QUIC <-> `localhost` vers un monde Minecraft ouvert en LAN.
- Transfert d'un fichier par hash (iroh-blobs), avec mesure du taux de hole punching et du fallback relais.
- Fichiers : `spike/p2p-tunnel/` (crate Rust isolée, jetable).
- Livrable : rapport (versions d'Iroh, taux de connexion directe, latence, décision go/no-go sur Iroh vs libp2p).

## Phase 1 : MVP launcher solo
Tâches (dépendances entre parenthèses) :
1. Squelette Tauri + workspace Rust : `src-tauri/`, `src/` (front), crates `core/` (logique), `p2p/` (réseau).
2. Auth Microsoft/Xbox/Minecraft (1) : `core/src/auth.rs`.
3. Manifestes de version Mojang, assets, libraries, natives, détection/installation de Java (1) : `core/src/game/`.
4. Installation des loaders Fabric/Quilt/NeoForge/Forge (3) : `core/src/loader/`.
5. Cache adressé par hash + hardlinks + index SQLite (1) : `core/src/store/`.
6. Client Modrinth (recherche, versions, téléchargement via cache) (5) : `core/src/providers/modrinth.rs`.
7. UI : bibliothèque d'instances, recherche de mods, lancement (2, 6).
- Livrable : on peut installer un modpack Modrinth et lancer le jeu.

## Phase 2 : CurseForge et modpacks
- Client CurseForge (clé API, gestion de `allowModDistribution=false`) : `core/src/providers/curseforge.rs`.
- Import `.mrpack` et zip CurseForge : `core/src/modpack/`.

## Phase 3 : Social P2P (le vrai produit)
1. Identité : paire ed25519, stockage local sécurisé : `p2p/src/identity.rs`.
2. Code ami (format, régénération), ajout à double sens, liste d'amis SQLite : `p2p/src/friends.rs`.
3. Présence (ping direct des amis).
4. Manifeste hôte : version MC, loader, fichiers + SHA-256, configs/resource packs/shaders : `core/src/manifest.rs`.
5. Transfert par hash des fichiers manquants (chunks vérifiés, reprise) : `p2p/src/transfer.rs`.
6. Profil invité isolé : loader auto, mods en hardlink, Java adapté : `core/src/guest_profile.rs`.
7. Tunnel de jeu QUIC <-> localhost : `p2p/src/tunnel.rs`.
8. UX de sécurité : écran de confirmation des mods, niveau de confiance par ami, marquage des hashes inconnus de Modrinth.
- Livrable : un ami rejoint ton monde en un clic avec les mêmes mods.

## Phase 4 : Infra et durcissement
- Relais `iroh-relay` auto-hébergé (petit VPS), plafonnement du débit pour les gros transferts.
- Mises à jour du launcher via GitHub Releases.
- Cas avancés : jars custom, découverte publique (optionnelle, plus tard).

## Organisation (agents)
Proposition (3 coéquipiers max, périmètres de fichiers isolés), à lancer seulement après ton go explicite :
- A : `spike/` puis `p2p/` (réseau).
- B : `core/` (auth, jeu, loaders, store, providers).
- C : `src/` + `src-tauri/` (UI et intégration).
Chaque tâche a un livrable vérifiable (code, tests, rapport). Tests et lint passés avant de marquer une tâche terminée. Pas de push forcé ni de merge sans validation humaine. Synthèse à la fin.

## Vérification
- Phase 0 : test réel sur deux réseaux différents, rapport chiffré.
- Phase 1 : lancer Minecraft avec un modpack Modrinth installé via le launcher ; `cargo test`, `cargo clippy`.
- Phase 3 : deux machines, ajout d'ami par code, join avec manifeste, vérification que les hashes des mods de l'invité correspondent à ceux de l'hôte, y compris un mod maison absent des catalogues.

## Points ouverts
- Hash : SHA-256 (manifeste) vs BLAKE3 (natif iroh-blobs).
- Format précis du code ami et du protocole de transfert.
- Politique de limitation du relais.
