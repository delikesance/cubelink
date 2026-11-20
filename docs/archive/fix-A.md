# fix-A : Accueil (Main) et Connexion (Login)

Fichiers modifiés : `Main.dc.html`, `Login.dc.html` (dossier `scratchpad\cv\project\`). Captures : `docs/review/fix-A/` (Main et Login, 1280x720 et 960x600), rendues par Chrome headless sur une copie statique (hors runtime du canvas).

## Tableau

| ID | Statut | Changement (une ligne) / raison |
|---|---|---|
| D-01, L-05 | Corrigé | Titre `clamp(36px,4.4vw,56px)`, bloc texte dans la colonne gauche ; « Mes instances » est une rangée séparée en bas du hero. Aucun chevauchement constaté sur les captures 1280x720 et 960x600. |
| D-02 | Corrigé | 3 cartes en grille `auto-fit`, 104 px min, bordure, voile de lisibilité plus dense, méta 13 px, nom 15 px ; la carte invité a la même bordure. |
| D-06 | Corrigé | Dégradé gauche renforcé, titre réduit, « Jouer » 56 px/18 px avec halo, remonté sous le paragraphe ; le profil devient bouton secondaire. |
| D-07 | Corrigé | Paragraphe `#E8E8EE`, méta 14 px `#D4D4DC`, `text-shadow` et voile. Contraste non re-mesuré avec le script de la review. |
| D-08 | Partiel | Barre orange + anneau sur l'instance active, survol, focus visible, `aria-label` et `title` sur chaque icône. Pas d'infobulle CSS ni de badge « ami en ligne » (le `overflow:hidden` des vignettes les clippe). |
| D-12 | Partiel | Lignes d'amis à rythme régulier (60 px, gap 8), « Joue à… » sur 2 lignes avec ellipsis et `title`, boutons à 44 px sans lourdeur excessive, pied « Ajouter un ami » qui comble le vide. Les états vide et injoignable de la liste ne sont pas dessinés (voir ci-dessous). |
| D-13 | Corrigé | Pastille verte (`#4ADE80` translucide, texte `#B6F5CC`, 14 px) et bouton de réglages d'instance (icône curseurs, bordure `#8A8A8A`) alignés sur une même rangée de 44 px. |
| L-01 | Corrigé | `opacity` retirée de la ligne de Léa ; « Hors ligne · il y a 2 jours » en `#9A9A9A`, 13 px. |
| L-15 | Corrigé | `line-clamp:2` + `title` + `aria-label="Rejoindre Alex, qui joue à Create: Aeronautics"`. |
| P-27 | Partiel | 94 mods, Minecraft 1.21.1, NeoForge 21.1.77, Java 21 alignés sur §7 ; « Stable » retiré. Les noms fictifs (All the Mods 10, 412 mods, etc.) restent : pas de bandeau « Maquette ». |
| D-11 | Corrigé | Fond plus visible (voile 35 % à 90 % au lieu de 85 % à 95 %) et bloc central en carte `#0D0D0DDB`, bordure, ombre, qui ancre le contenu. Les contrastes du texte dans la carte n'ont pas été mesurés au script. |
| P-13 | Corrigé | Bouton blanc `#FFFFFF`, texte `#5E5E5E`, 56 px, logo 4 couleurs non modifié, libellé imposé. Conformité de marque à faire valider. |
| P-01 | Corrigé | Texte exact de fix-rules.md en pied de page, 13 px `#B5B5B5`. À faire valider par un juriste. |
| P-08 | Corrigé | Phrase de fix-rules.md reprise telle quelle. |
| P-20 | Partiel | « Problème lors de la connexion ? » reste sur `Error.dc.html` (écran existant, pas une page d'aide). « En acheter un » pointe vers `https://www.minecraft.net/fr-fr` (page d'accueil, URL d'achat exacte non vérifiée), `target=_blank`, `rel=noopener noreferrer`. |
| L-11 | Corrigé | Soulignement permanent, `#FF8A4A` ; lien de pied de page également souligné. |
| L-13 | Partiel | Inter 400/500/600/**700** chargé, repli `system-ui` ajouté partout. Comportement hors ligne à confirmer dans le canvas. |

## Transversal
- Focus : `a:focus-visible,button:focus-visible{outline:2px solid #FF8A4A;outline-offset:2px}` dans `<helmet>` des deux pages.
- Liens : zéro `href="#"` (vérifié par grep). Zéro `opacity` et zéro `outline:0` dans les deux fichiers (grep). Balises : vérification par parseur, aucune balise de contenu non fermée (les seules alertes viennent des éléments SVG auto-fermés).
- Insécables ajoutées : `2&nbsp;amis`, `94&nbsp;mods`, `Amis&nbsp;· 3`, `?` après « compte Minecraft » et « connexion », `il y a 2&nbsp;jours`.
- Retirés : « Stable », duplicata d'icône rouage (le bouton d'instance a maintenant une icône à curseurs).
- Titre de page : `<h1>CUBELINK` (au lieu d'un `<span>`) sur Login.

## Limites et « à confirmer dans le canvas »
- Rendu statique : survol, focus clavier, `-webkit-line-clamp`, `min(720px,100vh)` et le comportement des polices dans le vrai canvas.
- États vide et injoignable de la liste d'amis : non faits (pas de logique `renderVals` utilisée) ; à décrire dans le canvas (ex. « Aucun ami pour l'instant. Ajoute ton premier ami avec son code ami. » et « Impossible de joindre le réseau d'amis. Réessaie. »).
- Boutons « Messages », « Nouvelle instance », « Steve_42 ▾ », « Jouer » n'ont pas de destination (ce sont des `<button>`, pas des liens).
- Contrôles de fenêtre de la barre de titre : 46x32 px (convention OS), sous les 44 px de haut.
- Les instances du rail et des cartes pointent toutes vers `Main.dc.html` (pas d'écran par instance).
- Contrastes non re-mesurés avec le script de `review-lisibilite`.
- Juridique à faire valider : P-01 (non-affiliation), P-13 (bouton Microsoft).
