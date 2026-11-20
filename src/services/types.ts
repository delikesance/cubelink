// Types des données échangées avec Rust. La plupart sont générés depuis Rust dans `bindings.ts` (`npm run bindings`) :
// ne pas les redéfinir ici, les réexporter. Restent écrits à la main : les entrées propres à `Services`, les charges
// utiles d'événements construites en JSON libre côté Rust, et quelques précisions (unions de chaînes) sur des champs
// que Rust garde en `String`.

import type * as B from "./bindings";

// Les structures que serde lit et écrit différemment (champ `serde(default)`, `Option`) existent en deux variantes
// générées : `_Serialize` (ce que Rust renvoie, tous les champs présents) et `_Deserialize` (ce que Rust accepte).
// Le front reçoit la première et envoie la seconde.
export type {
  AccountInfo, AccountKind, BrowseKind, BrowseSource, BrowseVersion, FixFailure, Friend, FriendCode, FriendRequest,
  LaunchPhase, Loader, LoaderSpec, ModEntry, Presence, Profile, StageState, TransferPhase, TrustLevel, UpdatePhase,
  VerifyCode, VersionVerdict,
} from "./bindings";
import type {
  BrowseKind, BrowseSource, BrowseVersion, FriendCode, FriendRequest, LoaderSpec, ModEntry, Presence, Profile, StageState,
  VerifyCode,
} from "./bindings";

export type AccountsState = B.AccountsState_Serialize;
export type BrowseHit = B.BrowseHit_Serialize;
export type BrowseInstallResult = B.BrowseInstallResult_Serialize;
export type BrowsePage = B.BrowsePage_Serialize;
export type ContentCheck = B.ContentCheck_Serialize;
/** `kind` : "install", "update", "enable" ou "disable" ; `source` : "modrinth" ou "curseforge". */
export type FixAction = B.FixAction_Serialize;
export type FixPlan = B.FixPlan_Serialize;
export type FixResult = B.FixResult_Serialize;
export type FixUnresolved = B.FixUnresolved_Serialize;
export type FriendWithPresence = B.FriendWithPresence_Serialize;
export type HostState = B.HostState_Serialize;
export type Instance = B.Instance_Serialize;
/** Illustration d'une instance. `source` : "curseforge", "modrinth" ou "custom". */
export type InstanceArt = B.InstanceArt_Serialize;
export type Invite = B.Invite_Serialize;
export type JoinPlan = B.JoinPlan_Serialize;
export type Manifest = B.Manifest_Serialize;
export type ManifestFile = B.ManifestFile_Serialize;
export type ModCheck = B.ModCheck_Serialize;
/** `kind` : "missing_dep", "incompatible", "wrong_minecraft", "wrong_loader" ou "duplicate" ; `severity` : "error" ou "warning". */
export type ModIssue = B.ModIssue_Serialize;
export type NetTest = B.NetTest_Serialize;
export type Settings = B.Settings_Serialize;
export type TransferProgress = B.TransferProgress_Serialize;
export type UpdateState = B.UpdateState_Serialize;

/** Recherche dans le catalogue. `sort` : "relevance", "downloads" ou "updated". */
export type BrowseQuery = B.BrowseQuery_Deserialize;
/** `ram_mb` (modpack) : RAM de la nouvelle instance, 0 ou absent = « Par défaut ». */
export type BrowseInstallInput = B.BrowseInstallInput_Deserialize;

export type Sha256Hex = string;

/** Champs modifiables d'une instance (instances_update). */
export interface InstancePatch { name?: string; ram_mb?: number; java_path?: string | null; jvm_args?: string; wrapper?: string }
/** Demande à rejoindre reçue d'un ami (hôte), en attente de sa décision. Timestamps en secondes Unix. */
export type JoinRequest = B.JoinRequest;
/** Réponse de l'hôte à ma demande à rejoindre. */
export type JoinAnswer = B.JoinAnswer;

