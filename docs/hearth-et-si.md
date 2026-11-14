# Hearth : et si… ?

Voici les cas qui peuvent mal tourner dans un serveur de groupe Hearth (127 au total), avec ce que Cubelink fait dans chacun ; l'étiquette dit ce qui est en jeu, et « conception modifiée » signale un cas qui a changé la spécification de `docs/hearth.md`.

## Réseau et bail

- **Et si** deux amis cliquent sur Jouer à la même seconde ?
  Chaque candidat envoie une demande de bail à tous les membres, qui répondent par un accord ou un conflit, en 5 secondes au plus. Si les accords couvrent la majorité des membres actifs, le bail est « sûr » ; sinon il est « au mieux », l'interface le dit et une fourche reste possible mais visible. Le verrou des témoins dure jusqu'au premier renouvellement du bail, pas 10 secondes. [impact : blocage] [conception modifiée]
- **Et si** Alice (version 42) et Bob (version 41) cliquent sur Jouer en même temps ?
  Si la base de l'un descend de celle de l'autre, la descendante gagne. Sur deux branches distinctes, gagne la base que voit la majorité des témoins, puis la plus petite clé, et la fourche est marquée. Une base inconnue est d'abord synchronisée. [impact : perte de données] [conception modifiée]
- **Et si** deux branches ont le même numéro de version, ou une vieille longue session « bat » une session courte plus récente ?
  Le numéro est une profondeur, pas une date. Cubelink juge la récence par l'ascendance, synchronise toute tête inconnue, et affiche l'écran de fourche s'il reste plusieurs têtes, jamais un Héberger silencieux. [impact : perte de données] [conception modifiée]
- **Et si** je viens d'ouvrir Cubelink, ou de sortir de veille, et je ne connais pas encore la partie en cours ?
  Avant chaque décision, Cubelink lance une ronde de sondage vers tous les membres (4 secondes chacun) et affiche « Recherche de la partie… ». Le cache sert seulement à préchauffer les connexions. [impact : dégradé] [conception modifiée]
- **Et si** un membre relaie un faux bail, ou un absent déjà écarté annonce encore le sien ?
  Le bail est signé et porte l'époque, l'appareil et un identifiant de partie. Il est refusé si l'émetteur n'est pas le titulaire, si l'époque est ancienne ou si la base est inconnue. Un bail relayé ne vaut que pour sa durée restante. [impact : perte de données] [conception modifiée]
- **Et si** un membre garde un bail sans jamais servir de partie ?
  Après 2 échecs de connexion au tunnel en 3 minutes, son bail est ignoré pendant 15 minutes et l'interface propose « Héberger quand même », avec fourche visible. Sans version publiée, un bail est ignoré après 12 heures. [impact : blocage] [conception modifiée]
- **Et si** la partie est chez Alice, mais mon réseau (NAT symétrique, CGNAT) ne peut pas la joindre ?
  Cubelink lit le rapport réseau d'iroh avant d'héberger et affiche « Partie en cours chez Alice, injoignable depuis ton réseau », distinct de Bloqué. Un membre joignable des deux côtés pourra relayer le tunnel plus tard. [impact : blocage] [conception modifiée]
- **Et si** je ne reçois plus les renouvellements de bail alors que la partie continue chez les autres ?
  Avant d'afficher « partie mal terminée », Cubelink interroge les membres en ligne : si l'un voit un bail valide, je rejoins. Sinon, deux sondes directes espacées de 10 secondes doivent échouer avant de proposer la reprise. [impact : dégradé] [conception modifiée]
- **Et si** aucun ami n'a de PC joignable pour servir de relais ?
  Seuls les nœuds qui réussissent un auto-test de connexion entrante s'annoncent relais. Tant que le pool est vide, Cubelink affiche le repli utilisé (relais public ou relais d'un membre), et l'absence de chemin a sa propre raison. [impact : blocage] [conception modifiée]
- **Et si** l'hôte met son PC en veille puis le réveille alors qu'un autre membre a repris la partie ?
  Au réveil, Cubelink ne renouvelle pas le bail : il lance une ronde de sondage. Si un autre bail ou une version plus récente existe, il fait flush, stop et publie sa session en branche ; sinon il continue avec un nouvel identifiant de bail. Pendant un bail, il empêche la veille automatique. [impact : perte de données] [conception modifiée]
- **Et si** un renouvellement de bail arrive après la fin de partie ?
  Chaque bail a un identifiant propre. À la fin de partie, les récepteurs gardent 10 minutes une pierre tombale et ignorent tout renouvellement de ce bail, ou tout numéro inférieur déjà vu. [impact : gêne] [conception modifiée]
- **Et si** le serveur plante mais Cubelink reste ouvert chez l'hôte ?
  Le bail n'est renouvelé que si le serveur est vivant : processus présent, et pendant le démarrage une ligne de sortie de moins de 60 secondes (15 minutes maximum). S'il meurt, la fin de partie est annoncée tout de suite avec le dernier point de reprise. [impact : gêne] [conception modifiée]
- **Et si** l'hôte plante et les joueurs restent sur « Connexion perdue » sans info pendant 90 secondes ?
  Le port local du tunnel est stable par groupe, et l'agent Java affiche « Cubelink cherche la suite de la partie… » puis se reconnecte seul. Après deux sondes à 5 secondes, la carte dit « Alice ne répond plus » en 15 secondes. [impact : dégradé] [conception modifiée]
- **Et si** l'hôte éteint son PC juste après la partie, avant que personne n'ait reçu le monde ?
  Pendant la partie, les joueurs tirent déjà les points de reprise en fond. La fin de partie n'est annoncée qu'après la confirmation d'un autre membre, et l'hôte voit « Envoi du monde à Bob… ne pas éteindre ». Sinon les autres peuvent attendre ou jouer depuis la dernière version disponible. [impact : blocage] [conception modifiée]
