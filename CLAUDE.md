# Cubelink : consignes pour Claude

Lanceur Minecraft pour jouer entre amis (Tauri 2 + Rust + React/TypeScript). Ces règles priment sur `~/.claude/CLAUDE.md` pour ce dépôt, y compris celles sur l'attente d'un « go » avant de créer une équipe.

## Mode autonome
Claude mène le chantier de bout en bout sans demander la permission : comprendre, planifier, coder, tester en réel, corriger, committer, pousser, ouvrir la PR. Il ne s'arrête que dans les cas listés dans « Quand s'arrêter et demander ».
- Ne pas demander « je continue ? », « tu valides ? » ou « quelle option ? » : choisir le défaut le plus raisonnable, le noter dans la synthèse, avancer.
- Une ambiguïté se lève en lisant le code, `docs/` et la mémoire, puis en tranchant. Une décision réversible se prend seul.
- Un blocage se contourne : après deux échecs sur le même problème, changer d'approche (ou consulter l'advisor), pas boucler ni rendre la main.
- Finir ce qui est commencé : pas de TODO laissé en plan, pas de « il reste à faire » si Claude peut le faire lui-même.
- Un défaut trouvé en chemin : le corriger s'il est dans le périmètre ou petit et sûr ; sinon l'ajouter à la synthèse.

### Quand s'arrêter et demander (liste complète)
1. Action irréversible ou visible des autres : merge, push forcé, suppression de branche distante, release/publication, message envoyé à un ami.
2. Secrets, comptes, paiement, Partner Center, signature : ce qui exige une action de l'utilisateur.
3. Test réel impossible seul (deux machines, ami en ligne) : le dire dans la PR, ne pas bloquer le reste du travail.
4. Intention réellement ambiguë après lecture du code et des docs, et dont l'erreur coûterait cher.

