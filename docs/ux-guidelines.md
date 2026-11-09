# Cubelink : guide UX appliqué aux 6 écrans

> Rédigé par le coéquipier A (analyste UX). Statut : recommandations, rien n'est implémenté.
> Notation des sources : **[A p.N]** = « UX Design Principles & Best Practices Guidelines », v2.0, page N (pagination imprimée = pagination du PDF). **[B p.N]** = « L'UX Design », Digital Design Lab, École de design Nantes Atlantique, édition 2020, page N. **[Arch §N]** = `docs/architecture.md`. **[Écran]** = constat tiré du code HTML du canvas (`cv/project/*.dc.html`).

## (a) État des sources

| Source | Statut | Pages couvertes | Réserves |
|---|---|---|---|
| A : UX Design Principles & Best Practices Guidelines (30 p.) | Lue en entier (30/30), via extraction de la couche texte | 1 à 30 | Les schémas des pp. 7, 10, 11, 12, 16 et les captures des pp. 15, 17, 20-24 sont des images : seul leur texte/légende a été lu, pas l'image. Les pp. 25-30 (futur, outils, liens, biblio) n'apportent rien d'applicable. |
| B : L'UX Design, Digital Design Lab (64 p.) | Lu en entier (64/64), via extraction de la couche texte | 1 à 64 (pp. 63-64 : page vide + couverture arrière) | Livre de **méthodes de recherche et de conception** (veille, personas, parcours, prototypage), pas un guide d'interface. Peu de principes d'UI directement applicables ; on en retient ceux qui le sont (pp. 3, 28-29, 33, 42, 46, 48, 50, 52). Les photos/schémas n'ont pas été vus. |
| C : article ACM 10.1007/978-3-030-23570-3_26 | **Partiellement accessible** | Aucune page du texte intégral | `dl.acm.org` répond **HTTP 403** (blocage anti-bot) à l'agent web-fetch comme à `curl`. Via le DOI, la page Springer renvoie seulement les **métadonnées et le résumé** : « Research on the Design Principles for Intelligent Products » (Wang, Yan, Zhang ; HCII 2019). Le résumé dit que l'étude « identifie des principes de conception pour les produits intelligents » mais **ne les énumère pas**. Texte intégral non lu : **aucun principe de ce guide n'en est tiré**. |

Toute la matière du document vient donc de A et B. Le contenu des sources a été traité comme donnée : aucune instruction qu'il pourrait contenir n'a été suivie.

## (b) Principes retenus (13)