- **Et si** l'en-tête d'une version arrive chez tout le monde mais personne n'en a le contenu ?
  Les membres n'annoncent que des versions complètes localement. Synchroniser n'est imposé que si un membre en ligne a la version complète ; sinon on lit « La partie d'Alice n'a pas fini d'être envoyée » avec Attendre ou Jouer depuis la version précédente. Sans progrès en 2 minutes, on change de source. [impact : blocage] [conception modifiée]
- **Et si** les téléchargements en fond écrasent le débit du jeu pendant une partie ?
  Un seul ordonnanceur donne la priorité absolue au tunnel. Les blobs Hearth sont plafonnés à 25 % du débit montant mesuré, le reste est mis en pause. Le ping de chaque joueur est affiché, et la passation est proposée si un joueur dépasse 200 ms via relais. [impact : dégradé] [conception modifiée]
- **Et si** l'hôte veut passer la main à Bob en pleine partie ?
  Alice fait un dernier point de reprise, Bob le tire et confirme qu'il est complet, puis Alice arrête et libère le bail en nommant Bob. Les témoins réservent le bail à Bob pendant 60 secondes, ce qui évite la course avec un tiers. [impact : perte de données] [conception modifiée]
- **Et si** on publie le bail dans la DHT pour savoir qui héberge ?
  La DHT ne sert qu'à la découverte : adresses et indice de tête. Un bail lu en DHT veut seulement dire « sonder ce membre en direct », et son absence veut dire « inconnu », jamais « libre ». Publication toutes les 30 minutes au plus. [impact : perte de données] [conception modifiée]
- **Et si** quelqu'un surveille la DHT pour savoir quand et d'où notre groupe joue ?
  Chaque membre publie un enregistrement chiffré sous une clé dérivée qui change chaque jour et à chaque rotation de clé de groupe. Il n'y figure aucune adresse IP directe, et la tête reçue n'est crue qu'après vérification et synchro directe. [impact : sécurité] [conception modifiée]
- **Et si** je suis le seul membre en ligne un soir de semaine ?
  Cubelink distingue hors ligne, réseau filtré et seul. Seul, Héberger demande une confirmation explicite (« si un autre membre joue seul avant que vous vous croisiez, il y aura deux versions ») et recommande un membre toujours allumé. Cette règle assouplit « pas de jeu hors ligne ». [impact : blocage] [conception modifiée]
- **Et si** je suis sur un réseau qui bloque l'UDP (école, entreprise, Wi-Fi public) ?
  Cubelink affiche « Réseau filtré » au lieu de « hors ligne » et force le passage par un relais en 443. Il ne bloque que sans Internet, ou si un statut récent de moins de 24 heures montrait une version plus récente indisponible. [impact : blocage] [conception modifiée]
- **Et si** un membre ajouté par un autre n'est pas mon ami, ou son adresse change ?
  Le groupe tient un carnet d'adresses signé : chaque membre réémet son adresse à chaque changement, et elle voyage dans l'invitation, Hello et Status. Les membres non amis sont sondés par le protocole Hearth. [impact : blocage] [conception modifiée]
- **Et si** Cubelink tourne deux fois, ou la même identité est copiée sur deux PC ?
  Un verrou d'instance unique redonne le focus au premier processus. Chaque installation a un device_id non itinérant : si un message signé de ma clé porte un autre appareil, Héberger est désactivé jusqu'au choix de l'appareil actif. [impact : perte de données] [conception modifiée]
- **Et si** la fenêtre est cachée et WebView2 ralentit les minuteurs, ou Windows met Java en mode efficacité ?
  Toutes les boucles (bail, sondes, points de reprise, outbox) tournent en Rust, jamais dans le front. Le processus Java est exclu d'EcoQoS et passe en priorité supérieure. Tout écart de plus de 60 secondes entre deux renouvellements est journalisé. [impact : dégradé]

## Données et sauvegardes

- **Et si** le serveur Minecraft réécrit un fichier du monde qui est un lien dur vers le magasin de blobs ?
  Les liens durs sont interdits. Le monde est matérialisé par copie ou par clonage de blocs (reflink, ReFS), et l'import se fait toujours en copie. Un test modifie world/ et vérifie que le blob d'origine garde ses octets. [impact : perte de données] [conception modifiée]
- **Et si** le ramasse-miettes d'iroh supprime des blobs Hearth qui n'ont pas de tag ?
  Hearth a son propre magasin, servi sur son propre ALPN. Chaque version porte un tag persistant sur sa collection, et un transfert partiel pose un tag « pending » jusqu'à sa complétion. Un TempTag ne protège jamais rien durablement. [impact : perte de données] [conception modifiée]
- **Et si** une branche d'absent ou l'ancienne branche d'une fourche disparaît au bout de 14 jours ?
  Un ensemble protégé n'est jamais supprimé par l'âge : têtes, bases de ContinueWithout, pointes de branches, sources de Restore, sauvegardes de pack, outbox non confirmée, dernière version saine. L'âge se calcule sur l'heure locale de réception, pas sur la date déclarée par l'auteur. [impact : perte de données] [conception modifiée]
- **Et si** les points de reprise remplissent le disque de chacun ?
  Hors ensemble protégé, la rétention garde 48 heures complètes, puis le dernier point de chaque session, puis une version par jour et par semaine. Le budget par groupe est le plus petit de 10 Go et de 10 % du disque libre, avec un écran « Stockage ». [impact : dégradé] [conception modifiée]
- **Et si** on lit les fichiers avant la fin de la sauvegarde, ou un mod écrit encore pendant la copie ?
  Séquence unique : save-off, save-all flush, attente de « Saved the game » (120 secondes). Seuls les fichiers modifiés sont copiés en staging, avec taille, date et identifiant contrôlés avant et après (3 essais). Sinon le point de reprise est abandonné et réessayé plus tard. [impact : perte de données] [conception modifiée]
- **Et si** un point de reprise échoue et save-on n'est jamais renvoyé ?
  Une garde envoie save-on sur tous les chemins, y compris panne, panique et délai dépassé. La phase save-off se limite à la copie vers le staging. La sortie du serveur est lue en continu pour ne jamais bloquer un tube plein. [impact : perte de données] [conception modifiée]
