# Synthèse de la review : design, lisibilité, professionnalisme

Date : 2026-11-09. Cible : les 7 écrans du canvas « Cubelink » (version 16 publiée).
Aucune correction n'a été appliquée : ce document ne fait que prioriser.

## Sources

| Rapport | Auteur | Constats | Bloquant / Majeur / Mineur |
|---|---|---|---|
| `review-design.md` | review-design | 23 | 3 / 8 / 12 |
| `review-lisibilite.md` | review-lisibilite | 24 | 0 / 10 / 14 |
| `review-professionnalisme.md` | review-pro | 32 | 2 / 13 / 17 |

79 constats bruts. Après dédoublonnage : environ 60 points distincts.
Les rendus sont une copie statique approximative du canvas (`docs/review/render/`, captures dans `docs/review/screenshots/`).

## Verdicts

- **Design : 5,5/10.** Le fond est solide, mais l'écran Accueil a de vrais défauts de composition et la maquette est peu finie.
- **Lisibilité : WCAG AA partiellement conforme.** Les échecs mesurés concernent 1.4.3, 1.4.11, 2.4.7, 1.4.1 et 4.1.2. Le texte sur image est sûr (pire cas 4,75:1) et les boutons orange sont à 6,40:1.
- **Professionnalisme : prêt pour une démo interne présentée comme maquette, pas pour des testeurs externes.**

## Désaccord entre reviewers, tranché

- **D-03 (design) : « aucun écran ne tient à 390 px », classé Bloquant.** `review-lisibilite` montre que c'est un artefact de capture : Chrome headless impose environ 500 px de large minimum, puis rogne à 390. Ses captures à 500 px et 960×600 (`docs/review/extra-lisibilite/`) tiennent.
- **Décision :** D-03 est requalifié en Mineur. Un launcher de bureau n'a pas à descendre sous 390 px. Reste à spécifier une taille de fenêtre minimale, par exemple 960×600, et à tester à cette taille.
- **Limite à garder en tête :** `review-design` n'a pas ouvert 6 captures (`Install-1280xfull`, `Friends-1280xfull`, `Error-1280xfull`, `Login-1280xfull`, `Join-390`, `Friends-390`). Ce qu'il en dit est déduit des vues 1280×720.

## Top 10 des corrections

