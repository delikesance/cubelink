---
name: cubelink-verify
description: Use before claiming a Cubelink task is done, before committing, or before opening a PR. Runs scripts/verify.sh and reports real output.
---

# cubelink-verify

Se déclenche tout seul dès que tu t'apprêtes à écrire « terminé », à committer ou à ouvrir une PR.

## Marche à suivre

1. Après un petit changement : `bash scripts/verify.sh --quick` (tsc, vitest, `cargo check -p <crate>` des crates Rust modifiées).
2. Avant la PR : `bash scripts/verify.sh --full` (fmt, clippy `-D warnings`, `cargo test --workspace`, tsc, vitest, `cargo deny check` si installé).
3. « déjà vert pour cet arbre » signifie que le tampon `.git/cubelink-verify.stamp` correspond à l'arbre actuel : citer cette ligne, ne pas relancer.
4. Ne jamais lancer ici les scénarios `host/tests/` ni `E2E_REAL*` : ils ouvrent des fenêtres ou le jeu et exigent l'accord explicite de l'utilisateur.
5. Un échec se rapporte tel quel. Ne pas reformater ni corriger des tests étrangers au chantier pour faire passer la vérification ; dire quelle commande échoue et pourquoi.
6. Les fichiers non suivis ne sont couverts par la signature que par leur nom : le signaler si le script en liste.

## Rapport final (obligatoire)

- Commandes lancées et leur code de sortie.
- Fichiers modifiés (`git diff --stat`).
- Tests ignorés et pourquoi.
- Risques restants.
- Ce qui n'a pas été vérifié (tests réels compris).

## Interdit

- « devrait fonctionner », « sans doute corrigé », « les tests n'ont pas été lancés ».
- Toute affirmation « terminé » sans sortie de commande, diff ou tampon à l'appui.
- Déclarer une feature faite sur les seuls tests unitaires : sans test réel, dire « non vérifiée en réel ».
