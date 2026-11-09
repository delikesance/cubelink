# Cubelink : mini-tâches pour sub-agents (implémentation + E2E)

Source : `plan.md`, `docs/architecture.md`, canvas de design (9 écrans : Login, Main, Create, Install, Join, Friends, Settings, Error, Legal).
Objectif : tout est implémenté et couvert par des tests E2E. Aucun agent n'est lancé sans go explicite.
Décision du 2026-11-09 : pour ce projet, la limite globale de 3 coéquipiers est levée. **Plafond : 6 agents en parallèle.**

## Règles de fonctionnement
- **Un fichier = un propriétaire à un instant donné.** Chaque tâche ne modifie que le fichier (ou le petit dossier) indiqué. Deux tâches d'un même lot ne partagent jamais un fichier.
- **Le team lead prépare tout ce qui est partagé** (vague 0) : tous les `Cargo.toml` avec leurs dépendances, tous les `mod.rs` avec des fichiers stub vides, `package.json`, les types de `core/src/types.rs`, la liste des commandes Tauri. Les agents ne touchent donc **jamais** à `Cargo.toml`, `Cargo.lock`, `package.json`, `lib.rs`, `mod.rs`, ni aux contrats.
- **Un agent a son propre `CARGO_TARGET_DIR`** (`target/<id>`) pour éviter les verrous. Les tests ne ciblent que son crate (`cargo test -p core <module>`).
- **Mode hors ligne pour les E2E** : le lancement du jeu et le scénario complet utilisent un profil hors ligne (UUID dérivé du pseudo), activé par un drapeau de test, absent de l'interface publique. L'auth Microsoft est testée à part contre de faux serveurs.
- **Pas de réseau réel en CI** : Microsoft/Mojang/Modrinth sont simulés (tâches E0.x). Le P2P se teste entre 2 nœuds locaux.
- **Terminé =** code + test(s) qui passent + `cargo clippy -D warnings` (ou lint front) sans erreur + rapport de 10 lignes max dans `docs/rapports/<ID>.md`.
- Pas de push forcé, pas de merge sans validation humaine. Synthèse finale par le team lead.

## Vague 0 : fondations (team lead, séquentiel)
| ID | Tâche | Livrable |
|---|---|---|
| F0.1 | Workspace Cargo (`core`, `p2p`, `src-tauri`), front Vite + TypeScript, script `npm test` (cargo test + clippy + tests front) | `cargo build` et `npm run build` OK |
| F0.2 | Dépendances de tous les crates dans les `Cargo.toml` et `package.json` | `cargo fetch` OK |
| F0.3 | Squelette des modules : un fichier stub par tâche ci-dessous, déclaré dans les `mod.rs` | `cargo build` OK avec stubs |
| F0.4 | Types partagés (`core/src/types.rs`) + `docs/contrats.md` (commandes et événements Tauri) | Compile, relu |
| F0.5 | Tokens de la charte et images du design dans `src/styles/` et `src/assets/` | Page de test |

## Vague 1 : 6 agents en parallèle, aucun fichier commun
| ID | Fichier | Tâche | Test |
|---|---|---|---|
| A1 | `core/src/auth/device.rs` | Demande du code d'appareil Microsoft et attente de validation | Faux serveur : code reçu, validation, expiration |
| A2 | `core/src/auth/xbox.rs` | Échange du jeton Microsoft contre Xbox Live puis XSTS | Faux serveur, cas d'erreur (compte sans Xbox) |
| A3 | `core/src/auth/minecraft.rs` | Échange XSTS contre jeton Minecraft + profil + possession du jeu | Faux serveur, compte sans licence |
| A4 | `core/src/auth/tokens.rs` | Stockage chiffré des jetons, rafraîchissement | Aller-retour, jeton altéré rejeté |
| A5 | `core/src/auth/offline.rs` | Profil hors ligne (UUID dérivé du pseudo) | UUID identique à celui du jeu pour un pseudo donné |
| S1 | `core/src/store/hash.rs` | Hash par blocs (flux), vérification | Fichier de 1 Go sans charger en mémoire |

## Vague 2 : le jeu et le cache (6 agents)
| ID | Fichier | Tâche | Dépend de | Test |
|---|---|---|---|---|
| M1 | `core/src/mojang/versions.rs` | Manifeste des versions + version choisie | F0.4 | Faux miroir |
| M2 | `core/src/mojang/libraries.rs` | Résolution des libraries selon l'OS, règles | M1 | Règles Windows/Linux/macOS |
| M3 | `core/src/mojang/natives.rs` | Extraction des natives | M2 | Archive factice |
| M4 | `core/src/mojang/assets.rs` | Index et téléchargement des assets | M1, S1 | Reprise après coupure |
| S2 | `core/src/store/index.rs` | Index SQLite du cache (hash → chemin, compteurs) | F0.4 | Concurrence, crash au milieu |
| S3 | `core/src/store/link.rs` | Hardlinks (repli copie si impossible) | S2 | Windows + volume différent |
| S4 | `core/src/store/gc.rs` | Nettoyage des fichiers non référencés | S2 | Rien de référencé n'est supprimé |
| J1 | `core/src/java.rs` | Détection de Java, version requise par version de jeu | F0.4 | Dossier de JRE factices |
| J2 | `core/src/java_install.rs` | Téléchargement/installation d'un JRE (21 par défaut) | J1, S1 | Faux miroir |

