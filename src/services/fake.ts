import { validateOfflineName } from "./accounts";
import type {
  AccountInfo, AccountsState, BrowseHit, BrowseInstallResult, BrowseKind, BrowseProgressEvent, BrowseSource, BrowseVersion, FixPlan, FixProgressEvent, FixResult, ModCheck, ModEntry, ModIssue,
  CreateInstanceInput, DeviceCode, HostState, Invite, ImportWarningEvent, LaunchLogEvent, FriendRequest, FriendWithPresence, Instance, InstanceArt,
  JoinPlan, LaunchState, Manifest, PresenceEvent, UnknownModsEvent, Profile, Services, Settings,
  TransferProgress, Unsubscribe, UpdateState,
} from "./types";

export interface FakeOptions {
  /** Durée d'un pas de simulation (transfert, lancement). 0 = aucune minuterie. */
  tickMs?: number;
  /** Délai de validation du faux login. */
  loginDelayMs?: number;
  /** Nombre de demandes d'amis initiales. */
  requestCount?: number;
  /** Valeur renvoyée par pickFiles (configurable aussi via setPickedFiles). */
  pickedFiles?: string[];
  /** Comptes présents au départ (le premier est actif). Vide par défaut : écran Connexion. */
  accounts?: AccountInfo[];
}

/** Commandes de test exposées par le faux service. */
export interface FakeControls {
  setPickedFiles(paths: string[]): void;
  /** Simule un glisser-déposer natif de fichiers sur la fenêtre. */
  dropFiles(paths: string[]): void;
  /** Test : émet un événement host://state. */
  emitHostState(e: HostState): void;
  /** Test : reçoit une invitation (friend://invite + friend://invites). */
  emitInvite(i: Partial<Invite> & { public_key: string }): Invite;
  /** Test : appels de commandes de jeu enregistrés (switch, join, invite). */
  readonly gameCalls: Array<{ kind: "switch" | "join" | "invite" | "close"; arg: string; instanceId?: string }>;
  /** Test : fusionne `patch` dans l'état de mise à jour et émet update://state (ex. available, downloading, ready). */
  emitUpdateState(patch: Partial<UpdateState>): void;
  /** Test : état (fusionné) auquel aboutit la prochaine updateCheck ; par défaut up_to_date. */
  setUpdateCheckResult(patch: Partial<UpdateState>): void;
}

const KIND_WORDS: Record<BrowseKind, string[]> = {
  modpack: ["Aventure", "Technologie", "Magie", "Skyblock", "Survie", "Exploration", "Industrie", "Médiéval"],
  mod: ["Sodium", "Iris", "Lithium", "Create", "Jade", "Xaero", "Farmers", "Biomes", "Backpacks", "Waystones"],
  resourcepack: ["Faithful", "Bare Bones", "Vanilla Tweaks", "Soartex", "Mizuno", "Stay True", "Default Dark", "Pixel"],
  shader: ["Complementary", "BSL", "Sildur", "Photon", "Rethinking Voxels", "Solas", "Nostalgia", "Chocapic"],
};
const AUTHORS = ["Mojave", "Kaelis", "Ninox", "Tarn", "Brindille", "Orfeo"];
const SUFFIXES = ["Reborn", "Plus", "Lite", "Remix", "Ultimate"];
const KIND_LABEL: Record<BrowseKind, string> = {
  modpack: "un modpack", mod: "un mod", shader: "un shader", resourcepack: "un pack de ressources",
};

function slug(s: string): string {
  return s.toLowerCase().normalize("NFD").replace(/[̀-ͯ]/g, "").replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
}

function makeHits(): BrowseHit[] {
  const hits: BrowseHit[] = [];
  for (const source of ["curseforge", "modrinth"] as BrowseSource[]) {
    for (const kind of Object.keys(KIND_WORDS) as BrowseKind[]) {
      const words = KIND_WORDS[kind];
      for (let i = 0; i < 36; i++) {
        const base = words[i % words.length];
        const name = i < words.length ? base : `${base} ${SUFFIXES[Math.floor(i / words.length) % 5]} ${i}`;
        hits.push({
          source, kind, id: `${source}-${kind}-${i}`, name,
          summary: `${name} : ${KIND_LABEL[kind]} pour Minecraft.`,
          author: AUTHORS[i % AUTHORS.length],
          icon_url: i % 4 === 3 ? null : `https://cdn.example.test/${source}/${slug(name)}.png`,
          downloads: Math.max(100, 5_000_000 - i * 131_071 + (source === "modrinth" ? 12_345 : 0)),
          page_url: source === "modrinth"
            ? `https://modrinth.com/${kind}/${slug(name)}`
            : `https://www.curseforge.com/minecraft/${kind}s/${slug(name)}`,
          downloadable: !(source === "curseforge" && i % 6 === 5),
        });
      }
    }
  }
  return hits;
}
const HITS = makeHits();

