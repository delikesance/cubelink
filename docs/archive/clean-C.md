# Clean C : Paramètres, Erreurs, Mentions légales

Fichiers : `Settings.dc.html`, `Error.dc.html`, `Legal.dc.html` (dossier `scratchpad\cv\project\`). Captures : `docs/review/clean-C/` (`*-avant.png`, `*-apres.png`, `*-ouvert.png` pour les états avec repli ouvert ou autre section).

## Principe
Une section / un état à la fois, choisi dans une liste à gauche (boutons radio masqués + libellés, 100 % CSS, accessibles au clavier, anneau de focus). Le contenu défile dans le panneau de droite. Aucun `<img>` dans ces trois pages (0 avant, 0 après) : aucune image à retirer ; le dégradé violet décoratif derrière les modales Paramètres et Légal est remplacé par l'aplat `#0A0A0A`.

## Mesures (outil : script injecté, éléments portant du texte, visibles sans interaction, rendu Chrome headless)

| Écran | Taille | Blocs de texte visibles AVANT | Sous le pli AVANT | Visibles APRÈS | Sous le pli APRÈS |
|---|---|---|---|---|---|
| Settings | 1280×720 | 20 | 20 | 15 | 0 |
| Settings | 960×600 | 15 | 25 | 14 | 1 |
| Error | 1280×720 | 27 | 15 | 17 | 0 |
| Error | 960×600 | 16 | 26 | 16 | 1 |
| Legal | 1280×720 | 14 | 18 | 14 | 0 |
| Legal | 960×600 | 12 | 20 | 14 | 0 |

Lecture : le décompte « visibles » APRÈS inclut la navigation de gauche (4 à 8 libellés courts) ; le contenu du panneau seul tient en 4 à 8 blocs. Le « 1 » sous le pli à 960×600 est le lien « Mentions légales » ou le bas du texte d'un panneau qui défile dans la modale (rien d'essentiel coupé sur les captures).

## Ce qui a changé
- **Settings** : 4 sections (Réseau P2P, Codes, Cache et partage, Crédits et licences) + lien « Mentions légales et confidentialité ». Écart par rapport à la consigne (3 sections) : « Codes » et « Cache et partage » sont séparés de « Réseau P2P » pour que chaque panneau reste léger. Un réglage = libellé + une ligne d'aide ; espacement 24 px entre blocs, cartes à 20/24 px.
- **Error** : 8 cartes remplacées par une liste d'états (groupes « Connexion Microsoft » : en attente, sans licence, hors ligne, annulée ; « Installation » : empreinte invalide, ami déconnecté, disque plein, Java manquant) et un seul panneau : icône, étiquette, titre, 1 à 2 phrases, actions. Tous les cas conservés.
- **Legal** : sommaire à gauche (Éditeur, Non-affiliation, Données traitées, Le relais, Licences, Contact), une section à la fois, phrases raccourcies.

## Déplacé en repli (`<details>`)
- Settings : « En savoir plus » (limite du test de connexion, identité ed25519, dernier relais testé) ; « En savoir plus » sur l'envoi de mods (config, mots de passe, bande passante) ; « Images (OGA-BY 3.0 et CC0) » et « Polices (SIL OFL 1.1) » dans Crédits ; confirmation de purge du cache (déjà repliée).
- Error : cause possible de l'empreinte invalide.
- Legal : précision sur la liste d'amis et la connexion Microsoft (Données traitées) ; licences images et polices.

## Supprimé ou raccourci
- Settings : aide du code de vérification raccourcie (« À comparer avec ton ami par un autre canal (Discord, SMS). ») ; phrase d'avertissement sur la connexion directe déplacée en repli ; aide « Partagés entre toutes tes instances » fusionnée dans une seule ligne avec la taille du cache.
- Error : « Annuler » retiré de « Connexion annulée » (déjà une seule action) ; phrases raccourcies (ex. « Tu peux aussi réessayer maintenant » retiré de « Ami déconnecté »).
- Legal : liste « Données traitées » condensée (4 puces plus courtes). Aucune perte de contenu juridique : tout ce qui a été retiré du visible est dans un repli ou reformulé.

## Textes imposés conservés
Réglage relais, « Connexion directe quand c'est possible, sinon via un relais », code ami, code de vérification (4 groupes, une ligne), non-affiliation exacte (Settings Crédits + Legal, vérifiée par grep : 1 occurrence chacun), OGA-BY + OFL, « Cubelink ne stocke ni ton compte ni ta liste d'amis sur un serveur. Un relais peut transmettre ton trafic quand la connexion directe échoue. » (Legal), placeholders « à compléter ». À faire valider par un juriste (non-affiliation, données, relais).

## Vérifications
`grep` : 0 `href="#"`, 0 `opacity`, 0 `outline:0`/`outline:none`, 0 `<img>`, ligne `<script src="./support.js"></script>` présente dans les trois fichiers. Captures lues : 1280×720 et 960×600 (avant/après), plus états ouverts (Crédits, Cache avec confirmation, Données traitées, Empreinte invalide).

## Limites
- Rendu mesuré dans Chrome headless sur une copie statique (sans runtime du canvas) : le sélecteur radio/label et `details` dépendent de CSS standard, mais le comportement dans le runtime `.dc.html` n'a pas été testé.
- Seules les sections par défaut et les états listés ci-dessus ont été capturés ; les autres panneaux d'Error (hors empreinte) et de Settings (Codes) n'ont pas été capturés individuellement.
- Le décompte de « blocs de texte » est un proxy (éléments avec texte direct), pas un comptage éditorial.
- Interrupteurs : cases à cocher natives, rendu blanc en headless quand décochées (contraste du composant non mesuré).
- Les liens sortants vers `Legal.dc.html` depuis Connexion restent à ajouter par A ou le team lead.