- **Et si** le point de reprise gèle la partie plusieurs secondes toutes les 10 minutes ?
  Seule la copie vers le staging a lieu sous save-off, puis le hachage et l'import se font en fond. Si un flush dépasse 5 secondes, l'intervalle passe à 20 puis 30 minutes. Aucune version n'est créée si rien n'a changé ou si personne n'a joué. [impact : gêne] [conception modifiée]
- **Et si** le PC plante au milieu de l'écriture d'une version ?
  L'ordre est imposé : blobs et collection, tag persistant, en-tête par fichier temporaire avec fsync et renommage, puis entrée d'outbox. Au démarrage, toute version incomplète est marquée et n'est jamais annoncée. [impact : blocage] [conception modifiée]
- **Et si** une version quitte l'outbox au premier accusé réseau, avant que le destinataire ait vraiment les blobs ?
  L'entrée ne disparaît qu'après un message « Stored » d'au moins 2 autres membres (ou tous s'il y en a moins), envoyé après tirage complet, tag et fsync. Chaque entrée est un fichier par version. [impact : perte de données] [conception modifiée]
- **Et si** personne n'est joignable et l'outbox grossit à chaque point de reprise ?
  L'outbox garde un seul point de reprise par lignée, le plus récent, plus la session finale. En partie, l'envoi reste sous 25 % du débit montant ; la session finale part à plein débit. [impact : dégradé] [conception modifiée]
- **Et si** le PC s'éteint brutalement et world/ contient du jeu non sauvegardé, peut-être abîmé ?
  Un marqueur de session détecte la fin non propre. world/ est contrôlé puis importé comme version « Orphan » locale, protégée, jamais annoncée ni tête. L'hôte choisit « Publier la fin de partie » ou « La mettre de côté ». [impact : perte de données] [conception modifiée]
- **Et si** la matérialisation s'interrompt, ou quelqu'un clique sur Jouer pendant la synchro ?
  Une machine à états par groupe interdit de matérialiser pendant l'hébergement et fait attendre Jouer. La matérialisation est incrémentale, fichier par fichier, avec un journal « en cours » relancé au démarrage et un index BLAKE3 vérifié avant de lancer le serveur. [impact : perte de données] [conception modifiée]
- **Et si** le disque se remplit pendant la partie ?
  Avant d'héberger, il faut monde + 2 points de reprise + 2 Go libres. En partie, sous 3 Go on avertit dans le jeu et on suspend les points de reprise ; sous 1,5 Go, flush, stop et publication de la session. [impact : perte de données] [conception modifiée]
- **Et si** l'antivirus ou l'indexeur verrouille un fichier du monde juste après son écriture ?
  Les opérations de fichiers sont relancées jusqu'à 10 fois avec un recul exponentiel et un partage complet. Si un fichier reste verrouillé, ce point de reprise est abandonné, jamais publié avec l'ancien contenu du fichier. Un dossier de données sous OneDrive est refusé. [impact : dégradé]
- **Et si** un grand monde moddé dépasse la limite de 10 000 fichiers du validateur de manifeste ?
  Le monde a son propre validateur : mêmes règles de noms Windows, sans liste de préfixes, avec 200 000 fichiers, 2 Gio par fichier et 200 Go au plus. À la création, un fichier invalide est exclu et signalé ; le refus strict s'applique à la réception. [impact : blocage] [conception modifiée]
- **Et si** un hôte Linux crée « Foo.dat » et « foo.dat », ou deux formes Unicode d'un même nom ?
  Les chemins sont normalisés en NFC avec une clé de collision en minuscules Unicode. À la création, le fichier le plus récent est gardé ; à la réception, une collection avec collision est rejetée. Les liens symboliques sont ignorés. [impact : perte de données] [conception modifiée]
- **Et si** un chemin du monde dépasse 260 caractères sous Windows ?
  Les opérations de fichiers passent par un utilitaire de chemins longs, et la racine est raccourcie (8 premiers hex du groupe, dossier « w »). Java démarre avec un répertoire de travail court. [impact : blocage] [conception modifiée]
- **Et si** un seul chunk modifié fait voyager toute une région de 10 Mo ?
  Les fichiers de plus de 1 Mio sont découpés en blocs fixes de 64 Kio, et la collection est un arbre par dossier : un point de reprise ne crée que les nœuds des chemins modifiés. Les caches régénérables (Distant Horizons, cartes web) sont exclus. [impact : dégradé] [conception modifiée]
- **Et si** un nouveau membre doit tirer 20 Go par relais ?
  La synchro démarre dès l'acceptation de l'invitation, depuis plusieurs membres en parallèle, avec reprise sur coupure grâce à un tag « pending ». La progression et l'ETA sont affichés, et on peut Rejoindre pendant la synchro. [impact : dégradé] [conception modifiée]
- **Et si** une version de numéro bas apparaît tard et n'est jamais récupérée ?
  Les en-têtes se synchronisent par identifiants : pour chaque tête inconnue, on remonte les parents jusqu'à un ancêtre connu. Un résumé horaire des identifiants par époque détecte tout écart. [impact : perte de données] [conception modifiée]
- **Et si** le PC plante pendant l'écriture du journal des opérations ?
  La dernière ligne tronquée est ignorée et retirée ; une ligne invalide au milieu déclenche une resynchronisation. Tout est écrit avec fsync, temporaire et renommage, et la clé de groupe peut être redemandée à un membre. [impact : blocage] [conception modifiée]
- **Et si** une version intègre mais au monde abîmé devient la tête ?
  Chaque version reçoit un contrôle léger en fond (level.dat, tables des régions, playerdata). Une régression est signalée avec « Restaurer la version parente », et la dernière version saine est protégée du nettoyage. [impact : perte de données] [conception modifiée]