const KNOWN_ART: Array<{ match: string; id: string; title: string; author: string; color: string; source: "curseforge" | "modrinth" }> = [
  { match: "all the mods 10", id: "atm10", title: "All the Mods 10", author: "ATM Team", color: "#2f6fed", source: "curseforge" },
  { match: "prominence", id: "prominence", title: "Prominence II", author: "Moosniffer", color: "#c0392b", source: "modrinth" },
  { match: "better minecraft", id: "bmc", title: "Better Minecraft", author: "Lunapixel", color: "#27ae60", source: "modrinth" },
];

function makeArt(id: string, title: string, author: string, color: string, source: "curseforge" | "modrinth" | "custom"): InstanceArt {
  return {
    source, project_id: id, title, author, summary: `${title} : un modpack de démonstration.`, color,
    icon: "icon.png", screenshots: [1, 2, 3, 4].map((n) => `screenshot-${n}.png`),
    page_url: source === "custom" ? null : `https://example.test/${source}/${id}`,
  };
}

function shade(hex: string, k: number): string {
  const n = parseInt(hex.slice(1), 16);
  const c = (v: number) => Math.max(0, Math.min(255, Math.round(v * k))).toString(16).padStart(2, "0");
  return `#${c(n >> 16)}${c((n >> 8) & 255)}${c(n & 255)}`;
}

