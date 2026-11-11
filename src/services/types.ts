// Types miroirs de docs/contrats.md et core/src/types.rs (JSON, énumérations en minuscules).

export type Sha256Hex = string;
export type Loader = "vanilla" | "neoforge" | "fabric" | "quilt" | "forge";
export interface LoaderSpec { loader: Loader; version: string }

export type Profile =
  | { kind: "online"; name: string; uuid: string }
  | { kind: "offline"; name: string; uuid: string };

/** Un compte enregistré (Microsoft ou hors ligne). */
export interface AccountInfo { id: string; kind: "online" | "offline"; name: string; uuid: string }
/** Comptes enregistrés et identifiant du compte actif (null si aucun n'est actif). */
export interface AccountsState { accounts: AccountInfo[]; active: string | null }

export interface Instance {
  id: string;
  name: string;
  minecraft: string;
  loader: LoaderSpec;
  ram_mb: number;
  mod_count: number;
  java_path?: string | null;
  jvm_args?: string;
  art?: InstanceArt | null;
}

/** Illustration d'une instance (icône, bannière, captures). */
export interface InstanceArt { source: "curseforge" | "modrinth" | "custom"; project_id: string; title: string; author: string; summary: string; color: string | null; icon: string | null; screenshots: string[]; page_url: string | null }

/** Champs modifiables d'une instance (instances_update). */
export interface InstancePatch { name?: string; ram_mb?: number; java_path?: string | null; jvm_args?: string }

export interface ManifestFile { path: string; size: number; sha256: Sha256Hex }
export interface Manifest {
  name: string;
  minecraft: string;
  loader: LoaderSpec;
  java_major: number;
  files: ManifestFile[];
}

/** Chaîne opaque `mcl1-…`. */
export type FriendCode = string;
/** 4 groupes de 4 caractères hexadécimaux majuscules. */
export type VerifyCode = string;

export type TrustLevel = "ask" | "always";
export interface Friend { public_key: string; name: string; trust: TrustLevel; added_at: number }
/** Invitation reçue à rejoindre le monde d'un ami (timestamps en secondes Unix). */
export interface Invite {
  id: string;
  public_key: string;
  name: string;
  world_name: string;
  minecraft: string;
  received_at: number;
  expires_at: number;
}
/** État du panneau en jeu (commande overlay_state, événement overlay://state). */
export interface OverlayState { available: boolean; game_running: boolean; hotkey: string; fullscreen_exclusive: boolean }
export interface FriendRequest {
  public_key: string;
  name: string;
  verify_code: VerifyCode;
  received_at: number;
}
export type Presence = "online" | "offline";

export type TransferPhase =
  | "preparing" | "receiving" | "verifying" | "launching" | "paused" | "failed" | "done";
export interface TransferProgress {
  phase: TransferPhase;
  files_done: number;
  files_total: number;
  bytes_done: number;
  bytes_total: number;
  speed_bps: number;
  via_relay: boolean;
  /** Message d'échec (phase "failed"), en français. */
  error?: string;
}

/** Non défini côté Rust à ce jour : forme provisoire, à confirmer avec le team lead. */
export interface Settings {
  language: string;
  default_ram_mb: number;
  java_path: string | null;
  trust_default: TrustLevel;
  allow_relay: boolean;
  close_on_launch: boolean;
  relay_only: boolean;
  share_mods: boolean;
  /** Retrouver ses amis même si leur adresse change (publication de présence via les serveurs n0). Défaut true. */
  discovery: boolean;
  /** Ouvrir mon monde solo au réseau local automatiquement (agent Java, instances perso). Défaut true. */
  lan_auto: boolean;
  /** Panneau en jeu (fenêtre superposée au jeu) activé. Défaut true. */
  overlay_enabled: boolean;
  /** Raccourci global du panneau en jeu, ex. « Ctrl+Shift+C ». */
  overlay_hotkey: string;
}

// Entrées / sorties de commandes
export interface DeviceCode { user_code: string; verification_uri: string; expires_in: number }
export interface ImportTarget { name?: string; minecraft?: string; loader?: LoaderSpec }
export interface CreateInstanceInput { name: string; minecraft: string; loader: LoaderSpec; ram_mb: number }
export interface FriendWithPresence { friend: Friend; presence: Presence }
export interface FriendRequestsQuery { offset: number; limit: number; query?: string }
export interface FriendRequestsPage { items: FriendRequest[]; total: number }
export interface JoinPlan {
  manifest: Manifest;
  cached: number;
  to_receive: number;
  to_receive_bytes: number;
  unknown_mods: string[];
  /** Jars non vérifiés (sha1 absent, réseau) : ni reconnus ni signalés inconnus. */
  unknown_unchecked?: string[];
}

// Événements
export type LaunchStateName = "preparing" | "running" | "exited" | "failed";
export interface LaunchState { id: string; state: LaunchStateName; message?: string }
export interface PresenceEvent { public_key: string; presence: Presence }

/** Événement transfer://unknown_mods : mods reçus mais absents des catalogues. */
export interface UnknownModsEvent { instance_id: string; unknown_mods: string[] }