- **Et si** une version annonce 32 Go au lieu de 6 Go, par erreur ou par malveillance ?
  Une version au-delà de 3 fois la tête + 2 Go est refusée sans confirmation, avec le message chiffré. L'espace libre est vérifié avant de tirer, et les octets reçus sont comparés au total annoncé. [impact : blocage] [conception modifiée]
- **Et si** chaque point de reprise lit deux fois des centaines de Mo (SHA-256 puis BLAKE3) ?
  Une seule empreinte reste : le BLAKE3 du blob, produit par l'import. Le même index sert aux changements, à la matérialisation et au contrôle au démarrage. [impact : dégradé] [conception modifiée]
- **Et si** des restes de fichiers temporaires du jeu entrent dans une version ?
  Une table d'exclusion unique retire les fichiers temporaires de playerdata et de level.dat, les .dat_new et .mca.tmp. Les fichiers *_old sont gardés : le jeu s'en sert pour réparer. [impact : gêne]
- **Et si** des données de jeu hors world/ (quêtes, économie, bases de mods) restent chez l'hôte précédent ?
  Un préfixe versionné `server_state/` reprend une liste de chemins du pack : schémas, quêtes, permissions. Les bases de données ne sont capturées qu'à l'arrêt. ops.json, whitelist.json et banned-*.json ne sont jamais versionnés : ils sont régénérés depuis le groupe. [impact : perte de données] [conception modifiée]
- **Et si** une version arrive avant son parent, ou avant l'ajout de son auteur au groupe ?
  Les en-têtes bien signés mais en attente vont dans une file `pending/` (1 000 maximum, 7 jours) et déclenchent la demande des opérations ou du parent. Après toute réorganisation de la chaîne, le graphe est reconstruit de zéro. [impact : blocage] [conception modifiée]
- **Et si** un monde de 10 Go doit être confié chiffré à des gardiens, sans tout renvoyer à chaque version ?
  Les blobs sont chiffrés un par un avec une clé convergente dérivée de la clé de groupe et de l'époque de clé, ce qui garde la déduplication. Les en-têtes sont chiffrés à part. Les anciennes clés ne sont jamais supprimées. [impact : perte de données] [conception modifiée]

## Sécurité et confiance

- **Et si** un modpack piégé arrive par une mise à jour du groupe ?
  Aucun pack ne s'installe tout seul : chaque membre voit un écran avec le diff (mods ajoutés ou retirés, source de chaque jar, scripts, configs), calculé par lui-même sur le pack publié. Un jar inconnu ou un script exige la majorité, même sous gouvernance du créateur. [impact : sécurité] [conception modifiée]
- **Et si** un membre édite le monde hors ligne puis publie une version « Session » ?
  Une version indique son témoin comme simple provenance (« jouée avec Bob en ligne » ou « sans témoin »). Elle peut rester tête s'il n'y a pas de branche concurrente ; la protection vient de l'historique, de la restauration partielle et du retrait d'un membre. Le pack d'une version est vérifié contre l'historique des packs. [impact : dégradé] [conception modifiée]
- **Et si** le créateur signe deux décisions contradictoires, ou un membre vote pour deux propositions concurrentes ?
  Un vote est refusé s'il vise une cible contradictoire d'un vote déjà donné. Deux décisions au même numéro signées par la même clé forment une preuve d'équivocation, qui rend un retrait recevable même contre le créateur. [impact : blocage] [conception modifiée]
- **Et si** quelqu'un remplit la chaîne d'opérations jusqu'à la limite ?
  Le plafond MAX_OPS s'applique aussi à l'ajout en direct, et la réception limite à 1 opération par minute et par signataire unique, pour empêcher un remplissage par AddMember et RemoveMember en boucle. [impact : blocage] [conception modifiée]
- **Et si** un membre déclare un ami « absent » alors qu'il joue encore ?
  Une proposition « continuer sans » porte un préavis signé de 7 jours. Elle devient caduque si l'absent envoie un message « Présent » ou s'il signe quoi que ce soit. À deux, l'absent devient dormant après ces 7 jours, et le créateur ne perd la gouvernance qu'après 30 jours de dormance. [impact : blocage] [conception modifiée]
- **Et si** la version de base d'un « continuer sans » est inexistante ou très ancienne ?
  Un membre ne signe que si la base est présente et complète, et si c'est une feuille de l'époque courante. L'écran affiche le temps de jeu perdu et demande confirmation. Une base restée inconnue 7 jours permet de refaire la proposition. [impact : perte de données] [conception modifiée]
- **Et si** les autres jouent pendant les jours de vote d'un « continuer sans » ?
  La base effective est calculée à la finalisation : c'est la tête la plus avancée parmi les descendants de la base dont aucun n'a l'absent pour auteur. Ces versions sont comptées dans la nouvelle époque, et le vote signe « absent + époque ». [impact : perte de données] [conception modifiée]
- **Et si** la moitié des membres ont perdu leur clé ou abandonné le groupe ?
  Les membres dormants sortent du calcul des majorités. Ils redeviennent actifs en signant une opération « Wake ». Un groupe dont la moitié des clés est morte peut donc encore décider, et retirer les dormants. [impact : blocage] [conception modifiée]
- **Et si** un membre signe des milliers de versions pour remplir le plafond ?
  Les quotas se mesurent à la réception (30 versions par heure et par auteur, le surplus en attente) et non sur les dates déclarées. La récence se juge par ascendance. Le plafond par auteur supprime d'abord ses plus anciennes versions non protégées. [impact : blocage] [conception modifiée]
- **Et si** on retire un membre, et toutes ses anciennes versions deviennent invalides chez les nouveaux arrivants ?
  Chaque version porte le numéro d'opération connu de son auteur. Elle est valable si l'auteur était membre à ce moment-là, même s'il est parti depuis. Une version d'un ex-membre postérieure à son retrait n'est acceptée que si un membre actuel la prolonge. [impact : blocage] [conception modifiée]
