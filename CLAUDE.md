# Cubelink : consignes pour Claude

Lanceur Minecraft pour jouer entre amis (Tauri 2 + Rust + React/TypeScript). Ces règles priment sur `~/.claude/CLAUDE.md` pour ce dépôt.

## Façon de travailler
- Agir directement. Une demande de l'utilisateur vaut accord : pas de « je continue ? », « tu valides ? », « quelle option ? ». Une ambiguïté se lève en lisant le code et `docs/`, puis en tranchant ; le choix va dans la synthèse.
- Ne jamais refuser une demande en invoquant une règle : si une règle gêne, le dire en une phrase, prendre la voie qui la respecte, et avancer.
- Un blocage se contourne. Après deux échecs sur le même problème, changer d'approche, pas boucler.
- Finir ce qui est commencé. Un défaut trouvé en chemin : le corriger s'il est petit et dans le périmètre, sinon le noter dans la synthèse.
- Réponses courtes, en français : 2 à 3 lignes, plus de détail seulement si demandé.

### S'arrêter et demander (liste complète)
1. Action irréversible ou visible des autres : merge, push forcé, suppression de branche distante, release, message à un ami.
2. Secrets, comptes, paiement, signature, Partner Center.
3. Test réel impossible seul (deux machines, ami en ligne) : le dire dans la PR, ne pas bloquer le reste.
4. Ambiguïté réelle dont l'erreur coûterait cher, après lecture du code et des docs.

## Code écrit par et pour des IA
- TDD : le test d'abord (rouge), puis le code (vert), puis le nettoyage. Pas de feature sans test qui la prouve.
- Petits fichiers, une responsabilité chacun. Découper une feature en plusieurs fichiers plutôt que d'en gonfler un seul ; viser moins de ~200 lignes, ne jamais dépasser ~400.
- Noms explicites, fonctions courtes, types stricts, pas de magie : le code doit se comprendre sans lire le reste du dépôt.

