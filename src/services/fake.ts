import type {
  CreateInstanceInput, DeviceCode, FriendRequest, FriendWithPresence, Instance,
  JoinPlan, LaunchState, Manifest, PresenceEvent, UnknownModsEvent, Profile, Services, Settings,
  TransferProgress, Unsubscribe,
} from "./types";

export interface FakeOptions {
  /** Durée d'un pas de simulation (transfert, lancement). 0 = aucune minuterie. */
  tickMs?: number;
  /** Délai de validation du faux login. */
  loginDelayMs?: number;
  /** Nombre de demandes d'amis initiales. */
  requestCount?: number;
}

const NAMES = ["Alex", "Steve", "Noor", "Mika", "Zoe", "Liam", "Inès", "Hugo", "Lola", "Ravi"];
const NOW = 1_760_000_000;

function hex(seed: number, len: number): string {
  let s = seed >>> 0 || 1;
  let out = "";
  while (out.length < len) {
    s = (Math.imul(s, 1664525) + 1013904223) >>> 0;
    out += s.toString(16).padStart(8, "0");
  }
  return out.slice(0, len);
}

function verifyCode(seed: number): string {
  const h = hex(seed, 16).toUpperCase();
  return [0, 4, 8, 12].map((i) => h.slice(i, i + 4)).join(" · ");
}

function makeRequest(i: number): FriendRequest {
  return {
    public_key: hex(1000 + i, 64),
    name: `${NAMES[i % NAMES.length]}${i}`,
    verify_code: verifyCode(i + 1),
    received_at: NOW - i * 60,
  };
}

function makeManifest(): Manifest {
  const files = Array.from({ length: 94 }, (_, i) => ({
    path: `mods/mod-${String(i + 1).padStart(2, "0")}.jar`,
    size: 1_000_000 + i * 10_000,
    sha256: hex(i + 1, 64),
  }));
  return {
    name: "Create: Aeronautics",
    minecraft: "1.21.1",
    loader: { loader: "neoforge", version: "21.1.77" },
    java_major: 21,
    files,
  };
}