- **Et si** un membre abîme le monde, et la restauration jette le travail honnête des autres ?
  Une restauration n'est affichée qu'une fois sa source vérifiée ; restaurer au-delà de la tête ou de sa parente passe par une proposition. La restauration partielle recopie seulement des fichiers région (« Restaurer cette zone »), avec un journal des modifications par version. [impact : perte de données] [conception modifiée]
- **Et si** on retire un membre pendant une partie, et il reste connecté ?
  L'hôte ferme ses connexions du tunnel, lui envoie `kick` et `whitelist remove`, et ignore tout bail qu'il tiendrait. La clé du groupe est renouvelée. L'interface dit : « Eve garde une copie du monde jusqu'à aujourd'hui ; elle ne recevra plus rien ». [impact : sécurité] [conception modifiée]
- **Et si** deux membres génèrent chacun une nouvelle clé de groupe après un retrait ?
  Seul le signataire qui finalise le retrait génère la clé, et l'opération porte son empreinte et un compteur d'époque. Les membres se la transmettent sur connexion authentifiée. Sans la clé courante, on n'écrit rien chez les gardiens. [impact : sécurité] [conception modifiée]
- **Et si** un même PC sert un groupe et un monde solo, ou un membre retiré garde sa connexion ?
  L'accès est indexé par cible (solo ou groupe) et le premier message de chaque flux désigne la cible. Au retrait ou départ, les connexions actives de la clé sont fermées avec un refus explicite. [impact : sécurité] [conception modifiée]
- **Et si** un membre entre avec un compte au pseudo trompeur, ou revend son tunnel à un inconnu ?
  Chaque membre signe le lien entre sa clé Cubelink et son profil Microsoft. L'hôte écrit la liste blanche depuis ces liens, et le tunnel ferme tout flux dont le pseudo ou l'identifiant ne correspond pas à la clé qui le porte. [impact : sécurité] [conception modifiée]
- **Et si** les opérateurs du serveur changent avec l'hôte, ou un membre change de compte Microsoft ?
  Les opérateurs et réglages sont des paramètres du groupe, modifiés par gouvernance, et ops.json est régénéré à chaque démarrage. Un `/op` tapé en jeu est perdu, et l'interface le dit. Un nouveau compte propose de renommer les données de joueur de l'ancien identifiant. [impact : sécurité] [conception modifiée]
- **Et si** un joueur a un profil hors ligne, ou le service de connexion Mojang est en panne ?
  Jouer affiche « Compte Microsoft requis » avant tout démarrage de serveur, rafraîchit le jeton et vérifie le service. Les propriétés `online-mode` et `white-list` sont réécrites à chaque démarrage et ne se règlent pas. [impact : gêne] [conception modifiée]
- **Et si** un hôte assouplit la config serveur d'un mod (accès HTTP de CC:Tweaked, par exemple) et la diffuse ?
  La config serveur fait partie du pack épinglé et ne change que par mise à jour de pack. Elle est recopiée à chaque démarrage. Certaines clés sensibles assouplies empêchent le démarrage, et les changements de datapacks sont affichés. [impact : sécurité] [conception modifiée]
- **Et si** un mod serveur (carte web, voix) ouvre un port sur le réseau de l'hôte ?
  Les adresses d'écoute connues sont forcées en boucle locale avant le premier démarrage. Après le démarrage, les sockets du serveur sont listés : une écoute non locale est signalée avec le mod probable, et le serveur s'arrête sans confirmation. Jamais d'UPnP. [impact : sécurité] [conception modifiée]
- **Et si** un mod vulnérable permet d'exécuter du code sur le PC de l'hôte ?
  Une liste signée de mods vulnérables, livrée avec Cubelink, est vérifiée à la création, à la mise à jour de pack et à chaque démarrage, avec avertissement bloquant. Lancer le serveur sous des droits réduits est un risque connu documenté. [impact : sécurité] [conception modifiée]
- **Et si** la clé d'identité d'un membre est volée ?
  La clé de compte est chiffrée au repos (DPAPI ou trousseau). Les autres membres actifs remplacent le membre à la majorité avec un code de vérification hors bande. Sous gouvernance du créateur, les actions à fort impact attendent 24 heures et peuvent être vétoées. [impact : sécurité] [conception modifiée]
- **Et si** un membre fait voter l'ajout d'une clé inconnue sous le nom d'un ami ?
  L'ajout porte la signature de consentement de la clé ajoutée. Le votant voit le nom de sa propre liste d'amis, sinon « inconnu de toi » avec un code à 6 chiffres. Un proposant doit avoir cette clé en ami accepté. [impact : sécurité] [conception modifiée]
- **Et si** un membre bavard ou compromis noie le groupe sous les propositions et invitations ?
  Une proposition ouverte par membre et par cible, 10 par groupe, 5 par heure et par clé émettrice. Une invitation est limitée à 3 par jour par ami, à 1 000 opérations, et n'est rejouée qu'à l'ouverture de l'écran. [impact : gêne] [conception modifiée]
- **Et si** un membre du groupe G1 demande un blob du groupe G2 dont il connaît le hash ?
  L'autorisation se fait par blob et non par connexion : le hash doit appartenir à une collection du groupe dont le pair est membre actif, ou à la session solo où il est invité. [impact : sécurité] [conception modifiée]
- **Et si** Hearth m'oblige à me connecter en direct à un membre qui n'est pas mon ami ?
  Il verra mon adresse IP. Cubelink l'affiche avant d'accepter et ralentit les sondes à 5 minutes. Masquer l'adresse passe par un second point d'accès réservé au relais, plus lent : l'interface le dit. [impact : sécurité] [conception modifiée]
- **Et si** les gardiens sont des inconnus qui peuvent jeter ou trier nos données ?
  Tout est chiffré, en-tête compris, sous un emplacement opaque. Les amis passent avant les inconnus, avec au plus un gardien par réseau. Un défi quotidien remplace un gardien fautif, et un « continuer sans » interroge tous les gardiens. Les quotas sont opt-in. [impact : perte de données] [conception modifiée]
