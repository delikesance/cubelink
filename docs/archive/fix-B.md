# Rapport fix-B : parcours ami (Rejoindre, Installation, Ajouter un ami)

Fichiers : `Join.dc.html`, `Install.dc.html`, `Friends.dc.html` (dossier `scratchpad\cv\project\`).
Captures : `docs\review\fix-B\{Join,Install,Friends}-{1280x720,960x600}.png`, plus variantes `Join-checked-*` (case cochée), `Friends-err-*` (code invalide) et `Friends-ok-*` (code valide). Rendu Chrome headless d'une copie statique (hors runtime du canvas).

## Tableau des constats

| ID | Statut | Changement (ou raison) |
|---|---|---|
| P-03 | corrigé | « fichiers vérifiés » supprimé ; texte d'intégrité imposé placé dans « Détails techniques ». |
| P-04 | corrigé | Plus de « Modrinth ou CurseForge » ; seul Modrinth est cité ; l'option « mods vérifiés » devient « mods référencés sur Modrinth » avec la mention « un mod connu n'est pas pour autant sans danger ». |
| P-05 | corrigé | Texte « mod inconnu » imposé repris tel quel (avec insécable avant « : »). |
| P-12 | partiel | Bouton principal désactivé (pointillés, grisé, texte d'aide) tant que la case n'est pas cochée ; activation en CSS pur (`:has(#trust-mod:checked)`), vérifiée en capture. Pas de logique réelle : le runtime canvas n'a pas de code, et l'état désactivé est un `span role="button" aria-disabled` remplacé par le lien actif. |
| D-05 / L-06 (Join) | corrigé | Modale limitée à `100vh - 80 px`, contenu défilant, pied d'action sticky ; bouton visible à 1280×720 et 960×600 (capturé). |
| D-05 (Friends) | partiel | Structure identique ; la demande en attente est placée en tête et visible sans défiler ; la zone « 2. Colle le code » reste en partie sous le pli à 960×600 (défilement interne à la modale, pas de pied fixe). |
| D-17 | corrigé | Tuiles Minecraft/Loader/Java fusionnées en une ligne de méta ; l'avertissement ambre est le seul bloc dominant ; intégrité et confiance repliées dans deux `details` avec chevron. |
| D-18 | corrigé | Cases et radios personnalisées (22 px, accent `#FF5C00`, bordure `#8A8A8A`, coche) ; focus visible. |
| D-16 | corrigé | Image de fond sans `pixelated`, floutée (8 px) et overlay `#0A0A0AEB`. Idem Friends. |
| P-23 | corrigé | 90 mods en cache, 3 à recevoir (38 Mo), dont 1 mod fait maison ; 24 ms ; Minecraft 1.21.1, NeoForge 21.1.77, Java 21. Install et Join concordent. |
| P-22 | corrigé | « profil invité » (au lieu de « profil séparé ») ; « modpack » absent ; « Rejoindre » pour l'action globale, « Réception des fichiers » pour l'étape. |
| P-10 | corrigé | « Si la connexion directe échoue, le transfert peut passer par un relais (un serveur intermédiaire) à débit limité : ce sera plus lent. » (cohérent avec le plafond de débit de `architecture.md` §3). |
| P-16 / D-22 / L-21 | corrigé | Lien « Voir l'état d'échec » supprimé. |
| P-24 | corrigé | Étapes : 1 Préparation, 2 Réception des fichiers, 3 Vérification d'intégrité, 4 Lancement, avec état en texte (Terminée / En cours / À venir). |
| L-08 | corrigé | `role="progressbar"` `aria-valuenow=26` `min=0` `max=100`, `aria-labelledby`, `aria-valuetext` ; « 26 % » en texte 32 px, plus porté par la couleur seule. |
| D-09 | corrigé | Barre 12 px (bordure `#8A8A8A`), pourcentage 32 px, carte sombre translucide au lieu de la carte blanche, dégradé plus léger : l'image est plus visible. Lune encore partiellement derrière le titre à 960×600. |
| D-10 | corrigé | « Monde d'Alex » en casse mixte, 52 px, `letter-spacing:-0.02em` ; pastille « Monde d'un ami » conservée. |
| L-16 | corrigé | `max-width:56ch` sur les paragraphes d'Install, 60ch sur les notes ; 56 à 62ch ailleurs. |
| L-17 | partiel | « Loader » retiré de l'affichage (remplacé par « NeoForge 21.1.77 ») ; « relais » expliqué à la première occurrence ; « SHA-256 » gardé dans le repli technique, sans « empreinte SHA-256 » en texte principal. |
| P-06 | corrigé | Texte du code ami imposé repris tel quel. |
| P-07 / L-07 / L-10 | corrigé | Code de vérification `3F9A · C21B · 77D0 · 5E14` sur une ligne, monospace, `nowrap`, 18 px ; noms « Mon code ami » / « Code de vérification » ; libellé de demande imposé ; phrase tautologique supprimée. La longueur (4 groupes = 64 bits) reste à valider côté architecture. |
| D-19 | corrigé | États par CSS (`:placeholder-shown`, `:invalid`, `:valid`, `pattern`) : vide (aide), erreur (bordure `#FF6B6B` + croix + texte), succès (bordure verte + coche + bouton actif). Capturés. |
| D-20 | corrigé | Carte neutre avec liseré ambre, avatar 40 px, texte pleine largeur, code + actions sur une ligne. |
| L-02 / L-04 | corrigé | Bordures de champs et boutons secondaires `#6A6A6A` ; placeholder `#9A9A9A`. |
| L-18 | corrigé | `…` partout, insécables avant `: ? !`, dans « » et entre nombre et unité. |
| P-32 | partiel | `mcl1-` et `launcher://` conservés conformément à `architecture.md` §4 (aucun schéma inventé) ; l'aide les cite. À trancher avec la marque (P-01). |
| P-21 | corrigé | Tutoiement ; plus de « nous » ni de « on ». |

Transversales : focus `:focus-visible` (`#FF8A4A`) dans le `<helmet>` des 3 pages ; texte de lecture ≥ 14 px (sauf pastille en majuscules 12 px gras) ; cibles ≥ 44 px ; zéro `href="#"`, zéro `opacity`, zéro `outline:0` (vérifié par grep) ; balises équilibrées (vérifié par un parseur Python).

Bilan : 22 corrigés, 4 partiels (P-12, D-05 Friends, L-17, P-32), 0 non corrigé (les lignes de regroupement comptent chacune pour un item).

## À confirmer dans le canvas
- Le bouton principal de Join : l'état activé/désactivé repose sur `:has()` ; à vérifier dans le runtime x-dc (et décider d'une vraie logique).
- `height:100vh` des pages : à vérifier que le canvas fournit bien une hauteur de viewport 1280×720 pour l'artboard.
- Validation de Friends (`pattern`, `:valid`) : le format réel du code (`architecture.md` §7) n'est pas défini ; le motif est provisoire.
- Rendu des polices Google (capturé en repli système si hors ligne) et des insécables.
- Photo Install : positionnement de la lune sur l'image publiée.
- Texte d'intégrité et de mod inconnu : validation juridique/sécurité (P-05, incident cité sans source dans la review).