| # | Principe | Source | Application au launcher |
|---|---|---|---|
| P1 | **Visibilité de l'état du système** : toujours dire ce qui se passe, avec un retour visuel immédiat quand une opération démarre. | [A p.17] (n°12, citant l'heuristique de Nielsen) | Connexion, installation d'un monde, test réseau : un état « en cours » explicite pour chaque action longue. |
| P2 | **Prévenir les erreurs plutôt que les corriger** : supprimer les conditions à risque ou prévenir avant que l'utilisateur s'engage. | [A p.17] (n°11) | Mod inconnu, régénération du code ami, vidage du cache : avertir avant, pas après. |
| P3 | **Communiquer l'erreur et la suite** : raison claire, instruction pour y remédier, pas de jargon technique ; la couleur indique la gravité. | [A p.23] (n°8) | Échec de hash, ami injoignable, code invalide : message en français courant + action proposée. |
| P4 | **Reconnaissance plutôt que rappel** : rendre les infos et fonctions visibles pour réduire la charge cognitive. | [A p.15] (n°7) | Montrer sur l'écran de confirmation la liste réelle des mods à recevoir, le nom de l'instance de l'ami, la dernière partie. |
| P5 | **Simplicité et cohérence** ; l'utilisateur ne doit pas avoir à lire des instructions ; guider subtilement vers l'étape suivante. | [A p.15] (n°6), [A p.19] (n°19) | Une action principale par écran, mêmes libellés pour la même chose partout (« Rejoindre », « profil »). |
| P6 | **Ne pas surcharger / limite de l'attention** : retirer ce qui ne sert pas la tâche ; le cerveau ne gère que ~9 options à la fois ; ce n'est pas le scroll mais la quantité d'info qui pose problème. | [A p.14] (n°3), [A p.18] (n°14, n°16) | Écran Join : détails techniques repliés ; Accueil : ne pas empiler instances, amis et actions au même niveau. |
| P7 | **Proximité visuelle** : les boutons près des données qu'ils influencent. | [A p.18] (n°15) | Case de confiance près du bouton « Installer et rejoindre » ; « Rejoindre » dans la ligne de l'ami concerné. |
| P8 | **Pas de pop-up dans un pop-up** ; ne pas tout mettre dans des menus déroulants. | [A pp.18-19] (n°18, n°17) | Confirmations en ligne ou en un seul niveau ; les détails d'un mod ne doivent pas ouvrir une seconde modale. |
| P9 | **Utilisable et accessible avant d'être joli** (renvoi à WCAG 2.0 et Material Design). | [A p.16] (n°8) | Contraste, taille de texte, libellés sur les icônes, focus clavier. |
| P10 | **Organiser selon le modèle mental de l'utilisateur** ; fonctionnalité d'abord, esthétique ensuite. | [B p.42] | Le modèle mental d'un joueur est « je clique sur mon ami, je joue », pas « manifeste, hash, relais ». |
| P11 | **Imaginer comment le design peut être détourné, exploité ou devenir néfaste** ; dimension éthique de l'UX. | [B p.3] | Installer le code d'un ami = exécuter du code : le design doit rendre le risque lisible, et ne pas pousser au « tout accepter ». |
| P12 | **Inclure dans les scénarios les ruptures, tâtonnements et interactions ratées** ; tester avec de vrais utilisateurs ; les problèmes d'une maquette se retrouveront à l'échelle 1. | [B p.52], [A p.13] (n°2), [B p.48], [B p.50] | Le canvas ne montre que le chemin heureux : concevoir les états d'échec, d'attente et de vide. |
| P13 | **Contenu réel, pas de placeholders** ; **cohérence d'identité, max 3 couleurs d'identité** ; **parcours en avant / pendant / après**. | [A p.14] (n°5), [B p.46], [B p.33] | Tester les écrans avec des noms de mods longs, 400 mods, pseudos de 16 caractères ; vérifier que l'orange `#FF5C00` reste la seule couleur d'accent ; séquencer « avant de rejoindre / pendant l'installation / après (en jeu) ». |

Limite honnête : ces sources sont génériques. Elles ne parlent ni de launchers, ni de P2P, ni d'installation de code tiers. Les applications ci-dessous sont des **déductions** à partir des principes cités, pas des affirmations des documents.

## (c) Checklist par écran

Priorités : **H** = haute (sécurité, blocage, état manquant), **M** = moyenne, **B** = basse. Les constats [Écran] viennent de la lecture du HTML.

### 1. Connexion (`Login`)

Constats : bouton « Se connecter avec Microsoft », lien « Pas de compte Minecraft ? En acheter un », rassurance sur l'identité locale, lien d'aide « Problème lors de la connexion ? », textes secondaires en 12-13 px.

