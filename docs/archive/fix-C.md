# Correctifs coéquipier C : Paramètres, Mentions légales, Erreurs

Fichiers : `Settings.dc.html`, `Error.dc.html`, `Legal.dc.html` (nouveau), dans `scratchpad\cv\project\`.
Captures de contrôle (1280x720 et 960x600) : `docs/review/fix-C/`. Vérifié par mesure : zéro `href="#"`, zéro `opacity` sur texte, zéro `outline:0`, balises appariées, texte de non-affiliation identique à `fix-rules.md` (Settings et Legal). Non vérifié : le bas des pages défilantes (crédits, bas de Legal, cartes basses d'Error) n'a pas été capturé, seulement le haut à 1280x720 et 960x600.

## Tableau

| ID | État | Changement (une ligne) |
|---|---|---|
| P-01 | corrigé | Non-affiliation exacte dans Crédits (Settings) et dans Legal. À faire valider par un juriste. |
| P-02 | corrigé | « Mentions légales et confidentialité » est un vrai lien vers `Legal.dc.html` ; la page existe, bouton Retour vers `Settings.dc.html`. |
| P-09 | corrigé | Réglage « Passer toujours par le relais », aide : texte exact « Utiliser uniquement le relais : … ». « Masquer mon adresse IP » supprimé. |
| P-11 | corrigé | Sous-titre : « Connexion directe quand c'est possible, sinon via un relais (…) ». |
| P-14 | corrigé | Crédit OGA-BY exact avec lien licence, polices Inter et Montserrat (OFL 1.1), « Crédits et licences » dans la navigation (ancre) et sa propre section, plus sous « Réseau P2P » comme sous-titre sans cacher. URL de la licence OGA-BY et de l'OFL à vérifier (voir ci-dessous). |
| P-17 | corrigé | Plus de titre ni sous-titre de spécification : 8 cartes d'état réelles, un seul message par carte. |
| P-18 | corrigé | Empreinte non concordante : fichier supprimé, non exécuté, cause possible, « Réessaie ; si cela recommence, préviens Alex ». |
| P-19 | corrigé | « Aucune licence Minecraft : Java Edition n'a été trouvée sur ce compte… », bouton « Obtenir Minecraft » vers minecraft.net. Plus de « nous ». |
| P-21 | corrigé | Le « nous » (Error) est supprimé. |
| P-22 | partiel | « instance », « profil invité » employés ; Error dit encore « le monde d'Alex » (carte Java), vocabulaire fixe de `fix-rules.md`. Glossaire dans `ux-guidelines.md` : non fait (hors périmètre). |
| P-25 | corrigé | « Dernier test : 14 h 03, relais joignable en 41 ms », réserve sur la connexion directe par ami, bouton « Tester ma connexion ». |
| P-26 | corrigé | « Les amis que tu as acceptés … mods et réglages (fichiers de configuration) … Ta sauvegarde n'est pas concernée. Vérifie tes fichiers de configuration si tu y as mis des mots de passe. » |
| P-28 | partiel | Liens `#` morts retirés (Comptes, Jeux, Diagnostic supprimés de la navigation) ; Error : tous les boutons de navigation relient Login/Install/Main ; la confirmation de purge est repliée (`details`). « Tester ma connexion », « Copier », « Vider le cache (12,4 Go) », « Rouvrir la fenêtre » restent des actions d'interface sans écran de destination. |
| P-29 | corrigé | « Cela libère 12,4 Go. Les mods dont tu auras besoin seront retéléchargés. En attendant, certaines instances ou certains profils invités peuvent ne pas se lancer. » Comportement des hardlinks à confirmer. |
| P-30 | partiel | Inter 400/500/600/700 chargées avec repli `system-ui`. Embarquement local des polices non fait : décision d'intégration (Tauri), pas faisable dans la maquette. |
| D-04 | corrigé | Tout dans la colonne droite défilante (padding 28 px), modale à hauteur bornée, cartes en `padding:16px 20px`, rythme régulier, crédits dans la même colonne. |
| D-21 | corrigé | Grille de cartes d'égale hauteur, actions alignées en bas, `nowrap`, icônes identiques (28 px) hors spinner. |
| L-02 | corrigé | Bordure du champ de recherche `#6A6A6A`, `outline:none` remplacé par `:focus-within` (anneau `#FF8A4A`). Mêmes bordures pour boutons secondaires et codes. |
| L-04 | corrigé | `::placeholder` `#9A9A9A`, `aria-label` ajouté, libellé raccourci à « Rechercher » (le texte long était tronqué). |
| L-10 | corrigé | « Mon code ami » (`mcl1-…`) et « Code de vérification » (4 groupes, une ligne, monospace, `nowrap`). « nœud » et ed25519 seulement dans Détails techniques. |
| L-13 | corrigé | Inter `wght@400;500;600;700`. Voir P-30 pour l'embarquement. |
| L-16 | corrigé | `max-width:68ch` sur tous les paragraphes et aides. |
| L-17 | corrigé | « relais » expliqué dès le sous-titre ; « empreinte » expliquée (Error) ; « nœud » remplacé par « installation » ; aucun « manifeste ». |
| L-22 | corrigé | Marges identiques à celles de la modale ; interrupteurs : ligne entière cliquable avec libellé d'action (« Envoyer des mods à mes amis », « Passer toujours par le relais »), case 24 px. |
| L-24 | corrigé | Plus de texte de spécification visible (titre h1 seulement pour lecteurs d'écran). |

Autres règles transversales : focus visible sur tous les éléments interactifs, cibles >= 44 px, texte >= 14 px (12 px seulement pour les majuscules grasses espacées et la barre de titre), insécables avant `: ; ?` et entre nombre et unité, tutoiement, hauteur fixe à 100vh avec défilement interne.

## À compléter (juridique, rien d'inventé) dans `Legal.dc.html`
- Éditeur : nom ou raison sociale.
- Éditeur : adresse et statut juridique.
- Durée de conservation et base légale du traitement.
- Contact : adresse e-mail.
Tout le texte (Non-affiliation, Données traitées, Relais) est à faire valider par un juriste.

## À confirmer dans le canvas
- Valeur d'exemple du code ami `mcl1-k3f9-a2c1-77d0` et clé `3f9a…5e14` : fictives ; la longueur du code de vérification (4 groupes) reste à trancher côté architecture.
- URL de la licence OGA-BY (`https://opengameart.org/content/oga-by-30-faq`) et de l'OFL (`https://openfontlicense.org/`) : non consultées, à vérifier. Noms des titulaires des polices (« The Inter Project Authors », « The Montserrat Project Authors ») à vérifier.
- « Rien n'a été exécuté » (empreinte invalide) : à tester avant de livrer.
- Relais : le texte de Legal dit que ce qu'il peut lire du contenu n'est pas établi ; à mettre à jour après validation du protocole.
- Purge du cache : comportement réel des hardlinks (P-29).
- Barre de recherche : sans fonction (maquette). Écrans Comptes, Jeux, Diagnostic retirés de la navigation, hors MVP à décider.
- Boutons d'action sans écran (« Rouvrir la fenêtre », « Copier », « Tester ma connexion », « Vider le cache ») ; le `onclick` de « Annuler » (ferme la confirmation) peut être retiré par le runtime `.dc.html`.
- Error : cartes d'état à utiliser une par une dans l'application, jamais ensemble ; la page défile à 1280x720 (8 cartes). Liens entrants vers `Legal.dc.html` (Connexion, etc.) à ajouter par le team lead, A et B.
