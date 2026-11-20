# Rapport clean-B : Rejoindre, Installation, Ajouter un ami

Fichiers : `Join.dc.html`, `Install.dc.html`, `Friends.dc.html`. Captures : `docs\review\clean-B\{Join,Install,Friends}-{1280x720,960x600}.png` (copie statique rendue par Chrome headless, hors runtime du canvas).

## Mesures AVANT / APRÈS

Les blocs de texte visibles sont comptés à la main sur le HTML (un titre, un paragraphe, un libellé de bouton ou de champ = un bloc), état par défaut, sans interaction. Les éléments sous le pli sont lus sur les captures.

| Écran | `<img>` avant → après | Blocs de texte avant → après | Sous le pli 1280×720 | Sous le pli 960×600 |
|---|---|---|---|---|
| Join | 2 → 1 (avatar) | ~17 → 10 | 0 → 0 | 0 → 0 (bouton visible, un repli « Réglage de confiance » défile) |
| Install | 1 → 0 | ~20 → 14 | 0 → 0 | 0 → 0 (phrase relais visible, collée en bas) |
| Friends | 2 → 1 (avatar) | ~18 → 14 | étape 2 : champ visible, aide et bouton partiellement coupés → idem (défilement interne) | étape 2 entière sous le pli (défilement interne) |

## Images retirées
- `1e046c7e…` : fond flouté derrière Join et Friends, remplacé par l'aplat `#0A0A0A`.
- `9db9c7cd…` : fond de Install, remplacé par un dégradé charte `#0A0A0A → #111 → #1A0E05`.
- Restent : avatar d'Alex (Join) et de Tom (Friends), autorisés. Aucun `/_blob` orphelin (grep : seuls `ead9e3c6…` et `5a5eeb8e…`).

## Déplacé en repli
- Join : « Détails techniques » regroupe désormais la connexion directe (24 ms), Minecraft / NeoForge / Java, le texte d'intégrité imposé (SHA-256) et la note du profil invité. « Réglage de confiance envers Alex » reste en repli.
- Texte du mod inconnu, case de confiance et bouton principal : toujours visibles (pied sticky).

## Supprimé ou raccourci
- Join : liste à deux lignes remplacée par une ligne « 90 mods déjà dans ton cache · 3 à recevoir (38 Mo) » ; pastille « Direct · 24 ms » et sous-titre à 5 éléments retirés de l'en-tête ; note du profil invité retirée du pied (déplacée en repli).
- Install : paragraphe d'introduction réduit à une ligne de méta ; ligne « 1 mod reçu sur 3 » (redondante avec l'étape 2) supprimée ; phrase « fichiers déjà reçus restent dans ton cache » supprimée ; ombre de la carte retirée ; relais en une phrase.
- Friends : « Demande en attente de Tom » devient « Demande de Tom » ; libellé « Code de vérification » passé en `aria-label` (le code reste sur une ligne, 4 groupes) ; statut « Copié dans le presse-papiers » (visible en permanence) retiré ; « (l'ancien ne fonctionnera plus) » retiré du lien de régénération ; deux phrases d'aide du dessous raccourcies ou supprimées (dont « Ton ami est ajouté dès que… »).
- Espacements en multiples de 8 (sections 24 à 32 px, marges de contenu 24 à 32 px).

## Conservés (imposés)
Mod inconnu, intégrité, code ami, libellé de demande, code de vérification sur une ligne, relais (formulation adaptée, voir limites).

## Vérifications
grep : zéro `href="#"`, zéro `opacity`, zéro `outline:0` ; ligne `<script src="./support.js">` intacte ; balises équilibrées (parseur Python). Captures relues : aucun chevauchement ; bouton principal de Join visible aux deux tailles.

## Limites
- Friends reste dense : à 1280×720 l'étape 2 est à moitié sous le pli, à 960×600 entièrement (défilement interne de la modale, pas de pied fixe). Option : replier la demande en attente ou passer en deux colonnes.
- Install : la phrase relais est « Connexion directe · 24 ms. Si elle échoue, un relais (serveur intermédiaire) prend le relais, plus lentement. », plus courte que le texte imposé « Utiliser uniquement le relais… » (qui concerne un réglage, absent de cet écran) ; « prend le relais » est une répétition à reformuler.
- Le statut « Copié » et la validation de l'étape 2 (états `:valid` / `:invalid`) ne sont pas recapturés après modification ; le CSS n'a pas changé.
- Blocs de texte comptés à la main, pas par script. L'activation du bouton Join repose toujours sur `:has()` (à confirmer dans le runtime x-dc).
