# Release et mises à jour

## Flux de travail
1. Chaque sujet a sa branche, partie de **`canary`**, et une pull request vers `canary`. Le workflow `ci` tourne sur chaque PR et chaque push vers `canary` et `main` (c'est lui qui déclenche la pré-version et la release) : tsc et vitest, agent LAN sous Java 8 et 17, clippy et tests Rust sous Windows et sous Linux (scénarios X11 sous Xvfb), E2E Playwright simulés sous Windows, `cargo deny` (licences, sources) et les alertes RustSec.
2. Après une fusion dans `canary`, si les vérifications de `ci` passent, `ci` appelle `canary.yml`, qui publie une **pré-version** (`v<x.y.z+1>-canary.<run>`) : **.msi** Windows, AppImage, paquet `.rpm` (Fedora) et paquet `.deb` (Ubuntu/Debian). Jamais « Latest », sans `latest.json` : les launchers installés ne la reçoivent pas. Les builds Windows et Linux tournent en parallèle et la pré-version n'est créée que si les deux réussissent (un build Windows rouge bloque donc aussi les fichiers Linux). Les alertes RustSec et, pour l'instant, les E2E Windows (specs remises à jour dans le ticket #66, à confirmer par un run Windows vert) ne bloquent pas cette publication. Une fois la pré-version publiée, le même job **supprime les plus anciennes** (release et tag) pour n'en garder que les **10 plus récentes** (seuil `KEEP` de l'étape « Purge des anciennes pré-versions »). Seules les pré-versions au tag `v<x.y.z>-canary.<run>[.<essai>]` sont concernées : jamais une release stable, un brouillon ni une pré-version publiée à la main. La suppression est irréversible ; un échec de purge ne fait pas échouer la publication.
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
| `MSIX_IDENTITY_NAME` | variable | Package/Identity/Name donné par le Partner Center (Microsoft Store). Sans les trois variables `MSIX_*`, le .msix porte l'identité Placeholder et ne sert qu'à tester |
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
- Pré-version canary, Windows : le .msi n'est pas signé (SmartScreen avertit) et il n'y a pas de .msix (réservé au Store, voir `release.yml`). WiX n'accepte pas le suffixe `-canary.N` : le ProductVersion du .msi est `x.y.z.<run>` (`bundle.windows.wix.version`, n° de run ≤ 65535) alors que l'app et le nom du fichier portent la version canary. Windows Installer ignore le 4e composant pour décider d'une mise à jour : un canary `x.y.z.13` n'en remplace pas un `x.y.z.12` (désinstaller d'abord) ; la vraie release suivante (`x.y.<run>`, patch plus grand) remplace normalement le canary. La CI installe le .msi en silencieux, vérifie le ProductVersion, le launcher et l'agent LAN, puis le désinstalle (`canary.yml`, job `windows`).
- Pré-version canary, Linux (.deb) : installation `sudo apt install ./Cubelink_<version>-canary.<run>_amd64.deb`. La version du paquet est `x.y.z` sans suffixe (Tauri exige du semver pur ; le suffixe n'est que dans le nom du fichier). La CI l'installe par `apt` dans un conteneur Ubuntu 22.04 où le launcher doit tenir 20 s sous Xvfb (`scripts/deb-smoke.sh`). Le .deb n'est pas encore joint aux vraies releases (`release.yml`).
- Les versions d'une pré-version sont calculées une fois par `scripts/canary-version.mjs` (testé par `scripts/canary-version.test.mjs`, lancé par le job `version`). La purge des anciennes pré-versions suit la même logique : `scripts/canary-prune.mjs` (testé par `scripts/canary-prune.test.mjs`, lancé aussi par le job `version`) choisit les tags à supprimer, triés par date de création et non par n° de run ; elle ne regarde que les 200 dernières releases (le stock existant se vide donc en plusieurs publications) et reste à valider en réel au premier push sur `canary`.
- Linux : l'AppImage est construite en CI ; non testée sur une vraie machine Linux (l'agent LAN, le lancement du jeu et la mise à jour y sont non éprouvés).
- Le .msi des releases GitHub n'est pas signé Authenticode : SmartScreen avertit. La voie sans alerte est le paquet MSIX du Store, que Microsoft signe ; il se téléverse à la main dans le Partner Center. Le .msix n'est **pas** joint à la release GitHub (non signé, il ne s'installe pas d'un double-clic) : on le récupère dans les artefacts du run `ci` (artefact `windows-msix`, conservé 90 jours par défaut par GitHub Actions), et il n'est valable que si les trois variables `MSIX_*` sont définies. Pour installer hors Store, prendre le .msi. Installé via le Store, le launcher coupe son updater interne.
