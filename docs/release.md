# Release et mises à jour

## Flux de travail
1. Chaque sujet a sa branche, partie de **`canary`**, et une pull request vers `canary`. Le workflow `ci` tourne sur chaque PR et chaque push vers `canary` et `main` (c'est lui qui déclenche la pré-version et la release) : tsc et vitest, agent LAN sous Java 8 et 17, clippy et tests Rust sous Windows et sous Linux (scénarios X11 sous Xvfb), E2E Playwright simulés sous Windows, `cargo deny` (licences, sources) et les alertes RustSec.
2. Après une fusion dans `canary`, si les vérifications de `ci` passent, `ci` appelle `canary.yml`, qui publie une **pré-version** Linux (`v<x.y.z+1>-canary.<run>`, AppImage et paquet `.rpm` pour Fedora, jamais « Latest », sans `latest.json` : les launchers installés ne la reçoivent pas). Les alertes RustSec et, pour l'instant, les E2E Windows (specs remises à jour dans le ticket #66, à confirmer par un run Windows vert) ne bloquent pas cette publication.
3. Pour une release, le mainteneur fusionne `canary` dans **`main`** par une pull request. Le merge dans `main` déclenche `ci` ; si les mêmes vérifications que pour canary passent (front, agent LAN, clippy et tests Windows/Linux, `cargo deny`), `ci` appelle `release.yml` : nouveau tag **`vMAJEUR.MINEUR.<n° de run ci>`**, build du **.msi** (Windows), de l'**AppImage** et du **.rpm** (Linux), signature des mises à jour, release GitHub avec `latest.json`. Si une vérification échoue, **rien n'est publié** : corriger, puis relancer les jobs échoués du run `ci` (la release part ensuite). Comme pour canary, les E2E Windows et les alertes RustSec ne bloquent pas la release. Une doc seule (`docs/**`, `*.md`) ne lance ni `ci` ni release.

Majeur/mineur : champ `version` de `src-tauri/tauri.conf.json` (changer à la main pour 0.2, 1.0…). Le patch est le numéro de run du workflow `ci` (partagé avec les PR et canary : les patchs sautent des numéros mais restent uniques et croissants ; au premier passage sur ce schéma, vérifier que le patch dépasse celui de la dernière release, sinon l'updater ne propose rien, et qu'il reste ≤ 65535),
écrit dans `tauri.conf.json` seulement pendant le build (rien n'est commité). Lancement manuel : Actions → release → Run workflow construit tout sans jamais publier (pas d'option de publication : elle contournerait `ci` et son n° de run, plus petit, produirait un patch plus ancien que la dernière release). Seul `ci` publie, sur un push vers `main`.

## Mises à jour dans le launcher (`src-tauri/src/commands_update.rs`)
- Vérification ~10 s après le lancement puis toutes les 6 h, et bouton « Chercher une mise à jour » (Réglages → À propos).
- La mise à jour trouvée est **téléchargée automatiquement** ; une bannière propose « Redémarrer et installer ». Refusé tant qu'une partie est en cours.
- Source : `https://github.com/delikesance/cubelink/releases/latest/download/latest.json`, signature minisign vérifiée avec la clé publique de `tauri.conf.json`.
- Désactivé en mode test (`CUBELINK_TEST_MODE`).

## Secrets / variables GitHub
| Nom | Type | Rôle |
|---|---|---|
| `TAURI_SIGNING_PRIVATE_KEY` | secret (**obligatoire pour publier**, vérifié dès le job `version`) | clé privée de signature des mises à jour (`~/.tauri/cubelink-updater.key` en local ; **à sauvegarder**, la perdre casse les mises à jour des versions déjà installées) |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | secret (optionnel) | mot de passe de la clé (vide aujourd'hui) |
| `CUBELINK_CF_API_KEY` | secret (**obligatoire pour publier**) | clé de développeur CurseForge embarquée dans l'app (reconnaissance des mods au « Rejoindre », import et parcours CurseForge). Sans elle, la release refuse de publier |
| `CUBELINK_MS_CLIENT_ID` | secret (optionnel) | identifiant d'application Azure embarqué ; à défaut, la valeur par défaut de l'app |
| `MSIX_IDENTITY_NAME` | variable | Package/Identity/Name donné par le Partner Center (Microsoft Store) |
| `MSIX_PUBLISHER` | variable | Package/Identity/Publisher du Partner Center (`CN=...`) |
| `MSIX_PUBLISHER_DISPLAY_NAME` | variable | nom d'éditeur affiché dans le Store |

Régénérer la clé : `npx tauri signer generate -w ~/.tauri/cubelink-updater.key`, puis mettre la clé publique dans `plugins.updater.pubkey`.
Les installations existantes ne pourront plus se mettre à jour automatiquement après un changement de clé.

Les secrets ne sont donnés qu'aux étapes `tauri build` (jamais dans l'environnement global d'un workflow), et `npm ci` tourne avec `--ignore-scripts` : un script d'installation npm ne peut pas lire la clé de signature.

## Outillage de la CI
- `rust-toolchain.toml` fixe la version de Rust, en local comme en CI. La monter à la main, dans une PR qui corrige les nouveaux lints de clippy.
- Les actions GitHub sont épinglées par SHA. `.github/dependabot.yml` ouvre chaque semaine des PR vers `canary` pour Cargo, npm et les actions.
- `deny.toml` règle `cargo deny` : licences permissives seulement, crates.io comme seule source, crates retirées refusées.
- `scripts/ci-apt.sh` installe les dépendances système Linux de Tauri sur les runners.

## Limites connues
- MSI : installation « par machine » (demande les droits administrateur) ; l'ancien NSIS s'installait par utilisateur.
- Linux (.rpm) : construit en CI avec l'AppImage, puis installé par `dnf` dans un conteneur Fedora où le launcher doit tenir 20 s sous Xvfb (`scripts/rpm-smoke.sh`). Installation : `sudo dnf install ./Cubelink-<version>-1.x86_64.rpm`. Le launcher installé ainsi ne se met pas à jour seul : il signale la nouvelle version, on réinstalle le .rpm suivant.
- Linux : l'AppImage est construite en CI ; non testée sur une vraie machine Linux (l'agent LAN, le lancement du jeu et la mise à jour y sont non éprouvés).
- Le .msi des releases GitHub n'est pas signé Authenticode : SmartScreen avertit. La voie sans alerte est le paquet MSIX du Store, que Microsoft signe ; il se téléverse à la main dans le Partner Center (artefact `windows-msix`). Installé via le Store, le launcher coupe son updater interne.
