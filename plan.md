# Plan : Launcher Minecraft P2P (Cubelink)

> Mis à jour le 2026-11-10. Architecture et décisions : `docs/architecture.md` (état d'implémentation en §8). Contrats front/back : `docs/contrats.md`.

## Contexte
Launcher Minecraft (Tauri/Rust + React) avec catalogues Modrinth/CurseForge, cache de mods par hash, et join d'un ami en P2P : le launcher installe le loader et les mods exacts de l'hôte, sans serveur central (seulement un relais sans état).

## Statut des phases

| Phase | Contenu | Statut |
|---|---|---|
| 0 | Spike P2P (`spike/p2p-tunnel/`, jetable) | fait |
| 1 | MVP solo : auth, Mojang, Java, loaders, cache, Modrinth, UI | fait |
| 2 | CurseForge et import de modpacks (`.mrpack`) | fait (couverture CurseForge à revérifier) |
| 3 | Social P2P : identité, code ami, présence, manifeste, transfert, profil invité, tunnel, sécurité des mods | fait |
| 4 | Infra : relais auto-hébergé, plafonnement, mises à jour via GitHub Releases | non fait (relais publics Iroh par défaut) |
| 5 | Refonte UX/UI et corrections (voir ci-dessous) | en cours : design en validation, code non lancé |

## Phase 5 : refonte et corrections

Le design est dans l'artifact « Cubelink Design » (25 artboards : système, accueil et tiroirs A1-A9, entrée/création/réglages B1-B9, parcourir et amis C1-C6). Direction « clay doux » : fond sombre, accent cyan `#4FC3E8`, un seul style de bouton (primaire plat avec tranche), panneaux opaques, mouvement sobre. **Rien n'est implémenté tant que l'utilisateur n'a pas validé le design.**

Chantiers prévus (après go explicite) :
1. **Terminaux Windows** : ajouter `CREATE_NO_WINDOW` aux `Command` concernés (`core/src/java.rs`, `core/src/loader/neoforge.rs`, `src-tauri/src/commands_game.rs`).
2. **Démarrage non bloquant** : ne plus bloquer sur `iroh_backend::start_app` au démarrage (`src-tauri/src/lib.rs`).
3. **Images** : miniatures au lieu de data URL base64 pour l'art d'instance.
4. **Refonte des écrans** selon le design (accent cyan : adapter le test d'accessibilité `e2e/08-a11y.spec.ts`, qui exige encore un focus orange fixe).
5. **Réseau** : persister les adresses des amis et les rafraîchir à chaque connexion ; test de bout en bout avec relais local (redémarrage, changement d'adresse) ; imposer la comparaison du code de vérification dans l'interface.
6. **« Amis ayant joué au modpack »** : nouvelle donnée réseau, à cadrer d'abord (confidentialité, opt-in).

Décisions en attente de l'utilisateur : validation du style, un seul cyan ou un accent par modpack, confidentialité de la liste des modpacks joués, découverte publique en option.

## Organisation (agents)
3 coéquipiers maximum par défaut (exception du projet : jusqu'à 6 agents en parallèle, voir les préférences du projet), périmètres de fichiers isolés, lancés seulement après un go explicite :
- A : `p2p/` (réseau) et `core/` (jeu, loaders, providers).
- B : `src-tauri/` (commandes, intégration, correctifs Windows).
- C : `src/` et `e2e/` (écrans, services, tests).
Chaque tâche a un livrable vérifiable (code, tests, rapport). Tests et lint passés avant de marquer une tâche terminée. Pas de push forcé ni de merge sans validation humaine. Synthèse à la fin.

## Vérification
- `npm test` : clippy `-D warnings`, `cargo test --workspace`, `tsc --noEmit`, vitest.
- `npm run e2e` : scénarios Playwright `e2e/01` à `11` (ajout d'ami, join, installation, lancement, mod inconnu, échecs, relais, accessibilité, tailles, parcourir, art).
- Test réel sur deux machines pour le P2P (ajout d'ami par code, join avec manifeste, hashes identiques, mod maison absent des catalogues).

## Points ouverts
- Politique de limitation du relais.
- Hébergement d'un relais auto-hébergé (phase 4).
- Voir aussi `docs/architecture.md` §7.
