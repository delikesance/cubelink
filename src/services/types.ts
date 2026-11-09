// Types miroirs de docs/contrats.md et core/src/types.rs (JSON, énumérations en minuscules).

export type Sha256Hex = string;
export type Loader = "vanilla" | "neoforge" | "fabric" | "quilt" | "forge";
export interface LoaderSpec { loader: Loader; version: string }

export type Profile =
  | { kind: "online"; name: string; uuid: string }
  | { kind: "offline"; name: string; uuid: string };

export interface Instance {
  id: string;
  name: string;
  minecraft: string;
  loader: LoaderSpec;
  ram_mb: number;
  mod_count: number;
}

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
}

// Entrées / sorties de commandes
export interface DeviceCode { user_code: string; verification_uri: string; expires_in: number }
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

export type Unsubscribe = () => void;

/** Une méthode par commande (rejet = message d'erreur en français), un abonnement par événement. */
export interface Services {
  authStatus(): Promise<Profile | null>;
  authBeginLogin(): Promise<DeviceCode>;
  authPollLogin(): Promise<Profile>;
  authLogout(): Promise<void>;
  authOfflineLogin(name: string): Promise<Profile>;

  instancesList(): Promise<Instance[]>;
  instancesCreate(input: CreateInstanceInput): Promise<Instance>;
  instancesImportMrpack(path: string): Promise<Instance>;
  instancesLaunch(id: string): Promise<void>;
  minecraftVersions(): Promise<string[]>;

  friendMyCode(): Promise<FriendCode>;
  friendRegenerateCode(): Promise<FriendCode>;
  friendList(): Promise<FriendWithPresence[]>;
  friendSendRequest(code: string): Promise<VerifyCode>;
  friendRequests(q: FriendRequestsQuery): Promise<FriendRequestsPage>;
  friendAccept(publicKey: string): Promise<void>;
  friendRefuse(publicKey: string): Promise<void>;
  friendRefuseAll(): Promise<number>;

  joinPlan(friendPublicKey: string): Promise<JoinPlan>;
  joinStart(friendPublicKey: string): Promise<void>;
  joinPause(): Promise<void>;
  joinResume(): Promise<void>;
  joinCancel(): Promise<void>;

  settingsGet(): Promise<Settings>;
  settingsSet(settings: Settings): Promise<Settings>;

  onAuthState(cb: (p: Profile | null) => void): Unsubscribe;
  onLaunchState(cb: (s: LaunchState) => void): Unsubscribe;
  onTransferProgress(cb: (p: TransferProgress) => void): Unsubscribe;
  onFriendPresence(cb: (e: PresenceEvent) => void): Unsubscribe;
  onFriendRequest(cb: (r: FriendRequest) => void): Unsubscribe;
  onUnknownMods(cb: (e: UnknownModsEvent) => void): Unsubscribe;
}
