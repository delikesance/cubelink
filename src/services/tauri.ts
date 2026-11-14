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
    offlineAvailable: async () => true,
    authAccounts: () => invoke("auth_accounts"),
    authSwitch: (id) => invoke("auth_switch", { id }),
    authSkinHead: (uuid) => invoke("auth_skin_head", { uuid }),
    skinHeadByName: (name) => invoke("skin_head_by_name", { name }),
    authRemove: (id) => invoke("auth_remove", { id }),

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
    instancesArtLookup: (instanceId, auto) => invoke("instances_art_lookup", { instance_id: instanceId, auto: auto ?? false }),
    minecraftVersions: () => invoke("minecraft_versions"),

    netTest: () => invoke("net_test"),
    friendMyCode: () => invoke("friend_my_code"),
    friendRegenerateCode: () => invoke("friend_regenerate_code"),
    friendList: () => invoke("friend_list"),
    friendSendRequest: (code) => invoke("friend_send_request", { code }),
    friendRequests: (q) => invoke("friend_requests", { ...q }),
    friendAccept: (public_key) => invoke("friend_accept", { public_key }),
    friendRefuse: (public_key) => invoke("friend_refuse", { public_key }),
    friendRefuseAll: () => invoke("friend_refuse_all"),
    friendInvite: (public_key) => invoke("friend_invite", { public_key }),
    friendInvites: () => invoke("friend_invites"),
    friendInviteDismiss: (id) => invoke("friend_invite_dismiss", { id }),
    friendJoinRequest: (public_key) => invoke("friend_join_request", { public_key }),
    friendJoinRequestAnswer: (public_key, accept) => invoke("friend_join_request_answer", { public_key, accept }),
    friendJoinRequests: () => invoke("friend_join_requests"),
    gameSwitch: (instance_id) => invoke("game_switch", { instance_id }),
    gameCurrent: () => invoke("game_current"),
    gameClose: () => invoke("game_close"),
    gameJoinFriend: (public_key, instance_id) => invoke("game_join_friend", { public_key, instance_id }),

    joinPlan: (friend_public_key) => invoke("join_plan", { friend_public_key }),
    joinStart: (friend_public_key, manifest_hash, accept_unknown, ram_mb) =>
      invoke("join_start", { friend_public_key, manifest_hash, accept_unknown, ram_mb: ram_mb || null }),
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
    browseCheck: (source, id, kind, instance_id, version_id) => invoke("browse_check", { source, id, kind, instance_id, version_id: version_id ?? null }),
    modsList: (instance_id) => invoke("mods_list", { instance_id }),
    modsIcon: (instance_id, file) => invoke("mods_icon", { instance_id, file }),
    modsCheck: (instance_id) => invoke("mods_check", { instance_id }),
    instancesDescription: (instance_id) => invoke("instances_description", { instance_id }),
    modsToggle: (instance_id, file, enabled) => invoke("mods_toggle", { instance_id, file, enabled }),
    modsFixPlan: (instance_id) => invoke("mods_fix_plan", { instance_id }),
    modsFixApply: (instance_id, plan_id, action_ids) => invoke("mods_fix_apply", { instance_id, plan_id, action_ids }),
    modsFixUndo: (instance_id, backup_id) => invoke("mods_fix_undo", { instance_id, backup_id }),
    modsRemove: (instance_id, file) => invoke("mods_remove", { instance_id, file }),
    modsAdd: (instance_id, path) => invoke("mods_add", { instance_id, path }),
    pickFiles: async ({ extensions, multiple }) => {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const r = await open({ multiple: !!multiple, directory: false, filters: [{ name: "Fichiers", extensions }] });
      return r == null ? [] : Array.isArray(r) ? r : [r];
    },

    onAuthState: (cb) => on("auth://state", cb),
    onAccountsState: (cb) => on("auth://accounts", cb),
    onLaunchState: (cb) => on("launch://state", cb),
    onLaunchLog: (cb) => on("launch://log", cb),
    onImportWarning: (cb) => on("import://warning", cb),
    hostState: () => invoke("host_state"),
    onHostState: (cb) => on("host://state", cb),
    onTransferProgress: (cb) => on("transfer://progress", cb),
    onFriendPresence: (cb) => on("friend://presence", cb),
    onFriendRequest: (cb) => on("friend://request", cb),
    onFriendInvite: (cb) => on("friend://invite", cb),
    onFriendInvites: (cb) => on("friend://invites", cb),
    onFriendJoinRequest: (cb) => on("friend://join_request", cb),
    onFriendJoinRequests: (cb) => on("friend://join_requests", cb),
    hearthList: () => invoke("hearth_list"),
    hearthDetail: (id) => invoke("hearth_detail", { id }),
    hearthWorlds: (instanceId) => invoke("hearth_worlds", { instance_id: instanceId }),
    hearthCreate: (i) => invoke("hearth_create", { name: i.name, instance_id: i.instanceId, world: i.world, members: i.members }),
    hearthInvites: () => invoke("hearth_invites"),
    hearthInviteAccept: (id) => invoke("hearth_invite_accept", { id }),
    hearthInviteDismiss: (id) => invoke("hearth_invite_dismiss", { id }),
    hearthPlay: (id) => invoke("hearth_play", { id }),
    hearthStop: (id) => invoke("hearth_stop", { id }),
    hearthUncleanChoice: (id, choice) => invoke("hearth_unclean_choice", { id, choice }),
    hearthSetInstance: (id, instanceId) => invoke("hearth_set_instance", { id, instance_id: instanceId }),
    hearthAddMember: (id, publicKey) => invoke("hearth_add_member", { id, public_key: publicKey }),
    hearthRemoveMember: (id, publicKey) => invoke("hearth_remove_member", { id, public_key: publicKey }),
    hearthLeave: (id) => invoke("hearth_leave", { id }),
    hearthVote: (id, proposalId) => invoke("hearth_vote", { id, proposal_id: proposalId }),
    hearthContinueWithout: (id, publicKey) => invoke("hearth_continue_without", { id, public_key: publicKey }),
    hearthPackUpdate: (id, instanceId) => invoke("hearth_pack_update", { id, instance_id: instanceId }),
    hearthRestore: (id, versionId) => invoke("hearth_restore", { id, version_id: versionId }),
    hearthResolveFork: (id, versionId) => invoke("hearth_resolve_fork", { id, version_id: versionId }),
    hearthSetBackground: (id, on) => invoke("hearth_set_background", { id, on }),
    hearthEula: () => invoke("hearth_eula"),
    hearthEulaAccept: () => invoke("hearth_eula_accept"),
    hearthEnabled: () => invoke("hearth_enabled"),
    hearthSetEnabled: (enabled) => invoke("hearth_set_enabled", { enabled }),
    onHearthState: (cb) => on("hearth://state", cb),
    onHearthInvites: (cb) => on("hearth://invites", cb),
    onHearthHandover: (cb) => on("hearth://handover", cb),
    onHearthHostPrompt: (cb) => on("hearth://host_prompt", cb),
    onUnknownMods: (cb) => on("transfer://unknown_mods", cb),
    onTransferErrorDetails: (cb) => on("transfer://error_details", cb),
    updateState: () => invoke("update_state"),
    updateCheck: () => invoke("update_check"),
    updateInstall: () => invoke("update_install"),
    onUpdateState: (cb) => on("update://state", cb),
    onBrowseProgress: (cb) => on("browse://progress", cb),
    onModsFixProgress: (cb) => on("mods-fix://progress", cb),
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