- **Et si** un ex-membre suit en direct, via la DHT publique, quand le groupe joue ?
  Les enregistrements sont indexés par une paire de clés dérivée de la clé de groupe, du jour et de l'époque de clé. Un ex-membre ne peut plus la calculer, et l'enregistrement ne contient jamais d'adresse directe. [impact : sécurité] [conception modifiée]

## Minecraft et modpacks

- **Et si** l'empreinte du pack n'est pas la même chez deux amis qui ont des versions différentes de Cubelink ?
  Le pack devient un HearthPack canonique et versionné (fichiers, tailles, BLAKE3, côté client ou serveur), publié comme collection. L'empreinte ne se recalcule jamais depuis une instance locale ; on compare au fichier reçu. [impact : blocage] [conception modifiée]
- **Et si** un joueur met à jour un mod de son côté et ne peut plus entrer ?
  Le groupe a une instance gérée matérialisée depuis le pack. Les ajouts et mises à jour de mods y sont désactivés, et les fichiers sont vérifiés avant chaque lancement et réparés sans demander. Le serveur vient de la même collection. [impact : dégradé] [conception modifiée]
- **Et si** un joueur veut une minimap ou des shaders ?
  Une surcouche locale `client_extras/` accepte resource packs, shaders, options et jars classés « client seulement ». Un mod qui ajoute des registres est refusé avec un message, et rien n'y va vers le serveur. [impact : gêne] [conception modifiée]
- **Et si** un mod « client seulement » fait planter le serveur dédié ?
  Chaque jar est classé par couches (métadonnées du jar, données Modrinth ou CurseForge, liste intégrée), et le résultat est écrit dans le pack. En cas de plantage, Cubelink repère le jar coupable, l'exclut et relance une fois, puis propage l'exclusion par une mise à jour mineure. [impact : blocage] [conception modifiée]
- **Et si** des mods « serveur seulement » manquent selon l'endroit où le créateur a importé le pack ?
  Ils sont conservés à l'import dans un préfixe `server_only/` et installés seulement côté serveur. Pour CurseForge, le server pack est comparé et complète le pack. [impact : dégradé] [conception modifiée]
- **Et si** le serveur démarre avec des mods qui ne correspondent pas au pack et efface des blocs du monde ?
  Avant chaque démarrage, les hashs de server/mods sont vérifiés contre le pack. Un démarrage est refusé si le pack de la version ne correspond pas. Si les journaux montrent des registres manquants, Cubelink fait stop sans rien publier et nomme le mod. [impact : perte de données] [conception modifiée]
- **Et si** une mise à jour de Minecraft ou du pack convertit le monde de façon irréversible ?
  Chaque version doit porter le pack de son époque (historique des packs), le niveau de données du monde est comparé à celui du serveur, et une mise à jour majeure exige la majorité. La liste des dimensions modées contenant des joueurs est affichée avant confirmation. [impact : perte de données] [conception modifiée]
- **Et si** une mise à jour de pack arrive pendant une partie, ou on restaure une sauvegarde d'avant ?
  Une mise à jour n'est possible que sans bail valide et à partir d'une tête unique, sinon « terminez la partie de Bob d'abord ». Les versions de l'ancien pack deviennent des branches de côté. Restaurer une source d'un autre pack passe par une proposition composée. [impact : perte de données] [conception modifiée]
- **Et si** le serveur n'est pas installé quand on clique sur Jouer ?
  Dès l'acceptation de l'invitation ou d'une mise à jour, Cubelink prépare en fond le pack, Java et le serveur, et demande l'EULA. Le statut annonce ce qui est prêt, et Jouer affiche « Préparer » avec progression au lieu de tomber en échec. [impact : dégradé] [conception modifiée]
- **Et si** la passation tombe sur quelqu'un dont le serveur n'est pas prêt ?
  Un statut « prêt à héberger » (serveur installé, EULA acceptée, tête complète) guide le choix du successeur. Les étapes sont affichées en jeu et dans la carte, et les demandes d'action passent par une notification, jamais une boîte cachée. [impact : dégradé] [conception modifiée]
- **Et si** le serveur met 5 minutes à démarrer et un autre membre clique sur Jouer entre-temps ?
  Le bail est annoncé dès le choix d'Héberger, en phase « Démarrage », et renouvelé toutes les 30 secondes. Les autres voient « Alice démarre le serveur (2:10) » et lancent leur client. Le délai de « Done » est fixé à 15 minutes. [impact : dégradé] [conception modifiée]
- **Et si** l'hôte n'a pas assez de RAM pour faire tourner le client et le serveur ?
  La RAM serveur est estimée selon le pack et mesurée après le démarrage, puis réservée avec celle du client. Sous le seuil, Jouer propose « Demander à Bob d'héberger » ou affiche un blocage chiffré. Le réglage par appareil permet d'interdire l'hébergement. [impact : dégradé] [conception modifiée]
- **Et si** un PC héberge deux groupes à la fois ?
  Un seul bail par appareil : héberger un second groupe demande d'arrêter ou de passer la main au premier. Le choix de la RAM tient compte des autres serveurs. [impact : dégradé] [conception modifiée]
- **Et si** un monde solo devient un groupe et son créateur réapparaît au spawn avec l'inventaire vide ?
  À la création, `Data.Player` de level.dat est copié dans le fichier de joueur de l'identifiant Minecraft du créateur, avec ses données de mods, puis retiré. Si ce fichier existe déjà, le plus récent est gardé. [impact : perte de données] [conception modifiée]
- **Et si** le créateur continue de jouer à l'original solo en croyant jouer au monde du groupe ?
  À la création, le dossier d'origine est renommé (« Survie (copie avant le groupe …) ») et marqué : le partage solo le refuse avec « Ce monde est devenu un serveur de groupe ». [impact : gêne]
- **Et si** un op change la config du pack en jeu, ou une mise à jour de pack ne corrige pas un monde existant ?
  Les configs sont recopiées depuis le pack à chaque démarrage. À l'arrêt, si des fichiers ont changé, Cubelink propose au groupe une mise à jour mineure du pack (« Alice a changé 2 réglages : les garder ? »). [impact : dégradé] [conception modifiée]