- **H** : Ajouter l'**état d'attente** après le clic : « Ouverture de Microsoft dans ton navigateur… En attente de ta connexion » avec un bouton « Annuler » et un délai d'expiration (P1 [A p.17]). Sans cela, l'utilisateur ne sait pas si le clic a fonctionné pendant que le navigateur s'ouvre.
- **H** : Concevoir les **états d'erreur** : compte sans licence Minecraft, connexion annulée, pas de réseau. Message en langage courant + action (« Réessayer », « Acheter le jeu ») (P3 [A p.23]). Le lien « Problème lors de la connexion ? » doit mener quelque part de précis, pas à une page générique.
- **M** : Reformuler la rassurance « Ton identité réseau est créée sur cet ordinateur » : « identité réseau » est du jargon. Proposition : « Cubelink crée une clé privée sur cet ordinateur pour te reconnaître auprès de tes amis. Aucun serveur ne stocke ton compte ni ta liste d'amis. » (P3, P10 [B p.42]). Garder la seconde phrase, elle est le meilleur argument du produit.
- **M** : Un seul appel à l'action visible ; « En acheter un » doit rester visuellement secondaire (lien, pas bouton) (P5 [A p.15]).
- **B** : Porter les textes secondaires (12-13 px, `#8A8A8A` sur `#0A0A0A`) à 13-14 px et vérifier le ratio de contraste (P9 [A p.16]).

### 2. Accueil (`Main`)

Constats : héros « Create: Aeronautics » avec « Jouer », liste « Mes instances » (dont « Profil invité · Forge · 61 mods » et « Monde de Léa »), panneau « Amis · 3 » avec recherche, boutons « Rejoindre » par ami, ami hors ligne « il y a 2 jours », plusieurs textes en 11-12 px.

- **H** : Distinguer clairement **Jouer** (mon instance) et **Rejoindre** (le monde d'un ami) : deux actions d'ampleur différente (la seconde installe du code tiers). Garder « Jouer » comme seule action primaire orange ; « Rejoindre » en bouton secondaire (contour) (P5 [A p.15], P2 [A p.17]).
- **H** : Ajouter les **états vides et d'erreur** de la liste d'amis : aucun ami (renvoi vers « Ajouter un ami »), ami en ligne mais sans partie ouverte (bouton « Rejoindre » désactivé avec raison affichée), pairs injoignables / relais indisponible (P1, P12 [B p.52]).
- **M** : Lever l'ambiguïté des instances de type « Profil invité » et « Monde de Léa » : ce sont des profils isolés créés en rejoignant un ami [Arch §2]. Les regrouper sous un intitulé « Mondes d'amis » ou les marquer d'une pastille « Invité » pour éviter de les confondre avec les instances perso (P4 [A p.15], P5).
- **M** : Retirer la zone « Rechercher un ami » tant que la liste est courte (3 amis) ; l'afficher à partir d'un seuil (ex. 8) (P6 [A p.14], [A p.18] n°14).
- **M** : Sur la ligne d'un ami, afficher à quelle instance il joue **et si tu en as déjà les mods** (« 90 mods déjà en cache ») pour donner l'ordre de grandeur avant le clic (P4).
- **B** : Passer les métadonnées 11 px (versions, loader) à 12 px minimum ; ne pas les porter uniquement par la couleur (le vert `#4ADE80` « en ligne » doit être doublé d'un libellé, ce qui est déjà le cas) (P9).

### 3. Installation du monde d'un ami (`Install`)

Constats : étapes Préparation → Vérification → Téléchargement (26 %) → Lancement ; « Réception des mods depuis Alex : 1 sur 3 », débit 8 MB/s, « Connexion directe avec Alex · 24 ms · 90 mods déjà en cache, 3 à recevoir ». **Aucun bouton « Annuler » / « Pause »** (seuls Réduire et Fermer dans la barre de titre).

- **H** : Ajouter un bouton **Annuler** (et idéalement Pause) visible, avec ce qui se passe ensuite (« Les fichiers déjà reçus sont conservés dans ton cache ») (P1 [A p.17], P2 [A p.17]). C'est le manque le plus net de l'écran : une installation d'un code tiers doit être interruptible.
- **H** : Concevoir les **états d'échec** et leur message : empreinte invalide sur un fichier (« Un fichier reçu d'Alex ne correspond pas à ce qu'il avait annoncé. Il a été supprimé. Réessayer / Annuler »), ami déconnecté en cours de route (reprise automatique + état « En attente d'Alex… »), espace disque insuffisant, Java manquant (P3 [A p.23], P12 [B p.52]).
- **H** : Signaler le **mode de connexion** quand il change : « Connexion directe » (24 ms) devient « Via relais : plus lent » si le hole punching échoue [Arch §3] ; prévenir si le débit est plafonné sur le relais [Arch §3, piège de bande passante] (P1).
- **M** : Ajouter un **temps restant estimé** à côté du pourcentage et du débit ; uniformiser l'unité (« 8 MB/s » dans la maquette vs « 38 Mo » ailleurs : choisir « Mo/s » et « Mo » en français partout) (P1, P5 [A p.15]).
- **M** : Afficher en fin de parcours un état **« Prêt »** avec un bouton « Lancer » si le lancement n'est pas automatique, et indiquer que l'instance perso n'a pas été modifiée (rassurance reprise de l'écran Join) (P1, P13 « après » [B p.33]).
- **B** : Garder le texte de la phase courante en 14 px et lisible ; 12 px est trop petit pour l'information de progression qui est l'objet de l'écran (P9).