export function createFakeServices(opts: FakeOptions = {}): Services {
  const tickMs = opts.tickMs ?? 250;
  const loginDelayMs = opts.loginDelayMs ?? 1200;
  const requestCount = opts.requestCount ?? 1000;

  let profile: Profile | null = null;
  let codeN = 1;
  let settings: Settings = {
    language: "fr", default_ram_mb: 4096, java_path: null,
    trust_default: "ask", allow_relay: true, close_on_launch: false, relay_only: false, share_mods: true,
  };
  const instances: Instance[] = [{
    id: "inst-aeronautics", name: "Create: Aeronautics", minecraft: "1.21.1",
    loader: { loader: "neoforge", version: "21.1.77" }, ram_mb: 6144, mod_count: 94,
  }];
  let requests: FriendRequest[] = Array.from({ length: requestCount }, (_, i) => makeRequest(i));
  const friends: FriendWithPresence[] = [
    { friend: { public_key: hex(1, 64), name: "Alex", trust: "ask", added_at: NOW - 86400 }, presence: "online" },
    { friend: { public_key: hex(2, 64), name: "Noor", trust: "always", added_at: NOW - 172800 }, presence: "offline" },
  ];

  const listeners = {
    auth: new Set<(p: Profile | null) => void>(),
    launch: new Set<(s: LaunchState) => void>(),
    transfer: new Set<(p: TransferProgress) => void>(),
    presence: new Set<(e: PresenceEvent) => void>(),
    request: new Set<(r: FriendRequest) => void>(),
    unknown: new Set<(e: UnknownModsEvent) => void>(),
  };
  function sub<T>(set: Set<(v: T) => void>, cb: (v: T) => void): Unsubscribe {
    set.add(cb);
    return () => { set.delete(cb); };
  }
  function emit<T>(set: Set<(v: T) => void>, v: T) { set.forEach((cb) => cb(v)); }
  const setProfile = (p: Profile | null) => { profile = p; emit(listeners.auth, p); };

  // --- transfert simulé ---
  const TOTAL_STEPS = 10;
  const files_total = 3, bytes_total = 3_000_000;
  let step = 0;
  let timer: ReturnType<typeof setInterval> | null = null;
  function progress(phase: TransferProgress["phase"]): TransferProgress {
    const ratio = phase === "done" ? 1 : step / TOTAL_STEPS;
    return {
      phase,
      files_done: Math.floor(ratio * files_total), files_total,
      bytes_done: Math.floor(ratio * bytes_total), bytes_total,
      speed_bps: phase === "receiving" ? 1_500_000 : 0, via_relay: false,
    };
  }
  function stopTimer() { if (timer) { clearInterval(timer); timer = null; } }
  function startTimer() {
    stopTimer();
    if (tickMs <= 0) return;
    timer = setInterval(() => {
      step += 1;
      if (step >= TOTAL_STEPS) {
        stopTimer();
        emit(listeners.transfer, progress("done"));
      } else {
        emit(listeners.transfer, progress("receiving"));
      }
    }, tickMs);
  }

  return {
    authStatus: async () => profile,
    authBeginLogin: async (): Promise<DeviceCode> => ({
      user_code: "AB12CD34", verification_uri: "https://www.microsoft.com/link", expires_in: 900,
    }),
    authPollLogin: () => new Promise<Profile>((resolve) => {
      const done = () => {
        const p: Profile = { kind: "online", name: "Steve", uuid: "8667ba71-b85a-4004-af54-457a9734eed7" };
        setProfile(p);
        resolve(p);
      };
      if (loginDelayMs <= 0) done(); else setTimeout(done, loginDelayMs);
    }),
    authLogout: async () => { setProfile(null); },
    authOfflineLogin: async (name) => {
      const n = name.trim();
      if (!n) throw new Error("Le pseudo est obligatoire.");
      const p: Profile = { kind: "offline", name: n, uuid: `00000000-0000-3000-8000-${hex(n.length, 12)}` };
      setProfile(p);
      return p;
    },

    instancesList: async () => instances.map((i) => ({ ...i })),
    instancesCreate: async (input: CreateInstanceInput) => {
      if (!input.name.trim()) throw new Error("Le nom de l'instance est obligatoire.");
      const inst: Instance = { id: `inst-${instances.length + 1}`, ...input, mod_count: 0 };
      instances.push(inst);
      return { ...inst };
    },
    instancesImportMrpack: async (path) => {
      if (!path.endsWith(".mrpack")) throw new Error("Fichier .mrpack attendu.");
      const inst: Instance = {
        id: `inst-${instances.length + 1}`, name: "Pack importé", minecraft: "1.21.1",
        loader: { loader: "fabric", version: "0.16.5" }, ram_mb: 4096, mod_count: 12,
      };
      instances.push(inst);
      return { ...inst };
    },
    instancesLaunch: async (id) => {
      if (!instances.some((i) => i.id === id)) throw new Error("Instance introuvable.");
      emit(listeners.launch, { id, state: "preparing" });
      if (tickMs > 0) setTimeout(() => emit(listeners.launch, { id, state: "running" }), tickMs);
    },
    minecraftVersions: async () => ["1.21.4", "1.21.1", "1.20.6", "1.20.1", "1.19.4", "1.18.2"],

    friendMyCode: async () => `mcl1-${hex(codeN * 7, 24)}`,
    friendRegenerateCode: async () => { codeN += 1; return `mcl1-${hex(codeN * 7, 24)}`; },
    friendList: async () => friends.map((f) => ({ friend: { ...f.friend }, presence: f.presence })),
    friendSendRequest: async (code) => {
      if (!/^mcl1-[0-9a-f]{8,}$/i.test(code.trim())) throw new Error("Code ami invalide.");
      return verifyCode(code.length);
    },
    friendRequests: async ({ offset, limit, query }) => {
      const q = query?.trim().toLowerCase();
      const all = q ? requests.filter((r) => r.name.toLowerCase().includes(q)) : requests;
      return { items: all.slice(offset, offset + limit), total: all.length };
    },
    friendAccept: async (pk) => {
      const r = requests.find((x) => x.public_key === pk);
      if (!r) throw new Error("Demande introuvable.");
      requests = requests.filter((x) => x !== r);
      friends.push({
        friend: { public_key: r.public_key, name: r.name, trust: "ask", added_at: NOW },
        presence: "offline",
      });
    },
    friendRefuse: async (pk) => {
      if (!requests.some((x) => x.public_key === pk)) throw new Error("Demande introuvable.");
      requests = requests.filter((x) => x.public_key !== pk);
    },
    friendRefuseAll: async () => { const n = requests.length; requests = []; return n; },

    joinPlan: async (): Promise<JoinPlan> => ({
      manifest: makeManifest(), cached: 91, to_receive: 3, to_receive_bytes: bytes_total,
      unknown_mods: ["mods/mod-maison.jar"],
    }),
    joinStart: async () => {
      step = 0;
      emit(listeners.transfer, progress("preparing"));
      startTimer();
    },
    joinPause: async () => { stopTimer(); emit(listeners.transfer, progress("paused")); },
    joinResume: async () => { emit(listeners.transfer, progress("receiving")); startTimer(); },
    joinCancel: async () => { stopTimer(); step = 0; emit(listeners.transfer, progress("failed")); },

    settingsGet: async () => ({ ...settings }),
    settingsSet: async (s) => { settings = { ...s }; return { ...settings }; },

    onAuthState: (cb) => sub(listeners.auth, cb),
    onLaunchState: (cb) => sub(listeners.launch, cb),
    onTransferProgress: (cb) => sub(listeners.transfer, cb),
    onFriendPresence: (cb) => sub(listeners.presence, cb),
    onFriendRequest: (cb) => sub(listeners.request, cb),
    onUnknownMods: (cb) => sub(listeners.unknown, cb),
  };
}