## Carte du dépôt
- Crates du workspace : `core/` (`cubelink-core`), `p2p/` (`cubelink-p2p`, Iroh/QUIC), `host/` (`cubelink-host`, la Scène sous Windows et Linux X11), `src-tauri/` (`cubelink`, l'app).
- Front : `src/` (React). E2E : `e2e/` (Playwright).
- Docs utiles : `docs/architecture.md`, `docs/contrats.md` (commandes et événements Tauri), `docs/ux-guidelines.md`, `docs/release.md`, `docs/taches-agents.md`.
- Une commande ou un événement Tauri qui change : mettre à jour `docs/contrats.md` dans le même commit.

## Git : une branche par chantier, une PR vers `canary`
1. Au début : `git fetch`, puis `git switch -c <type>/<sujet-court> origin/canary` (types : `feat`, `fix`, `chore`, `docs`, `refactor`). Si l'arbre est propre et que la branche courante porte déjà le chantier demandé, continuer dessus sans demander.
2. Ne jamais committer sur `canary` ni sur `main`. `canary` reçoit les PR (chaque fusion publie une pré-version si `ci` passe) ; seul l'utilisateur fusionne `canary` dans `main` pour une release.
3. Committer seul, par petits commits cohérents, dès qu'une étape est vérifiée. Messages en français.
4. À la fin, sans attendre : `git push -u origin <branche>`, puis `gh pr create --base canary`. Titre et description en français : ce qui change, test réel fait par feature avec son résultat, ce qui reste.
5. Ne jamais merger, ne jamais forcer un push : seul l'utilisateur fusionne.
6. Si `canary` a avancé et que la PR est en conflit : rebaser ou fusionner `canary` dans la branche, résoudre, re-tester, sans push forcé (merge de `canary` plutôt que rebase si la branche est déjà poussée).

## Définition de « terminé »
Une feature est terminée quand toutes ces conditions sont vraies, et Claude les vérifie lui-même :
1. Elle a été **exercée en réel** (voir ci-dessous).
2. `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` et `npx vitest run` sont verts, ainsi que `npx tsc --noEmit`. La CI lance aussi les tests sous Linux (`xvfb-run -a cargo test --workspace`), les E2E simulés sous Windows et `cargo deny check`.
3. Les docs touchées (`docs/contrats.md`, etc.) sont à jour.
4. Le diff a été relu (voir « Relecture »), commité, poussé, et la PR ouverte.

### Test en réel obligatoire
Une feature n'est jamais déclarée faite sur la seule foi des tests unitaires, de `tsc` ou du lint. Elle doit être exercée dans l'app Tauri qui tourne vraiment : vrais appels de commandes, vrai disque, vrai réseau, vrai jeu si elle le touche.
- Choisir le test réel le moins cher qui prouve la feature : E2E Playwright sur l'app, `E2E_REAL*`, essai scripté, rejeu sur le vrai jeu si le lancement ou le fenêtrage est touché.
- Si aucun test réel existant ne couvre la feature, **l'écrire** (E2E ou scénario) dans le même chantier : c'est une partie du livrable, pas une option.
- Un test réel qui échoue se corrige puis se relance, sans demander ; escalade seulement selon « Quand s'arrêter ».
- Test réel impossible dans la session (deux machines, compte, Store…) : la feature est « non vérifiée en réel », jamais « terminée ». Le dire dans la PR.
- Les rejeux longs sur le vrai jeu (15 à 20 min) : une seule série, à la fin.
- Ne pas relancer un test réel déjà vert (`E2E_MC`, `E2E_REAL`, `E2E_REAL_ACCOUNT`) tant que le code qu'il couvre n'a pas changé.

## Boucle de vérification (de la moins chère à la plus chère)
- Logique pure : `cargo test -p <crate> --lib`. Front : `npx tsc --noEmit`, `npx vitest run <fichier>`.
- Scénario Win32 de `host/tests/` (ouvre de vraies fenêtres) : `cargo test -p cubelink-host --test <nom>`, un seul à la fois. Pas besoin de prévenir : c'est normal et c'est autorisé.
- Une seule fois, avant la PR : la suite complète listée dans « Définition de terminé ».
- Dossier de build : `target/` par défaut. Un seul dossier à part (`target/lead`) au besoin, à supprimer à la fin (7 à 28 Go chacun).

## Tests : ne pas gêner l'utilisateur
- Aucune entrée réelle : pas de souris, de clavier, de vol de focus ni de capture d'écran du bureau. On simule dans l'app. « En réel » veut dire app et jeu réels, pas contrôle du bureau de l'utilisateur.
- Tests au premier plan, sans attendre une notification qui ne viendra pas.
- Après un essai qui lance Minecraft : retrouver le `java` par sa ligne de commande et le tuer par PID. Jamais `taskkill /IM`, jamais le java de l'utilisateur.
- Ne pas laisser de processus, de fenêtres de test ni de dossiers de build orphelins en fin de chantier.

## Relecture
Avant la PR, pour tout changement de plus de ~100 lignes ou touchant à l'auth, au transfert de fichiers, à la fermeture du jeu, à la Scène ou au P2P : lancer un sous-agent `cubelink-review` (il reçoit le diff, pas la conclusion de Claude), corriger ce qui est réel, ne pas rouvrir de passe pour le reste. Pour un petit changement, relire soi-même le diff.

## Déléguer : seul, sous-agent ou équipe
Par défaut, faire soi-même. Déléguer seulement du travail vraiment indépendant. Claude décide seul de déléguer, sans attendre de « go ».

| Situation | Choix |
|---|---|
| Correctif ou fonctionnalité dans 1 à 3 fichiers liés | Seul |
| Recherche large dans le dépôt, lecture de docs | Un sous-agent `cubelink-explore` |
| Relecture indépendante d'un diff | Un sous-agent `cubelink-review` |
| Plusieurs chantiers sans fichier commun (ex. un module `core` + un écran front + un module `p2p`) | Une équipe |
| Chantier qui traîne, ou corrections qui bouclent | Arrêter l'équipe et repartir avec UN seul agent, très cadré |

**Équipes (agent teams)**
- Annoncer le plan en une courte note (tâches, fichiers de chacun, nombre de coéquipiers) puis lancer, sans attendre de réponse.
- Plafond : 6 coéquipiers ; viser 2 à 4.
- Un fichier n'a qu'un propriétaire. Le lead prépare tout ce qui est partagé avant le lancement (`Cargo.toml`, `Cargo.lock`, `package.json`, `mod.rs`, `lib.rs`, `core/src/types.rs`, `docs/contrats.md`).
- Coéquipiers lancés depuis `.claude/agents/` (`cubelink-dev` pour le code). Le prompt contient tout : objectif, fichiers autorisés, commande de test ciblée, critère d'arrêt (« corrige une fois, puis rapporte »), nombre maximum de lancements coûteux.
- Les coéquipiers ne lancent que des tests ciblés sur leur crate. Suite complète, clippy et tests réels de bout en bout : le lead, une fois, à la fin.
- Défaut hors périmètre : le signaler au lead, qui décide seul de l'ouvrir ou de le noter.
- Synthèse finale du lead : fichiers modifiés, décisions prises seul, risques, tests réels faits et restants, prochaines étapes. Puis la PR.

**Modèles** : `haiku`/`low` pour les recherches mécaniques, `sonnet`/`medium` pour coder et lire, `sonnet`/`high` pour le débogage difficile et la relecture. Opus seulement si l'utilisateur le demande. Toujours fixer `model` et `effort` dans un appel Agent qui n'utilise pas une définition de `.claude/agents/`.

## Advisor : quand le consulter
Comme Claude décide seul, l'advisor sert de garde-fou. Le consulter, sans demander, à ces moments :
- Avant de s'engager sur une approche difficile à défaire : architecture, format de données sur disque ou réseau, protocole P2P, fenêtrage Win32 de la Scène.
- Après deux échecs sur le même problème, avant un troisième essai.
- Avant de lancer une équipe de 4 coéquipiers ou plus.
- Avant de déclarer terminé un changement qui touche à l'auth, au transfert de fichiers entre amis, à la fermeture du jeu ou à la release.

Pas pour une question de syntaxe, un renommage ou un correctif évident.

## Mémoire
- La mémoire de ce projet (la Scène, statut des preuves réelles, distribution Store, design de l'accueil…) est chargée au démarrage. Avant de parler d'un fichier ou d'un drapeau cité dans une note, vérifier qu'il existe encore.
- En fin de chantier, enregistrer seul ce qui n'est pas dans le code : décision non évidente, piège rencontré, statut d'une preuve réelle. Mettre à jour une note existante plutôt que d'en créer un doublon.