// Entrées / sorties de commandes
export type DeviceCode = B.DeviceCodeInfo;
/** `ram_mb` : 0 ou absent = « Par défaut » (valeur globale des paramètres). */
export interface ImportTarget { name?: string; minecraft?: string; loader?: LoaderSpec; ram_mb?: number }
export interface CreateInstanceInput { name: string; minecraft: string; loader: LoaderSpec; ram_mb: number }
export interface FriendRequestsQuery { offset: number; limit: number; query?: string }
export type FriendRequestsPage = B.RequestsPage;


// Événements
export type LaunchStateName = B.LaunchPhase;
export type LaunchState = B.LaunchStateEvent_Serialize;

/** Liaison du tunnel invité : `closed` = le tunnel vient d'être fermé (fin de partie, remplacement) ; `lost` est terminal. */
export type TunnelLink = "ok" | "reconnecting" | "lost" | "closed";
/** Charge utile de tunnel://state (construite en JSON côté Rust) ; `friend_public_key` : l'hôte, absent pour `closed`. */
export interface TunnelStateEvent { state: TunnelLink; friend_public_key?: string }
/** `in_game` : un jeu tourne chez l'ami (faux hors ligne et pour un ami d'une ancienne version). */
export interface PresenceEvent { public_key: string; presence: Presence; in_game: boolean }

/** Événement transfer://unknown_mods : mods reçus mais absents des catalogues. */
export interface UnknownModsEvent { instance_id: string; unknown_mods: string[] }

/** Événement transfer://error_details : échec d'une étape d'installation invitée (avant la phase `failed`). */
export interface TransferErrorDetails { step: string; message: string; details: string; log_file: string }

/** Événement launch://log : une ligne de sortie du jeu. */
export interface LaunchLogEvent { id: string; line: string }

/** Événement import://warning : mods introuvables lors d'un import. */
export interface ImportWarningEvent { missing: string[] }

export interface FixProgressEvent { done: number; total: number; label: string }
export interface BrowseProgressEvent { done: number; total: number; label: string }
/** Événement `stage://fallback` : le jeu n'a pas pu être affiché dans la fenêtre de Cubelink, il a gardé la sienne. */
export interface StageFallback {
  /** Identifiant technique de la raison (journal `stage.log`), jamais montré au joueur. */
  reason: string;
  /** Trop d'échecs de suite : le jeu s'ouvrira dans sa propre fenêtre jusqu'au redémarrage de Cubelink (le message dit comment revenir). */
  session_off: boolean;
}
/** Rectangle de la zone de jeu et largeur du panneau, en pixels physiques (origine : coin haut-gauche de la zone cliente). */
export interface StageLayout { x: number; y: number; w: number; h: number; panel_w: number }

export interface PickFilesOptions { extensions: string[]; multiple?: boolean }

export type Unsubscribe = () => void;

/** Une méthode par commande (rejet = message d'erreur en français), un abonnement par événement. */

// ---------------------------------------------------------------------------
// Serveurs de groupe (Hearth) : types générés depuis core/src/types.rs, voir docs/contrats.md.
// ---------------------------------------------------------------------------

export type HearthStateKind = B.HearthStateKind;
export type HearthMemberLite = B.HearthMemberLite;
export type HearthUnclean = B.HearthUnclean_Serialize;
export type HearthCard = B.HearthCard_Serialize;
export type HearthInvite = B.HearthInvite;
export type HearthWorld = B.HearthWorld;
// Champs `String` côté Rust, à valeurs fixes : le front les resserre (voir `core/src/types.rs`).
export type HearthMember = Omit<B.HearthMember_Serialize, "sync"> & { sync: "ok" | "behind" | "unknown" };
export type HearthVersion = Omit<B.HearthVersion, "kind"> & { kind: "session" | "checkpoint" | "restore" };
export type HearthProposal = Omit<B.HearthProposal_Serialize, "kind"> & {
  kind: "continue_without" | "add_member" | "remove_member" | "pack_update" | "set_governance" | "leave";
};
export type HearthBlocker = B.HearthBlocker;
export type HearthBranch = B.HearthBranch;
export type HearthSettings = B.HearthSettings_Serialize;
export type HearthDetail = Omit<B.HearthDetail_Serialize, "members" | "versions" | "proposals"> & {
  members: HearthMember[];
  versions: HearthVersion[];
  proposals: HearthProposal[];
};
export type HearthEula = B.HearthEula;