/** Événement transfer://error_details : échec d'une étape d'installation invitée (avant la phase `failed`). */
export interface TransferErrorDetails { step: string; message: string; details: string; log_file: string }

/** Événement launch://log : une ligne de sortie du jeu. */
export interface LaunchLogEvent { id: string; line: string }
/** État du partage hôte (événement host://state, commande host_state). world_port = port du monde LAN détecté, null tant que le monde n'est pas ouvert au réseau local. */
export interface HostState { instance_id: string; sharing: boolean; world_port: number | null }

/** Événement import://warning : mods introuvables lors d'un import. */
export interface ImportWarningEvent { missing: string[] }

export type BrowseSource = "curseforge" | "modrinth";
export type BrowseKind = "modpack" | "mod" | "resourcepack" | "shader";
export interface BrowseQuery { source: BrowseSource; kind: BrowseKind; query: string; minecraft?: string; loader?: string; sort?: "relevance" | "downloads" | "updated"; offset: number; limit: number }
export interface BrowseHit { source: BrowseSource; id: string; kind: BrowseKind; name: string; summary: string; author: string; icon_url: string | null; downloads: number; page_url: string; downloadable: boolean }
export interface BrowsePage { hits: BrowseHit[]; total: number }
export interface BrowseVersion { id: string; name: string; minecraft: string[]; loaders: string[]; published: string; downloadable: boolean }
export interface BrowseInstallInput { source: BrowseSource; id: string; kind: BrowseKind; version_id?: string; instance_id?: string }
export interface BrowseInstallResult { instance: Instance; installed: string[]; missing: string[] }
export interface ModEntry { file: string; enabled: boolean; size: number; custom: boolean }
export interface BrowseProgressEvent { done: number; total: number; label: string }
export interface PickFilesOptions { extensions: string[]; multiple?: boolean }

export type Unsubscribe = () => void;

/** Une méthode par commande (rejet = message d'erreur en français), un abonnement par événement. */
export type UpdatePhase = "idle" | "checking" | "up_to_date" | "available" | "downloading" | "ready" | "error";

/** État de la mise à jour automatique du launcher (octets pour downloaded/total, message d'erreur en français). */
export interface UpdateState {
  phase: UpdatePhase;
  /** Version installée. */
  current: string;
  /** Version proposée. */
  version: string | null;
  notes: string | null;
  downloaded: number;
  total: number | null;
  message: string | null;
}

export interface Services {
  authStatus(): Promise<Profile | null>;
  authBeginLogin(): Promise<DeviceCode>;
  authPollLogin(): Promise<Profile>;
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
  instancesArtLookup(instanceId: string): Promise<Instance>;
  minecraftVersions(): Promise<string[]>;

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
  overlayToggle(): Promise<void>;
  overlayState(): Promise<OverlayState>;
  /** Ferme le jeu en cours puis lance l'instance. */
  gameSwitch(instanceId: string): Promise<void>;
  /** Ferme le jeu si besoin puis rejoint l'ami (plan, transfert, jeu). */
  gameJoinFriend(publicKey: string, instanceId?: string): Promise<void>;

  joinPlan(friendPublicKey: string): Promise<JoinPlan>;
  joinStart(friendPublicKey: string): Promise<void>;
  /** Invité, en un geste : ouvre le tunnel vers le monde de l'ami, lance l'instance invitée avec connexion automatique à 127.0.0.1:port, referme le tunnel à la fin du jeu. Suit les événements de lancement habituels. */
  joinPlay(friendPublicKey: string, instanceId: string): Promise<void>;
  joinPause(): Promise<void>;
  joinResume(): Promise<void>;
  joinCancel(): Promise<void>;

  settingsGet(): Promise<Settings>;
  settingsSet(settings: Settings): Promise<Settings>;

  browseSearch(q: BrowseQuery): Promise<BrowsePage>;
  browseVersions(source: BrowseSource, id: string, kind: BrowseKind, minecraft?: string, loader?: string): Promise<BrowseVersion[]>;
  browseInstall(input: BrowseInstallInput): Promise<BrowseInstallResult>;
  modsList(instanceId: string): Promise<ModEntry[]>;
  modsToggle(instanceId: string, file: string, enabled: boolean): Promise<void>;
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
  onOverlayState(cb: (s: OverlayState) => void): Unsubscribe;
  onUnknownMods(cb: (e: UnknownModsEvent) => void): Unsubscribe;
  onTransferErrorDetails(cb: (e: TransferErrorDetails) => void): Unsubscribe;
  onBrowseProgress(cb: (e: BrowseProgressEvent) => void): Unsubscribe;
  /** Fichiers déposés sur la fenêtre (glisser-déposer natif). */
  onFilesDropped(cb: (paths: string[]) => void): Unsubscribe;
  updateState(): Promise<UpdateState>;
  /** Vérification manuelle ; si une version existe, le téléchargement démarre en arrière-plan. */
  updateCheck(): Promise<UpdateState>;
  /** Redémarre et installe la mise à jour téléchargée (rejette si une partie est en cours). */
  updateInstall(): Promise<void>;
  onUpdateState(cb: (s: UpdateState) => void): Unsubscribe;
}
