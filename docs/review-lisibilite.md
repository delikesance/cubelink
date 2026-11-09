# Review LISIBILITÉ et ACCESSIBILITÉ — Cubelink (7 écrans)

Reviewer B. Review critique fondée sur des mesures. Date : 2026-11-09.

## 0. Méthode et limites (à lire d'abord)

Mesures réellement exécutées (scripts dans le scratchpad, copies instrumentées des rendus dans `docs\review\extra-lisibilite\audit\`) :

1. **Styles calculés** : chaque rendu `render\*.html` a été copié avec un script d'audit (ajouté avant `</body>`) puis chargé dans Chrome headless en 1280 px de large. Pour chaque nœud texte : couleur calculée, fond effectif (composition alpha des ancêtres, `opacity` incluse), `font-size`, graisse, rectangle. Ratios WCAG = formule de luminance relative (sRGB linéarisé, seuil 0,03928).
2. **Texte sur image** : couleur de fond estimée à partir des pixels réels des captures Chrome du même rendu (1280x720) : 95e centile de la luminance des pixels « sombres » (< 25 % de la luminance du texte) dans le rectangle du texte ±6 px. C'est une estimation du pire cas, pas un maximum absolu (voir limites). Pour Login, borne théorique supplémentaire : pixel source blanc 255 sous l'overlay réel (`#0A0A0AD9` à `#0A0A0AF2`).
3. **Cibles** : rectangles des `a, button, input, label, summary` à 1280x720.
4. **Structure** : analyse du HTML (titres, landmarks, labels, `aria-*`, règles `outline`/`:focus`).
5. Captures relues : les 21 captures fournies (1280x720, 1280xfull, 390) plus 4 captures en 960x600 et 1 en 500 px (`extra-lisibilite\`).

Limites :
- Rendu statique approximatif : pas d'états `:hover`/`:focus`/animations, pas de comportement du canvas. Les points marqués **« à confirmer dans le canvas »** ne sont pas établis.
- **Les captures 390 px sont un artefact de capture** : Chrome headless impose une largeur mini d'environ 500 px, la page est mise en page à ~500 px puis rognée à 390 (contenu centré décalé, tout est coupé à droite). Je ne les compte pas comme des défauts. À 500 px (`Main-500.png`) et 960x600 (`Main/Settings-960x600.png`) la mise en page tient. Aucune taille minimale de fenêtre n'est spécifiée dans `ux-guidelines.md`/`architecture.md` (recherche vide) : à fixer.
- L'estimation sur image ne couvre que les images du rendu (hero de « Create: Aeronautics », fond Login, fond Install). Les héros des autres instances (« All the Mods 10 », « Vanilla+ ») peuvent être plus clairs : à confirmer dans le canvas.
- Les ratios sont des ratios de couleurs ; l'anticrénelage et le rendu LCD de Chrome peuvent déplacer les valeurs échantillonnées de quelques dixièmes.
- Polices : Google Fonts se charge en ligne ; hors-ligne le rendu retombe sur `system-ui` (largeurs différentes). Mes mesures de largeur supposent Inter/Montserrat chargées.

## 1. Verdict global WCAG 2.2 AA : **PARTIELLEMENT conforme**

La base est solide : 100 % des couples texte/fond « de série » passent 4,5:1 sauf un (« Hors ligne », 3,92:1), le texte sur image est sûr (Login au pire 4,75:1 sur pixel blanc, Main/Install ≥ 4,8:1 estimé), les cibles interactives principales font 44 à 56 px, les icônes sont `aria-hidden`, les dialogues ont `role="dialog"`, `aria-modal`, `aria-labelledby`, les groupes radio ont `fieldset/legend`, les erreurs utilisent icône + libellé + couleur.

Non conforme sur 5 critères mesurés :
- **1.4.3** Contraste du texte : « Hors ligne · il y a 2 jours » 3,92:1 ; placeholders 4,3:1 et 3,92:1 (limite).
- **1.4.11** Contraste des composants : bordures de champs 1,32:1 (Friends) et 1,16:1 (Settings) ; boutons « ghost » 1,65:1.
- **2.4.7** Focus visible : aucun style de focus défini ; `outline:0` sur la recherche Settings sans substitut.
- **1.4.1** Couleur seule : lien inline « En acheter un » distingué par la couleur seule (1,51:1 avec le texte voisin, soulignement au survol uniquement).
- **4.1.2** Nom/rôle/valeur : la progression d'installation n'expose ni `progressbar` ni valeur.

Aucun constat Bloquant (rien n'empêche l'usage complet). 10 Majeurs, 14 Mineurs.

## 2. Constats triés par gravité

### Majeurs

**L-01 — Main — « Hors ligne · il y a 2 jours » — Majeur**
- MESURE : texte rendu `#707070` sur `#0D0D0D` = **3,92:1** (12 px / 400, seuil 4,5:1). Cause : `<li style="opacity:0.7">` sur la ligne de Léa ; couleur source `#9A9A9A` (6,91:1) ramenée à `#707070` par l'opacité. « Joue à… » des autres amis (non atténué) : 6,91:1.
- CORRECTION : supprimer l'`opacity` de la ligne (l'avatar est déjà en niveaux de gris, la pastille grise `#6A6A6A` et le texte portent l'état) ; garder `#9A9A9A` (6,91:1). Si l'atténuation est voulue : `#8A8A8A` sur `#0D0D0D` = 5,63:1 (attention : `#767676` ne donne que 4,28:1 sur ce fond).

**L-02 — Friends / Settings — champs de saisie sans bordure perceptible — Majeur (1.4.11)**
- MESURE : Friends, champ « code de l'ami » : bordure `#2A2A2A` sur panneau `#111111` = **1,32:1** ; son fond `#0A0A0A` vs panneau = 1,05:1. Settings, champ de recherche : bordure `#1F1F1F` sur `#0F0F0F` = **1,16:1**, fond `#141414`. Capture Friends-1280x720 : le champ n'est reconnaissable que par son léger creux de fond.
- CORRECTION : bordure `#6A6A6A` (3,49:1 sur `#111111`, 3,41:1 sur `#141414`) ou `#7A7A7A` (4,40:1 sur `#111111`). Même valeur pour les champs d'une même famille.

**L-03 — Tous écrans / Settings — focus clavier — Majeur (2.4.7) — à confirmer dans le canvas**
- MESURE : aucune règle `:focus`/`:focus-visible` dans les 7 rendus (0 occurrence). Settings : `<input type="search" … outline:0>` posé dans un `<label>` sans `:focus-within` : **aucun** indicateur de focus sur ce champ. Ailleurs, on dépend de l'anneau natif du navigateur, qui n'est pas garanti dans l'app finale ni cohérent sur les boutons orange et sur la carte blanche d'Install.
- CORRECTION : définir globalement `:focus-visible{outline:2px solid #FF8A4A;outline-offset:2px}` (8,47:1 sur `#0A0A0A`) ; sur la carte blanche d'Install `outline-color:#B33F00` (5,79:1 sur blanc) ; `label:focus-within{border-color:#FF8A4A;outline:2px solid #FF8A4A}` pour la recherche.

**L-04 — Friends / Settings — placeholders — Majeur (limite)**
- MESURE : placeholder sans couleur déclarée = défaut Chrome `#757575` : **4,30:1** sur `#0A0A0A` (Friends, « mcl1-… ») et **3,92:1** sur `#141414` (Settings, « Rechercher un réglage... »), mesurés par échantillonnage des pixels de la capture (`#757575` sur `#0A0A0A` ; `#737472` sur `#141414`).
- CORRECTION : `::placeholder{color:#8A8A8A}` = 5,73:1 sur `#0A0A0A`, 5,34:1 sur `#141414`. Le champ Settings n'a ni `aria-label` ni libellé visible : ajouter `aria-label="Rechercher un réglage"`.

**L-05 — Main — titre 72 px en collision avec « Mes instances » — Majeur**
- MESURE : `<h1>` en 72 px/800, boîte x 112→508 (396 px) mais le mot « Aeronautics » mesure ≈ 448 px (x 112→≈560, capture Main-1280x720) : débordement de ≈ 52 px hors de sa boîte, soit ≈ **20 px de recouvrement** avec le libellé « Mes instances » (x 540, y 481→500) et la première carte (x 540). À 960x600 la collision disparaît (les cartes passent sous le titre).
- CORRECTION : `font-size:clamp(40px,4.9vw,62px)` (62 px → ≈ 386 px, tient dans 396 px), ou `overflow-wrap:anywhere`/`hyphens:auto` + réserver une colonne dédiée aux cartes (`min-width` de la colonne titre ≥ 460 px).

**L-06 — Join / Friends / Settings — contenu essentiel sous le pli à 1280x720 — Majeur — à confirmer dans le canvas**
- MESURE : hauteurs de page 885 px (Join), 784 px (Friends), 1 081 px (Settings) pour une fenêtre de 720. Join : le bouton principal « Installer le mod d'Alex et rejoindre » (y ≈ 735→783) et la phrase de rassurance sont **hors écran** ; en 1280x720 on voit la case de confiance mais pas l'action. Friends : la carte « Demande en attente » est coupée en bas (boutons Accepter/Refuser à y ≈ 640→684, texte du code jusqu'à y ≈ 718). Settings : les deux interrupteurs et les crédits sont sous le pli.
- CORRECTION : dialogue `max-height:calc(100vh - 64px)` avec zone centrale défilante et **pied fixe** contenant l'action principale (Join) ; vérifier le comportement réel dans le canvas.

**L-07 — Friends — code de sécurité de l'ami coupé et libellé tautologique — Majeur**
- MESURE : « 3F9A · C21B · 77D0 » (13 px mono gras) est **coupé en deux lignes** (« 3F9A » en fin de ligne 4, « · C21B · 77D0 » ligne 5, capture Friends-1280x720) alors que l'utilisateur doit le comparer caractère par caractère avec un autre canal. Texte : « Tom dit s'appeler « Tom ». Cubelink ne peut pas le prouver » : l'information utile (le nom affirmé diffère-t-il ?) est noyée ; 5 lignes dans une colonne de 260 px.
- CORRECTION : mettre le code sur sa propre ligne, `display:block; white-space:nowrap; font:600 16px/1.4 ui-monospace; letter-spacing:.06em`, couleur `#F0D9A8` (12,43:1 sur `#231A08`). Reformuler : « Tom se présente comme « Tom ». Cubelink ne peut pas vérifier ce nom. Compare ce code avec le sien (Discord, SMS) avant d'accepter : »

**L-08 — Install — progression non exposée — Majeur (4.1.2, 1.4.1)**
- MESURE : carte `role="status" aria-live="polite"` contenant la liste de 4 étapes : aucun `role="progressbar"`, ni `aria-valuenow`, ni `aria-current="step"` ; l'étape courante n'est portée que par la graisse (700 vs 600) et le remplissage de la barre `#FF5C00` ; les étapes terminées ne portent pas de texte « terminé ». Toute la carte (≈ 12 lignes de texte changeantes) est une région live : annonces verbeuses à chaque mise à jour. Barres : remplissage `#FF5C00` / blanc **3,10:1** (passe 3:1 de justesse), piste `#E4E4E4` / blanc **1,27:1**.
- CORRECTION : `role="progressbar" aria-label="Téléchargement" aria-valuemin="0" aria-valuemax="100" aria-valuenow="26"` sur la barre ; `aria-current="step"` sur l'`<li>` courant ; texte visuellement masqué « terminé » sur les précédents ; limiter `aria-live` à une ligne (« 26 %, environ 1 min restante ») ; piste `#BDBDBD` (≥ 3:1 sur blanc) ou bordure.

**L-09 — Tous écrans — texte de lecture sous 14 px — Majeur**
- MESURE : sur 166 nœuds texte, **29 en 12 px et 30 en 13 px**, soit 59 (35,5 %) sous 14 px. En 13 px : consignes (Friends « Ce code est public… », liste d'étapes), détails Install/Join/Settings, crédits, descriptions de réglages, sous-titres de lignes. En 12 px : sous-titres amis, tags, titre de fenêtre. Aucun texte < 12 px (bien).
- CORRECTION : plancher à **14 px** pour tout texte de lecture ; 12 px réservé aux libellés courts en capitales (étiquettes) et au titre de fenêtre ; 13 px supprimé de l'échelle.

**L-10 — Friends / Settings — trois « codes » nommés de façon incohérente — Majeur**
- MESURE : (1) le code d'ami `mcl1-k3f9-…` est appelé « code » ; (2) `3F9A · C21B · 77D0` est « code de sécurité » dans Friends mais **« Identifiant Cubelink »** dans Settings, et « Identité du nœud : ed25519:3f9a…c21b77d0 » dans le détail technique ; (3) « Générer un nouveau code » rend l'ancien invalide sans dire lequel (le 1). Un utilisateur ne peut pas relier l'« Identifiant » de ses réglages au « code de sécurité » qu'on lui demande de comparer.
- CORRECTION : un nom par objet : « Code d'ami » (mcl1-…) et « Code de vérification » (3F9A · C21B · 77D0, partout, y compris Settings) ; « nœud » et « ed25519 » uniquement dans « Détails techniques ».

### Mineurs

**L-11 — Login — lien inline « En acheter un » — Mineur (1.4.1)**
- MESURE : orange `#FF5C00` dans une phrase `#B5B5B5` : rapport entre les deux couleurs **1,51:1** (< 3:1) ; pas de soulignement hors survol (`a.link:hover`). Contraste sur fond : 6,39:1 (sur `#0A0A0A`), estimé 5,33:1 sur l'image, 5,24:1 pire cas pixel blanc. Graisse 600 vs 400 aide, mais ne suffit pas.
- CORRECTION : `text-decoration:underline; text-underline-offset:3px` en permanence.

**L-12 — Error / Friends / Settings / Main — bordures de boutons secondaires — Mineur (1.4.11)**
- MESURE : « Rouvrir la fenêtre » et « Acheter le jeu » bordure `#3A3A3A` = **1,65:1** (sur `#121212`/`#161010`) ; « Refuser » `#6B4A10` sur `#231A08` = **2,14:1** ; « Vider le cache… » `#8A4A32` sur `#141414` = **2,72:1** ; carte alerte Join bordure `#6B4A10` sur `#111111` = 2,35:1. Le texte (12–16:1) identifie le bouton, d'où Mineur ; en revanche « Rejoindre » (`#6A6A6A` sur `#0D0D0D` = 3,59:1) est conforme.
- CORRECTION : `#6A6A6A` (≈ 3,5:1) pour les boutons neutres ; « Refuser » `#A8782A` (4,40:1 sur `#231A08`) ; « Vider le cache… » `#B8654A` (4,38:1 sur `#141414`).

**L-13 — Tous — graisse 700 d'Inter non chargée ; polices distantes — Mineur — à confirmer dans le canvas**
- MESURE : la feuille Google Fonts demande `Inter:wght@400;500;600` (et Montserrat 500–800). Or **une trentaine de nœuds** utilisent le 700 (boutons « Jouer », « Réessayer », titres de cartes, tags) : le navigateur retombe sur 600, donc 600 et 700 sont identiques et la hiérarchie par graisse est plus plate que prévu. De plus la police est chargée depuis Internet pour une application de bureau (hors-ligne : repli `system-ui`).
- CORRECTION : `wght@400;500;600;700` ou ramener tous les 700 à 600 ; embarquer Inter et Montserrat dans l'application.

**L-14 — Tous — échelle typographique éclatée — Mineur**
- MESURE : 11 tailles distinctes (12, 13, 14, 15, 16, 18, 20, 24, 32, 44, 72 px) dont quatre dans une plage de 3 px (12/13/14/15), 5 graisses (400, 500, 600, 700, 800). Interlignage correct sur les paragraphes (1,5 à 1,6) ; titres à `line-height:1` (72 px) ; beaucoup de `line-height:normal` sur les libellés.
- CORRECTION : échelle de 7 pas : 12 (étiquettes) / 14 (corps) / 16 / 20 / 24 / 32 / 56–72 ; graisses 400 / 600 / 800 (Montserrat).

**L-15 — Main — nom du jeu des amis tronqué — Mineur**
- MESURE : « Joue à Create: Ae… » et « Joue à All the Mo… » (`text-overflow:ellipsis`, boîte de 116 px, 12 px, aside max 300 px) : l'information qui motive le clic sur « Rejoindre » est coupée ; aucun `title`. Le nom accessible du lien est « Rejoindre Alex » (le jeu manque).
- CORRECTION : deux lignes autorisées (`-webkit-line-clamp:2`) ou ligne « Joue à » seule + nom du jeu en dessous ; `aria-label="Rejoindre Alex, qui joue à Create: Aeronautics"`.

**L-16 — Settings / Install — longueur de ligne — Mineur**
- MESURE : crédits Settings : 1 038 px de large en 13 px, **157 caractères sur une seule ligne** (capture Settings-1280xfull) ; description d'interrupteur ≈ 125 caractères/ligne ; paragraphe Install en 16 px sur 640 px ≈ 84 caractères. Cible usuelle 45–75.
- CORRECTION : `max-width:68ch` sur les paragraphes (≈ 600 px en 14 px) ; Install : `max-width:560px`.

**L-17 — Join / Settings / Install — jargon — Mineur**
- MESURE (libellés relevés) : « empreinte SHA-256 » (Join, détails), « Identité du nœud : ed25519:3f9a…c21b77d0 » (Settings, détails), « serveur relais public » / « passe par un relais » (Settings, Install), « Loader » (étiquette anglaise à côté de « Minecraft » et « Java », Join), « mod fait maison, inconnu des catalogues », « bande passante », « profil séparé ». « Manifeste » n'apparaît pas (bien). L'effet de levier est limité : empreinte et nœud sont repliés dans « Détails techniques » (bon choix) ; « relais » est en revanche dans le texte principal d'Install.
- CORRECTION : « relais » → « un serveur intermédiaire (plus lent) » à la première occurrence ; « Loader » → « Chargeur de mods » ; « empreinte SHA-256 » → « signature de vérification (SHA-256) ».

**L-18 — Settings / Friends — ponctuation et typographie — Mineur**
- MESURE : « Test à 14:03 : 41 ms. » (deux `:` consécutifs, sens ambigu) ; points de suspension incohérents : « Rechercher un réglage... » (3 points) vs « Vider le cache… » / « mcl1-… » (caractère …) ; espaces ordinaires avant `?`/`:`/`»` (césure possible entre le mot et le signe, ex. « En acheter / un ? ») ; « Direct · 24 ms » sans unité de contexte.
- CORRECTION : « Test effectué à 14:03 — 41 ms » ; utiliser `…` partout ; espaces insécables (U+00A0 / U+202F) avant `: ; ? !` et dans « ».

**L-19 — Tous — cibles < 44 px et incohérentes — Mineur (2.5.8)**
- MESURE : voir tableau (c) : 13 boutons de contrôle de fenêtre en **46 × 32** (hauteur 32 < 44) ; radios 16 px, cases 18–20 px (la zone cliquable est le `<label>` `min-height:44px`, conforme), liens de crédits inline 17 px (exception « lien dans un texte »). Espacements : fenêtre 0 px entre les contrôles (usage système), Main « Messages »/« Paramètres » 6 px, autres ≥ 12 px.
- CORRECTION : contrôles de fenêtre natifs ou 46 × 44 hors zone d'en-tête ; harmoniser case/radio à 20 px.

**L-20 — Login / Install / ordre de tabulation — Mineur**
- MESURE : Login n'a **aucun titre** (la marque CUBELINK est un `<span>`) ; Install a un `<h1>` « ALEX » peu descriptif (nom seul) sans `h2` pour les sections ; Main : `h1` = nom d'instance (acceptable), `h2` « Mes instances » ; Settings/Friends/Error cohérents. Dans le DOM, les boutons Réduire/Agrandir/Fermer précèdent tout le contenu sur 6 écrans : le premier arrêt de tabulation est « Réduire ». Ordre DOM = ordre visuel ailleurs (nav → main → aside sur Main).
- CORRECTION : `<h1>` visuellement masqué « Connexion à Cubelink » (Login) ; Install : `<h1>` « Rejoindre le monde d'Alex » ; barre de titre après le contenu dans le DOM (ou `tabindex="-1"`, si zone de glisser-déposer native).

**L-21 — Install — lien « Voir l'état d'échec » accolé à une phrase — Mineur**
- MESURE : le lien (13 px, orange `#FF8A4A`, 8,39:1 estimé sur image, rectangle 102 × 44) est immédiatement suivi de « · si la connexion directe échoue, l'installation passera par un relais, plus lent. » : lu d'un bloc, le lien pointe vers un état d'échec alors que la phrase décrit un cas de repli. Capture 1280x720 : les deux se lisent comme une seule phrase.
- CORRECTION : séparer en deux éléments (phrase informative d'abord, lien « En savoir plus sur les échecs » en dessous) ; libellé de lien explicite.

**L-22 — Settings — rangées et crédits collés au bord, libellés d'interrupteurs ambigus — Mineur — à confirmer dans le canvas**
- MESURE : à partir de « Envoyer des mods à mes amis » les rangées, « Détails techniques » et « Crédits et licences » démarrent à x = 121 (bord du dialogue) sans marge, alors que le haut du dialogue a 24 px de marge (capture Settings-1280xfull). Le texte des crédits touche le contour. Les libellés des cases : « Activé » (état, pas action ; resterait « Activé » décoché ?) et « Relais uniquement » (autre forme) : deux grammaires pour deux interrupteurs voisins.
- CORRECTION : même `padding:0 24px` que le reste du dialogue ; libellés d'action : « Envoyer des mods à mes amis » (case seule) et « Passer toujours par le relais ».

**L-23 — Texte sur image — marges faibles — Mineur — à confirmer dans le canvas**
- MESURE : valeurs minimales estimées (95e centile du fond réel) : Install « MONDE D'UN AMI » 12 px **4,83:1** ; Login « En acheter un » **5,33:1** ; Main « 2 amis en ligne » 6,23:1, « NeoForge · 412 mods » 7,37:1. Tout le reste ≥ 8:1. Les textes sur la carte blanche d'Install sont conformes (≥ 5,33:1). Les overlays Main/Install sont plus légers à droite (`#0A0A0A4D`, `#0A0A0A80`) : avec un héros plus clair, ces marges fondent.
- CORRECTION : conserver les overlays actuels et tester les héros « city » et « forest » ; plancher d'overlay 0,6 sous tout texte < 24 px.

**L-24 — Error — texte de spécification dans l'interface — Mineur**
- MESURE : titre « Quand ça ne se passe pas comme prévu », sous-titre « Trois états à prévoir : … » et barre « Cubelink · États d'attente et d'échec » sont du texte de planche de conception, pas d'interface. Les trois cartes elles-mêmes sont claires (icône + libellé « échec » + couleur, 12,2 à 16,8:1).
- CORRECTION : ne pas livrer cette copie ; garder la page comme planche de spécification ou la scinder en états réels.

## 3. Tableaux de mesures

### (a) Contrastes par écran

Texte « normal » : seuil 4,5:1 ; grand texte (≥ 24 px, ou ≥ 18,66 px gras) : 3:1. Fonds opaques : ratio calculé ; sur image : estimation 95e centile (voir §0).

| Écran | Élément | Couleurs (texte / fond) | Taille | Ratio | Verdict |
|---|---|---|---|---|---|
| Login | Logo « CUBELINK » | `#FF5C00` / `#0A0A0A` (image) | 44/800 | 6,39 (échantillon 6,23 ; pire pixel blanc 4,75) | OK |
| Login | Accroche | `#B5B5B5` / `#0A0A0A` | 16/400 | 9,66 (échantillon 9,20 ; pire 7,45) | OK |
| Login | Bouton « Se connecter » | `#140700` / `#FF5C00` | 18/700 | 6,40 | OK |
| Login | Lien « En acheter un » | `#FF5C00` / `#0A0A0A` | 14/600 | 6,39 (échantillon 5,33 ; pire 5,24) | OK ; lien/texte 1,51 (L-11) |
| Login | Paragraphe clé privée | `#B5B5B5` / `#0A0A0A` | 14/400 | 9,66 (échantillon 7,29 ; pire 8,13) | OK |
| Login | Pied de page | `#B5B5B5` / `#0A0A0A` | 14/600 | 9,66 (échantillon 7,66–8,99) | OK |
| Main | **Hors ligne · il y a 2 jours** | `#707070` (`#9A9A9A` × opacité 0,7) / `#0D0D0D` | 12/400 | **3,92** | **Échec** (L-01) |
| Main | « Joue à … » | `#9A9A9A` / `#0D0D0D` | 12/400 | 6,91 | OK |
| Main | « En ligne » | `#4ADE80` / `#121212` | 12/500 | 10,75 | OK |
| Main | Titre héros | `#F2F2F2` / image | 72/800 | 17,68 (échantillon 14,97) | OK |
| Main | Description héros | `#D4D4DC` / image | 16/400 | 13,43 (échantillon 10,17) | OK |
| Main | Méta « Stable · Minecraft… » | `#C0C0CC` / image | 13/400 | 10,99 (échantillon 10,56) | OK |
| Main | Pastille « 2 amis en ligne » | `#D8D0FF` / `#181338` | 12/600 | 12,10 (échantillon 6,23) | OK |
| Main | Cartes « NeoForge · 412 mods » | `#E4E4EC` / image | 12/400 | 15,66 (échantillon 7,37) | OK |
| Main | Avatar « S » | `#F2F2F2` / `#6D4AE8` | 14/600 | 4,94 | OK (limite) |
| Main | Titre de fenêtre | `#8A8A8A` / `#0A0A0A` | 12/400 | 5,73 | OK |
| Install | Étapes inactives « Lancement » | `#6B6B6B` / `#FFFFFF` | 12/600 | 5,33 | OK |
| Install | « 26 % » | `#B33F00` / `#FFFFFF` | 12/700 | 5,79 | OK |
| Install | Statut / débit | `#2A2A2A` / `#FFFFFF` | 14/400–600 | 14,35 | OK |
| Install | « Les fichiers déjà reçus… » | `#4A4A4A` / `#FFFFFF` | 13/400 | 8,86 | OK |
| Install | Étiquette « MONDE D'UN AMI » | `#F2F2F2` / image | 12/700 | 17,68 (échantillon 4,83) | OK (limite) |
| Install | Description | `#E4D8CC` / image | 16/400 | 14,13 (échantillon 10,41) | OK |
| Install | Lien « Voir l'état d'échec » | `#FF8A4A` / image | 13/400 | 8,47 (échantillon 8,33) | OK |
| Install | Barre de progression (UI) | `#FF5C00` / `#FFFFFF` | — | 3,10 | OK (limite) |
| Install | Piste (UI) | `#E4E4E4` / `#FFFFFF` | — | 1,27 | non porteuse d'info seule |
| Join | Sous-titres 13–14 px | `#B5B5B5` / `#111111` | 13–14 | 9,21 | OK |
| Join | « 38 Mo · fichiers vérifiés » | `#9A9A9A` / `#111111` | 14/400 | 6,71 | OK |
| Join | Alerte (titre / corps / case) | `#FFD27A`, `#F5D79A`, `#FFF3DA` / `#231A08` | 14 | 12,06 / 12,32 / 15,61 | OK |
| Join | Bouton principal | `#140700` / `#FF5C00` | 15/700 | 6,40 | OK |
| Friends | Code d'ami | `#FF8A4A` / `#0A0A0A` | 13 mono | 8,47 | OK |
| Friends | « Copié dans le presse-papiers » | `#4ADE80` / `#111111` | 13/600 | 10,84 | OK |
| Friends | Erreur « Ce code n'est pas reconnu » | `#FFB4AB` / `#111111` | 13/400 | 11,12 | OK |
| Friends | **Placeholder « mcl1-… »** | `#757575` / `#0A0A0A` | 13 mono | **4,30** (échantillon) | **Limite/échec** (L-04) |
| Friends | **Bordure du champ (UI)** | `#2A2A2A` / `#111111` | — | **1,32** | **Échec** (L-02) |
| Friends | Demande en attente (corps / code) | `#F0D9A8` / `#231A08` | 13 | 12,43 | OK |
| Friends | « Envoyer la demande » (désactivé) | `#B5B5B5` / `#1A1A1A` | 14/600 | 8,49 | exempt (désactivé), mais paraît actif |
| Settings | **Placeholder « Rechercher… »** | `#757575` / `#141414` | 13 | **3,92** (échantillon) | **Échec** (L-04) |
| Settings | **Bordure recherche (UI)** | `#1F1F1F` / `#0F0F0F` | — | **1,16** | **Échec** (L-02) |
| Settings | Descriptions | `#B5B5B5` / `#141414` | 13 | 8,98 | OK |
| Settings | « Vider le cache… » | `#FF9A7A` / `#141414` | 14/600 | 8,91 | OK ; bordure 2,72 (L-12) |
| Settings | Crédits | `#D0D0D0` / `#0F0F0F` | 13 | 12,43 | OK |
| Settings | Lien licence | `#FF8A4A` / `#0F0F0F` | 13 | 8,20 | OK (souligné) |
| Error | Étiquettes « échec » | `#FF8A80` / `#161010` | 12/700 | 8,25 | OK |
| Error | Corps de cartes | `#D0D0D0` / `#161010` | 14/400 | 12,20 | OK |
| Error | Listes | `#B5B5B5` / `#161010` | 14/400 | 9,18 | OK |
| Error | Titres de cartes | `#F2F2F2` / `#161010` | 20/700 | 16,81 | OK |
| Error | Bordure « Rouvrir la fenêtre » (UI) | `#3A3A3A` / `#121212` | — | 1,65 | Échec mineur (L-12) |

### (b) Tailles de police (166 nœuds texte, 7 écrans)

| font-size | Occurrences | Usage principal | Jugement |
|---|---|---|---|
| 12 px | 29 | titre de fenêtre, sous-titres, tags, étiquettes en capitales | OK pour étiquettes ; < 14 px pour du texte de lecture : à relever |
| 13 px | 30 | consignes, aides, crédits, détails, descriptions | **à supprimer** (lecture sous 14 px) |
| 14 px | 87 | corps de l'application, boutons | référence |
| 15 px | 2 | bouton Join | à fondre dans 14/16 |
| 16 px | 6 | accroches, héros, boutons | OK |
| 18 px | 2 | bouton Login, logo Install | OK |
| 20 px | 4 | titres de cartes Error, H2 Settings | OK |
| 24 px | 2 | H1 de dialogues | OK |
| 32 px | 1 | H1 Error | OK |
| 44 px | 1 | logo Login | OK |
| 72 px | 2 | H1 Main / Install | OK (cf. L-05) |
| < 12 px | 0 | — | conforme |

Graisses : 400 (≈ 73 nœuds), 500 (1), 600 (≈ 52), 700 (33), 800 (7) ; seules 400/500/600 sont chargées pour Inter (L-13). Interlignage : 1,5 à 1,6 sur les paragraphes (bon), `normal` ailleurs (≈ 1,2). Longueur de ligne : 45–60 caractères (Error, Join, Friends) bon ; 84 (Install), ≈ 125 (interrupteurs Settings), 157 (crédits Settings) trop longs (L-16).

### (c) Cibles < 44 px (à 1280x720)

| Écran | Élément | Taille (l × h) | Remarque |
|---|---|---|---|
| Login, Main, Install, Join, Friends, Settings | Boutons Réduire / Agrandir / Fermer (13 occurrences) | 46 × 32 | hauteur < 44 ; usage système |
| Join | `<input checkbox>` (confiance) | 20 × 20 | zone cliquable = `<label>` `min-height:44px` : OK |
| Join | `<input radio>` ×2 | 16 × 16 | idem ; plus petit que les cases |
| Settings | `<input checkbox>` ×2 | 18 × 18 | idem |
| Settings | `<input search>` | 159 × 19 | zone = `<label>` 44 px : OK |
| Settings | Lien licence (crédits, inline) | 408 × 17 | exception « lien dans un texte » |

Toutes les autres cibles mesurées sont ≥ 44 px : « Jouer » 124 × 48, sélecteur de profil 146 × 48, tuiles d'instance 48 × 48, « Rejoindre » 91 × 44, « Ajouter un ami » 44 × 44, Paramètres 44 × 44 / 46 × 46, boutons Error/Friends/Settings 44–56 px de haut. Espacement entre cibles : ≥ 12 px sauf Messages/Paramètres de Main (6 px) et contrôles de fenêtre (0 px, usage système).

## 4. Ce qui est conforme

- Contraste global du texte : toutes les paires de série ≥ 4,5:1 sauf « Hors ligne » ; texte sur images ≥ 4,83:1 (échantillon) et, pour Login, ≥ 4,75:1 même sur pixel blanc.
- Boutons orange à texte `#140700` : 6,40:1 partout.
- Cibles principales ≥ 44 px, `<label>` de 44 px autour des cases et radios.
- Structure : landmarks (`nav`, `main`, `aside`, `header`, `footer`), `aria-label` sur toutes les icônes-boutons de Main/Settings/Friends/Join , `aria-hidden` sur les SVG décoratifs, `aria-current="page"`, `role="dialog"` + `aria-modal` + `aria-labelledby` (Join, Friends, Settings), `alertdialog` pour la confirmation de vidage du cache, `fieldset/legend` pour le groupe de confiance, `role="status"`/`role="alert"` sur les états, `<details>/<summary>` de 44 px pour les détails techniques.
- État jamais porté par la couleur seule : erreurs = icône + libellé « échec » + couleur ; amis = pastille + texte (« En ligne », « Joue à… », « Hors ligne ») ; avatar en niveaux de gris pour hors ligne.
- Langue : tutoiement cohérent, phrases courtes (≤ 25 mots), unités et nombres au format français (« 12,4 Go », « 1 842 fichiers », « Mo/s »), jargon technique replié dans « Détails techniques ».
- Texte du lien de licence et de « Générer un nouveau code » souligné.

## 5. Synthèse

- 24 constats : 0 Bloquant, 10 Majeurs (L-01 à L-10), 14 Mineurs (L-11 à L-24).
- Priorités de correction à faible coût : retirer l'`opacity` de la ligne hors ligne ; bordure de champs `#6A6A6A` ; `:focus-visible` global ; placeholders `#8A8A8A` ; code de vérification insécable.
- Hors périmètre de ce que je peux établir : comportement réel des dialogues (défilement, focus, piège de focus), animations (`prefers-reduced-motion`), zoom 200 %, taille minimale de fenêtre → **à confirmer dans le canvas**.