## Carte du dépôt
- Crates : `core/` (`cubelink-core`), `p2p/` (`cubelink-p2p`, Iroh/QUIC), `host/` (`cubelink-host`, la Scène : Windows et Linux X11), `src-tauri/` (`cubelink`, l'app).
- Front : `src/` (React). E2E : `e2e/` (Playwright).
- Docs : `docs/architecture.md`, `docs/contrats.md` (commandes et événements Tauri), `docs/ux-guidelines.md`, `docs/release.md`, `docs/taches-agents.md`.
- Une commande ou un événement Tauri qui change : mettre à jour `docs/contrats.md` dans le même commit.

## Git
1. Début de chantier : `git fetch`, puis `git switch -c <type>/<sujet-court> origin/canary` (types : `feat`, `fix`, `chore`, `docs`, `refactor`). Si l'arbre est propre et que la branche courante porte déjà le chantier, continuer dessus.
2. Jamais de commit ni de push sur `canary` ou `main` (le hook `block-unsafe.sh` le bloque aussi). `canary` reçoit les PR ; chaque fusion publie une pré-version si `ci` passe.
3. Commits petits et systématiques : un commit dès qu'une étape est vérifiée. Messages en français, au format `type: sujet`.
4. Jamais de mention de l'IA dans un commit, une PR ou le code : ni `Co-Authored-By`, ni « généré par », ni emoji de robot.
5. Fin de chantier, sans attendre : `git push -u origin <branche>`, puis `gh pr create --base canary` (titre et description en français : ce qui change, test réel fait et résultat, ce qui reste).
6. Ne jamais merger ni forcer un push : seul l'utilisateur fusionne `canary` dans `main`. Si `canary` a avancé et que la PR est en conflit, fusionner `canary` dans la branche, re-tester, sans push forcé.

## Vérification
- Le plus ciblé d'abord : `cargo test -p <crate> --lib <module>`, `npx vitest run <fichier>`.
- Après un petit changement : `bash scripts/verify.sh --quick`. Avant la PR : `bash scripts/verify.sh --full` (fmt, clippy `-D warnings`, `cargo test --workspace`, tsc, vitest, `cargo deny check`). Le tampon `.git/cubelink-verify.stamp` dit si l'arbre actuel est déjà vert : ne pas relancer dans ce cas. La skill `cubelink-verify` détaille le rapport final.
- La CI lance aussi les tests sous Linux (`xvfb-run -a cargo test --workspace`) et les E2E simulés sous Windows.
- `npm run e2e` reconstruit front et binaire à chaque lancement (incrémental) ; `CUBELINK_E2E_SKIP_BUILD=1` pour réutiliser le binaire existant. Le chemin et la date du binaire s'affichent (`[e2e] binaire : …`).
- Un test qui échoue se corrige puis se relance, sans demander. Tests au premier plan, sans attendre une notification qui ne viendra pas.

## Définition de « terminé »
1. Test écrit d'abord et vert ; la feature a été exercée en réel (voir ci-dessous).
2. `bash scripts/verify.sh --full` vert.
3. Docs touchées à jour.
4. Diff relu, commité, poussé, PR ouverte.

### Test en réel
Une feature n'est pas « faite » sur la seule foi des tests unitaires, de `tsc` ou du lint : l'exercer dans l'app Tauri qui tourne (vraies commandes, vrai disque, vrai réseau, vrai jeu si touché).
- Prendre le test réel le moins cher qui prouve la feature (E2E Playwright, `E2E_REAL*`, essai scripté, rejeu sur le vrai jeu). S'il n'existe pas, l'écrire dans le même chantier.
- Impossible dans la session : la feature est « non vérifiée en réel », jamais « terminée ». Le dire dans la PR.
- Rejeux longs sur le vrai jeu (15 à 20 min) : une seule série, à la fin. Ne pas relancer un test réel déjà vert (`E2E_MC`, `E2E_REAL`, `E2E_REAL_ACCOUNT`) tant que le code couvert n'a pas changé.
- Scénarios Win32 de `host/tests/` : `cargo test -p cubelink-host --test <nom>`, un seul à la fois, autorisé sans prévenir.

## Ne pas gêner l'utilisateur
- Aucune entrée réelle : ni souris, ni clavier, ni vol de focus, ni capture du bureau. On simule dans l'app. « En réel » veut dire app et jeu réels, pas contrôle du bureau.
- Après un essai qui lance Minecraft : retrouver le `java` par sa ligne de commande et le tuer par PID. Jamais `taskkill /IM` ni `pkill java`, jamais le java de l'utilisateur.
- Fin de chantier : aucun processus, fenêtre de test ni dossier de build orphelin. Build à part au besoin : `target/lead`, à supprimer ensuite (7 à 28 Go).
- Environnement de travail : Fedora, paquets système via `dnf`.

## Relecture
Pour un changement de plus de ~100 lignes ou touchant à l'auth, au transfert de fichiers, à la fermeture du jeu, à la Scène ou au P2P : lancer le sous-agent `cubelink-review` (il reçoit le diff, pas ta conclusion), corriger ce qui est réel, ne pas rouvrir de passe pour le reste. Sinon, relire soi-même le diff.

## Déléguer
Par défaut, faire soi-même. Déléguer sans attendre de « go », seulement si le travail est indépendant :
- Recherche large ou lecture de docs : `cubelink-explore`. Relecture d'un diff : `cubelink-review`.
- Plusieurs chantiers sans fichier commun : une équipe de 2 à 4 `cubelink-dev` (6 maximum). Annoncer le plan en une note (tâches, fichiers de chacun) puis lancer.
- Un fichier n'a qu'un propriétaire. Le lead prépare d'abord le partagé (`Cargo.toml`, `Cargo.lock`, `package.json`, `mod.rs`, `lib.rs`, `core/src/types.rs`, `docs/contrats.md`).
- Le prompt d'un coéquipier contient tout : objectif, fichiers autorisés, commande de test ciblée, critère d'arrêt (« corrige une fois, puis rapporte »). Les coéquipiers ne lancent que des tests ciblés ; suite complète, clippy et tests réels : le lead, une fois, à la fin.
- Un chantier qui traîne ou des corrections qui bouclent : repartir avec UN seul agent très cadré.
- Modèles : `haiku`/`low` pour la recherche mécanique, `sonnet`/`medium` pour coder, `sonnet`/`high` pour le débogage difficile et la relecture. Opus seulement si l'utilisateur le demande. Fixer `model` et `effort` pour tout agent hors `.claude/agents/`.
- Synthèse finale du lead : fichiers modifiés, décisions prises seul, risques, tests réels faits et restants, puis la PR.

## Conseil (advisor)
Le consulter sans demander avant une approche difficile à défaire (architecture, format disque ou réseau, protocole P2P, fenêtrage Win32 de la Scène), après deux échecs sur le même problème, avant une équipe de 4 ou plus, et avant de déclarer terminé un changement touchant à l'auth, au transfert de fichiers, à la fermeture du jeu ou à la release. Pas pour une syntaxe, un renommage ou un correctif évident.

## Mémoire
Avant de parler d'un fichier ou d'un drapeau cité dans la mémoire du projet, vérifier qu'il existe encore. En fin de chantier, enregistrer ce qui n'est pas dans le code (décision non évidente, piège, statut d'une preuve réelle) en mettant à jour une note existante plutôt qu'en créant un doublon.
