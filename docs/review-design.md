# Review du design visuel — Cubelink (7 écrans)

Reviewer A. Base : rendus statiques `docs/review/render/*.html` et captures `docs/review/screenshots/` (1280x720, 1280xfull, 390). Les points non constatables hors runtime sont marqués « à confirmer dans le canvas ». Les noms de capture sont donnés sans le dossier.

## Constats (triés par gravité)

### Bloquants

**D-01 — Main — Titre du hero en collision avec « Mes instances »** — Bloquant
- Preuve : `Main-1280x720.png` : le « s » de « Aeronautics » (x≈560, y≈468) touche et recouvre le libellé « Mes instances » (x≈540, y≈492). Source `Main.html` l.58 : h1 72px/800 sans largeur maximale ; l.67 : section `flex:0 1 400px;min-width:260px`.
- Correction : donner au bloc hero `flex:1 1 0; min-width:0; max-width:calc(100% - 440px)` et au h1 `font-size:clamp(40px,5.6vw,72px); overflow-wrap:anywhere`. Réserver à « Mes instances » une colonne fixe (`flex:0 0 400px`) ou le descendre sous le titre. Le titre n'a aucune garantie de tenir sur 2 lignes avec un nom d'instance plus long : à confirmer dans le canvas avec « All the Mods 10 — Extended ».

**D-02 — Main — Vignettes d'instances rognées et illisibles, 3e carte quasi invisible** — Bloquant
- Preuve : `Main-1280x720.png` : cartes ≈195x95 px, texte blanc 12px posé directement sur l'image (« NeoForge · 412 mods » à peine lisible sur les fenêtres claires de la ville). « Monde de Léa » (y≈615-690) se fond dans le fond du hero (même teinte bleu nuit, pas de bordure ni d'ombre). Le texte du bas est collé à la bordure de la carte (≈10 px).
- Correction : ajouter un dégradé de lisibilité `linear-gradient(to top, rgba(10,10,10,.92) 0, rgba(10,10,10,.5) 45%, transparent 80%)` sur chaque carte, `border:1px solid rgba(255,255,255,.12)`, `min-height:112px`, `padding:12px`, `font-size:13px` pour la ligne méta. Appliquer la même bordure à la carte « invité ».

**D-03 — Main, Login, Install, Settings, Error — Aucun comportement responsive : débordements horizontaux à 390 px** — Bloquant (si le launcher est redimensionnable ; sinon Majeur)
- Preuve : `Main-390.png` : le titre « Aeronaut… » et le paragraphe sont coupés à droite, le panneau d'amis fait 300 px et n'occupe pas la largeur, le rail passe au-dessus (≈490 px de haut) au lieu d'être une barre. `Login-390.png` : « CUBELINK », le bouton et « Version… » sortent du viewport. `Install-390.png` : l'étape « Lancement » est tronquée, « Téléchargement » écrase la suivante. `Settings-390.png` : le contenu droit est hors champ, les lignes de réglages (case « Activé ») sont coupées. `Error-390.png` : les titres et boutons sont tronqués à droite.
- Correction : si une fenêtre minimale est imposée (ex. 960x600 via Tauri/Electron `minWidth`), le dire dans `ux-guidelines.md` et ne pas livrer de rendus à 390. Sinon : `min-width:0` sur tous les enfants flex, `flex-wrap:wrap` sur les rangées d'actions, rail en barre basse horizontale sous 720 px, panneau d'amis en tiroir, `overflow-x:hidden` sur `body`. À confirmer dans le canvas : les rendus statiques peuvent avoir perdu des media queries.

### Majeurs

**D-04 — Settings — Cartes « Envoyer des mods » / « Masquer mon adresse IP » et « Crédits » sortent de la modale et du rythme** — Majeur
- Preuve : `Settings-1280xfull.png` (y≈708-1045) : ces blocs sont sous le panneau de contenu, sans marge latérale (texte « Crédits et licences » collé au bord gauche, x=121), leurs fonds touchent la bordure de la modale ; le séparateur horizontal de la zone gauche s'arrête avant ; le pied de modale est coupé à 720 px dans `Settings-1280x720.png` (barre grise à y≈708 sans contenu). Le contenu clé de la confidentialité (masquer l'IP) n'est même pas visible à la taille de conception.
- Correction : placer toutes les sections dans la colonne de droite scrollable (`overflow:auto; padding:28px`), `padding-inline:28px` sur les cartes et sur « Crédits », et laisser la modale à `max-height:calc(100vh - 88px)`. Le contenu hors écran doit être une sous-section de « Réseau P2P », pas un bloc flottant.

