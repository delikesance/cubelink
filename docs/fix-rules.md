# Règles communes de correction (fix all)

Source des constats : `docs/review-design.md` (D-xx), `docs/review-lisibilite.md` (L-xx), `docs/review-professionnalisme.md` (P-xx), priorités dans `docs/review-synthese.md`.
Chaque coéquipier applique ces règles **uniquement à ses fichiers**.

## Périmètres (un fichier = un propriétaire)
- A : `Main.dc.html`, `Login.dc.html`
- B : `Join.dc.html`, `Install.dc.html`, `Friends.dc.html`
- C : `Settings.dc.html`, `Error.dc.html`, `Legal.dc.html` (nouveau)
- Team lead seul : `canvas.json`, `support.js`, `docs/fix-rules.md`, publication.
Dossier : `C:\Users\delikesance\AppData\Local\Temp\claude\3210c1024d30bc35\e7fcb10a-dcee-43e6-911b-be1db873fdaf\scratchpad\cv\project\`
Sauvegarde intacte : `...\scratchpad\cv-backup\` (lecture seule). Ne rien publier, aucun git.
Chaque artboard garde sa structure `.dc.html` : `<x-dc>`, `<helmet>`, script `text/x-dc` final avec `renderVals`. Ne jamais modifier la ligne `<script src="./support.js">`.
Images : références `/_blob/<id>` inchangées (13 images déjà publiées). Ne pas en ajouter.

## Charte (inchangée)
Fond `#0A0A0A`, panneaux `#0D0D0D`/`#111`, accent `#FF5C00` (texte sombre `#140700` dessus), vert statut `#4ADE80`, Montserrat (titres) + Inter.