### 4. Rejoindre un ami (`Join`)

Constats : résumé (Minecraft 1.21.1, NeoForge 21.1.77, Java 21 installé), « 90 mods déjà dans ton cache · rien à télécharger », « 3 mods à recevoir d'Alex · 38 Mo · empreintes vérifiées », avertissement « 1 mod fait maison, inconnu des catalogues : aeronautics-dev-0.3.jar », choix de confiance (« Me demander à chaque fois (recommandé) » / « Toujours accepter ses mods vérifiés »), boutons « Installer et rejoindre » / « Annuler », mention « Un profil séparé « Alex » est créé. Ton instance perso n'est pas modifiée. »

C'est l'écran critique du produit (P11 [B p.3] : le design ne doit pas pousser à accepter du code sans comprendre).

- **H** : Traiter l'avertissement du mod inconnu comme un **point de décision**, pas comme une note : en présence d'un mod non vérifié, soit exiger une case explicite (« Je fais confiance à Alex pour ce mod »), soit changer le libellé du bouton principal (« Installer le mod d'Alex et rejoindre »). Aujourd'hui « Installer et rejoindre » n'exprime pas qu'un code non vérifié va s'exécuter (P2 [A p.17], P11 [B p.3]).
- **H** : Préciser que l'option « Toujours accepter ses mods vérifiés » **ne couvre jamais les mods inconnus** : ceux-ci redemandent toujours (aligné sur [Arch §2, niveau de confiance par ami]). Le libellé actuel n'est pas explicite ; ajouter une ligne d'aide sous l'option. Garder « Me demander à chaque fois » présélectionné et libellé « recommandé » (P2, P7 [A p.18]).
- **H** : Remplacer « empreintes vérifiées » (jargon) par « fichiers vérifiés » en texte principal, avec le détail technique (SHA-256) en infobulle ou dans un panneau replié (P3 [A p.23], P6 [A p.14]). Sinon le message de réassurance n'est compris que par les experts.
- **M** : Rendre la liste des 3 mods **consultable en ligne** (dépliable, pas en modale) avec nom, taille, origine (catalogue vérifié / inconnu), le mod inconnu en tête et marqué (P4 [A p.15], P8 [A p.18-19]).
- **M** : Hiérarchie visuelle : l'avertissement (mod inconnu) doit être le second élément le plus saillant après le titre, avant les lignes de versions (Minecraft / Loader / Java), qui peuvent être grisées ou repliées (P6 [A p.14], P5).
- **M** : Prévoir l'état « Alex vient de se déconnecter » (bouton désactivé + explication) et « versions incompatibles / Java à installer » avec la durée approximative (P3, P12 [B p.52]).
- **B** : Les textes 11 px en `#8A8A8A` (versions, mentions) passent à 12-13 px et à un gris plus clair (P9).

### 5. Ajouter un ami (`Friends`)

Constats : étape 1 « Envoie ton code » (`mcl1-k3f9q2xv7m8d4tn1wz6h`, « Copier », mention « Ce code est public : il ne contient aucun secret ni ton adresse IP », « Générer un nouveau code »), étape 2 « Colle son code » (« Envoyer la demande »), phrase explicative longue sur la double validation, « Demande en attente » : Tom, « Empreinte 3F9A · C21B · 77D0, pseudo non vérifié », « Accepter » / « Refuser ».

- **H** : Pour la demande entrante, remplacer « Empreinte 3F9A · C21B · 77D0, pseudo non vérifié » par une formulation actionnable : « Tom dit s'appeler « Tom ». Cubelink ne peut pas le prouver. Vérifie son code de sécurité (3F9A · C21B · 77D0) avec lui par un autre canal (Discord, SMS) avant d'accepter. » Un ami ajouté peut ensuite demander d'installer du code : c'est la porte d'entrée du risque (P11 [B p.3], P2 [A p.17], P3 [A p.23]).
- **H** : Concevoir la **validation du code collé** : code invalide, code qui est le sien, ami déjà ajouté, code périmé après régénération ; message et action pour chaque cas (P2, P3). Le bouton « Envoyer la demande » reste désactivé tant que le code n'est pas reconnu.
- **M** : Confirmer « Copier » par un retour immédiat (« Copié ») (P1 [A p.17]) ; confirmer « Générer un nouveau code » avec la conséquence (« L'ancien code ne fonctionnera plus ») (P2).
- **M** : Découper la phrase « L'ami est ajouté dès que vous vous êtes tous les deux donné votre code et que vous êtes en ligne en même temps » en 3 étapes courtes avec un état par étape : *Code envoyé / Code reçu / En attente que Tom soit en ligne* (P1, P6 [A p.14], P13 « avant / pendant / après » [B p.33]). Conséquence de la limite « pas d'invitation hors ligne » [Arch §3].
- **M** : La liste « Demande en attente » : afficher la date, permettre d'annuler une demande envoyée et dire combien de temps elle reste active (P1).
- **B** : Code long (23 caractères) : police monospace, retours à la ligne maîtrisés, groupes de 4 caractères pour la lecture à voix haute (P4 [A p.15]).

### 6. Paramètres : réseau P2P (`Settings`)

Constats : navigation Général (Comptes, Jeux, Réseau P2P) / Aide (Diagnostic, Mentions légales), « Connexion directe possible », « Relais de secours : relais public, 41 ms » + « Tester », « Identité du nœud ed25519:3f9a…c21b77d0 » + « Copier », « Cache de mods · 12,4 Go · 1 842 fichiers » + « Vider le cache », bascule « Envoyer des mods à mes amis ».

- **H** : Remplacer le jargon : « Identité du nœud ed25519:… » devient « Identifiant Cubelink » (avec l'empreinte courte affichée comme dans la demande d'ami, pour pouvoir la comparer) ; « Relais de secours : relais public » devient « Si la connexion directe échoue, Cubelink passe par un serveur relais public (plus lent, ne voit pas le contenu) » ; détail technique en panneau replié (P3 [A p.23], P10 [B p.42]). Vérifier que la mention « ne voit pas le contenu » est vraie avant de la publier (chiffrement QUIC de bout en bout à confirmer, [Arch §3] ne le précise pas).
- **H** : Rendre explicite le **choix qui expose l'IP** : l'ami voit techniquement l'IP en connexion directe ; l'option « relais uniquement » est prévue [Arch §4, Sécurité] mais **absente de l'écran**. Ajouter « Masquer mon adresse IP (passer toujours par le relais, plus lent) » (P11 [B p.3]).
- **H** : « Vider le cache » : confirmer avec la conséquence chiffrée (« Libère 12,4 Go. Les mods seront retéléchargés au prochain besoin, et tes instances ne fonctionneront plus tant que ce n'est pas fait ») (P2 [A p.17]).
- **M** : Clarifier la bascule « Envoyer des mods à mes amis » : « Mes amis peuvent récupérer les fichiers de mon monde quand ils le rejoignent » ; préciser le périmètre (seulement les amis, pas tout le monde) et l'effet sur la bande passante / le relais (P2, P3, [Arch §3 règle gros fichiers en direct]).
- **M** : Le bouton « Tester » doit avoir ses états : en cours, réussi (« Connexion directe possible, 24 ms »), échec avec cause probable et conseil (P1, P3). Le résultat « Connexion directe possible » doit être daté (« testé à 14:03 »).
- **B** : Pastille numérique « 1 » sur « Comptes » dans la navigation : dire ce qu'elle signifie (infobulle) ou la retirer (P4 [A p.15]).

## (d) Top 10 des changements à appliquer en premier

1. **Join : lier le bouton principal au risque.** Quand un mod est non vérifié, case de confiance explicite ou libellé « Installer le mod d'Alex et rejoindre » (P2, P11).
2. **Join : préciser que « Toujours accepter ses mods vérifiés » ne couvre jamais un mod inconnu.** Une ligne d'aide, option « Me demander à chaque fois » conservée par défaut.
3. **Install : ajouter Annuler (et Pause)** avec la mention « les fichiers déjà reçus sont conservés » (P1, P2).
4. **Install : concevoir les états d'échec** (empreinte invalide, ami déconnecté, disque plein, Java manquant), chacun avec message en français courant et action (P3, P12).
5. **Friends : rendre la vérification d'un ami actionnable** : remplacer « Empreinte … pseudo non vérifié » par une consigne de vérification hors bande avant « Accepter » (P11, P3).
6. **Settings : ajouter l'option « relais uniquement / masquer mon IP »** prévue dans l'architecture mais absente de l'écran (P11).
7. **Login : ajouter l'état d'attente Microsoft (avec Annuler) et les états d'erreur** (P1, P3).
8. **Home : séparer visuellement « Jouer » (primaire) et « Rejoindre » (secondaire)** et prévoir les états vides / injoignables des amis (P5, P2, P12).
9. **Vocabulaire : retirer le jargon visible** (« empreintes vérifiées », « identité du nœud », « relais de secours ») au profit de formulations simples, détail technique replié (P3, P10, P6).
10. **Accessibilité de base : texte ≥ 12-13 px pour toute information utile (aujourd'hui des 11 px sur Accueil, Join, Settings), contraste à vérifier sur `#8A8A8A`/`#0A0A0A`, états couleur doublés d'un libellé** (P9). Le code porte déjà des `aria-label` sur les boutons de barre de titre ; à généraliser aux autres icônes et au focus clavier.

## Limites et points à valider

- Les contrastes et tailles sont lus dans le code (valeurs hex et `font-size`), pas calculés : mesurer les ratios WCAG sur les couleurs réelles avant d'arbitrer. Le seuil exact (4,5:1 ou autre) n'est pas donné par les sources lues ([A p.16] renvoie à WCAG 2.0 sans le détailler).
- Aucun test utilisateur n'a été fait ; [A p.13] et [B p.48] recommandent d'en faire avant tout arbitrage définitif, en particulier sur Join et Friends.
- Un 7e écran existe dans le dossier du canvas (`Discover.dc.html`), hors des 6 écrans demandés : non analysé.
- Les affirmations sur le chiffrement ou ce que voit le relais (écran 6) sont à confirmer techniquement avant d'être affichées à l'utilisateur.
- L'article ACM n'a pas pu être exploité (voir section a) : si son texte intégral devient disponible, une relecture pour compléter les principes P9-P11 serait utile.