| # | Correction | Écran | Renvois | Effort |
|---|---|---|---|---|
| 1 | Corriger la collision du titre de 72 px avec « Mes instances ». Le titre déborde de sa boîte d'environ 20 px. Réduire le titre, ou lui donner une hauteur réelle, ou déplacer le bloc des instances. J'ai vu la collision moi-même sur `Main-1280x720.png`. | Accueil | D-01, L-05 | Faible |
| 2 | Ajouter une mention de non-affiliation à Mojang/Microsoft : « NOT AN OFFICIAL MINECRAFT PRODUCT. NOT APPROVED BY OR ASSOCIATED WITH MOJANG OR MICROSOFT » (formule des Minecraft Usage Guidelines, version française à faire valider). Prévoir un pied de page de la Connexion et les Crédits. | Connexion, Paramètres | P-01 | Faible |
| 3 | Rendre « Mentions légales » réel, ajouter une politique de confidentialité (pseudo, clé publique persistante, IP) et supprimer les liens `#` sans destination. | Paramètres | P-02, P-28 | Moyen |
| 4 | Corriger les promesses de sécurité de Rejoindre : remplacer « fichiers vérifiés » par « intégrité contrôlée, pas une garantie d'innocuité ». Ne plus citer « Modrinth ou CurseForge » : l'architecture ne prévoit un contrôle par hash que sur Modrinth. Dire clairement qu'un mod connu n'est pas pour autant sûr. | Rejoindre, Installation | P-03, P-04, P-05, P-12 | Faible |
| 5 | Corriger les promesses réseau et vie privée. Le code ami ne « contient » pas d'adresse IP selon le texte, mais `architecture.md` y met des adresses. « Masquer mon adresse IP » ne la masque pas au relais. Le repli par relais pour l'installation doit rester cohérent avec « envoi direct uniquement » (voir les points ouverts d'`architecture.md`). | Ajouter un ami, Paramètres, Installation | P-06, P-09, P-10, P-11, P-08 | Faible (texte) |
| 6 | Revoir le code de sécurité de l'ami. 48 bits (`3F9A · C21B · 77D0`) est faible, il porte trois noms selon l'écran, il est coupé sur deux lignes alors qu'il faut le comparer caractère par caractère, et le libellé « Tom dit s'appeler « Tom » » est tautologique. Afficher le code sur une seule ligne en monospace, avec un seul nom partout. | Ajouter un ami, Paramètres | P-07, L-07, L-10 | Moyen |
| 7 | Rendre les actions principales visibles à 720 px de haut. Join fait 885 px et le bouton « Installer… » est hors fenêtre. Même problème pour la demande d'ami en attente et pour les cartes sous Paramètres. Réduire les blocs, passer les détails en repli, fixer le pied d'action en bas de la modale. Spécifier aussi la taille minimale de fenêtre. | Rejoindre, Ajouter un ami, Paramètres | D-04, D-05, L-06, D-03 | Moyen |
| 8 | Corriger les contrastes mesurés. « Hors ligne · il y a 2 jours » est à 3,92:1 à cause de `opacity:0.7` : retirer l'opacité. Bordures de champs à 1,32:1 et 1,16:1 : passer à `#6A6A6A` (≈ 3,5:1). Placeholders à 4,30:1 et 3,92:1 : éclaircir. | Accueil, Ajouter un ami, Paramètres | L-01, L-02, L-04, L-12 | Faible |
| 9 | Définir un focus visible, par exemple `:focus-visible` en `#FF8A4A` (8,47:1), sur tous les éléments interactifs. Retirer `outline:0` du champ de recherche des Paramètres. Exposer la progression de l'Installation (`role="progressbar"` avec valeur). | Tous, Installation | L-03, L-08, L-20 | Faible |
| 10 | Remettre de la hiérarchie sur l'Accueil : un point focal net (« Jouer » arrive en troisième position), cartes d'instances moins rognées avec du texte lisible sur l'image, description et méta plus contrastées sur l'image. | Accueil | D-06, D-02, D-07, D-08 | Moyen |

## Autres points à traiter ensuite

- **Logo Microsoft** : il est monochrome dans un bouton orange, alors que les règles de Microsoft interdisent de l'altérer. Utiliser le bouton « Sign in with Microsoft » officiel (P-13).
- **Crédits** : le crédit OGA-BY est exact mais sans lien vers la licence, placé sous « Réseau P2P », et les polices ne sont pas créditées (P-14).
- **Typographie française** : aucune espace insécable dans le rendu (P-15, L-18). À confirmer dans le canvas source.
- **Tailles de texte** : beaucoup de texte de lecture sous 14 px et une échelle typographique éclatée (L-09, L-14). Cibles inférieures à 44 px (L-19).
- **Écran d'erreurs** : c'est une feuille de spécification, pas un écran (P-17, L-24, D-21). Le lien « Voir l'état d'échec » dans l'écran de succès est un artefact de prototype (P-16, D-22, L-21).
- **Cohérence** : « ALEX » en majuscules sur Installation contre le titre de l'Accueil (D-10), trois langages de surface différents (D-14), barres de titre et fonds (D-15), terminologie instance / profil / monde / modpack (P-22), chiffres incohérents entre écrans (P-23, 90 mods contre 94 mods).
- **Installation et Connexion** : écran Install quasi vide avec barre de progression discrète (D-09), fond de Connexion quasi invisible (D-11).

## Risques

1. **Juridique** : P-01, P-02, P-13 et P-31 sont à faire valider par un juriste. Aucun de ces points n'a été tranché ici.
2. **Nom « Cubelink »** : la recherche web est non concluante. Seul « Cubion Launcher for Minecraft » est apparu, vu dans des extraits de recherche. Ni INPI, ni EUIPO, ni USPTO n'ont été consultés.
3. **Sécurité** : l'incident Fractureiser cité en P-05 vient de la connaissance du reviewer et n'est pas sourcé. Le code de sécurité à 48 bits est à faire valider par un cryptographe. Le point peut changer l'architecture, pas seulement le texte.
4. **Rendu** : toutes les mesures viennent d'une copie statique, pas du runtime du canvas. Focus réel, survol, comportement des dialogues et espaces insécables sont « à confirmer dans le canvas ».
5. **Polices** : les mesures supposent Inter et Montserrat chargées depuis Google Fonts. Hors ligne, le repli `system-ui` change les largeurs (L-13, P-30).
6. **Images** : le texte sur image a été évalué sur les seuls fonds fournis. D'autres instances peuvent avoir des fonds plus clairs.

## Tests restants

- Ouvrir le canvas réel et vérifier les points marqués « à confirmer dans le canvas » : focus clavier, survol, case de confiance (le bouton « Installer… » est-il désactivé tant qu'elle n'est pas cochée ?), Découvrir, espaces insécables.
- Re-mesurer les contrastes après correction (script de `review-lisibilite`, dans `docs/review/extra-lisibilite/audit/`).
- Rendre les écrans corrigés à 960×600 et à 1280×720, et vérifier qu'aucune action principale n'est hors fenêtre.
- Passer le français au correcteur avec la typographie française (espaces insécables, guillemets).
- Faire valider les points juridiques (marque, non-affiliation, bouton Microsoft, confidentialité).
- Test d'usabilité avec 3 à 5 personnes : ajout d'un ami, rejoindre un monde avec un mod inconnu.

## Fichiers produits

- `docs/review-design.md`
- `docs/review-lisibilite.md`
- `docs/review-professionnalisme.md`
- `docs/review-synthese.md` (ce document)
- `docs/review/render/` (7 pages + index) et `docs/review/screenshots/` (21 captures), créés par le team lead.
- `docs/review/extra-lisibilite/` (copies instrumentées dans `audit/`, captures 960 et 500 px).

Le canvas, `assets/`, `architecture.md` et les autres documents n'ont pas été modifiés. Rien n'a été publié.

## Prochaines étapes proposées

1. **Lot A, textes uniquement** (points 2, 3, 4, 5, 6) : il se règle presque sans toucher à la mise en page.
2. **Lot B, CSS** (points 1, 8, 9, 10 et la hiérarchie de l'Accueil) : correctifs rapides sur les écrans existants.
3. **Lot C, structure** (point 7 et l'écran Erreurs) : modales plus courtes, pied d'action fixe, taille minimale de fenêtre.
4. Republier le canvas, refaire les rendus, relancer les mesures de lisibilité.
5. Décider avec toi si le code de sécurité à 48 bits doit changer dans `architecture.md`.