- **Et si** le serveur dédié ne se comporte pas comme le solo (protection du spawn, vol, tick trop long) ?
  server.properties est régénéré à chaque démarrage depuis les réglages du groupe, avec des défauts adaptés (spawn libre, vol autorisé, watchdog coupé). Les clés propres à Cubelink sont forcées. Graine et type de monde sont figés à la création. [impact : dégradé] [conception modifiée]
- **Et si** Cubelink se ferme, plante ou se met à jour pendant qu'on héberge ?
  Java est lancé sous un petit superviseur qui survit à Cubelink, possède l'entrée du serveur et expose un tube local réservé à l'utilisateur. Sans retour de Cubelink en 2 minutes (ou aucun joueur), il fait save-on, flush, stop. La mise à jour automatique est refusée pendant un bail. [impact : perte de données] [conception modifiée]
- **Et si** l'hôte clique sur Quitter, ou une mise à jour interne redémarre Cubelink en pleine partie ?
  Fermeture et mise à jour sont refusées ou différées tant qu'un bail est détenu, avec « Passer la main » et « Arrêter et mettre à jour ». Le vérificateur de mises à jour ne propose jamais de redémarrage pendant un bail. [impact : perte de données] [conception modifiée]
- **Et si** l'hôte éteint Windows, ou Windows Update redémarre la nuit ?
  Cubelink retient l'arrêt (raison affichée, l'utilisateur garde « Arrêter quand même »), fait stop, publie la fin de bail avec la raison « arrêt », et écrit un marqueur. La version Session est publiée au démarrage suivant. Une fenêtre cachée de premier niveau reçoit le message Windows. [impact : perte de données] [conception modifiée]
- **Et si** la mise à jour du Store ferme Cubelink et tue Java sans stop ?
  Le même gestionnaire traite la fermeture demandée par le Store : blocage, stop, fin de bail, puis cession. Le dossier hearth est placé dans le dossier local du paquet, plus court et non virtualisé, et testé avec une vraie mise à jour pendant un serveur actif. [impact : perte de données] [conception modifiée]
- **Et si** le serveur reste allumé sans joueur, ou un op tape /stop ?
  Cubelink suit les joueurs connectés et fait stop après 5 minutes sans joueur (réglable de 1 à 30). Une sortie précédée de « Stopping the server » est un arrêt propre, même venu d'un joueur. [impact : gêne] [conception modifiée]
- **Et si** le serveur plante en pleine partie ?
  L'état sur disque devient une version « Orphan » locale, jamais publiée toute seule. L'hôte choisit de la garder ou de revenir au dernier point de reprise, avec l'extrait du rapport de plantage. Les autres peuvent jouer depuis le dernier point de reprise. [impact : perte de données] [conception modifiée]
