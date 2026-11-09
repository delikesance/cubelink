# clean-A : Accueil (Main) et Connexion (Login)

Fichiers modifiés : `Main.dc.html`, `Login.dc.html`. Captures : `docs/review/clean-A/` (après) et `docs/review/clean-A/avant/` (avant), 1280x720 et 960x600, Chrome headless sur copie statique (hors runtime du canvas).

## Mesures AVANT / APRÈS

Méthode : script injecté dans la copie statique ; « blocs de texte » = éléments visibles ayant du texte direct ; « sous le pli » = liens, boutons, titres, paragraphes, items dont le haut dépasse la hauteur de fenêtre (la hauteur mesurée par Chrome headless est inférieure de quelques dizaines de pixels à la taille demandée, donc ce compte est pessimiste ; les captures font foi).

| Écran | `<img>` avant → après | Blocs de texte 1280x720 | Sous le pli 1280x720 | Sous le pli 960x600 |
|---|---|---|---|---|
| Main | 10 → 4 | 28 → 27 | 0 → 0 | 0 → 3 (mesure pessimiste ; sur la capture tout est visible : Jouer, instances, Ajouter un ami) |
| Login | 1 → 1 | 10 → 12 (le repli « En savoir plus » compte 2 blocs de plus, un seul est visible sans clic) | 3 → 0 | 3 → 1 (mesure pessimiste ; la capture montre le pied complet) |

Les 4 `<img>` restants sur Main : hero + 3 avatars d'amis. Sur Login : le fond.
Le gain sur Main est surtout visuel (4 couches d'image + voiles retirées, lignes secondaires supprimées) plus que numérique : les blocs supprimés sont compensés par la méta-ligne conservée.

## Images retirées
- Main : `ef7eecfe…` (rail), `5dbe3cc8…` (rail + carte), `b27b2f96…` (rail + carte), `387c8976…` (carte). Remplacées par aplat `#111` + bordure `#2A2A2A` + initiale (C, A, V) dans le rail ; cartes d'instances en aplat `#111` + bordure, sans voile.
- Aucun `/_blob` orphelin (grep : seuls `ec6600c2`, `ead9e3c6`, `f1cac254`, `6de79548` sur Main ; `37111a19` sur Login).

## Main : ce qui a changé
- Point focal unique : Jouer (56 px, halo) ; le profil `Steve_42` devient un bouton fantôme (sans bordure) à côté.
- Hero : marges 48 px, espacement 32 px ; phrase d'intro raccourcie à « 94 mods, installés une seule fois et partagés entre tes instances. » ; version (Minecraft 1.21.1 · NeoForge 21.1.77 · Java 21) déplacée sous les boutons.
- « Mes instances » : cartes plus basses (72 px), 2 lignes de texte, espacement 16 px ; « Forge » retiré de la carte invité.
- Rail : bouton Messages et avatar de profil retirés (doublons/sans destination) ; reste logo, 3 instances, « + », Paramètres.
- Panneau d'amis : élargi à 320 px (le « Joue à Create: Aeronautics » tient sur 2 lignes sans coupure), marges 20 px, bouton « + » de l'en-tête et bloc « Steve_42 (toi) / En ligne » retirés ; un seul bouton « Ajouter un ami » en pied.
- Conservé : info de statut (pastille « 2 amis en ligne », « Hors ligne · il y a 2 jours » en texte), `aria-label` des boutons Rejoindre, focus visible.

## Login : ce qui a changé
- Fond conservé, voile plus dense (70 % → 95 %) donc plus sobre ; carte centrale plus aérée (padding 48/40, espacement 32 px ; resserrée par media query si hauteur ≤ 640 px pour que le pied reste visible à 960x600).
- Réassurance : une seule phrase visible (« Cubelink ne stocke ni ton compte ni ta liste d'amis sur un serveur. »).
- **Déplacé en repli** « En savoir plus » (`<details>`, résumé 44 px souligné) : « Un relais peut transmettre ton trafic quand la connexion directe échoue. Cubelink crée une clé privée sur cet ordinateur pour te faire reconnaître de tes amis. » Le sens de la phrase imposée est conservé, mais elle est scindée (première moitié visible, seconde dans le repli).
- Conservés : non-affiliation (texte imposé, à faire valider par un juriste), lien Mentions légales, « Problème lors de la connexion ? », version, bouton Microsoft imposé.

## Supprimé
Messages (rail), avatar de profil (rail), bouton « + » de l'en-tête d'amis, ligne « Steve_42 (toi) », phrase « Machines volantes, mécanismes et automatisation. ».

## Non fait / limites
- États vide et injoignable de la liste d'amis : non dessinés (les afficher en statique ajouterait du bruit ; pas de logique `renderVals`). Textes proposés : « Aucun ami pour l'instant. Ajoute ton premier ami avec son code ami. » / « Impossible de joindre le réseau d'amis. Réessaie. »
- Étiquette « Maquette » : non ajoutée (noms fictifs 412 mods, Vanilla+ toujours présents).
- Le dépliage « En savoir plus » n'est vérifié qu'en structure (pas de capture dépliée) ; à tester dans le canvas.
- Menu de profil (`Steve_42 ▾`) toujours sans destination ; cibles des contrôles de fenêtre 46x32 px (convention OS).
- Contrastes non re-mesurés au script ; rendu statique : polices, survol et `min(720px,100vh)` à confirmer dans le canvas.
- Contrôles grep : zéro `href="#"`, zéro `opacity`, zéro `outline:0`, ligne `<script src="./support.js">` intacte.