**D-05 — Join / Friends — Modales hautes qui dépassent 720 px, actions principales invisibles à la taille de conception** — Majeur
- Preuve : `Join-1280x720.png` : le bouton « Installer le mod d'Alex et rejoindre » est hors écran (visible seulement dans `Join-1280xfull.png`, y≈735-783). `Friends-1280x720.png` : la carte « Demande en attente » est coupée (« 77D0 » à y≈710), les boutons « Accepter/Refuser » sont à peine dans le cadre. La modale démarre à y=65 et n'a pas de pied fixe.
- Correction : structure `header / body (overflow:auto) / footer sticky` avec `max-height:calc(100vh - 96px)`. Footer collant (`position:sticky;bottom:0;background:#141414`) pour le bouton principal.

**D-06 — Main — Le hero n'a pas de point focal unique : le titre 72 px, la lune et les cartes se disputent l'attention** — Majeur
- Preuve : `Main-1280x720.png` : lune détaillée au centre haut (x≈590, y≈225), titre blanc massif en bas à gauche, « Jouer » 48 px orange isolé sous un paragraphe de 3 lignes. L'œil lit d'abord le titre, puis la lune, et le CTA arrive en troisième position à y≈632. Le grand vide en haut du hero (y 100-360) ne sert à rien.
- Correction : remonter le bloc titre/CTA (`justify-content:center` ou `padding-bottom:20vh`), ajouter un dégradé sombre à gauche (`linear-gradient(90deg,rgba(10,10,10,.85) 0,transparent 60%)`) pour calmer l'image, passer « Jouer » à `height:56px; font-size:18px; padding:0 36px` et le hisser juste sous le titre, avant le paragraphe.

