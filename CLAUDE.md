# Cubelink : consignes pour Claude

Lanceur Minecraft pour jouer entre amis (Tauri 2 + Rust + React/TypeScript). Ces règles priment sur `~/.claude/CLAUDE.md` pour ce dépôt.

## Carte du dépôt
- Crates du workspace : `core/` (`cubelink-core`), `p2p/` (`cubelink-p2p`, Iroh/QUIC), `host/` (`cubelink-host`, la Scène sous Windows), `src-tauri/` (`cubelink`, l'app).
- Front : `src/` (React). E2E : `e2e/` (Playwright).
- Docs utiles : `docs/architecture.md`, `docs/contrats.md` (commandes et événements Tauri), `docs/ux-guidelines.md`, `docs/release.md`, `docs/taches-agents.md`.

## Git : une branche par session, une PR vers `development`
1. Au début de la session, avant tout commit : `git fetch`, puis `git switch -c <type>/<sujet-court> origin/development`. Types : `feat`, `fix`, `chore`, `docs`, `refactor`.
2. Ne jamais committer directement sur `development` ni sur `main`.
3. À la fin : `git push -u origin <branche>`, puis `gh pr create --base development`. Le titre et la description sont en français : ce qui change, comment c'est vérifié, ce qui reste.
4. Ne jamais merger, ne jamais forcer un push. C'est l'utilisateur qui fusionne.
5. Si la session reprend un chantier déjà commencé, demander s'il faut continuer sur sa branche.

## Boucle de vérification (de la moins chère à la plus chère)
- Logique pure, en quelques secondes : `cargo test -p <crate> --lib`. Front : `npx tsc --noEmit`, `npx vitest run <fichier>`.
- Scénario Win32 de `host/tests/` (ouvre de vraies fenêtres) : `cargo test -p cubelink-host --test <nom>`. Prévenir l'utilisateur avant, et en lancer un seul à la fois.
- Une seule fois, avant la PR : `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` et `npx vitest run`. C'est ce que fait la CI.
- Rejeu sur le vrai jeu (15 à 20 min) : une seule série, à la fin, et seulement si le changement touche au lancement ou au fenêtrage.
- Dossier de build : `target/` par défaut. Un seul dossier à part (`target/lead`) au besoin, à supprimer à la fin, car chaque dossier pèse 7 à 28 Go.

## Tests : ne pas gêner l'utilisateur
- Aucune entrée réelle : pas de souris, de clavier, de vol de focus ni de capture d'écran du bureau. On simule dans l'app.
- Tests au premier plan, sans attendre une notification qui ne viendra pas.
- Après un essai qui lance Minecraft : retrouver le `java` par sa ligne de commande et le tuer par PID. Jamais `taskkill /IM`, jamais le java de l'utilisateur.
- Ne pas relancer les tests réels déjà verts (`E2E_MC`, `E2E_REAL`, `E2E_REAL_ACCOUNT`).

## Déléguer : seul, sous-agent ou équipe
Par défaut, faire soi-même. Déléguer seulement du travail vraiment indépendant.

| Situation | Choix |
|---|---|
| Correctif ou fonctionnalité dans 1 à 3 fichiers liés | Seul |
| Recherche large dans le dépôt, lecture de docs | Un sous-agent `cubelink-explore` |
| Relecture indépendante d'un diff | Un sous-agent `cubelink-review`, à qui on donne le code et non sa propre conclusion |
| Plusieurs chantiers sans fichier commun (ex. un module `core` + un écran front + un module `p2p`) | Une équipe |
| Chantier qui traîne, ou corrections qui bouclent | Arrêter l'équipe et repartir avec UN seul agent, très cadré |

**Équipes (agent teams)**
- Proposer d'abord un plan : tâches, dépendances, fichiers de chaque coéquipier, nombre de coéquipiers. Attendre un go explicite avant de lancer qui que ce soit.
- Plafond pour ce projet : 6 coéquipiers. Viser 2 à 4 : au-delà, la coordination coûte plus qu'elle ne rapporte.
- Un fichier n'a qu'un propriétaire à la fois. Le lead prépare tout ce qui est partagé avant de lancer l'équipe (`Cargo.toml`, `Cargo.lock`, `package.json`, `mod.rs`, `lib.rs`, `core/src/types.rs`, `docs/contrats.md`). Les coéquipiers n'y touchent pas.
- Lancer les coéquipiers à partir des définitions de `.claude/agents/` (`cubelink-dev` pour le code), qui fixent déjà le modèle et l'effort. Le prompt de lancement doit tout contenir, car un coéquipier ne voit pas la conversation du lead : objectif, fichiers autorisés, commande de test ciblée, critère d'arrêt (« corrige une fois, puis rapporte »), nombre maximum de lancements coûteux.
- Les coéquipiers ne lancent que des tests ciblés sur leur crate. La suite complète, clippy et le rejeu réel sont faits par le lead, une fois, à la fin.
- Un défaut trouvé hors du périmètre : le signaler, ne pas ouvrir une nouvelle passe sans accord.
- Synthèse finale du lead : fichiers modifiés, décisions, risques, tests restants, prochaines étapes. Puis la PR vers `development`.

**Modèles** : `haiku`/`low` pour les recherches mécaniques, `sonnet`/`medium` pour coder et lire, `sonnet`/`high` pour le débogage difficile et la relecture. Opus seulement si l'utilisateur le demande. Toujours fixer `model` et `effort` dans un appel Agent qui n'utilise pas une définition de `.claude/agents/`.

## Advisor : quand le consulter
L'advisor est un modèle plus fort qui relit tout le transcript et donne un avis. Il est activé avec `advisorModel` (commande `/advisor`). Chaque appel coûte des tokens, donc on ne le consulte qu'à ces moments :
- Avant de s'engager sur une approche difficile à défaire : architecture, format de données sur disque ou réseau, protocole P2P, fenêtrage Win32 de la Scène.
- Après deux échecs sur le même problème, avant un troisième essai.
- Avant de proposer un plan d'équipe à 4 coéquipiers ou plus.
- Avant de déclarer terminé un changement qui touche à l'auth, au transfert de fichiers entre amis, à la fermeture du jeu ou à la release.

Pas pour une question de syntaxe, un renommage ou un correctif évident.

## Mémoire
La mémoire de ce projet (la Scène, statut des preuves réelles, distribution Store, design de l'accueil…) est chargée au démarrage. Avant de parler d'un fichier ou d'un drapeau cité dans une note, vérifier qu'il existe encore.
