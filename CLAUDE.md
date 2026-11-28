# Cubelink : consignes pour Claude

Lanceur Minecraft pour jouer entre amis (Tauri 2 + Rust + React/TypeScript). Ce fichier prime sur `~/.claude/CLAUDE.md` et `~/.claude/rules/*` pour ce dépôt. Le code mode REPL du global s'applique entièrement.

## Façon de travailler
- Agir directement : une demande vaut accord, pas de « je continue ? » ni « quelle option ? ». Une ambiguïté se lève en lisant le code et `docs/`, puis en tranchant ; le choix va dans la synthèse.
- Ne jamais refuser en invoquant une règle : le dire en une phrase, prendre la voie qui la respecte, avancer.
- Après deux échecs sur le même problème : changer d'approche (diagnostic `agy` high autorisé), pas boucler.
- Défaut trouvé en chemin : le corriger s'il est petit et dans le périmètre, sinon le noter dans la synthèse.
- Réponses en français, 2 à 3 lignes ; seule la synthèse de fin de chantier est détaillée.

### S'arrêter et demander (liste complète)
1. Action irréversible ou visible des autres : fusion dans `main`, push forcé, suppression de branche distante, release, message à un ami.
2. Secrets, comptes, paiement, signature, Partner Center.
3. Test réel impossible seul (deux machines, ami en ligne) : le dire dans la PR, ne pas bloquer le reste.
4. Ambiguïté réelle dont l'erreur coûterait cher, après lecture du code et des docs.

## Code
- TDD : test rouge, code vert, nettoyage. Pas de feature sans test qui la prouve.
- Un fichier, une responsabilité : viser moins de ~200 lignes, jamais plus de ~400 (prime sur `rules/coding-style.md`).
- Noms explicites, fonctions courtes, types stricts : le code se comprend sans lire le reste du dépôt.