(Ordre en lots de 6 : [M1, S2, J1, A-restant…] ; le team lead enchaîne dès qu'un agent termine.)

## Vague 3 : loaders, providers, lancement (6 agents)
| ID | Fichier | Tâche | Dépend de | Test |
|---|---|---|---|---|
| L1 | `core/src/loader/neoforge.rs` | Installation NeoForge | M2, J1 | Faux miroir |
| L2 | `core/src/loader/fabric.rs` | Installation Fabric | M2 | idem |
| L3 | `core/src/loader/quilt.rs` | Installation Quilt | M2 | idem |
| L4 | `core/src/loader/forge.rs` | Installation Forge | M2, J1 | idem |
| P1m | `core/src/providers/modrinth_api.rs` | Recherche + versions Modrinth | E0.3 | Faux Modrinth |
| P2m | `core/src/providers/modrinth_dl.rs` | Téléchargement via le cache, vérification du hash | P1m, S2 | Fichier altéré rejeté |
| I1 | `core/src/instance.rs` | Création et lecture d'une instance sur disque | F0.4 | Aller-retour |
| I2 | `core/src/launch_args.rs` | Construction de la ligne de commande (classpath, arguments) | M2, M3, L2 | Comparaison avec une ligne de référence |
| I3 | `core/src/launch_run.rs` | Lancement du processus, suivi, journal | I2, A5 | Faux client Java qui écrit un fichier-témoin |
| I4 | `core/src/mrpack.rs` | Import `.mrpack` | P2m, I1 | Pack de test, mods présents |

## Vague 4 : P2P (6 agents, en parallèle de la vague 3 côté fichiers)
| ID | Fichier | Tâche | Test |
|---|---|---|---|
| N1 | `spike/p2p-tunnel/` | Spike Iroh : 2 nœuds, tunnel, transfert par hash, taux de connexion directe | Rapport go/no-go Iroh vs libp2p |
| N2 | `p2p/src/identity.rs` | Clé ed25519, stockage sécurisé | Stable après redémarrage |
| N3 | `p2p/src/friend_code.rs` | Format `mcl1-…`, parsing, régénération | Tests de propriété |
| N4 | `p2p/src/verify_code.rs` | Code de vérification (4 groupes) | Mêmes codes des deux côtés ; MITM simulé → codes différents |
| N5 | `p2p/src/friends_db.rs` | Liste d'amis SQLite | CRUD, migrations |
| N6 | `p2p/src/requests.rs` | File de demandes (99+, pagination, tout refuser, une demande à la fois) | 1 000 demandes |
| N7 | `p2p/src/friend_proto.rs` | Protocole d'ajout à double sens | 2 nœuds locaux |
| N8 | `p2p/src/presence.rs` | Présence (ping direct) | En ligne / hors ligne |
| N9 | `core/src/manifest.rs` | Manifeste hôte (version, loader, fichiers + SHA-256) | Déterministe |
| N10 | `p2p/src/transfer/plan.rs` | Calcul des fichiers manquants (cache de l'ami) | 90 présents / 3 à recevoir |
| N11 | `p2p/src/transfer/send.rs` | Envoi par blocs vérifiés | 2 nœuds |
| N12 | `p2p/src/transfer/recv.rs` | Réception + vérification | Fichier altéré rejeté |
| N13 | `p2p/src/transfer/resume.rs` | Reprise après coupure | Coupure au milieu |
| N14 | `p2p/src/tunnel.rs` | Tunnel QUIC ↔ localhost | Faux serveur TCP |
| N15 | `core/src/guest.rs` | Profil invité isolé (hardlinks, loader auto) | Instance de l'hôte intacte |
| N16 | `core/src/unknown_mods.rs` | Marquage des mods hors Modrinth | Le mod maison signalé, les 90 connus non |
| N17 | `p2p/src/relay.rs` | Bascule relais + message « plus lentement » | Relais local simulé |

## Vague 5 : interface (6 agents ; commence dès que F0.5 est prêt, branchée sur de faux services)
| ID | Fichier | Tâche | Test |
|---|---|---|---|
| U1 | `src/app/shell.tsx` | Barre de titre, routeur, thème, focus | Navigation clavier |
| U2 | `src/screens/Login/` | Écran Connexion | Composant + E2E |
| U3 | `src/screens/Legal/` | Mentions légales (une section à la fois) | idem |
| U4 | `src/screens/Main/` | Accueil : hero, liste d'instances, Jouer | idem |
| U5 | `src/screens/Create/` | Créer une instance | idem |
| U6 | `src/screens/Settings/` | Paramètres (4 sections) | Persistance après redémarrage |
| U7 | `src/screens/Error/` | États d'attente et d'échec | Chaque état déclenchable |
| U8 | `src/screens/Friends/MyCode.tsx` | Bloc « Mon code » fixe | Copier, régénérer |
| U9 | `src/screens/Friends/Requests.tsx` | Onglet Demandes (99+) | 1 000 demandes fluides |
| U10 | `src/screens/Friends/Add.tsx` | Onglet Ajouter + validation du code | Valide / invalide / envoi |
| U11 | `src/screens/Join/` | Rejoindre (confirmation, case de confiance) | Bouton désactivé tant que non coché |
| U12 | `src/screens/Install/` | Installation d'un monde d'ami (progression, pause, aperçu) | 0 → 100 %, annulation |
| U13 | `src-tauri/src/commands_*.rs` (un fichier par domaine) | Commandes Tauri selon `docs/contrats.md` | Test d'appel |

## Vague 6 : E2E (6 agents, un scénario chacun)
| ID | Fichier | Scénario |
|---|---|---|
| E0.1 | `tests-fixtures/auth/` | Faux serveurs Microsoft/Xbox/Minecraft |
| E0.2 | `tests-fixtures/mojang/` | Faux miroir Mojang + faux client Java |
| E0.3 | `tests-fixtures/modrinth/` | Faux Modrinth + mod maison |
| E0.4 | `e2e/harness/` | `tauri-driver` + Playwright, 2 profils isolés, captures |
| T1 | `e2e/01-add-friend.spec.ts` | A et B s'ajoutent, codes de vérification identiques |
| T2 | `e2e/02-join.spec.ts` | B rejoint le monde de A, case de confiance |
| T3 | `e2e/03-install.spec.ts` | Installation, progression, annulation, reprise |
| T4 | `e2e/04-launch.spec.ts` | Lancement hors ligne et connexion par le tunnel |
| T5 | `e2e/05-unknown-mod.spec.ts` | Mod maison transféré, signalé, lancé, hash identique |
| T6 | `e2e/06-failures.spec.ts` | Hôte hors ligne, coupure, hash incorrect, disque plein, jeton expiré |
| T7 | `e2e/07-relay.spec.ts` | Direct impossible → relais |
| T8 | `e2e/08-a11y.spec.ts` | Clavier seul, focus, contrastes |
| T9 | `e2e/09-sizes.spec.ts` | 960×600 et 1280×720, aucune action hors écran |
| T10 | `e2e/10-full.spec.ts` | Scénario complet T1→T5 enchaîné, 10 fois de suite |

## Vague 7 : durcissement (2 à 3 agents)
| ID | Tâche |
|---|---|
| H1 | Relais `iroh-relay` auto-hébergé + plafonnement du débit |
| H2 | Mises à jour via GitHub Releases |
| H3 | Revue de sécurité : jetons, code ami, intégrité ≠ innocuité, IP vue par le relais |

## Points à trancher (impactent des tâches)
1. **Hash** : SHA-256 (manifeste, N9) vs BLAKE3 (natif iroh-blobs). Proposition : SHA-256 pour le manifeste, BLAKE3 en interne.
2. **Longueur du code de vérification** (48 bits aujourd'hui, jugé faible). Bloque N4.
3. **Relais pour l'installation** : autorisé ou « direct uniquement ». Impacte N17, T7.
4. **Mode hors ligne dans le produit** : réservé aux tests ou proposé aux utilisateurs ? À vérifier avec les conditions de Mojang/Microsoft. Le spike N1 doit confirmer qu'un monde LAN accepte un invité hors ligne, sinon serveur de test `online-mode=false`.
5. **CurseForge** : hors périmètre (seul `.mrpack` est prévu).
6. **Juridique** : non-affiliation, bouton Microsoft, nom « Cubelink » à faire valider.

## Ordonnancement
Le team lead garde 6 agents occupés en permanence. Dès qu'un agent termine, il prend la prochaine tâche dont les dépendances sont satisfaites et dont le fichier est libre. Ordre de priorité : chemin critique du scénario complet d'abord (A1-A5, S1-S3, M1-M4, L2, I1-I3, N1-N16, U13), puis écrans, puis E2E, puis durcissement. Les écrans (vague 5) démarrent tôt, sur de faux services, pour que tout soit prêt à brancher.