/** SVG dégradé encodé en data URL, distinct par (couleur, texte). */
function svgImage(color: string, label: string, file: string): string {
  const esc = label.replace(/[<>&"]/g, "");
  const big = file.startsWith("icon");
  const [w, h] = big ? [128, 128] : [320, 180];
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}">`
    + `<defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="${color}"/>`
    + `<stop offset="1" stop-color="${shade(color, 0.45)}"/></linearGradient></defs>`
    + `<rect width="${w}" height="${h}" fill="url(#g)"/>`
    + `<text x="${w / 2}" y="${h / 2}" fill="#fff" font-family="sans-serif" font-size="${big ? 18 : 20}" text-anchor="middle">${esc}</text>`
    + `<text x="${w / 2}" y="${h - 10}" fill="#ffffffaa" font-family="sans-serif" font-size="11" text-anchor="middle">${file}</text></svg>`;
  return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
}

function makeVersions(hit: BrowseHit): BrowseVersion[] {
  const defs: Array<[string, string[], string[]]> = [
    ["1.2.0", ["1.21.4", "1.21.1"], ["neoforge", "fabric"]],
    ["1.1.3", ["1.21.1"], ["neoforge"]],
    ["1.0.5", ["1.20.1"], ["forge", "fabric"]],
  ];
  const withLoader = hit.kind === "mod" || hit.kind === "modpack";
  return defs.map(([v, mc, loaders], i) => ({
    id: `${hit.id}-v${v}`, name: `${hit.name} ${v}`, minecraft: mc, loaders: withLoader ? loaders : [],
    published: new Date(Date.UTC(2025, 10 - i * 3, 12)).toISOString(), downloadable: hit.downloadable,
  }));
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

export function createFakeServices(opts: FakeOptions = {}): Services & FakeControls {
  const tickMs = opts.tickMs ?? 250;
  const loginDelayMs = opts.loginDelayMs ?? 1200;
  const requestCount = opts.requestCount ?? 1000;

  let accounts: AccountInfo[] = (opts.accounts ?? []).map((a) => ({ ...a }));
  let active: string | null = accounts[0]?.id ?? null;
  const MS_ACCOUNTS: Array<{ name: string; uuid: string }> = [
    { name: "Steve", uuid: "8667ba71-b85a-4004-af54-457a9734eed7" },
    { name: "Alex", uuid: "ec561538-f3fd-461d-aff5-086b22154bce" },
    { name: "Efe", uuid: "61699b2e-d327-4a01-9f1e-0ea8c3f06bc6" },
  ];
  let codeN = 1;
  let settings: Settings = {
    language: "fr", default_ram_mb: 4096, java_path: null,
    trust_default: "ask", allow_relay: true, close_on_launch: false, relay_only: false, share_mods: true, discovery: true, lan_auto: true,
    overlay_enabled: true, overlay_hotkey: "Ctrl+Shift+C", embedded_game: true,
  };
  const instances: Instance[] = [{
    id: "inst-aeronautics", name: "Create: Aeronautics", minecraft: "1.21.1",
    loader: { loader: "neoforge", version: "21.1.77" }, ram_mb: 6144, mod_count: 94,
  }, {
    id: "inst-atm10", name: "All the Mods 10", minecraft: "1.21.1",
    loader: { loader: "neoforge", version: "21.1.77" }, ram_mb: 8192, mod_count: 420,
    art: makeArt("atm10", "All the Mods 10", "ATM Team", "#2f6fed", "curseforge"),
  }, {
    id: "inst-prominence", name: "Prominence II", minecraft: "1.20.1",
    loader: { loader: "fabric", version: "0.15.11" }, ram_mb: 6144, mod_count: 310,
    art: makeArt("prominence", "Prominence II", "Moosniffer", "#c0392b", "modrinth"),
  }];
  const artOf = (id: string) => need(id);
  let requests: FriendRequest[] = Array.from({ length: requestCount }, (_, i) => makeRequest(i));
  const friends: FriendWithPresence[] = [
    { friend: { public_key: hex(1, 64), name: "Alex", trust: "ask", added_at: NOW - 86400 }, presence: "online" },
    { friend: { public_key: hex(2, 64), name: "Noor", trust: "always", added_at: NOW - 172800 }, presence: "offline", last_seen: NOW - 3 * 3600 },
  ];

  let invites: Invite[] = [];
  let inviteN = 0;
  const gameCalls: Array<{ kind: "switch" | "join" | "invite" | "close"; arg: string; instanceId?: string }> = [];

  const listeners = {
    auth: new Set<(p: Profile | null) => void>(),
    accounts: new Set<(s: AccountsState) => void>(),
    launch: new Set<(s: LaunchState) => void>(),
    transfer: new Set<(p: TransferProgress) => void>(),
    presence: new Set<(e: PresenceEvent) => void>(),
    request: new Set<(r: FriendRequest) => void>(),
    unknown: new Set<(e: UnknownModsEvent) => void>(),
    log: new Set<(e: LaunchLogEvent) => void>(),
    browse: new Set<(e: BrowseProgressEvent) => void>(),
    fix: new Set<(e: FixProgressEvent) => void>(),
    drop: new Set<(p: string[]) => void>(),
    warning: new Set<(e: ImportWarningEvent) => void>(),
    host: new Set<(e: HostState) => void>(),
    invite: new Set<(i: Invite) => void>(),
    invites: new Set<(l: Invite[]) => void>(),
    update: new Set<(s: UpdateState) => void>(),
  };
  let updateSt: UpdateState = { phase: "idle", current: "0.1.0", version: null, notes: null, downloaded: 0, total: null, message: null };
  let updateCheckPatch: Partial<UpdateState> = { phase: "up_to_date" };
  const setUpdate = (patch: Partial<UpdateState>) => { updateSt = { ...updateSt, ...patch }; emit(listeners.update, { ...updateSt }); };
  function sub<T>(set: Set<(v: T) => void>, cb: (v: T) => void): Unsubscribe {
    set.add(cb);
    return () => { set.delete(cb); };
  }
  function emit<T>(set: Set<(v: T) => void>, v: T) { set.forEach((cb) => cb(v)); }
  const profileOf = (a: AccountInfo | undefined): Profile | null => (a ? { kind: a.kind, name: a.name, uuid: a.uuid } : null);
  const accountsState = (): AccountsState => ({ accounts: accounts.map((a) => ({ ...a })), active });
  /** Applique un changement de comptes et émet les deux événements (profil actif, liste). */
  const commitAccounts = () => {
    emit(listeners.auth, profileOf(accounts.find((a) => a.id === active)));
    emit(listeners.accounts, accountsState());
  };

  // --- jeu simulé : logs et lancement ---
  const logs = new Map<string, string[]>();
  const launchTimers = new Map<string, ReturnType<typeof setTimeout>[]>();
  const pushLog = (id: string, line: string) => {
    logs.set(id, [...(logs.get(id) ?? []), line]);
    emit(listeners.log, { id, line });
  };
  let current: string | null = null;
  const clearLaunch = (id: string) => {
    if (current === id) current = null;
    launchTimers.get(id)?.forEach(clearTimeout);
    launchTimers.delete(id);
  };
  const need = (id: string) => {
    const inst = instances.find((i) => i.id === id);
    if (!inst) throw new Error("Instance introuvable.");
    return inst;
  };

  // --- mods en mémoire ---
  let picked = opts.pickedFiles ?? [];
  const mods = new Map<string, ModEntry[]>();
  mods.set("inst-aeronautics", [
    ...["create", "sodium", "iris", "lithium", "jade", "xaero-minimap", "waystones", "farmers-delight"].map((n, i) => (
      { file: `${n}-1.${i}.0.jar`, enabled: i !== 3, size: 800_000 + i * 345_678, custom: false }
    )),
    { file: "mod-maison.jar", enabled: true, size: 52_340, custom: true },
  ]);
  const modsOf = (id: string) => {
    need(id);
    if (!mods.has(id)) mods.set(id, []);
    return mods.get(id)!;
  };
  const modFile = (id: string, file: string) => {
    const m = modsOf(id).find((x) => x.file === file);
    if (!m) throw new Error("Mod introuvable.");
    return m;
  };
  /** Corrections appliquées par instance (le contrôle de démo ne signale alors plus les erreurs). */
  const fixed = new Set<string>();
  const FIX_ADDED = "sodium-0.6.0.jar";
  const FIX_PLAN: FixPlan = {
    plan_id: "plan-demo",
    actions: [
      { id: "enable-lithium", kind: "enable", title: "Activer Lithium", detail: "Requis par Iris.", file: "lithium-1.3.0.jar", issue_files: ["iris-1.2.0.jar"] },
      { id: "install-sodium", kind: "install", title: "Installer Sodium 0.6.0 (Modrinth)", detail: "Iris en a besoin.", source: "modrinth", file: FIX_ADDED, issue_files: ["iris-1.2.0.jar"] },
      { id: "update-waystones", kind: "update", title: "Mettre à jour Waystones vers 1.7.0 (CurseForge)", detail: "Version prévue pour Minecraft 1.21.", source: "curseforge", file: "waystones-1.7.0.jar", issue_files: ["waystones-1.6.0.jar"] },
    ],
    unresolved: [
      { file: "sodium-1.1.0.jar", mod_name: "Sodium", message: "Incompatible avec « Xaero's Minimap ».", reason: "Choisis lequel garder : la correction automatique ne décide pas." },
      { file: "create-1.0.0.jar", mod_name: "Create", message: "Version trop ancienne pour Minecraft 1.21.1.", reason: "Fichier non distribuable : à télécharger depuis la page du mod.", page_url: "https://www.curseforge.com/minecraft/mc-mods/create" },
    ],
  };
  const checkOf = (id: string): ModCheck => {
    const list = modsOf(id);
    const has = (f: string) => list.some((m) => m.file === f);
    const issues: ModIssue[] = [];
    if (id === "inst-aeronautics") {
      if (!fixed.has(id) && has("iris-1.2.0.jar")) issues.push({ file: "iris-1.2.0.jar", mod_name: "Iris", kind: "missing_dep", severity: "error", message: "Il lui manque le mod « Sodium » en version 0.6 ou plus.", });
      if (!fixed.has(id) && has("sodium-1.1.0.jar") && has("xaero-minimap-1.5.0.jar")) issues.push({ file: "sodium-1.1.0.jar", mod_name: "Sodium", kind: "incompatible", severity: "error", message: "Incompatible avec « Xaero's Minimap ».", related: "xaero-minimap-1.5.0.jar" });
      if (has("waystones-1.6.0.jar")) issues.push({ file: "waystones-1.6.0.jar", mod_name: "Waystones", kind: "wrong_minecraft", severity: "warning", message: "Prévu pour Minecraft 1.20 : il peut ne pas fonctionner." });
    }
    return { issues, checked: list.filter((m) => m.enabled).length };
  };
  const syncCount = (id: string) => { need(id).mod_count = modsOf(id).length; };

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
    authStatus: async () => profileOf(accounts.find((a) => a.id === active)),
    authBeginLogin: async (): Promise<DeviceCode> => ({
      user_code: "AB12CD34", verification_uri: "https://www.microsoft.com/link", expires_in: 900,
    }),
    authPollLogin: () => new Promise<Profile>((resolve) => {
      const done = () => {
        const known = accounts.filter((a) => a.kind === "online").length;
        const m = MS_ACCOUNTS[known % MS_ACCOUNTS.length];
        let acc = accounts.find((a) => a.kind === "online" && a.uuid === m.uuid);
        if (!acc) { acc = { id: `ms-${m.uuid}`, kind: "online", name: m.name, uuid: m.uuid }; accounts = [...accounts, acc]; }
        active = acc.id;
        commitAccounts();
        resolve(profileOf(acc)!);
      };
      if (loginDelayMs <= 0) done(); else setTimeout(done, loginDelayMs);
    }),
    authLogout: async () => {
      accounts = accounts.filter((a) => a.id !== active);
      active = accounts[0]?.id ?? null;
      commitAccounts();
    },
    offlineAvailable: async () => true,
    authOfflineLogin: async (name) => {
      const n = name.trim();
      const err = validateOfflineName(n);
      if (err) throw new Error(err);
      const id = `off-${n.toLowerCase()}`;
      let acc = accounts.find((a) => a.id === id);
      if (!acc) { acc = { id, kind: "offline", name: n, uuid: `00000000-0000-3000-8000-${hex(n.length, 12)}` }; accounts = [...accounts, acc]; }
      active = acc.id;
      commitAccounts();
      return profileOf(acc)!;
    },
    authAccounts: async () => accountsState(),
    authSwitch: async (id) => {
      const acc = accounts.find((a) => a.id === id);
      if (!acc) throw new Error("Ce compte n'existe plus.");
      active = id;
      commitAccounts();
      return profileOf(acc)!;
    },
    authRemove: async (id) => {
      if (!accounts.some((a) => a.id === id)) throw new Error("Ce compte n'existe plus.");
      accounts = accounts.filter((a) => a.id !== id);
      if (active === id) active = accounts[0]?.id ?? null;
      commitAccounts();
    },
    authSkinHead: async () => null,
    skinHeadByName: async () => null,

    instancesList: async () => instances.map((i) => ({ ...i })),
    instancesCreate: async (input: CreateInstanceInput) => {
      if (!input.name.trim()) throw new Error("Le nom de l'instance est obligatoire.");
      const inst: Instance = { id: `inst-${instances.length + 1}`, ...input, mod_count: 0 };
      instances.push(inst);
      return { ...inst };
    },
    instancesImportMrpack: async (path) => {
      if (!/\.(mrpack|zip)$/i.test(path)) throw new Error("Fichier .mrpack ou .zip attendu.");
      if (/manquant|missing/i.test(path)) emit(listeners.warning, { missing: ["sodium", "iris"] });
      const inst: Instance = {
        id: `inst-${instances.length + 1}`, name: "Pack importé", minecraft: "1.21.1",
        loader: { loader: "fabric", version: "0.16.5" }, ram_mb: 4096, mod_count: 12,
      };
      instances.push(inst);
      return { ...inst };
    },
    instancesLaunch: async (id) => {
      if (!instances.some((i) => i.id === id)) throw new Error("Instance introuvable.");
      clearLaunch(id);
      current = id;
      emit(listeners.launch, { id, state: "preparing" });
      pushLog(id, "[Cubelink] Préparation du jeu…");
      if (tickMs > 0) {
        launchTimers.set(id, [
          setTimeout(() => { emit(listeners.launch, { id, state: "running" }); pushLog(id, "[Minecraft] Démarrage de Minecraft…"); }, tickMs),
          setTimeout(() => pushLog(id, "[Minecraft] Chargement des mods…"), tickMs * 2),
        ]);
      }
    },
    instancesKill: async (id) => {
      need(id);
      clearLaunch(id);
      pushLog(id, "[Cubelink] Processus arrêté.");
      emit(listeners.launch, { id, state: "exited" });
    },
    instancesDelete: async (id) => {
      const inst = need(id);
      clearLaunch(id);
      instances.splice(instances.indexOf(inst), 1);
      logs.delete(id);
    },
    instancesUpdate: async (id, patch) => {
      const inst = need(id);
      if (patch.name !== undefined) {
        if (!patch.name.trim()) throw new Error("Le nom de l'instance est obligatoire.");
        inst.name = patch.name.trim();
      }
      if (patch.ram_mb !== undefined) inst.ram_mb = patch.ram_mb;
      if (patch.java_path !== undefined) inst.java_path = patch.java_path;
      if (patch.jvm_args !== undefined) inst.jvm_args = patch.jvm_args;
      return { ...inst };
    },
    instancesArtImage: async (id, file) => {
      const art = artOf(id).art;
      if (!art) throw new Error("Cette instance n'a pas d'illustration.");
      if (file !== art.icon && !art.screenshots.includes(file)) throw new Error("Image introuvable.");
      return svgImage(art.color ?? "#555555", art.title, file);
    },
    instancesArtSet: async (id, path, role) => {
      const inst = artOf(id);
      const file = path.split(/[\\/]/).pop() ?? path;
      if (!/\.(png|jpe?g|webp|gif)$/i.test(file)) throw new Error("Image attendue (png, jpg, webp ou gif).");
      const art = inst.art ?? { ...makeArt(`custom-${id}`, inst.name, "", "#6c5ce7", "custom"), icon: null, screenshots: [] };
      art.title = inst.name;
      if (role === "icon") art.icon = file;
      else art.screenshots = [file, ...art.screenshots.filter((x) => x !== file)];
      inst.art = { ...art, source: art.source === "custom" ? "custom" : art.source };
      return { ...inst };
    },
    instancesArtLookup: async (id) => {
      const inst = artOf(id);
      const name = inst.name.trim().toLowerCase();
      const k = KNOWN_ART.find((x) => name.includes(x.match));
      if (!k) throw new Error(`Aucun modpack correspondant à « ${inst.name} » trouvé. Choisis une image à la main.`);
      inst.art = makeArt(k.id, k.title, k.author, k.color, k.source);
      return { ...inst };
    },
    instancesOpenFolder: async (id) => { need(id); },
    instancesLogs: async (id) => {
      need(id);
      return (logs.get(id) ?? ["[Cubelink] Aucun lancement pour le moment."]).join("\n");
    },
    minecraftVersions: async () => ["1.21.4", "1.21.1", "1.20.6", "1.20.1", "1.19.4", "1.18.2"],

    netTest: async () => ({ relay_connected: true, relay: "euc1-1.relay.n0.iroh.link", relay_ms: 28, udp: true, ipv6: false, direct: true, relay_only: false, partial: false }),
    friendMyCode: async () => `mcl1-${hex(codeN * 7, 24)}`,
    friendRegenerateCode: async () => { codeN += 1; return `mcl1-${hex(codeN * 7, 24)}`; },
    friendList: async () => friends.map((f) => ({ friend: { ...f.friend }, presence: f.presence, last_seen: f.last_seen })),
    friendInvite: async (pk) => {
      if (!friends.some((f) => f.friend.public_key === pk)) throw new Error("Ami introuvable.");
      gameCalls.push({ kind: "invite", arg: pk });
    },
    friendInvites: async () => invites.map((i) => ({ ...i })),
    friendInviteDismiss: async (id) => {
      invites = invites.filter((i) => i.id !== id);
      emit(listeners.invites, invites.map((i) => ({ ...i })));
    },
    gameSwitch: async (id) => { need(id); gameCalls.push({ kind: "switch", arg: id }); },
    gameCurrent: async () => current,
    gameClose: async () => {
      const id = current;
      if (!id) return;
      gameCalls.push({ kind: "close", arg: id });
      clearLaunch(id);
      emit(listeners.launch, { id, state: "exited" });
    },
    gameJoinFriend: async (pk, instanceId) => { gameCalls.push({ kind: "join", arg: pk, instanceId }); },
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
    joinPlay: async () => {},
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

    browseSearch: async (q) => {
      const t = q.query.trim().toLowerCase();
      let all = HITS.filter((h) => h.source === q.source && h.kind === q.kind
        && (!t || h.name.toLowerCase().includes(t) || h.summary.toLowerCase().includes(t)));
      if (q.sort === "downloads") all = [...all].sort((a, b) => b.downloads - a.downloads);
      else if (q.sort === "updated") all = [...all].reverse();
      return { hits: all.slice(q.offset, q.offset + q.limit).map((h) => ({ ...h })), total: all.length };
    },
    browseVersions: async (source, id, kind, minecraft, loader) => {
      const hit = HITS.find((h) => h.source === source && h.id === id && h.kind === kind);
      if (!hit) throw new Error("Projet introuvable.");
      return makeVersions(hit).filter((v) => (!minecraft || v.minecraft.includes(minecraft))
        && (!loader || !v.loaders.length || v.loaders.includes(loader)));
    },
    browseInstall: async (input): Promise<BrowseInstallResult> => {
      const hit = HITS.find((h) => h.source === input.source && h.id === input.id && h.kind === input.kind);
      if (!hit) throw new Error("Projet introuvable.");
      if (!hit.downloadable) throw new Error("Ce projet n'est pas téléchargeable depuis le launcher.");
      const steps = input.kind === "modpack" ? 5 : 2;
      for (let i = 0; i <= steps; i++) {
        emit(listeners.browse, { done: i, total: steps, label: i === steps ? "Terminé" : `Téléchargement de ${hit.name}` });
        if (tickMs > 0) await new Promise((r) => setTimeout(r, tickMs / 5));
      }
      if (input.kind === "modpack") {
        const inst: Instance = {
          id: `inst-${instances.length + 1}`, name: hit.name, minecraft: "1.21.1",
          loader: { loader: "neoforge", version: "21.1.77" }, ram_mb: 4096, mod_count: 24,
        };
        inst.art = makeArt(`${hit.source}-${hit.id}`, hit.name, hit.author, hit.source === "modrinth" ? "#1bd96a" : "#f16436", hit.source);
        instances.push(inst);
        return {
          instance: { ...inst }, installed: [`${slug(hit.name)}.mrpack`],
          missing: hit.source === "curseforge" ? ["mod-exclusif"] : [],
        };
      }
      const inst = need(input.instance_id ?? instances[0]?.id ?? "");
      const ver = input.version_id?.split("-v")[1] ?? "1.2.0";
      const file = `${slug(hit.name)}-${ver}.${input.kind === "mod" ? "jar" : "zip"}`;
      if (input.kind === "mod") {
        const list = modsOf(inst.id);
        if (!list.some((m) => m.file === file)) list.push({ file, enabled: true, size: 1_200_000, custom: false });
        syncCount(inst.id);
      }
      return { instance: { ...inst }, installed: [file], missing: [] };
    },
    browseCheck: async (source, id, kind, instanceId, versionId) => {
      const hit = HITS.find((h) => h.source === source && h.id === id && h.kind === kind);
      if (!hit || kind !== "mod") return { installed: [], recommended: null, verdicts: [], blockers: [] };
      const prefix = `${slug(hit.name)}-`;
      const installed = modsOf(need(instanceId).id).filter((m) => m.file.startsWith(prefix)).map((m) => m.file.replace(/\.disabled$/, ""));
      const first = makeVersions(hit)[0]?.id ?? null;
      const vid = versionId ?? first;
      return { installed, recommended: versionId ? null : first, verdicts: vid ? [{ version_id: vid, ok: true, problems: [] }] : [], blockers: [] };
    },
    modsList: async (id) => modsOf(id).map((m) => ({ ...m })),
    modsIcon: async () => null,
    modsCheck: async (id) => checkOf(id),
    instancesDescription: async (id) => {
      const art = artOf(id).art;
      if (!art || art.source === "custom") return null;
      return `<h2>${art.title}</h2><p>${art.summary}</p><ul><li>Plus de 90 mods</li><li>Quêtes</li></ul><p><a href="https://modrinth.com" target="_blank" rel="noopener noreferrer">Page du projet</a></p>`;
    },
    modsToggle: async (id, file, enabled) => { modFile(id, file).enabled = enabled; },
    modsFixPlan: async (id) => {
      need(id);
      if (id !== "inst-aeronautics") return { plan_id: "plan-vide", actions: [], unresolved: [] };
      return structuredClone(FIX_PLAN);
    },
    modsFixApply: async (id, _planId, actionIds): Promise<FixResult> => {
      const list = modsOf(id);
      const picked = FIX_PLAN.actions.filter((a) => actionIds.includes(a.id));
      const total = picked.length;
      const applied: string[] = [];
      const failed: FixResult["failed"] = [];
      let done = 0;
      for (const a of picked) {
        emit(listeners.fix, { done, total, label: a.title });
        done++;
        if (a.id === "update-waystones") { failed.push({ title: a.title, error: "Téléchargement impossible (hors ligne ?)." }); continue; }
        if (a.id === "enable-lithium") modFile(id, "lithium-1.3.0.jar").enabled = true;
        if (a.id === "install-sodium" && !list.some((m) => m.file === FIX_ADDED)) list.push({ file: FIX_ADDED, enabled: true, size: 1_100_000, custom: false });
        applied.push(a.title);
      }
      emit(listeners.fix, { done: total, total, label: "Terminé" });
      if (applied.length) { fixed.add(id); syncCount(id); }
      const check = checkOf(id);
      return { applied, failed, backup_id: applied.length ? "backup-demo" : null, check };
    },
    modsFixUndo: async (id) => {
      const list = modsOf(id);
      fixed.delete(id);
      const lith = list.find((m) => m.file === "lithium-1.3.0.jar");
      if (lith) lith.enabled = false;
      const k = list.findIndex((m) => m.file === FIX_ADDED);
      if (k >= 0) list.splice(k, 1);
      syncCount(id);
      return checkOf(id);
    },
    modsRemove: async (id, file) => {
      const m = modFile(id, file);
      const list = modsOf(id);
      list.splice(list.indexOf(m), 1);
      syncCount(id);
    },
    modsAdd: async (id, path) => {
      const list = modsOf(id);
      const file = path.split(/[\\/]/).pop() ?? path;
      if (!/\.jar$/i.test(file)) throw new Error("Fichier .jar attendu.");
      if (list.some((m) => m.file === file)) throw new Error("Ce mod est déjà présent.");
      const m: ModEntry = { file, enabled: true, size: 250_000, custom: true };
      list.push(m);
      syncCount(id);
      return { ...m };
    },
    pickFiles: async ({ extensions, multiple }) => {
      const ok = picked.filter((p) => extensions.some((e) => p.toLowerCase().endsWith(`.${e.toLowerCase()}`)));
      return multiple ? ok : ok.slice(0, 1);
    },
    setPickedFiles: (paths) => { picked = paths; },
    dropFiles: (paths) => emit(listeners.drop, paths),
    emitHostState: (e) => emit(listeners.host, e),
    emitInvite: (p) => {
      inviteN += 1;
      const f = friends.find((x) => x.friend.public_key === p.public_key);
      const inv: Invite = {
        id: `inv-${inviteN}`, name: f?.friend.name ?? "Ami", world_name: "Monde de survie", minecraft: "1.21.1",
        received_at: Math.floor(Date.now() / 1000), expires_at: Math.floor(Date.now() / 1000) + 600, ...p,
      };
      invites = [...invites, inv];
      emit(listeners.invite, { ...inv });
      emit(listeners.invites, invites.map((i) => ({ ...i })));
      return { ...inv };
    },
    gameCalls,
    emitUpdateState: (patch) => setUpdate(patch),
    setUpdateCheckResult: (patch) => { updateCheckPatch = patch; },
    updateState: async () => ({ ...updateSt }),
    updateCheck: async () => {
      setUpdate({ phase: "checking", message: null });
      await Promise.resolve();
      setUpdate(updateCheckPatch);
      return { ...updateSt };
    },
    updateInstall: async () => {
      if (updateSt.phase !== "ready") throw new Error("Aucune mise à jour prête à installer.");
    },
    onUpdateState: (cb) => sub(listeners.update, cb),

    onAuthState: (cb) => sub(listeners.auth, cb),
    onAccountsState: (cb) => sub(listeners.accounts, cb),
    onLaunchState: (cb) => sub(listeners.launch, cb),
    onLaunchLog: (cb) => sub(listeners.log, cb),
    onImportWarning: (cb) => sub(listeners.warning, cb),
    hostState: async () => null,
    onHostState: (cb) => sub(listeners.host, cb),
    onTransferProgress: (cb) => sub(listeners.transfer, cb),
    onFriendPresence: (cb) => sub(listeners.presence, cb),
    onFriendRequest: (cb) => sub(listeners.request, cb),
    onFriendInvite: (cb) => sub(listeners.invite, cb),
    onFriendInvites: (cb) => sub(listeners.invites, cb),
    onUnknownMods: (cb) => sub(listeners.unknown, cb),
    onTransferErrorDetails: () => () => {},
    onBrowseProgress: (cb) => sub(listeners.browse, cb),
    onModsFixProgress: (cb) => sub(listeners.fix, cb),
    onFilesDropped: (cb) => sub(listeners.drop, cb),
  };
}
