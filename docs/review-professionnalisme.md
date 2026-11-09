# Review PROFESSIONNALISME : launcher « Cubelink » (nom de travail)

Reviewer : coéquipier C. Date : 2026-11-09. Matériel : rendus `docs/review/render/*.html` (texte extrait), captures Login/Join/Settings regardées, `architecture.md` (« Arch »), `assets-sources.md` (« Assets »), `ux-guidelines.md`, `plan.md`.
Les citations de texte d'écran sont littérales (apostrophes droites, espaces ordinaires, comme dans les sources). Aucun point ci-dessous n'est un avis juridique : les points juridiques sont « à faire valider par un juriste ».

Légende gravité : Bloquant = à corriger avant toute diffusion hors équipe ; Majeur = à corriger avant des testeurs externes ; Mineur = finition.

---

## Constats

### Bloquants

**P-01 · Settings (Crédits) / tous écrans · Juridique · Bloquant : aucune mention de non-affiliation à Mojang/Microsoft.**
Texte le plus proche : "Aucune ressource Minecraft ou Mojang n'est utilisée." Cela parle des images, pas du produit ; ce n'est pas une mention de non-affiliation. Le produit s'appelle un launcher « Minecraft », affiche « Minecraft 1.21.1 » partout, le bouton "Se connecter avec Microsoft" et "Pas de compte Minecraft ? En acheter un".
Preuve : Minecraft Usage Guidelines (https://www.minecraft.net/en-us/usage-guidelines, consultée le 2026-11-09), section « Essential guidelines » : ne pas faire croire que le produit est « official or approved by, endorsed by, associated with… » ; indiquer qui est responsable et comment le contacter ; « Prominently include the disclaimer similar to the following: "NOT AN OFFICIAL MINECRAFT [PRODUCT/SERVICE/EVENT/etc.]. NOT APPROVED BY OR ASSOCIATED WITH MOJANG OR MICROSOFT" ». Les mêmes lignes disent aussi que ces lignes directrices visent les fans et que l'usage commercial est encadré : point à faire valider par un juriste, surtout si le produit devient commercial.
Correction : ajouter (a) sur l'écran Connexion, sous la version, et (b) dans Paramètres > Mentions légales : "Cubelink n'est pas un produit Minecraft officiel. Non approuvé par Mojang ni Microsoft, et sans lien avec eux. Minecraft est une marque de Mojang AB. Éditeur : [nom de l'éditeur] · Contact : [adresse e-mail]." (rédaction en miroir de la formule anglaise ; fournir aussi la version anglaise exacte si diffusion hors France).

**P-02 · Settings > navigation · Juridique / Finition · Bloquant : « Mentions légales » est un lien mort, aucune politique de confidentialité.**
Élément : "Mentions légales" → `href="#"`. Aucun écran ne présente éditeur, hébergeur, contact, ni politique de confidentialité. Pourtant le produit traite des données personnelles : pseudo Minecraft, clé publique persistante, adresses IP exposées aux amis et au relais, compte Microsoft (Arch §3 et §4). Une clé publique stable est un identifiant persistant ; l'IP est une donnée personnelle (RGPD, à faire valider par un juriste).
Preuve : Settings.html : 5 liens `href="#"` dont "Mentions légales" ; Arch §4 : « L'ami voit techniquement l'IP » ; Arch §3 relais public tiers.
Correction : créer l'écran Mentions légales (éditeur, contact, licence du logiciel, non-affiliation, lien vers la politique de confidentialité) et une page Confidentialité listant : ce qui reste local (clés, amis, cache), ce que voient les amis (pseudo, IP en direct), ce que voit le relais, ce qui va à Microsoft/Mojang.

### Majeurs

**P-03 · Join · Honnêteté technique / Sécurité · Majeur : « fichiers vérifiés » induit un faux sentiment de sécurité.**
Texte : "3 mods à recevoir d'Alex" + "38 Mo · fichiers vérifiés", et option "Toujours accepter ses mods vérifiés". Arch §2 : les hash SHA-256 sont ceux que l'hôte lui-même annonce dans son manifeste ; la vérification prouve l'intégrité du transfert, pas l'innocuité. Le seul sens de « vérifié » côté sécurité est « hash connu de Modrinth » (Arch §2 « Risques à gérer »). Le même mot recouvre donc deux notions. Le détail dit bien la vérité ("Chaque fichier est comparé à une empreinte SHA-256 annoncée par Alex."), mais il est replié derrière "Détails techniques des fichiers".
Correction : séparer les deux notions. Ligne : "38 Mo · intégrité contrôlée". Pour les mods connus : "Mods présents dans le catalogue Modrinth" (si c'est bien le test). Option : "Toujours accepter ses mods connus de Modrinth". Dans le détail : "Contrôlé : le fichier reçu est identique à celui qu'Alex a annoncé. Cela ne garantit pas qu'il soit sans danger."

**P-04 · Join · Honnêteté technique · Majeur : vérification « auprès de Modrinth ou CurseForge » non fondée.**
Texte : "aeronautics-dev-0.3.jar n'a pas pu être vérifié auprès de Modrinth ou CurseForge." Arch §2 : lookup par hash sur Modrinth seulement ; « Ne pas passer par l'API CurseForge pour ce transfert » ; Arch §1 : CurseForge arrive en phase 2 du plan. La promesse n'est pas tenable au MVP.
Correction : "aeronautics-dev-0.3.jar est inconnu de Modrinth : personne d'autre ne l'a publié, nous ne pouvons pas le contrôler. Installe-le seulement si tu fais confiance à Alex." (réintroduire CurseForge seulement si le contrôle existe). Voir aussi P-05 sur la portée de « catalogues ».

**P-05 · Join / Install · Honnêteté technique · Majeur : un mod « connu d'un catalogue » est présenté comme sûr par omission.**
Texte : "1 mod fait maison, inconnu des catalogues". Le message sous-entend que les 93 autres sont dignes de confiance. Un catalogue public peut héberger un fichier malveillant (incident Fractureiser 2023 : fait connu de l'auteur de la review, non vérifié dans cette session ; à sourcer avant publication). Rien n'indique non plus que Cubelink contrôle l'hôte lui-même.
Correction : ajouter dans le résumé : "Les mods connus de Modrinth sont publics, mais Cubelink ne les analyse pas." et ne pas utiliser de coche verte pour "90 mods déjà dans ton cache" comme gage de sécurité (le cache contient aussi des mods déjà acceptés).

**P-06 · Friends · Honnêteté technique · Majeur : le code ami « ne contient ni ton adresse IP » est contredit par l'architecture, et sa longueur est incompatible.**
Textes : "Ce code est public : il ne contient aucun secret ni ton adresse IP." et le code `mcl1-k3f9-q2xv-7m8d-4tn1-wz6h`. Arch §4 : le code contient « sa clé publique + indications de connexion (relais d'attache, adresses) » et « pas d'IP en clair ». Ces « adresses » ne sont pas définies : si ce sont des adresses IP, la phrase est fausse. Par ailleurs 20 caractères base32 ≈ 100 bits ne peuvent pas contenir une clé ed25519 de 256 bits plus des indications ; le code montré est soit un identifiant résolu par DHT (donc dépendant d'un service tiers), soit une maquette irréaliste. À confirmer par le prototype (Arch §7 « Format précis du code ami »).
Correction du texte, valable dans tous les cas : "Ce code est public : il ne contient aucun secret. Toute personne qui l'a peut t'envoyer une demande d'ami, que tu peux refuser. Pour invalider ce code : Générer un nouveau code." Ne promettre « pas d'IP » que si le format final le garantit, et alors préciser "Ton adresse IP n'est visible que des amis que tu acceptes."

**P-07 · Friends / Settings · Honnêteté technique / Sécurité · Majeur : « code de sécurité » de 48 bits, présenté comme moyen de vérification, et valeur identique à l'identifiant de l'utilisateur.**
Textes : "compare ce code de sécurité avec lui par un autre canal (Discord, SMS) : 3F9A · C21B · 77D0" (Friends) et "Identifiant Cubelink" = "3F9A · C21B · 77D0" (Settings). 12 chiffres hexadécimaux = 48 bits : trop court pour résister à un attaquant qui génère des clés jusqu'à obtenir une collision préfixe (travail de l'ordre de 2^48, à confirmer par un cryptographe). L'identifiant de Tom et celui de l'utilisateur sont identiques dans la maquette, ce qui est un artefact de maquette mais ce serait pris pour un bug (ou pour un code non unique). Arch §4 ne spécifie aucun code de sécurité : c'est un ajout du design.
Correction : concevoir un « numéro de sécurité » plus long (au moins 60 à 80 bits, p. ex. 4 groupes de 5 chiffres ou une phrase de mots) ; valeurs différentes dans la maquette. Texte : "Tom dit s'appeler « Tom ». Cubelink ne peut pas le prouver. Avant d'accepter, compare ce numéro avec lui par un autre moyen (appel, Discord, SMS)."
Remarque : « un autre canal » est du jargon léger ; "un autre moyen" est plus clair.

**P-08 · Login · Honnêteté technique · Majeur : "Aucun serveur ne stocke ton compte ni ta liste d'amis." : vrai mais trompeur par omission.**
Le compte vit chez Microsoft/Mojang (serveurs), les jetons de session sont stockés localement (stockage non précisé : trousseau système ou fichier ? Arch ne le dit pas, à confirmer), le relais public tiers voit des métadonnées de connexion (Arch §3). "Aucun serveur" peut être lu par un juriste ou un journaliste comme faux.
Correction : "Cubelink crée une clé privée sur cet ordinateur pour te faire reconnaître de tes amis. Ta liste d'amis et tes clés restent sur cet ordinateur ; Cubelink n'a pas de serveur de comptes." Et pour le jeton Microsoft : "Ta connexion Microsoft se fait directement avec Microsoft ; Cubelink n'en voit pas le mot de passe." (vrai uniquement si OAuth dans le navigateur, ce que l'écran d'attente suggère).

**P-09 · Settings · Honnêteté technique · Majeur : "Masquer mon adresse IP" surpromet.**
Texte : "Masquer mon adresse IP" / "En connexion directe, tes amis peuvent voir ton adresse IP. Activé, Cubelink passe toujours par le relais : c'est plus lent." En mode relais, l'IP est visible du relais (public, tiers), et probablement des services de découverte DNS/DHT d'Iroh (Arch §3). Elle est masquée aux amis uniquement. Par ailleurs Arch §3 « Piège » : gros transferts « en direct uniquement, ou débit plafonné sur le relais » : combiné à "Relais uniquement", l'installation d'un monde pourrait être impossible ou très lente, ce que l'écran ne dit pas. Le nom du réglage "Relais uniquement" est correct, le titre ne l'est pas.
Correction : titre "Cacher mon adresse IP à mes amis" ; texte "Tes amis ne verront pas ton adresse IP : tes échanges passent par un serveur relais, qui, lui, peut la voir. Plus lent ; l'envoi et la réception de mods peuvent être limités." Ne pas écrire « anonyme » ni « privé ».

**P-10 · Install · Honnêteté technique · Majeur : promesse de repli par relais pour l'installation.**
Texte : "Voir l'état d'échec · si la connexion directe échoue, l'installation passera par un relais, plus lent." Arch §3 : règle de transfert de gros fichiers en direct uniquement, ou débit plafonné sur le relais ; politique de limitation « à définir » (Arch §7). La promesse de bascule transparente n'est pas décidée.
Correction : formuler au conditionnel produit, ou décider la règle : "Si la connexion directe échoue, Cubelink essaie un relais ; le débit peut être limité."

**P-11 · Settings · Honnêteté technique · Majeur : "Connexion directe avec tes amis, sans serveur central."**
Le même écran décrit "un serveur relais public" ; Arch §3 : un relais est « nécessaire » (10-20 % des cas), et c'est un binaire iroh-relay opéré par l'éditeur ou par un tiers (relais publics « best effort, pas pour la production »). Ce n'est pas faux au sens « sans serveur de comptes », mais l'inférence « décentralisé » est excessive.
Correction : "Cubelink se connecte directement à tes amis quand c'est possible. Il n'y a pas de serveur de comptes ; un serveur relais sert de secours."

**P-12 · Join · Sécurité · Majeur : consentement éclairé incomplet pour le mod inconnu.**
Points positifs : case explicite non cochée, bouton nommé "Installer le mod d'Alex et rejoindre", option "Toujours accepter ses mods vérifiés" avec "Ne s'applique jamais aux mods inconnus : Cubelink te les redemandera toujours." (cohérent avec Arch §2).
Manques : (a) le message ne dit pas ce qu'un mod peut faire : accès à tes fichiers, ton compte Minecraft, tout ce que ton utilisateur Windows peut faire ; (b) la case n'est pas liée à la désactivation du bouton (à confirmer dans le canvas : si le bouton reste actif sans case cochée, c'est un défaut) ; (c) pas d'alternative "Rejoindre sans ce mod" ; (d) pas de taille/empreinte/date du fichier pour comparer avec Alex ; (e) le titre "Un mod non vérifié va s'exécuter sur ton ordinateur" est juste, le lien avec "(recommandé)" sur "Me demander à chaque fois" aussi.
Correction : "Ce mod peut faire tout ce que fait ton compte Windows : lire tes fichiers, utiliser Internet, modifier ton ordinateur. Cubelink ne peut pas l'analyser. Demande à Alex de le confirmer (nom, taille 412 Ko) avant d'accepter." Case : "Je connais Alex et je lui fais confiance pour ce mod". Désactiver "Installer le mod d'Alex et rejoindre" tant que la case n'est pas cochée ; ajouter le lien "Rejoindre sans ce mod" si techniquement possible.

**P-13 · Login · Juridique · Majeur : logo Microsoft modifié dans le bouton.**
Élément : bouton "Se connecter avec Microsoft" avec carré à 4 vignettes monochrome (SVG `fill="currentColor"`, 20 px) sur fond orange. Microsoft : « DON'T alter the Microsoft logo » et fournit des visuels officiels (https://learn.microsoft.com/en-us/entra/identity-platform/howto-add-branding-in-apps, consultée le 2026-11-09). Le bouton orange avec logo blanc n'est pas l'un des visuels fournis. Ce guide vise surtout Entra ID ; son application à un launcher Minecraft est à faire valider par un juriste.
Correction : utiliser le bouton officiel "Sign in with Microsoft" (fond clair ou sombre, logo quatre couleurs) comme second élément visuel, ou retirer le logo et garder le seul texte "Se connecter avec un compte Microsoft".

**P-14 · Settings (Crédits) · Juridique · Majeur : crédit OGA-BY conforme mais incomplet et peu découvrable.**
Exigé (Assets, ligne hero-1) : "Moon and Sea Pixel Art Background" by CraftPix.net 2D Game Assets, https://opengameart.org/content/moon-and-sea-pixel-art-background, licence OGA-BY 3.0. Modifie : converti en JPEG.
Affiché : "« Moon and Sea Pixel Art Background » par CraftPix.net 2D Game Assets, https://opengameart.org/content/moon-and-sea-pixel-art-background , licence OGA-BY 3.0. Modifié : converti en JPEG ; recadré pour la vignette « Créer un monde »."
Écarts : (a) élément essentiel (titre, auteur, URL, licence, mention de modification) présent, et l'ajout "recadré" est exact (vignette instance-create.jpg, Assets) : c'est bon ; (b) "by" traduit en "par" : acceptable ; (c) aucun lien vers le texte de la licence OGA-BY 3.0 (à ajouter) ; (d) l'URL est suivie d'un espace avant la virgule ("… , licence") : défaut typographique et risque de lien cassé ; (e) la section Crédits n'a aucune entrée dans la navigation (Général : Comptes, Jeux, Réseau P2P ; Aide : Diagnostic, Mentions légales) : elle est en bas de la page Réseau P2P, un endroit incongru ; (f) les crédits des polices Inter et Montserrat (SIL OFL, usage à vérifier) sont absents, comme ceux des bibliothèques open source (Tauri, Iroh) dont les licences demandent généralement un avis ; point à faire valider par un juriste.
Correction : déplacer vers "Aide > Crédits et licences" (ou Mentions légales) ; ajouter "Licence OGA-BY 3.0 : https://opengameart.org/content/oga-by-30-faq" (URL non consultée : à vérifier avant d'utiliser) ; supprimer l'espace avant la virgule.

**P-15 · Tous écrans (rendu) · Copy · Majeur : aucune espace insécable ; typographie française à finir.**
Vérifié par script sur les 7 rendus : 0 espace insécable (U+00A0), 0 espace fine insécable (U+202F), 0 `&nbsp;`. Conséquences visibles : "Pas de compte Minecraft ? En acheter un", "Problème lors de la connexion ?", "Test à 14:03 : 41 ms.", "« mcl1- »", "Tom dit s'appeler « Tom »", "12,4 Go", "8 Mo/s", "26 %" peuvent se couper en fin de ligne (point d'interrogation seul sur la ligne suivante). À confirmer dans le canvas : si le rendu statique a perdu des insécables d'origine (le canvas source n'a pas été comparé octet par octet).
Correction : insécable avant "?", ":", ";", "!", "%" et à l'intérieur de « … » ; insécable entre nombre et unité ("12,4 Go", "38 Mo", "8 Mo/s", "24 ms"). Les guillemets français « » sont déjà utilisés : bien.

### Mineurs

**P-16 · Install · Finition · Mineur : texte de navigation de prototype dans l'écran.**
Texte : "Voir l'état d'échec · si la connexion directe échoue, l'installation passera par un relais, plus lent." (lien vers `Error.html`). C'est un lien de maquette présenté comme du contenu. L'utilisateur final n'aura pas de « Voir l'état d'échec ».
Correction : retirer le lien ; garder seulement le message conditionnel (voir P-10) ou l'afficher comme bannière d'état quand le relais est réellement utilisé : "Connexion indirecte (relais) : plus lente."

**P-17 · Error · Finition · Mineur : l'écran est une feuille de spécification, pas un écran.**
Textes : "Trois états à prévoir : l'attente de la connexion Microsoft, une connexion qui échoue, et un fichier d'ami qui ne correspond pas à ce qu'il avait annoncé." et "Quand ça ne se passe pas comme prévu". Contient aussi 3 variantes de message empilées (p. ex. "Connexion annulée : relance la connexion." / "Pas de réseau : vérifie ta connexion Internet." / "Alex s'est déconnecté : l'installation reprend dès son retour."). À ne pas confondre avec des textes visibles en même temps. Mineur car la maquette sert de catalogue ; à noter dans le livrable pour que le développement n'affiche jamais plusieurs messages à la fois.

**P-18 · Error · Copy / Sécurité · Mineur : message d'empreinte non concordante clair mais neutre à l'excès.**
Texte : "Un fichier reçu d'Alex ne correspond pas à ce qu'il avait annoncé" / "Le fichier aeronautics-dev-0.3.jar a été supprimé par sécurité. Rien n'a été lancé. Les 2 autres mods reçus restent dans ton cache."
Bon : ton calme, action claire. Manque : une cause plausible et une action ("Si cela recommence, demande à Alex de vérifier son monde ; une erreur de transfert est possible"). "Rien n'a été lancé" est une garantie forte : doit être testée (Arch ne la documente pas). "Les 2 autres mods reçus" contredit Join (voir P-23).
Correction : "Un fichier reçu d'Alex ne correspond pas à ce qu'il avait annoncé. aeronautics-dev-0.3.jar a été supprimé et n'a pas été lancé. Cela peut venir d'une erreur de transfert ou d'un changement chez Alex. Réessaie ; si cela recommence, préviens Alex."

**P-19 · Error · Copy · Mineur : "Acheter le jeu" / "Nous n'avons trouvé aucune licence Minecraft Java sur ce compte".**
Ton : le message passe de « tu » à « Nous » (voir P-21). Les comptes Game Pass (PC) donne accès à Minecraft : "achète le jeu" est incomplet. Lien "Acheter le jeu" → `href="#"` (mort).
Correction : "Ce compte Microsoft ne possède pas Minecraft : Java Edition. Connecte-toi avec un autre compte ou obtiens le jeu (achat ou abonnement incluant Minecraft). Si tu viens de l'acheter, attends quelques minutes puis réessaie." Lier à la page officielle minecraft.net.

**P-20 · Login · Finition · Mineur : liens "En acheter un" et "Problème lors de la connexion ?".**
"En acheter un" → `href="#"` (Login.html). "Problème lors de la connexion ?" → `Error.html` (mène vers un catalogue d'états, pas vers de l'aide). À remplacer par des URL officielles et une page d'aide.

**P-21 · Copy · Mineur : tutoiement cohérent mais deux "nous" et un « on » impersonnel.**
Le tutoiement est tenu partout (bien pour un public joueur), sauf "Nous n'avons trouvé aucune licence Minecraft Java sur ce compte." (Error). Aucun vouvoiement détecté. Si le produit a des utilisateurs plus âgés ou institutionnels, choisir et documenter une règle dans `ux-guidelines.md` (pas d'obligation).
Correction : "Aucune licence Minecraft : Java Edition n'a été trouvée sur ce compte."

**P-22 · Copy · Mineur : terminologie flottante (instance / profil / monde / modpack).**
Occurrences : "Mes instances" (Main), "Rejoindre le monde d'Alex" (Join), "Installation du monde d'un ami" (Install), "Un profil séparé « Alex » est créé. Ton instance perso n'est pas modifiée." (Join), "Monde d'un ami" (Install), "Envoyer des mods à mes amis ... les fichiers de ton monde" (Settings), "Partagés entre toutes tes instances." (Settings), "Invité · Forge · 61 mods Monde de Léa" (Main). Le mot « monde » désigne ici un modpack + sa configuration, alors que pour un joueur, « monde » est la sauvegarde. Risque : "les fichiers de ton monde" peut faire croire que la carte (la sauvegarde) est transférée, ce que Arch §2 ne prévoit pas (mods, configs, resource packs, shaders).
Correction : choisir « instance » (les tiens) et « profil invité » ; remplacer "monde d'Alex" par "la partie d'Alex" (ce qui est rejoint) et "les fichiers de ton monde" par "les mods et réglages de ton instance (pas ta sauvegarde)". Fixer le glossaire dans `ux-guidelines.md`.

**P-23 · Join / Install / Error · Finition · Mineur : chiffres incohérents entre écrans.**
Join : "90 mods déjà dans ton cache" + "3 mods à recevoir d'Alex" + "1 mod fait maison" = 94 (= "94 mods" de Main) si le mod maison est en plus des 3. Install : "90 mods déjà en cache, 3 à recevoir" et "Réception des mods depuis Alex : 1 sur 3" ; Error : "Les 2 autres mods reçus" (donc le mod maison est l'un des 3). Total 93 ≠ 94. Avec "38 Mo" et "8 Mo/s", le temps restant à 26 % devrait être de quelques secondes, pas "environ 1 min restante". À rendre cohérent pour que la maquette serve de vérité aux testeurs.
Correction : fixer un scénario unique (p. ex. 90 en cache + 3 à recevoir dont 1 mod maison = 93 ; ou 4 à recevoir) et dériver durée et taille.

**P-24 · Install · Copy · Mineur : ordre des étapes trompeur.**
Étapes : "Préparation", "Vérification", "Téléchargement 26 %", "Lancement". Les fichiers sont vérifiés au fil du téléchargement (Arch §2 : chunks vérifiés par hash), pas avant. Une étape « Vérification » avant « Téléchargement » peut laisser croire à un contrôle préalable du mod (dont il est question en P-03).
Correction : "Préparation", "Réception et contrôle des fichiers", "Installation", "Lancement".

**P-25 · Settings · Copy / Honnêteté · Mineur : "Connexion directe possible … Test à 14:03 : 41 ms." et "Relais : relais public, 41 ms".**
Le statut est « directe possible », mais la latence de 41 ms est celle du relais (Détails techniques), alors que la latence d'une connexion directe à un ami est de 24 ms ailleurs. Le test ne mesure pas avec quel ami la connexion est possible. Un test sans ami ne prouve pas le hole punching (NAT par pair, Arch §3 : 10-20 % d'échecs).
Correction : "Ta connexion permet les liaisons directes avec la plupart de tes amis. Dernier test : 14 h 03, relais joignable en 41 ms." et libeller le test "Tester ma connexion".

**P-26 · Settings · Honnêteté technique · Mineur : "Envoyer des mods à mes amis … (et eux seulement)" et configs.**
Texte : "Tes amis (et eux seulement) peuvent récupérer les fichiers de ton monde quand ils le rejoignent. Cela utilise ta bande passante." Arch §2 : le manifeste inclut aussi configs, resource packs et shaders ; certaines configs contiennent des chemins, noms d'utilisateur, parfois des clés ou mots de passe de serveur. De plus, « tes amis » inclut tout ami à qui tu as accepté une demande, pas seulement ceux auxquels tu penses (cf. P-07).
Correction : "Tes amis acceptés peuvent récupérer les mods et les réglages de ton instance quand ils te rejoignent. Vérifie tes fichiers de configuration si tu y as mis des mots de passe." Option : liste d'exclusion de fichiers.

**P-27 · Main · Finition · Mineur : données fictives plausibles et inégales.**
"Steve_42", "Create: Aeronautics", "NeoForge 21.1.77", "All the Mods 10", "412 mods", "Vanilla+ 1.21.4", "il y a 2 jours" : noms de projets réels, versions plausibles ; risque : être pris pour des promesses ou des versions réelles (« Create: Aeronautics » n'a pas été vérifié par moi ; à confirmer par l'équipe). "Version 1.0.0" sur l'écran Connexion alors que le statut du projet est « conception, rien n'est implémenté » (Arch, en-tête) : numéro de version trop affirmatif ; préférer "Version 0.1.0 (préversion)". Le panneau Main affiche aussi "Stable · Minecraft 1.21.1" qui laisse penser à un canal de publication non décrit.
Correction : bandeau "Maquette : données fictives" dans la phase de démo ; ou noms génériques ("Exemple de pack"). Version : "0.1.0 · préversion".

**P-28 · Tous · Finition · Mineur : états manquants et boutons sans destination.**
Boutons sans destination à confirmer dans le canvas : "Pause" (Install), "Tester", "Copier", "Vider le cache…", "Rouvrir la fenêtre". Settings : "Comptes", "Jeux", "Diagnostic", "Mentions légales" → `href="#"`. Écrans absents : Discover (présent dans le canvas, non rendu), Comptes (déconnexion, suppression des données/clés), Diagnostic, gestion des amis (retirer/bloquer un ami, voir la confiance par ami). Aucune page ne permet de supprimer sa clé/son identité ni de retirer un ami (nécessaire pour le consentement et le RGPD). Le rendu Settings affiche en permanence la boîte de confirmation « Vider le cache de mods ? » (état dépliée) : à confirmer comme état de présentation.
Correction : lister ces écrans dans le périmètre du MVP ou les retirer de la navigation.

**P-29 · Settings · Copy · Mineur : confirmation de purge : formulation et « et tes instances ne se lanceront plus ».**
Texte : "Libère 12,4 Go. Les mods seront retéléchargés au prochain besoin, et tes instances ne se lanceront plus tant que ce n'est pas fait." Message honnête mais maladroit : « libère » à l'impératif ressemble à un ordre. Vidage du cache = mods des profils d'amis supprimés aussi ? Arch §2 : hardlinks vers les instances ; supprimer le cache ne supprime pas les hardlinks (les instances peuvent continuer à fonctionner). La promesse "ne se lanceront plus" est peut-être fausse techniquement (à confirmer).
Correction : "Cela libère 12,4 Go. Les mods dont tu auras besoin seront retéléchargés ; certains profils, dont ceux de tes amis, devront attendre qu'ils soient revenus." Vérifier le comportement réel des hardlinks avant de publier.

**P-30 · Tous · Finition · Mineur : chargement des polices depuis Google Fonts.**
Chaque rendu contient `<link href="https://fonts.googleapis.com/css2?family=Inter...Montserrat...">`. Dans l'application finale (Tauri), cela envoie l'IP de l'utilisateur à Google à chaque lancement : en contradiction avec le discours de confidentialité (P-08) et inutilisable hors ligne. À confirmer dans le canvas : si les polices sont embarquées à l'intégration. Correction : embarquer les polices en local et créditer leurs licences (voir P-14 f).

**P-31 · Marque · Juridique · Mineur : collision possible du nom « Cubelink », non conclusive.**
Recherche web (2026-11-09, outil de recherche de l'équipe, requête « Cubelink Minecraft launcher OR app OR software "Cubelink" ») : aucun produit « Cubelink » trouvé dans les résultats ; seul un produit au nom voisin est ressorti, « Cubion Launcher for Minecraft » (https://www.producthunt.com/products/cubion-launcher-for-minecraft/launches, lancé le 16 février 2026 selon la page ; liste https://alternativeto.net/software/cubion-minecraft/about/). Un résultat négatif d'un moteur de recherche ne prouve pas l'absence d'un autre « Cubelink » (marque déposée, société, produit hors Minecraft, produit réseau/matériel). Les pages n'ont pas été lues au-delà des extraits renvoyés par la recherche. Recherche à faire : bases de marques (INPI, EUIPO TMview, USPTO), registres GitHub/npm/crates, noms de domaine. « Cubelink » contient « Cube », terme évoquant le jeu ; ne pas ajouter « MC » ni « Minecraft » au nom (voir "mcl1-", P-32). Point à faire valider par un juriste.

**P-32 · Friends · Finition · Mineur : préfixe « mcl1- » et schéma `launcher://`.**
Texte : "Vérifie qu'il commence par « mcl1- » et qu'il est complet." Le préfixe (Arch §4 : `mcl1-...`) évoque « Minecraft Launcher » et non « Cubelink » ; c'est un identifiant technique visible. Il porte aussi la mention de « Minecraft » dans un identifiant (voir les consignes d'usage du nom en P-01). Le message d'erreur est bon (cause + action).
Correction : préfixe `cbl1-` ou `cubelink1-` ; schéma `cubelink://`.

---

## Ce qui est professionnel

1. **Parcours d'ajout d'un ami responsable** : "Tom dit s'appeler « Tom ». Cubelink ne peut pas le prouver." reconnaît honnêtement que le pseudo est déclaratif (conforme à Arch §4), propose une vérification hors bande, avec les boutons "Accepter" / "Refuser".
2. **Mod inconnu bien traité dans l'intention** : case explicite, bouton qui nomme le risque ("Installer le mod d'Alex et rejoindre"), niveau de confiance par ami avec "Me demander à chaque fois (recommandé)" présélectionné, et "Ne s'applique jamais aux mods inconnus : Cubelink te les redemandera toujours." (cohérent avec Arch §2).
3. **Honnêteté sur le réseau** : la mention "Si la connexion directe échoue, Cubelink passe par un serveur relais public (plus lent)" et "En connexion directe, tes amis peuvent voir ton adresse IP." disent des choses vraies selon Arch §3 et §4 ; pas de « chiffré de bout en bout » ni « anonyme » non étayé.
4. **Messages d'erreur réussis** : cause + action ("Espace disque insuffisant : libère 1,2 Go puis réessaie.", "Java 21 manquant : Cubelink peut l'installer (environ 2 minutes)."), état d'attente avec expiration ("la demande expire dans 5 minutes"), "Annuler" présent.
5. **Rassurance utile et vraie** : "Les fichiers déjà reçus restent dans ton cache." ; "Un profil séparé « Alex » est créé. Ton instance perso n'est pas modifiée." (Arch §2, profil invité isolé) ; "Aucune ressource Minecraft ou Mojang n'est utilisée." est conforme à Assets.
6. **Crédit OGA-BY présent et exact** pour l'image obligatoire, avec mention de modification, avant même la sortie ; images sous CC0 créditées par courtoisie ; guillemets français « » utilisés ; ton tutoyant constant ; unités françaises "Mo", "Go" sur les écrans (corrigé par rapport à "MB/s" signalé dans `ux-guidelines.md`).

---

## Verdict global

**Pas encore prêt pour des testeurs externes ; prêt pour une démo interne ou fermée, à condition de la présenter comme une maquette (bandeau « maquette », version "0.1.0 · préversion").**
Le fond du design est sérieux (consentement, confiance par ami, honnêteté sur le P2P), mais quatre sujets bloquent une diffusion plus large : l'absence de mention de non-affiliation et de mentions légales / confidentialité (P-01, P-02), des promesses de sécurité plus fortes que ce que l'architecture garantit (« fichiers vérifiés », Modrinth/CurseForge, code sans IP, masquage d'IP : P-03, P-04, P-06, P-09), et une finition de maquette (liens morts, texte de prototype, chiffres incohérents, pas d'insécables).
La corrigibilité est élevée : presque tout se règle par du texte et deux écrans ajoutés.

## Limites de cette review

- Analyse sur rendu statique : états interactifs (case à cocher qui active ou non le bouton, survols, infobulles, "Détails techniques", états de l'écran Discover, présence éventuelle d'insécables dans le canvas source) non vérifiés : marqués « à confirmer dans le canvas ».
- Recherche web limitée : 2 recherches générales, 2 pages lues (minecraft.net Usage Guidelines ; Microsoft Learn Sign in branding) ; pas de recherche de marques officielles (INPI/EUIPO/USPTO) ; pas de lecture de l'EULA Minecraft ni des termes Microsoft. L'incident Fractureiser (P-05) et l'URL de la FAQ OGA-BY (P-14) n'ont pas été consultés.
- Aucune évaluation du code ; la faisabilité du code ami (P-06) et de la robustesse du numéro de sécurité (P-07) relève de la cryptographie et du prototype.
- Aucun conseil juridique : tous les points juridiques sont à faire valider par un juriste.

URL consultées : https://www.minecraft.net/en-us/usage-guidelines · https://learn.microsoft.com/en-us/entra/identity-platform/howto-add-branding-in-apps · résultats de recherche (Cubelink) : https://www.producthunt.com/products/cubion-launcher-for-minecraft/launches, https://alternativeto.net/software/cubion-minecraft/about/ (extraits des résultats de recherche uniquement).