// Charges d'événements (typées à la main).
export interface HearthHandover { group_id: string; name: string; from_name: string }
export interface HearthHostPrompt { group_id: string; name: string; players: number }

export interface HearthCreateInput {
  name: string;
  instanceId: string;
  /** Nom du monde existant de l'instance ; null : nouveau monde. */
  world: string | null;
  /** Clés publiques des amis invités. */
  members: string[];
}

/** Choix après une partie mal terminée : attendre l'hôte, reprendre au point de reprise ou à la dernière version. */
export type HearthUncleanChoice = "wait" | "checkpoint" | "last";

/** Erreur renvoyée par `hearthPlay` tant que le contrat d'utilisation de Minecraft n'est pas accepté. */
export const HEARTH_EULA_REQUIRED = "eula_required";

export interface Services {
  authStatus(): Promise<Profile | null>;
  authBeginLogin(): Promise<DeviceCode>;
  authPollLogin(): Promise<Profile>;
  /** Abandonne la connexion Microsoft en cours côté Rust (sans effet si aucune). */
  authCancelLogin(): Promise<void>;
  authLogout(): Promise<void>;
  authOfflineLogin(name: string): Promise<Profile>;
  /** Les comptes hors ligne sont libres : toujours vrai (méthode conservée pour compatibilité). */
  offlineAvailable(): Promise<boolean>;
  /** Comptes enregistrés et compte actif. */
  authAccounts(): Promise<AccountsState>;
  /** Rend le compte actif ; renvoie son profil. */
  authSwitch(id: string): Promise<Profile>;
  /** Retire un compte. */
  authRemove(id: string): Promise<void>;
  /** Tête du skin (data URL PNG 8×8) d'un compte Microsoft, null si indisponible. */
  authSkinHead(uuid: string): Promise<string | null>;
  /** Tête du skin (data URL PNG 8×8) du joueur au pseudo donné (amis), null si inconnu ou hors réseau. */
  skinHeadByName(name: string): Promise<string | null>;

  instancesList(): Promise<Instance[]>;
  instancesCreate(input: CreateInstanceInput): Promise<Instance>;
  /** Importe un .mrpack, un zip CurseForge ou un zip brut. `target` ne sert que pour un zip sans métadonnées. */
  instancesImportMrpack(path: string, target?: ImportTarget): Promise<Instance>;
  instancesLaunch(id: string): Promise<void>;
  instancesKill(id: string): Promise<void>;
  instancesDelete(id: string): Promise<void>;
  instancesUpdate(id: string, patch: InstancePatch): Promise<Instance>;
  instancesOpenFolder(id: string): Promise<void>;
  instancesLogs(id: string): Promise<string>;
  /** Data URL d'un fichier d'illustration (`icon` ou une capture de `art.screenshots`). */
  /** `maxPx` : côté maximal de la miniature (64..2048, 512 par défaut) ; grand pour un fond ou une visionneuse. */
  instancesArtImage(instanceId: string, file: string, maxPx?: number): Promise<string>;
  instancesArtSet(instanceId: string, path: string, role: "icon" | "banner"): Promise<Instance>;
  /** `auto` : recherche silencieuse de l'accueil (nom identique seulement, jamais pour une instance créée à la main). */
  instancesArtLookup(instanceId: string, auto?: boolean): Promise<Instance>;
  /** Description longue du modpack (HTML déjà nettoyé côté Rust), null sans projet d'origine. */
  instancesDescription(instanceId: string): Promise<string | null>;
  minecraftVersions(): Promise<string[]>;

