import { listen } from "@tauri-apps/api/event";
import { commands } from "./bindings";
import type { HearthDetail, Services, Unsubscribe } from "./types";

// Commandes : fonctions générées depuis Rust (`bindings.ts`, `npm run bindings`), donc noms, arguments et types de retour
// vérifiés par tsc. Une erreur Rust (`Result<T, String>`) rejette la promesse avec le message en français.
// Les événements restent écoutés par leur nom (docs/contrats.md).

/** Les commandes sans valeur de retour résolvent `null` côté Tauri : `Services` les expose en `Promise<void>`. */
async function done(p: Promise<unknown>): Promise<void> {
  await p;
}

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
    authStatus: () => commands.authStatus(),
    authBeginLogin: () => commands.authBeginLogin(),
    authPollLogin: () => commands.authPollLogin(),
    authCancelLogin: () => commands.authCancelLogin(),
    authLogout: () => done(commands.authLogout()),
    authOfflineLogin: (name) => commands.authOfflineLogin(name),
    offlineAvailable: async () => true,
    authAccounts: () => commands.authAccounts(),
    authSwitch: (id) => commands.authSwitch(id),
    authSkinHead: (uuid) => commands.authSkinHead(uuid),
    skinHeadByName: (name) => commands.skinHeadByName(name),
    authRemove: (id) => done(commands.authRemove(id)),

    instancesList: () => commands.instancesList(),
    instancesCreate: (i) => commands.instancesCreate(i.name, i.minecraft, i.loader, i.ram_mb),
    instancesImportMrpack: (path, t) =>
      commands.instancesImportMrpack(path, t?.name ?? null, t?.minecraft ?? null, t?.loader ?? null, t?.ram_mb ?? null),
    instancesLaunch: (id) => done(commands.instancesLaunch(id)),
    instancesKill: (id) => done(commands.instancesKill(id)),
    instancesDelete: (id) => done(commands.instancesDelete(id)),
    instancesUpdate: (id, p) =>
      commands.instancesUpdate(id, p.name ?? null, p.ram_mb ?? null, p.java_path ?? null, p.jvm_args ?? null, p.wrapper ?? null),
    instancesOpenFolder: (id) => done(commands.instancesOpenFolder(id)),
    instancesLogs: (id) => commands.instancesLogs(id),
    instancesArtImage: (instanceId, file, maxPx) => commands.instancesArtImage(instanceId, file, maxPx ?? null),
    instancesArtSet: (instanceId, path, role) => commands.instancesArtSet(instanceId, path, role),
    instancesArtLookup: (instanceId, auto) => commands.instancesArtLookup(instanceId, auto ?? false),
    minecraftVersions: () => commands.minecraftVersions(),

    netTest: () => commands.netTest(),
    friendMyCode: () => commands.friendMyCode(),
    friendRegenerateCode: () => commands.friendRegenerateCode(),
    friendList: () => commands.friendList(),
    friendSendRequest: (code) => commands.friendSendRequest(code),
    friendRequests: (q) => commands.friendRequests(q.offset, q.limit, q.query ?? null),
    friendAccept: (publicKey) => done(commands.friendAccept(publicKey)),
    friendRefuse: (publicKey) => done(commands.friendRefuse(publicKey)),
    friendRefuseAll: () => commands.friendRefuseAll(),
    friendInvite: (publicKey) => done(commands.friendInvite(publicKey)),
    friendInvites: () => commands.friendInvites(),
    friendInviteDismiss: (id) => done(commands.friendInviteDismiss(id)),
    friendJoinRequest: (publicKey) => commands.friendJoinRequest(publicKey),
    friendJoinRequestAnswer: (publicKey, accept) => done(commands.friendJoinRequestAnswer(publicKey, accept)),
    friendJoinRequests: () => commands.friendJoinRequests(),
    gameSwitch: (instanceId) => done(commands.gameSwitch(instanceId)),
    gameCurrent: () => commands.gameCurrent(),
    gameClose: () => done(commands.gameClose()),
    gameJoinFriend: (publicKey, instanceId) => done(commands.gameJoinFriend(publicKey, instanceId ?? null)),

    joinPlan: (friendPublicKey) => commands.joinPlan(friendPublicKey),
    joinStart: (friendPublicKey, manifestHash, acceptUnknown, ramMb) =>
      done(commands.joinStart(friendPublicKey, ramMb || null, manifestHash, acceptUnknown)),
    joinPlay: (friendPublicKey, instanceId) => done(commands.joinPlay(friendPublicKey, instanceId)),
    joinPause: () => done(commands.joinPause()),
    joinResume: () => done(commands.joinResume()),
    joinCancel: () => done(commands.joinCancel()),

    settingsGet: () => commands.settingsGet(),
    settingsSet: (settings) => commands.settingsSet(settings),

    browseSearch: (query) => commands.browseSearch(query),
    browseVersions: (source, id, kind, minecraft, loader) =>
      commands.browseVersions(source, id, kind, minecraft ?? null, loader ?? null),
    browseInstall: (input) => commands.browseInstall(input),
    browseCheck: (source, id, kind, instanceId, versionId) =>
      commands.browseCheck(source, id, kind, instanceId, versionId ?? null),
    modsList: (instanceId) => commands.modsList(instanceId),
    modsIcon: (instanceId, file) => commands.modsIcon(instanceId, file),
    modsCheck: (instanceId) => commands.modsCheck(instanceId),
    instancesDescription: (instanceId) => commands.instancesDescription(instanceId),
    modsToggle: (instanceId, file, enabled) => done(commands.modsToggle(instanceId, file, enabled)),
    modsFixPlan: (instanceId) => commands.modsFixPlan(instanceId),
    modsFixApply: (instanceId, planId, actionIds) => commands.modsFixApply(instanceId, planId, actionIds),
    modsFixUndo: (instanceId, backupId) => commands.modsFixUndo(instanceId, backupId),
    modsRemove: (instanceId, file) => done(commands.modsRemove(instanceId, file)),
    modsAdd: (instanceId, path) => commands.modsAdd(instanceId, path),
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
    hostState: () => commands.hostState(),
    onHostState: (cb) => on("host://state", cb),
    onTransferProgress: (cb) => on("transfer://progress", cb),
    onFriendPresence: (cb) => on("friend://presence", cb),
    onFriendRequest: (cb) => on("friend://request", cb),
    onFriendInvite: (cb) => on("friend://invite", cb),
    onFriendInvites: (cb) => on("friend://invites", cb),
    onFriendJoinRequest: (cb) => on("friend://join_request", cb),
    onFriendJoinRequests: (cb) => on("friend://join_requests", cb),
    hearthList: () => commands.hearthList(),
    // Champs à valeurs fixes resserrés dans `types.ts` (Rust les envoie en `String`).
    hearthDetail: (id) => commands.hearthDetail(id) as Promise<HearthDetail>,
    hearthWorlds: (instanceId) => commands.hearthWorlds(instanceId),
    hearthCreate: (i) => commands.hearthCreate(i.name, i.instanceId, i.world, i.members),
    hearthInvites: () => commands.hearthInvites(),
    hearthInviteAccept: (id) => done(commands.hearthInviteAccept(id)),
    hearthInviteDismiss: (id) => done(commands.hearthInviteDismiss(id)),
    hearthPlay: (id) => done(commands.hearthPlay(id)),
    hearthStop: (id) => done(commands.hearthStop(id)),
    hearthUncleanChoice: (id, choice) => done(commands.hearthUncleanChoice(id, choice)),
    hearthSetInstance: (id, instanceId) => done(commands.hearthSetInstance(id, instanceId)),
    hearthAddMember: (id, publicKey) => done(commands.hearthAddMember(id, publicKey)),
    hearthRemoveMember: (id, publicKey) => done(commands.hearthRemoveMember(id, publicKey)),
    hearthLeave: (id) => done(commands.hearthLeave(id)),
    hearthForget: (id) => done(commands.hearthForget(id)),
    hearthVote: (id, proposalId) => done(commands.hearthVote(id, proposalId)),
    hearthContinueWithout: (id, publicKey) => done(commands.hearthContinueWithout(id, publicKey)),
    hearthPackUpdate: (id, instanceId) => done(commands.hearthPackUpdate(id, instanceId)),
    hearthRestore: (id, versionId) => done(commands.hearthRestore(id, versionId)),
    hearthResolveFork: (id, versionId) => done(commands.hearthResolveFork(id, versionId)),
    hearthSetBackground: (id, on) => done(commands.hearthSetBackground(id, on)),
    hearthEula: () => commands.hearthEula(),
    hearthEulaAccept: () => commands.hearthEulaAccept(),
    hearthEnabled: () => commands.hearthEnabled(),
    hearthSetEnabled: (enabled) => commands.hearthSetEnabled(enabled),
    onHearthState: (cb) => on("hearth://state", cb),
    onHearthInvites: (cb) => on("hearth://invites", cb),
    onHearthHandover: (cb) => on("hearth://handover", cb),
    onHearthHostPrompt: (cb) => on("hearth://host_prompt", cb),
    onUnknownMods: (cb) => on("transfer://unknown_mods", cb),
    onTransferErrorDetails: (cb) => on("transfer://error_details", cb),
    updateState: () => commands.updateState(),
    updateCheck: () => commands.updateCheck(),
    updateInstall: () => done(commands.updateInstall()),
    onUpdateState: (cb) => on("update://state", cb),
    onBrowseProgress: (cb) => on("browse://progress", cb),
    onModsFixProgress: (cb) => on("mods-fix://progress", cb),
    stageSupported: () => commands.stageSupported(),
    stageState: () => commands.stageState(),
    stageView: (game) => done(commands.stageView(game)),
    stagePanel: (open) => done(commands.stagePanel(open)),
    stageLayout: (l) => done(commands.stageLayout(l.x, l.y, l.w, l.h, l.panel_w)),
    setFullscreen: async (on) => {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      await getCurrentWindow().setFullscreen(on);
    },
    onStageState: (cb) => on("stage://state", cb),
    onStageFallback: (cb) => on("stage://fallback", cb),
    onStageFullscreen: (cb) => on("stage://fullscreen", cb),
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