## Règles transversales
1. **Fenêtre minimale 960×600, conception 1280×720.** Aucune action principale (bouton primaire, case de consentement) sous le pli à 1280×720 : fixer le pied d'action en bas de la modale (`position:sticky;bottom:0`) et laisser le contenu défiler dans la modale (`max-height` + `overflow:auto`).
2. **Focus** : sur chaque élément interactif, `:focus-visible{outline:2px solid #FF8A4A;outline-offset:2px}` via un bloc `<style>` dans `<helmet>`. Jamais de `outline:0` sans substitut.
3. **Contrastes** : texte normal ≥ 4,5:1, grand texte et composants ≥ 3:1. Interdit : `opacity` sur du texte. Bordures de champs et de boutons secondaires : `#6A6A6A` minimum. Placeholders : `#9A9A9A` minimum. Statuts hors ligne : `#9A9A9A`, avec un libellé texte (ne pas porter l'info par la couleur seule).
4. **Tailles** : texte de lecture ≥ 14 px (13 px toléré seulement pour de l'aide secondaire, jamais 12 px sauf majuscules espacées de 12 px gras). Cibles interactives ≥ 44 px de haut.
5. **Typographie française** : espace insécable (`&nbsp;`, ou U+202F devant `; ! ? :` selon l'usage) avant `: ? ! ;`, avant `%`, entre nombre et unité (`8&nbsp;Mo/s`, `12,4&nbsp;Go`), guillemets « et » avec insécables intérieures. Tutoiement partout. Pas de « on » impersonnel ni de « nous » sans sujet clair.
6. **Vocabulaire fixe** : *instance* (une installation locale de jeu), *monde* (celui d'un ami), *profil invité* (la copie isolée créée pour rejoindre), *ami* / *amie* selon la personne. Éviter « modpack » dans l'UI sauf nom propre. « Rejoindre » = l'action globale ; « Installer » = seulement l'étape de réception des fichiers.
7. **Chiffres fixes** (cohérence entre écrans) : instance « Create: Aeronautics » = **94 mods** ; monde d'Alex = **93 mods** dont **90 déjà dans ton cache** et **3 à recevoir** (38 Mo), dont **1 mod fait maison** (`aeronautics-dev-0.3.jar`) ; latence directe **24 ms** ; Minecraft 1.21.1, NeoForge 21.1.77, Java 21 ; version launcher **1.0.0**.
8. **Liens** : plus aucun `href="#"`. Un lien ou bouton sans destination est soit relié à un écran existant, soit retiré. Liens entre écrans : `Main.dc.html`, `Login.dc.html`, `Install.dc.html`, `Join.dc.html`, `Friends.dc.html`, `Settings.dc.html`, `Error.dc.html`, `Legal.dc.html`. Retirer les liens « de prototype » dans l'interface (ex. « Voir l'état d'échec »).
9. **Polices** : conserver Google Fonts avec repli `system-ui` ; charger Inter 400/500/600/700.

## Textes imposés (à reprendre tels quels)
- **Non-affiliation** (pied de page Connexion + Crédits + Mentions légales) :
  « Cubelink est un projet indépendant. Il n'est pas un produit Minecraft officiel, n'est pas approuvé par Mojang ou Microsoft et n'est pas associé à eux. »
  Marquer dans `docs/fix-*.md` : « à faire valider par un juriste ».
- **Intégrité des fichiers** (à la place de « fichiers vérifiés ») :
  « Chaque fichier est comparé à l'empreinte annoncée par Alex. Cela garantit qu'il n'a pas été altéré en route, pas qu'il est sans danger. »
- **Mod inconnu** : « Ce mod n'est pas référencé sur Modrinth : Cubelink ne peut pas le comparer à une version connue. Il s'exécutera sur ton ordinateur avec les mêmes droits que le jeu. » Ne plus citer CurseForge pour la vérification.
- **Code ami** : « Ce code ne contient aucun secret. Il permet à ton ami de te joindre : ne le publie pas. » (ne jamais écrire qu'il ne contient pas d'adresse IP).
- **Vocabulaire des codes** (un seul nom par objet, partout) : **« Mon code ami »** (`mcl1-…`, à partager) et **« Code de vérification »** (court, à comparer avec l'ami par un autre canal). Le code de vérification s'affiche sur **une seule ligne**, monospace, `white-space:nowrap`, **4 groupes** : `3F9A · C21B · 77D0 · 5E14` (la décision finale sur sa longueur reste à confirmer côté `architecture.md`). Libellé de demande : « Tom veut t'ajouter. Cubelink ne peut pas prouver son identité : compare ce code avec lui par un autre canal (Discord, SMS) avant d'accepter. »
- **Adresse IP / relais** : « Utiliser uniquement le relais : tes amis ne voient pas ton adresse IP, mais le relais la voit. Les transferts seront plus lents. » (ne plus écrire « Masquer mon adresse IP »).
- **Connexion directe** : « Connexion directe quand c'est possible, sinon via un relais » (ne pas écrire « sans serveur central » seul).
- **Aucun serveur ne stocke…** : « Cubelink ne stocke ni ton compte ni ta liste d'amis sur un serveur. Un relais peut transmettre ton trafic quand la connexion directe échoue. »
- **Connexion Microsoft** : bouton avec le logo Microsoft **en 4 couleurs** non modifié, sur fond blanc `#FFFFFF`, texte `#5E5E5E`, libellé « Se connecter avec Microsoft », hauteur 56 px (conforme aux règles de marque Microsoft ; à faire valider).

## Livrable par coéquipier
`docs/fix-<A|B|C>.md` : tableau ID → corrigé / non corrigé / partiel, changement effectué (une ligne) ou raison du refus. Aucune affirmation « vérifié » sans l'avoir mesuré. Contrôle avant rendu : aucune apparition de `href="#"`, d'`opacity` sur texte, de `outline:0` sans substitut ; HTML bien formé (balises fermées).

---

# Addendum « clean » (2026-11-09) : densité et images

Objectif : propre, pro, mais un launcher Minecraft qui respire. Sauvegarde de l'état précédent (v17) : `...\scratchpad\cv-v17\` (lecture seule). Toutes les règles ci-dessus restent valables ; textes imposés conservés (non-affiliation, intégrité, mod inconnu, code ami, relais).

## Images
- À GARDER : hero de l'instance sur Accueil (`ec6600c2…`), fond de Connexion (`37111a19…`), avatars d'amis (`ead9e3c6…`, `f1cac254…`, `6de79548…`, `08ee7c9d…`, `5a5eeb8e…`).
- À RETIRER : vignettes d'instances (`ef7eecfe…`, `5dbe3cc8…`, `b27b2f96…`, `387c8976…`) → aplat `#111` + bordure + initiale ou icône SVG ; fond `1e046c7e…` derrière les modales Rejoindre/Ami → aplat `#0A0A0A` ; fond `9db9c7cd…` de Installation → aplat `#0A0A0A` (avec léger dégradé de la charte autorisé).
- Aucune nouvelle image. Aucun `<img>` décoratif restant.

## Densité (règles de respiration)
1. Un écran = un message principal + une action principale. Le reste est secondaire (repli `<details>`, second niveau, ou retiré).
2. Maximum indicatif : ~12 blocs de texte visibles sans interaction par écran/modale ; aide = 1 phrase de ≤ 110 caractères, sinon repli « En savoir plus ».
3. Espacement : multiples de 8 px ; sections séparées par ≥ 24 px ; marge de contenu ≥ 24 px ; pas d'éléments collés.
4. Ne rien retirer de ce qui est juridique/sécurité imposé : le déplacer en repli visible (résumé d'une ligne) plutôt que le supprimer.
5. Ne pas casser les règles transversales (focus, contrastes, ≥ 14 px, cibles ≥ 44 px, nbsp).

## Livrable
`docs/clean-<A|B|C>.md` : mesures AVANT/APRÈS par écran (nombre d'`<img>`, de blocs de texte visibles à 1280×720, éléments sous le pli), liste des images retirées, ce qui a été déplacé en repli, ce qui a été supprimé. Captures 1280×720 et 960×600 dans `docs/review/clean-<A|B|C>/`. Périmètres inchangés (A: Main, Login ; B: Join, Install, Friends ; C: Settings, Error, Legal).