  /** Teste la connexion réseau entre amis (relais, UDP, routeur). */
  netTest(): Promise<NetTest>;
  friendMyCode(): Promise<FriendCode>;
  friendRegenerateCode(): Promise<FriendCode>;
  friendList(): Promise<FriendWithPresence[]>;
  friendSendRequest(code: string): Promise<VerifyCode>;
  friendRequests(q: FriendRequestsQuery): Promise<FriendRequestsPage>;
  friendAccept(publicKey: string): Promise<void>;
  friendRefuse(publicKey: string): Promise<void>;
  friendRefuseAll(): Promise<number>;
  /** Invite un ami dans mon monde ouvert (rejette en français si aucun monde n'est ouvert). */
  friendInvite(publicKey: string): Promise<void>;
  /** Invitations reçues en attente. */
  friendInvites(): Promise<Invite[]>;
  friendInviteDismiss(id: string): Promise<void>;
  /** Invité : demande à un ami de rejoindre sa partie ; attend sa décision (au plus ~2 min). */
  friendJoinRequest(publicKey: string): Promise<JoinAnswer>;
  /** Hôte : accepte (ouvre l'accès à la partie en cours) ou refuse la demande d'un ami. Rejette si elle n'est plus valable. */
  friendJoinRequestAnswer(publicKey: string, accept: boolean): Promise<void>;
  /** Hôte : demandes à rejoindre en attente de décision. */
  friendJoinRequests(): Promise<JoinRequest[]>;
  /** Ferme le jeu en cours puis lance l'instance. */
  gameSwitch(instanceId: string): Promise<void>;
  /** Instance du jeu en cours (en préparation ou lancé), `null` sinon. */
  gameCurrent(): Promise<string | null>;
  /** Ferme le jeu en cours sans rien relancer (dans la zone intégrée : fermeture propre, le monde est enregistré). */
  gameClose(): Promise<void>;
  /** Ferme le jeu si besoin puis rejoint l'ami (plan, transfert, jeu). */
  gameJoinFriend(publicKey: string, instanceId?: string): Promise<void>;

  joinPlan(friendPublicKey: string): Promise<JoinPlan>;
  /** `manifestHash` : `JoinPlan.manifest_hash` du plan montré. `acceptUnknown` : case de confiance cochée (exigée par
   *  le backend s'il y a des mods inconnus ou non vérifiés, ou des scripts). `ramMb` : RAM de l'instance invitée
   *  (0 ou absent = « Par défaut »). */
  joinStart(friendPublicKey: string, manifestHash: string, acceptUnknown: boolean, ramMb?: number): Promise<void>;
  /** Invité, en un geste : ouvre le tunnel vers le monde de l'ami, lance l'instance invitée avec connexion automatique à 127.0.0.1:port, referme le tunnel à la fin du jeu. Suit les événements de lancement habituels. */
  /** `instanceId` vide : l'instance invitée préparée par le dernier transfert. */
  joinPlay(friendPublicKey: string, instanceId: string): Promise<void>;
  joinPause(): Promise<void>;
  joinResume(): Promise<void>;
  joinCancel(): Promise<void>;

  settingsGet(): Promise<Settings>;
  settingsSet(settings: Settings): Promise<Settings>;

