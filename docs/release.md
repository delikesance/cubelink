# Release et mises à jour

## Flux de travail
1. On travaille et on pousse sur **`development`** (le workflow `ci` vérifie : clippy, tests Rust, tsc, vitest sous Windows, compilation Linux).
2. On merge `development` dans **`main`** (de préférence par pull request : `ci` tourne aussi sur les PR vers `main`).
3. Le merge dans `main` déclenche `release` : nouveau tag **`vMAJEUR.MINEUR.<n° de run>`**, build du **.msi** (Windows) et de l'**AppImage** (Linux), signature des mises à jour, release GitHub avec `latest.json`.

Majeur/mineur : champ `version` de `src-tauri/tauri.conf.json` (changer à la main pour 0.2, 1.0…). Le patch est le numéro de run CI,
écrit dans `tauri.conf.json` seulement pendant le build (rien n'est commité). Lancement manuel sans publier : Actions → release → Run workflow (`publish` décoché).

## Mises à jour dans le launcher (`src-tauri/src/commands_update.rs`)
- Vérification ~10 s après le lancement puis toutes les 6 h, et bouton « Chercher une mise à jour » (Réglages → À propos).
- La mise à jour trouvée est **téléchargée automatiquement** ; une bannière propose « Redémarrer et installer ». Refusé tant qu'une partie est en cours.
- Source : `https://github.com/delikesance/cubelink/releases/latest/download/latest.json`, signature minisign vérifiée avec la clé publique de `tauri.conf.json`.
- Désactivé en mode test (`CUBELINK_TEST_MODE`).

## Secrets / variables GitHub
| Nom | Type | Rôle |
|---|---|---|
| `TAURI_SIGNING_PRIVATE_KEY` | secret | clé privée de signature des mises à jour (`~/.tauri/cubelink-updater.key` en local ; **à sauvegarder**, la perdre casse les mises à jour des versions déjà installées) |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | secret (optionnel) | mot de passe de la clé (vide aujourd'hui) |
| `MSIX_IDENTITY_NAME` | variable | Package/Identity/Name donné par le Partner Center (Microsoft Store) |
| `MSIX_PUBLISHER` | variable | Package/Identity/Publisher du Partner Center (`CN=...`) |
| `MSIX_PUBLISHER_DISPLAY_NAME` | variable | nom d'éditeur affiché dans le Store |

Régénérer la clé : `npx tauri signer generate -w ~/.tauri/cubelink-updater.key`, puis mettre la clé publique dans `plugins.updater.pubkey`.
Les installations existantes ne pourront plus se mettre à jour automatiquement après un changement de clé.

## Limites connues
- MSI : installation « par machine » (demande les droits administrateur) ; l'ancien NSIS s'installait par utilisateur.
- Linux : l'AppImage est construite en CI ; non testée sur une vraie machine Linux (l'agent LAN, le lancement du jeu et la mise à jour y sont non éprouvés).
- Le .msi des releases GitHub n'est pas signé Authenticode : SmartScreen avertit. La voie sans alerte est le paquet MSIX du Store, que Microsoft signe ; il se téléverse à la main dans le Partner Center (artefact `windows-msix`). Installé via le Store, le launcher coupe son updater interne.
