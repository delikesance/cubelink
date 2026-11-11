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
    // `test_state` n'est enregistrée qu'en mode test : son absence = pas de connexion hors ligne.
    offlineAvailable: () => invoke("test_state").then(() => true, () => false),

    instancesList: () => invoke("instances_list"),
    instancesCreate: (input) => invoke("instances_create", { ...input }),
    instancesImportMrpack: (path, target) => invoke("instances_import_mrpack", { path, ...target }),
    instancesLaunch: (id) => invoke("instances_launch", { id }),
    instancesKill: (id) => invoke("instances_kill", { id }),
    instancesDelete: (id) => invoke("instances_delete", { id }),
    instancesUpdate: (id, patch) => invoke("instances_update", { id, ...patch }),
    instancesOpenFolder: (id) => invoke("instances_open_folder", { id }),
    instancesLogs: (id) => invoke("instances_logs", { id }),
    instancesArtImage: (instanceId, file, maxPx) => invoke("instances_art_image", { instance_id: instanceId, file, max_px: maxPx ?? null }),
    instancesArtSet: (instanceId, path, role) => invoke("instances_art_set", { instance_id: instanceId, path, role }),
    instancesArtLookup: (instanceId) => invoke("instances_art_lookup", { instance_id: instanceId }),
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
    joinPlay: (friend_public_key, instance_id) => invoke("join_play", { friend_public_key, instance_id }),
    joinPause: () => invoke("join_pause"),
    joinResume: () => invoke("join_resume"),
    joinCancel: () => invoke("join_cancel"),

    settingsGet: () => invoke("settings_get"),
    settingsSet: (settings) => invoke("settings_set", { ...settings }),

    browseSearch: (query) => invoke("browse_search", { query }),
    browseVersions: (source, id, kind, minecraft, loader) =>
      invoke("browse_versions", { source, id, kind, minecraft, loader }),
    browseInstall: (input) => invoke("browse_install", { input }),
    modsList: (instance_id) => invoke("mods_list", { instance_id }),
    modsToggle: (instance_id, file, enabled) => invoke("mods_toggle", { instance_id, file, enabled }),
    modsRemove: (instance_id, file) => invoke("mods_remove", { instance_id, file }),
    modsAdd: (instance_id, path) => invoke("mods_add", { instance_id, path }),
    pickFiles: async ({ extensions, multiple }) => {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const r = await open({ multiple: !!multiple, directory: false, filters: [{ name: "Fichiers", extensions }] });
      return r == null ? [] : Array.isArray(r) ? r : [r];
    },

    onAuthState: (cb) => on("auth://state", cb),
    onLaunchState: (cb) => on("launch://state", cb),
    onLaunchLog: (cb) => on("launch://log", cb),
    onImportWarning: (cb) => on("import://warning", cb),
    hostState: () => invoke("host_state"),
    onHostState: (cb) => on("host://state", cb),
    onTransferProgress: (cb) => on("transfer://progress", cb),
    onFriendPresence: (cb) => on("friend://presence", cb),
    onFriendRequest: (cb) => on("friend://request", cb),
    onUnknownMods: (cb) => on("transfer://unknown_mods", cb),
    onTransferErrorDetails: (cb) => on("transfer://error_details", cb),
    updateState: () => invoke("update_state"),
    updateCheck: () => invoke("update_check"),
    updateInstall: () => invoke("update_install"),
    onUpdateState: (cb) => on("update://state", cb),
    onBrowseProgress: (cb) => on("browse://progress", cb),
    onFilesDropped: (cb) => {
      let off: (() => void) | null = null;
      let cancelled = false;
      void import("@tauri-apps/api/webview").then(({ getCurrentWebview }) =>
        getCurrentWebview().onDragDropEvent((e) => {
          if (e.payload.type === "drop") cb(e.payload.paths);
        }),
      ).then((fn) => { if (cancelled) fn(); else off = fn; }).catch(() => {});
      return () => { cancelled = true; off?.(); };
    },
  };
}