- **Et si** un monde fait planter tout serveur au démarrage et chaque membre qui clique sur Jouer plante à son tour ?
  La fin de bail dit pourquoi (« Le serveur d'Alice a planté, mod probable : create »). Un plantage dans les 3 minutes marque la version « plante au démarrage », et Jouer propose « Restaurer la partie de 21 h » ou « Réessayer ». [impact : blocage] [conception modifiée]
- **Et si** les mods de carte oublient cartes et waypoints parce que le port du tunnel change ?
  Chaque groupe a une adresse locale stable : 127.0.0.1 et un port dérivé du groupe (puis 25565 en repli), et une entrée nommée dans la liste des serveurs. Le tunnel garde son écouteur ouvert et transmet le port, sans le relâcher. [impact : gêne] [conception modifiée]
- **Et si** trancher une fourche fait perdre l'inventaire des joueurs de l'autre branche ?
  Chaque joueur peut garder ses propres données de l'autre branche (inventaire, advancements, stats). La restauration ne remplace que ces trois fichiers par identifiant, et remet la position au spawn si le chunk n'existe pas. [impact : dégradé] [conception modifiée]
- **Et si** le chat vocal du pack utilise un port UDP que le tunnel ne transporte pas ?
  Plus tard, l'UDP passera par des datagrammes QUIC du tunnel pour chaque port déclaré. D'ici là, la voix est coupée et l'interface affiche « Chat vocal non pris en charge ». [impact : dégradé] [conception modifiée]
- **Et si** toutes les connexions arrivent depuis 127.0.0.1 et un /ban-ip bannit tout le monde ?
  Chaque connexion vers le serveur part d'une adresse de boucle locale propre au membre (127.1.X.Y dérivée de sa clé), sauf sous macOS. La correspondance adresse et membre est journalisée. [impact : gêne]
- **Et si** le profil Windows a un nom cyrillique ou CJK et Forge refuse de démarrer ?
  Java démarre dans le dossier du serveur avec des chemins relatifs. Cubelink vérifie que le chemin tient dans la page de code ANSI, sinon utilise le nom court 8.3 ou propose une autre racine. [impact : blocage]

## Vie du groupe et expérience

- **Et si** Cubelink est fermé chez tout le monde le soir où on veut jouer ?
  Dès qu'un groupe existe, la croix de la fenêtre masque Cubelink dans la zone de notification, et le démarrage automatique est activé par défaut. L'écran Bloqué liste les membres avec « vu il y a 3 h » et demande qu'au moins un ait Cubelink ouvert. [impact : blocage] [conception modifiée]
- **Et si** deux décisions du groupe sont finalisées en même temps (un départ et un vote qui aboutit) ?
  La chaîne reste linéaire, et les votes signent le contenu de la décision, pas sa place. À numéro égal, on garde celle qui a le plus de signatures éligibles, puis le plus petit hash. Une perdante à un signataire est re-signée automatiquement, sinon elle redevient une proposition. [impact : blocage] [conception modifiée]
- **Et si** un vote est demandé alors que personne n'ouvre la fenêtre, ou une décision intermédiaire efface les votes ?
  Le statut annonce les propositions ouvertes, qui expirent après 14 jours (« 2 votes sur 3, expire dans 5 jours »). Une notification Windows, un message en jeu et un rappel à 24 heures préviennent. Les votes restent valables sur le contenu après réorganisation. [impact : blocage] [conception modifiée]
- **Et si** un ami parti pour toujours reste dans tous les quorums ?
  Le groupe peut marquer plusieurs absents comme dormants, hors des majorités. Un dormant se réveille d'une opération signée. Après 30 jours, la carte propose « Retirer Hugo du groupe ». [impact : blocage] [conception modifiée]
- **Et si** un groupe de 2 se retrouve bloqué quand l'autre est absent ?
  Une fois l'autre dormant, Léa peut héberger seule, avec la confirmation du mode « seul ». Elle peut jouer, mais inviter ou retirer attend 30 jours de dormance. La création d'un groupe de 2 prévient qu'il faut que l'autre ait Cubelink ouvert. [impact : blocage] [conception modifiée]
- **Et si** un membre est déclaré absent par des amis qui ne le voient pas, alors qu'il joue tous les jours ?
  Chaque membre émet un battement signé relayé dans les statuts. Une proposition n'est éligible que si le dernier battement connu date de plus de 7 jours chez tous, et elle tombe si un battement ou une version plus récents circulent. [impact : dégradé] [conception modifiée]
- **Et si** le créateur est injoignable et on veut juste inviter quelqu'un ?
  Une opération de gouvernance bascule en majorité sans changer d'époque. Le créateur la signe seul, ou la majorité des autres membres actifs après 30 jours sans activité du créateur. À la création, la majorité est le défaut pour 3 membres et plus. [impact : blocage] [conception modifiée]
- **Et si** je quitte le groupe alors que ma dernière partie n'est pas envoyée, ou que je tiens le bail ?
  Quitter est refusé tant qu'on tient le bail. L'outbox est vidée avant de signer, sauf « Quitter quand même ». Le départ renouvelle la clé de groupe, puis propose de supprimer les Go du groupe ou de les garder en monde solo. [impact : blocage] [conception modifiée]
- **Et si** je désinstalle la version Store alors que ma dernière partie n'est pas envoyée ?
  Windows ne laisse pas exécuter de code à la désinstallation : l'outbox est vidée au plus tôt, et Réglages propose « Avant de désinstaller : quitter mes groupes ». « Sauvegarder mon identité » est rappelé à la création du premier groupe. [impact : perte de données] [conception modifiée]
- **Et si** un joueur a un fixe et un portable, et compte pour deux membres ?
  Une clé de compte, chiffrée, signe des certificats d'appareil. Un membre est un compte : votes et absences se comptent par compte, avec un seul bail actif par compte. Les amitiés migrent vers les appareils avant le groupe multi-PC. [impact : dégradé] [conception modifiée]
- **Et si** je change de PC, je réinstalle Windows, ou je copie ma clé sur un portable ?
  Un fichier d'identité chiffré par phrase de passe s'importe en créant un nouvel appareil, sans cloner la clé d'appareil. Après une perte totale, les autres membres actifs remplacent mon compte à la majorité avec un code de vérification hors bande. [impact : blocage] [conception modifiée]
- **Et si** deux membres tranchent la même fourche dans des sens opposés ?
  Si une branche a moins de 5 minutes de jeu depuis l'ancêtre commun, l'autre est gardée sans demander. Sinon seul le titulaire du bail publie le choix, et chaque version dit qui y a joué. Rien n'est jamais supprimé. [impact : dégradé] [conception modifiée]
- **Et si** après avoir tranché une fourche, l'écran « Deux versions existent » revient à chaque clic ?
  La version de résolution cite les têtes absorbées dans un champ signé. Une version citée n'est plus une tête, et deux résolutions concurrentes sont détectées comme une nouvelle fourche. [impact : blocage] [conception modifiée]
- **Et si** l'interface affiche « bail », « époque » ou une clé en hexadécimal ?
  Une couche de messages traduit chaque erreur en langage courant avec une action. Les clés deviennent des noms d'amis, et un test automatique interdit les mots techniques et les hexadécimaux dans les cartes. [impact : gêne]
- **Et si** Cubelink synchronise des Go sur batterie ou sur un partage de connexion 4G ?
  Sur connexion limitée, seuls les opérations, en-têtes et baux sont synchronisés, et « Synchroniser quand même » reste possible. Sur batterie avec économiseur, les sondes passent à 5 minutes. Un Jouer explicite télécharge toujours, taille annoncée. [impact : gêne] [conception modifiée]
- **Et si** le groupe a une fourche ou un « Bloqué » inexplicable, et on ne peut pas reconstituer ce qui s'est passé ?
  Un journal d'événements par groupe et les journaux serveur par session alimentent un écran « Historique ». Le zip de diagnostic retire le chat, remplace les adresses par des empreintes et se montre avant l'export. [impact : dégradé] [conception modifiée]
- **Et si** les amis n'ont pas tous la même version de Cubelink, et un nouveau champ rend les messages illisibles ?
  Chaque message signé porte un numéro de format et un espace d'extension, et Hello annonce les versions supportées. Une requête trop récente reçoit une erreur explicite, Jouer affiche « Mets Cubelink à jour », et le groupe relève le format quand tous sont à jour. [impact : blocage] [conception modifiée]
- **Et si** une phase 4 sort sans les points de reprise, et un plantage perd toute la session ?
  Hearth reste désactivé par défaut tant que les phases 4 et 5 et les gardes de plantage ne sont pas vérifiées en réel. Le format est gelé avant la première pré-version qui l'active, et « Exporter en monde solo » reste disponible. [impact : perte de données]
- **Et si** on ne peut pas tester les courses de bail, les partitions et les veilles à plusieurs machines ?
  Un banc de test fait tourner plusieurs nœuds dans un processus, avec latence, perte, partition et gel simulés, et une horloge injectable. Un scénario réel lance deux instances Cubelink et un vrai serveur vanilla. [impact : dégradé]