  browseSearch(q: BrowseQuery): Promise<BrowsePage>;
  browseVersions(source: BrowseSource, id: string, kind: BrowseKind, minecraft?: string, loader?: string): Promise<BrowseVersion[]>;
  browseInstall(input: BrowseInstallInput): Promise<BrowseInstallResult>;
  /** Mods seulement (autres types : résultat vide). `versionId` : juger cette version précise. */
  browseCheck(source: BrowseSource, id: string, kind: BrowseKind, instanceId: string, versionId?: string): Promise<ContentCheck>;
  modsList(instanceId: string): Promise<ModEntry[]>;
  /** Icône d'un mod lue dans son .jar (data URL 64 px), null s'il n'en a pas. */
  modsIcon(instanceId: string, file: string): Promise<string | null>;
  /** Vérifie la compatibilité des mods installés (dépendances, incompatibilités, version, loader, doublons). */
  modsCheck(instanceId: string): Promise<ModCheck>;
  modsToggle(instanceId: string, file: string, enabled: boolean): Promise<void>;
  /** Calcule les corrections possibles (sans rien modifier). */
  modsFixPlan(instanceId: string): Promise<FixPlan>;
  /** Applique les actions choisies du plan ; sauvegarde les fichiers remplacés (annulable). */
  modsFixApply(instanceId: string, planId: string, actionIds: string[]): Promise<FixResult>;
  modsFixUndo(instanceId: string, backupId: string): Promise<ModCheck>;
  modsRemove(instanceId: string, file: string): Promise<void>;
  modsAdd(instanceId: string, path: string): Promise<ModEntry>;
  /** Sélecteur de fichiers natif ; renvoie les chemins choisis ([] si annulé). */
  pickFiles(opts: PickFilesOptions): Promise<string[]>;

