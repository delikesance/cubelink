import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Services, Unsubscribe } from "./types";

// Les erreurs Tauri arrivent en chaîne (Result<T, String>) : on les relaie telles quelles.
// Les arguments sont transmis avec les noms du contrat (docs/contrats.md).
function on<T>(event: string, cb: (p: T) => void): Unsubscribe {
  let off: (() => void) | null = null;
  let cancelled = false;
  void listen<T>(event, (e) => cb(e.payload)).then((fn) => {
    if (cancelled) fn();
    else off = fn;
  });
  return () => {
    cancelled = true;
    off?.();
  };
}

export function createTauriServices(): Services {
  return {
    authStatus: () => invoke("auth_status"),
    authBeginLogin: () => invoke("auth_begin_login"),
    authPollLogin: () => invoke("auth_poll_login"),
    authLogout: () => invoke("auth_logout"),
    authOfflineLogin: (name) => invoke("auth_offline_login", { name }),

    instancesList: () => invoke("instances_list"),
    instancesCreate: (input) => invoke("instances_create", { ...input }),
    instancesImportMrpack: (path) => invoke("instances_import_mrpack", { path }),
    instancesLaunch: (id) => invoke("instances_launch", { id }),
    minecraftVersions: () => invoke("minecraft_versions"),

    friendMyCode: () => invoke("friend_my_code"),
    friendRegenerateCode: () => invoke("friend_regenerate_code"),
    friendList: () => invoke("friend_list"),
    friendSendRequest: (code) => invoke("friend_send_request", { code }),
    friendRequests: (q) => invoke("friend_requests", { ...q }),
    friendAccept: (public_key) => invoke("friend_accept", { public_key }),
    friendRefuse: (public_key) => invoke("friend_refuse", { public_key }),
    friendRefuseAll: () => invoke("friend_refuse_all"),

    joinPlan: (friend_public_key) => invoke("join_plan", { friend_public_key }),
    joinStart: (friend_public_key) => invoke("join_start", { friend_public_key }),
    joinPause: () => invoke("join_pause"),
    joinResume: () => invoke("join_resume"),
    joinCancel: () => invoke("join_cancel"),

    settingsGet: () => invoke("settings_get"),
    settingsSet: (settings) => invoke("settings_set", { ...settings }),

    onAuthState: (cb) => on("auth://state", cb),
    onLaunchState: (cb) => on("launch://state", cb),
    onTransferProgress: (cb) => on("transfer://progress", cb),
    onFriendPresence: (cb) => on("friend://presence", cb),
    onFriendRequest: (cb) => on("friend://request", cb),
    onUnknownMods: (cb) => on("transfer://unknown_mods", cb),
  };
}