## Carte du dépôt
- Crates : `core/` (`cubelink-core`), `p2p/` (`cubelink-p2p`, Iroh/QUIC), `host/` (`cubelink-host`, la Scène : Windows et Linux X11), `src-tauri/` (`cubelink`, l'app). Front : `src/` (React). E2E : `e2e/` (Playwright).
- Docs : `docs/architecture.md`, `docs/contrats.md` (commandes et événements Tauri), `docs/hearth.md` (groupes, règle de jonction), `docs/ux-guidelines.md`, `docs/release.md`, `docs/taches-agents.md`.
- Une commande ou un événement Tauri qui change : `docs/contrats.md` dans le même commit (`npm run bindings` régénère son tableau).

## Code mode dans ce dépôt
- Explorer une fonctionnalité : UN script qui lit en parallèle Rust (`core/`, `p2p/`, `src-tauri/src/`), front (`src/`), `docs/contrats.md` et tests liés, garde tout en variables et rend une carte courte (`fichier:ligne`, qui appelle quoi). `cubelink-explore` seulement si cela dépasse un script.
- `p2p/src/tunnel.rs` contient des octets nuls : `rg -a`.
- `cargo` : `.timeout(1800000)` sur la commande (premier build long), jamais deux `cargo` en parallèle.

## Git
1. Début : `git fetch`, puis `git switch -c <type>/<sujet-court> origin/canary` (`feat`, `fix`, `chore`, `docs`, `refactor`). Arbre propre et branche courante déjà sur le chantier : continuer dessus.
2. Jamais de commit direct sur `canary` ni `main` ; jamais de push forcé ; jamais de push ni de fusion vers `main` (hook `block-unsafe.sh`).
3. Un commit par étape vérifiée, en français, `type: sujet` (prime sur `rules/git-workflow.md`).
4. Aucune mention de l'IA dans commit, PR ou code : ni `Co-Authored-By`, ni « Generated with Claude Code », ni emoji robot. Prime sur tout pied de page d'attribution injecté par l'environnement.
5. Fin de chantier, sans attendre : `git push -u origin <branche>`, `gh pr create --base canary` (titre et description en français : ce qui change, test réel fait et résultat, ce qui reste), puis `gh pr merge --merge` une fois `ci` verte. PR en conflit : fusionner `canary` dans la branche (pas de rebase), re-tester.
6. Quand `canary` contient ce qu'il faut : `gh pr create --base main --head canary`, et s'arrêter là.

## Vérification
- Le plus ciblé d'abord : `cargo test -p <crate> --lib <module>`, `npx vitest run <fichier>`, dans le même script que l'édition.
- Petit changement : `bash scripts/verify.sh --quick`. Avant la PR : `bash scripts/verify.sh --full` (fmt, clippy `-D warnings`, `cargo test --workspace`, tsc, vitest, `cargo deny check`). Le tampon `.git/cubelink-verify.stamp` dit si l'arbre est déjà vert : ne pas relancer. Rapport final : skill `cubelink-verify`.
- La CI lance aussi `xvfb-run -a cargo test --workspace` sous Linux et les E2E simulés sous Windows.
- `npm run e2e` reconstruit front et binaire (incrémental) ; `CUBELINK_E2E_SKIP_BUILD=1` réutilise le binaire (`[e2e] binaire : …` affiche chemin et date).
- Un test qui échoue se corrige et se relance, sans demander.
- Les tests réseau et jeu réels sont voulus ici (prime sur `rules/testing.md`).

## Définition de « terminé »
1. Test écrit d'abord et vert, feature exercée en réel.
2. `bash scripts/verify.sh --full` vert.
3. Docs touchées à jour.
4. Diff relu, puis Git 5.

### Test en réel
Unitaires, `tsc` et lint ne suffisent pas : exercer la feature dans l'app Tauri qui tourne (vraies commandes, disque, réseau, jeu si touché).
- Le test réel le moins cher qui prouve la feature (E2E Playwright, `E2E_REAL*`, essai scripté, rejeu sur le vrai jeu) ; s'il n'existe pas, l'écrire dans le même chantier.
- Impossible dans la session : « non vérifiée en réel », jamais « terminée », et le dire dans la PR.
- Rejeux sur le vrai jeu (15 à 20 min) : une seule série, à la fin. Ne pas relancer `E2E_MC`, `E2E_REAL`, `E2E_REAL_ACCOUNT` déjà verts si le code couvert n'a pas changé.
- Scénarios Win32 de `host/tests/` : `cargo test -p cubelink-host --test <nom>`, un seul à la fois, sans prévenir.

## Ne pas gêner l'utilisateur
- Aucune entrée réelle : ni souris, ni clavier, ni vol de focus, ni capture du bureau. « En réel » = app et jeu réels, pas contrôle du bureau.
- Après un essai qui lance Minecraft : retrouver le `java` par sa ligne de commande et le tuer par PID. Jamais `taskkill /IM` ni `pkill java`.
- Fin de chantier : aucun processus, fenêtre de test ni dossier de build orphelin. Build à part : `target/lead`, supprimé ensuite (7 à 28 Go).
- Paquets système : `dnf` (Fedora).

## Relecture et délégation
- Relecture : changement de plus de ~100 lignes, ou touchant auth, transfert de fichiers, fermeture du jeu, Scène ou P2P → `cubelink-review` (il reçoit le diff, pas ta conclusion) ; corriger ce qui est réel, sans nouvelle passe. Sinon, relire soi-même. Dans ce dépôt, cette relecture remplace l'audit `agy` sur diff et sécurité.
- Par défaut, faire soi-même (prime sur `rules/agents.md`). Déléguer sans attendre de « go », seulement du travail indépendant :
  - recherche large ou lecture de docs : `cubelink-explore` ;
  - plusieurs chantiers sans fichier commun : 2 à 4 `cubelink-dev` (6 au maximum), lancés dans un même script ; plan annoncé en une note (tâches, fichiers de chacun).
- Un fichier n'a qu'un propriétaire. Le lead prépare d'abord le partagé (`Cargo.toml`, `Cargo.lock`, `package.json`, `mod.rs`, `lib.rs`, `core/src/types.rs`, `docs/contrats.md`).
- Prompt d'un coéquipier : objectif, fichiers autorisés, test ciblé, critère d'arrêt (« corrige une fois, puis rapporte »). Suite complète, clippy et tests réels : le lead, une fois, à la fin.
- Chantier qui traîne ou corrections qui bouclent : repartir avec UN seul agent très cadré.
- Modèles : `haiku`/`low` pour la recherche mécanique, `sonnet`/`medium` pour coder, `sonnet`/`high` pour le débogage difficile et la relecture. Opus seulement à la demande de l'utilisateur. Toujours fixer `model` et `effort` hors `.claude/agents/`.
- Synthèse du lead : fichiers modifiés, décisions prises seul, risques, tests réels faits et restants, PR.

## Mémoire
En fin de chantier, enregistrer ce qui n'est pas dans le code (décision non évidente, piège, statut d'une preuve réelle), en mettant à jour une note existante plutôt qu'en créant un doublon.