  onAuthState(cb: (p: Profile | null) => void): Unsubscribe;
  /** Changements de la liste des comptes ou du compte actif. */
  onAccountsState(cb: (s: AccountsState) => void): Unsubscribe;
  onLaunchState(cb: (s: LaunchState) => void): Unsubscribe;
  onLaunchLog(cb: (e: LaunchLogEvent) => void): Unsubscribe;
  onImportWarning(cb: (e: ImportWarningEvent) => void): Unsubscribe;
  /** État du partage hôte au montage (null si rien n'est partagé). */
  hostState(): Promise<HostState | null>;
  /** Changements d'état du partage hôte (partage prêt, monde ouvert, arrêt). */
  onHostState(cb: (e: HostState) => void): Unsubscribe;
  onTransferProgress(cb: (p: TransferProgress) => void): Unsubscribe;
  onFriendPresence(cb: (e: PresenceEvent) => void): Unsubscribe;
  onFriendRequest(cb: (r: FriendRequest) => void): Unsubscribe;
  onFriendInvite(cb: (i: Invite) => void): Unsubscribe;
  /** Liste complète des invitations à chaque changement. */
  onFriendInvites(cb: (l: Invite[]) => void): Unsubscribe;
  /** Hôte : une demande à rejoindre vient d'arriver. */
  onFriendJoinRequest(cb: (r: JoinRequest) => void): Unsubscribe;
  /** Hôte : liste complète des demandes en attente, à chaque changement. */
  onFriendJoinRequests(cb: (l: JoinRequest[]) => void): Unsubscribe;
  // --- serveurs de groupe (Hearth) ---
  hearthList(): Promise<HearthCard[]>;
  hearthDetail(id: string): Promise<HearthDetail>;
  /** Mondes existants d'une instance, proposés à la création. */
  hearthWorlds(instanceId: string): Promise<HearthWorld[]>;
  hearthCreate(input: HearthCreateInput): Promise<HearthCard>;
  hearthInvites(): Promise<HearthInvite[]>;
  hearthInviteAccept(groupId: string): Promise<void>;
  hearthInviteDismiss(groupId: string): Promise<void>;
  /** Le bouton unique de la carte : jouer ou rejoindre selon l'état. Rejette avec `eula_required` au premier hébergement. */
  hearthPlay(id: string): Promise<void>;
  hearthStop(id: string): Promise<void>;
  hearthUncleanChoice(id: string, choice: HearthUncleanChoice): Promise<void>;
  /** Associe le modpack du groupe à une instance locale. */
  hearthSetInstance(id: string, instanceId: string): Promise<void>;
  hearthAddMember(id: string, publicKey: string): Promise<void>;
  hearthRemoveMember(id: string, publicKey: string): Promise<void>;
  hearthLeave(id: string): Promise<void>;
  /** Supprime le serveur de CE PC seulement, sans prévenir le groupe (repli quand quitter est impossible : aucun membre joignable). */
  hearthForget(id: string): Promise<void>;
  hearthVote(id: string, proposalId: string): Promise<void>;
  hearthContinueWithout(id: string, publicKey: string): Promise<void>;
  hearthPackUpdate(id: string, instanceId: string): Promise<void>;
  hearthRestore(id: string, versionId: string): Promise<void>;
  hearthResolveFork(id: string, versionId: string): Promise<void>;
  hearthSetBackground(id: string, on: boolean): Promise<void>;
  hearthEula(): Promise<HearthEula>;
  hearthEulaAccept(): Promise<void>;
  /** Interrupteur de sécurité « Serveurs de groupe (expérimental) » : désactivé par défaut. Coupé, les commandes `hearth*` refusent. */
  hearthEnabled(): Promise<boolean>;
  hearthSetEnabled(enabled: boolean): Promise<void>;
  /** Liste complète des cartes à chaque changement. */
  onHearthState(cb: (l: HearthCard[]) => void): Unsubscribe;
  /** Liste complète des invitations de groupe en attente. */
  onHearthInvites(cb: (l: HearthInvite[]) => void): Unsubscribe;
  onHearthHandover(cb: (e: HearthHandover) => void): Unsubscribe;
  onHearthHostPrompt(cb: (e: HearthHostPrompt) => void): Unsubscribe;
  onUnknownMods(cb: (e: UnknownModsEvent) => void): Unsubscribe;
  onTransferErrorDetails(cb: (e: TransferErrorDetails) => void): Unsubscribe;
  onBrowseProgress(cb: (e: BrowseProgressEvent) => void): Unsubscribe;
  onModsFixProgress(cb: (e: FixProgressEvent) => void): Unsubscribe;
  /** Fichiers déposés sur la fenêtre (glisser-déposer natif). */
  onFilesDropped(cb: (paths: string[]) => void): Unsubscribe;
  updateState(): Promise<UpdateState>;
  /** Vérification manuelle ; si une version existe, le téléchargement démarre en arrière-plan. */
  updateCheck(): Promise<UpdateState>;
  /** Redémarre et installe la mise à jour téléchargée (rejette si une partie est en cours). */
  updateInstall(): Promise<void>;
  onUpdateState(cb: (s: UpdateState) => void): Unsubscribe;
  /** Invité : liaison du tunnel vers l'hôte (tunnel://state). Émis seulement pendant un tunnel ouvert. */
  onTunnelState(cb: (e: TunnelStateEvent) => void): Unsubscribe;

  /** Zone de jeu intégrée (Windows, Linux X11) : elle existe sur ce système. */
  stageSupported(): Promise<boolean>;
  stageState(): Promise<StageState>;
  /** Bascule entre le jeu (`true`) et le launcher (bouton de la barre de titre). */
  stageView(game: boolean): Promise<void>;
  /** Ouvre ou ferme le panneau « en jeu » (Échap, bouton Fermer). */
  stagePanel(open: boolean): Promise<void>;
  /** Publie à Rust le rectangle de la zone de jeu mesuré dans le DOM. */
  stageLayout(layout: StageLayout): Promise<void>;
  /** Met la fenêtre du launcher en plein écran ou l'en sort (plein écran du jeu intégré). */
  setFullscreen(on: boolean): Promise<void>;
  onStageState(cb: (s: StageState) => void): Unsubscribe;
  /** Charge utile brute : `useStageFallback` la valide. */
  onStageFallback(cb: (f: StageFallback) => void): Unsubscribe;
  /** Le jeu demande (F11) ou quitte le plein écran. */
  onStageFullscreen(cb: (on: boolean) => void): Unsubscribe;
}