**D-07 — Main — Paragraphe de description et méta sans contraste suffisant sur l'image** — Majeur
- Preuve : `Main-1280x720.png` : « Machines volantes… » (#F2F2F2 estimé) passe sur des nuages bleus clairs ; la ligne « Stable · Minecraft 1.21.1 · NeoForge 21.1.77 » est en `#C0C0CC` 13px (`Main.html` l.64) sur fond bleu nuit variable. Ni ombre de texte ni voile.
- Correction : `text-shadow:0 1px 12px rgba(0,0,0,.7)` sur le bloc texte ou voile de dégradé (cf. D-06) ; passer la méta à 14px/`#D4D4DC`.

**D-08 — Main — Rail d'instances : états et lisibilité insuffisants** — Majeur
- Preuve : `Main-1280x720.png` : 3 vignettes 48 px (x=12-60), la sélection n'a qu'un anneau orange ; les deux autres vignettes sont des morceaux d'image non reconnaissables (pixels) sans nom ; pas de badge « ami en ligne » ni de tooltip visibles. Icônes de chat et paramètres (y≈583, 634) sans libellé, avec un faible contraste. Deux fois le même icône « soleil/rouage » : bouton dans le hero (x=917, y=87) et rail (y=634) pour des rôles probablement différents.
- Correction : tooltip au survol (à confirmer dans le canvas), pastille d'état (point vert) sur l'instance où des amis jouent, `opacity:.6` sur les non-actives qui passe à 1 au hover, et supprimer le doublon de bouton réglages dans le hero.

**D-09 — Install — Page quasi vide, image peu exploitée et barre de progression trop discrète** — Majeur
- Preuve : `Install-1280x720.png` : la carte blanche (x 56-696, y 395-573) est le seul élément clair de tout l'écran : c'est la seule carte blanche de l'application (cf. D-14). La barre « Téléchargement » est un trait de 4 px à 26 % (x≈382-418), alors que c'est l'information principale. L'image (silhouette de ville + reflet) occupe la moitié droite sans rapport avec « Monde d'Alex ». La lune est coupée par la carte (y≈540-580).
- Correction : barre globale unique `height:8px; border-radius:4px` avec pourcentage grand (`font-size:32px`) à droite ; garder les 4 étapes comme légende ; carte en `background:rgba(20,20,20,.85); backdrop-filter:blur(12px); border:1px solid rgba(255,255,255,.1)` comme les autres modales ; repositionner la lune hors de la carte.

**D-10 — Install — Titre « ALEX » en majuscules incohérent avec le titre du hero de Main** — Majeur
- Preuve : `Install-1280x720.png` : « ALEX » en 72 px en capitales, alors que Main utilise « Create: Aeronautics » en casse mixte ; la page de téléchargement affiche donc le nom du joueur, pas du monde (« Monde d'Alex » étant en petite pastille). Le lecteur voit d'abord « ALEX ».
- Correction : titre `Monde d'Alex` (casse mixte, `font-size:56px`) et pastille « Ami » à la place. Aligner `letter-spacing:-0.02em` sur Main.

**D-11 — Login — Fond pixel-art quasi invisible et bloc central sans ancrage** — Majeur
- Preuve : `Login-1280x720.png` : bâtiments gris #1A1A1A à 6-8 % d'opacité, motif de pastilles (x 0-200, y 330-600) qui évoque un tableau de bord plutôt qu'un monde Minecraft ; pas de hero plein écran alors que la charte le demande. Le texte de confidentialité (y≈470-520) est de la même taille que l'accroche, la hiérarchie est plate. Le logo à y≈180 flotte au-dessus d'un grand vide. Le « Problème lors de la connexion ? » (x=48) n'a pas l'apparence d'un lien.
- Correction : monter l'image à une opacité visible (`opacity:.55` + voile `rgba(10,10,10,.55)`), ou utiliser l'image hero Create comme sur Main ; mettre le texte de confidentialité en `font-size:13px; color:#9A9AA6` ; souligner « Problème… » au hover et lui donner `color:#C0C0CC`.

### Mineurs

**D-12 — Main — Panneau d'amis : espacement irrégulier et ligne tronquée** — Mineur
- Preuve : `Main-1280x720.png` : « Joue à Create: Ae… » et « Joue à All the Mo… » tronqués (x≈1157) alors que 14 px de marge restent ; bouton « Rejoindre » blanc sur bordure, deux fois côte à côte, aussi lourds que le bouton « Jouer » ; grand vide sous Léa (y 340-720). La carte « Steve_42 / En ligne » a une bordure, pas les amis.
- Correction : `title` sur la ligne tronquée (« à confirmer dans le canvas ») et réduire le bouton à `height:32px; font-size:13px`; ajouter un état vide ou « Ajouter un ami » en pied de panneau.

**D-13 — Main — Pastille « 2 amis en ligne » et bouton paramètres flottant** — Mineur
- Preuve : `Main-1280x720.png` : pastille violette (x 112-240, y 75-100) de teinte différente du reste de la charte (violet absent partout ailleurs, à part l'avatar « S »), bouton réglages carré à (917, 87) sans lien visible avec le reste de la barre.
- Correction : pastille en `background:rgba(255,92,0,.12); color:#FFB07A; border:1px solid rgba(255,92,0,.35)`, ou en vert comme le reste des états « en ligne ».

**D-14 — Cohérence — Trois « langages » de surface** — Mineur (cumul Majeur)
- Preuve : carte Install blanche (`Install-1280x720.png`), modales sombres avec rayon 16 px (Join/Friends, `Join.html` : `border-radius:16px`), Settings avec rayon 14 px, cartes d'erreur à 14 px ; rayons de boutons : 10 px quasi partout, `Install.html` mélange 2 px, 6 px, 10 px, 12 px. Les champs de la modale Amis sont aussi plus clairs que ceux de Settings (fond #0F0F0F contre #141414).
- Correction : définir 3 jetons : `--r-card:14px`, `--r-control:10px`, `--r-chip:999px`, et un seul fond de surface `#141414`, bordure `rgba(255,255,255,.08)`. Appliquer à la carte Install.

**D-15 — Cohérence — Barre de titre et fonds d'écran** — Mineur
- Preuve : barre de titre noire avec libellé centré sur Main/Install/Join/Friends/Settings/Error, mais absente sur Login (`Login-1280x720.png` : seulement les boutons réduire/fermer, sans titre, y≈15). Fonds : bleu nuit (Main), forêt rouge (Join/Friends), violet très sombre uni (Settings), noir uni (Error). Le titre d'Error « États d'attente et d'échec » est un titre de planche de design, pas de fenêtre.
- Correction : un libellé de barre de titre identique ou absent partout ; un fond de modale unique (forêt floutée, `filter:blur(6px) brightness(.5)`) pour Join/Friends/Settings ; ne pas livrer la planche Error comme écran.

**D-16 — Join / Friends — Fond forêt rouge trop pixellisé et agressif derrière les modales** — Mineur
- Preuve : `Join-1280x720.png`, `Friends-1280x720.png` : l'image est agrandie et montre de gros pixels rouges/bruns, dominante rouge qui entre en conflit avec l'orange de la charte et l'avertissement ambre. Sa lisibilité derrière la modale (x 340-940) est nulle donc l'image ne sert qu'à salir les bords.
- Correction : `filter:blur(8px) saturate(.6) brightness(.45)` + voile `rgba(10,10,10,.6)` ; éviter la dominante rouge (les alertes d'erreur sont rouges).

**D-17 — Join — Hiérarchie : trois blocs d'information concurrents avant l'action** — Mineur
- Preuve : `Join-1280x720.png` : trois tuiles (Minecraft/Loader/Java), trois lignes d'état, un avertissement ambre, un « Détails techniques », puis un choix de confiance à deux radios : 6 niveaux avant le bouton. L'avertissement est bien le point focal (ambre), mais « Détails techniques des fichiers » (y≈538) est un `summary` sans chevron ni affordance, et « Confiance envers Alex » est un titre en gras sans séparation.
- Correction : chevron + `cursor:pointer` sur le `summary`, regrouper « Confiance envers Alex » dans une carte, tuiles en une seule ligne de méta.

**D-18 — Join / Friends — Cases et radios aux styles natifs** — Mineur
- Preuve : `Join-1280x720.png` : case à cocher blanche carrée (x=400, y=459) et radio blanche non cochée (x=382, y=672) natives, alors que la radio cochée est orange ; dans `Settings-1280xfull.png`, la case « Activé » est orange mais « Relais uniquement » est un carré blanc. Incohérence visuelle des contrôles.
- Correction : `appearance:none` + `border:1.5px solid #6A6A75; background:transparent; border-radius:4px` pour décochée, `background:#FF5C00` pour cochée.

**D-19 — Friends — Champ de code et états de validation** — Mineur
- Preuve : `Friends-1280x720.png` : l'état « Copié dans le presse-papiers » (vert) et l'erreur « Ce code n'est pas reconnu » (rouge) sont affichés simultanément dans la même vue (états de maquette, pas de problème en soi), mais le champ d'erreur n'a pas de bordure rouge (`border` identique au champ normal, y≈403), et « Envoyer la demande » est gris désactivé en même temps qu'une erreur de code.
- Correction : `border-color:#FF6B6B` sur le champ en erreur, `aria-invalid`, message sous le champ (déjà), et ne montrer qu'un état par vue dans les rendus de revue.

**D-20 — Friends — Carte « Demande en attente » surchargée** — Mineur
- Preuve : `Friends-1280x720.png` (y≈590-720) : avatar 32 px minuscule, paragraphe de 5 lignes à `line-height` serré, code de sécurité (« 3F9A · C21B · 77D0 ») coupé sur deux lignes (y≈692-711), boutons « Accepter/Refuser » à droite qui compriment le texte.
- Correction : passer en colonne (texte pleine largeur, actions en pied de carte), code de sécurité sur sa propre ligne en monospace 16 px `white-space:nowrap`.

**D-21 — Error — Écran « planche » : cartes de hauteur égale avec grand vide et actions non alignées** — Mineur
- Preuve : `Error-1280x720.png` : la carte 1 a ≈180 px de vide entre texte et boutons (boutons alignés en bas), la carte 3 a ses boutons « Réessayer » et « Annuler et revenir… » empilés (y 591/652), alors que les cartes 1 et 2 ont leurs boutons côte à côte en bas (y≈651) : trois alignements d'actions différents. Le contenu de la 3e carte est coupé en bas (marge 20 px).
- Correction : `display:flex; flex-direction:column` avec `margin-top:auto` sur la rangée de boutons, `flex-wrap:nowrap` + libellé court (« Annuler ») pour la carte 3. Ajouter une icône d'état identique en taille entre les 3 cartes (carte 1 = spinner 20 px, cartes 2-3 = triangle 28 px).

**D-22 — Install — Lien « Voir l'état d'échec » dans l'écran de succès** — Mineur
- Preuve : `Install-1280x720.png` (y≈612) : lien orange souligné « Voir l'état d'échec » placé comme un lien de navigation de production, suivi de texte en gris sombre, alors qu'il s'agit d'un lien de revue/maquette.
- Correction : retirer de l'UI finale ou le placer dans un menu de diagnostic.

**D-23 — Global — États hover/actif/focus/désactivé non démontrables** — Mineur
- Preuve : aucun état :hover ou :focus-visible dans les 7 rendus statiques ; le bouton désactivé n'existe que dans Friends (« Envoyer la demande », gris). À confirmer dans le canvas.
- Correction : définir et documenter `:hover` (`filter:brightness(1.08)`), `:active` (`transform:translateY(1px)`), `:focus-visible` (`outline:2px solid #FFB07A; outline-offset:2px`), désactivé (`opacity:.45; cursor:not-allowed`).

## Ce qui fonctionne bien

1. Palette tenue : noir #0A0A0A, un seul accent orange #FF5C00 réservé aux actions principales ; texte sombre sur orange lisible (`#140700` sur `#FF5C00`).
2. Login : composition centrée, logo + un seul CTA, ombre orange douce qui guide le regard (`Login-1280x720.png`).
3. Join : l'avertissement ambre sur mod non vérifié est le vrai point focal, clair, et la hiérarchie des couleurs (vert OK / flèche orange / triangle ambre) se lit en un coup d'œil.
4. Typographie : le couple Montserrat gras pour les titres / Inter pour le texte est cohérent entre tous les écrans, titres grands et lisibles.
5. Settings : navigation latérale claire, état actif marqué (bordure orange + fond chaud), carte de confirmation « Vider le cache » bien différenciée par la teinte.
6. Les avatars pixel-art des amis (Alex, Maëlle) donnent une identité cohérente avec le sujet.

## Score global : 5,5 / 10

Le langage visuel (noir + orange + pixel-art) est cohérent et reconnaissable, et Login, Join et Settings ont de bons morceaux. Mais l'écran d'accueil, le plus important, souffre d'une collision titre/instances, de cartes illisibles et d'un hero sans point focal net ; aucun écran n'est responsive et trois modales dépassent la fenêtre de conception de 720 px, ce qui les rend « maquette » plutôt que « produit fini ». Avec les correctifs D-01 à D-06, le score atteindrait environ 7,5.

## Limites de cette review

- Basée sur un rendu statique approximatif, hors runtime : hover, focus, animations, tooltips, comportement réel du canvas à confirmer.
- Les rendus à 390 px ne sont pertinents que si la fenêtre du launcher peut descendre à cette largeur (peu probable pour une application de bureau) ; D-03 doit être reclassé si une largeur minimale est définie.
- Contrastes estimés visuellement (sauf mention de valeur lue dans la source), non mesurés au sens WCAG.
- Je n'ai pas regardé `Install-1280xfull`, `Friends-1280xfull`, `Error-1280xfull`, `Login-1280xfull`, `Join-390` ni `Friends-390` ; les constats portant sur eux sont déduits de leurs équivalents 1280x720.
