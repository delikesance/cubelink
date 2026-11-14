import { validateOfflineName } from "./accounts";
import { HEARTH_EULA_REQUIRED } from "./types";
import type {
  AccountInfo, AccountsState, BrowseHit, BrowseInstallResult, BrowseKind, BrowseProgressEvent, BrowseSource, BrowseVersion, FixPlan, FixProgressEvent, FixResult, ModCheck, ModEntry, ModIssue,
  CreateInstanceInput, DeviceCode, HostState, Invite, JoinAnswer, JoinRequest, ImportWarningEvent, LaunchLogEvent, FriendRequest, FriendWithPresence, Instance, InstanceArt,
  JoinPlan, LaunchState, Manifest, PresenceEvent, UnknownModsEvent, Profile, Services, Settings,
  TransferProgress, Unsubscribe, UpdateState,
  HearthCard, HearthDetail, HearthHandover, HearthHostPrompt, HearthInvite, HearthMember, HearthMemberLite,
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
  /** Test : un ami demande à rejoindre ma partie (friend://join_request + friend://join_requests). */
  emitJoinRequest(p: Partial<JoinRequest> & { public_key: string }): JoinRequest;
  /** Test : réponse de l'hôte à `friendJoinRequest` ; `"hold"` la laisse en attente jusqu'à `resolveJoinRequest`. Défaut `"accepted"`. */
  setJoinAnswer(a: JoinAnswer | "hold"): void;
  /** Test : résout la demande en attente (`"hold"`) envoyée à cet ami. */
  resolveJoinRequest(publicKey: string, a: JoinAnswer): void;
  /** Test : demandes envoyées (`ask`) et décisions prises (`answer`). */
  readonly joinRequestCalls: Array<{ kind: "ask" | "answer"; arg: string; accept?: boolean }>;
  /** Test : appels de commandes de jeu enregistrés (switch, join, invite). */
  readonly gameCalls: Array<{ kind: "switch" | "join" | "invite" | "close"; arg: string; instanceId?: string }>;
  /** Test : fusionne `patch` dans l'état de mise à jour et émet update://state (ex. available, downloading, ready). */
  emitUpdateState(patch: Partial<UpdateState>): void;
  /** Test : état (fusionné) auquel aboutit la prochaine updateCheck ; par défaut up_to_date. */
  setUpdateCheckResult(patch: Partial<UpdateState>): void;
  /** Test : ajoute un serveur de groupe (état « idle » par défaut) et émet hearth://state. */
  seedHearth(g: Partial<HearthCard> & { id: string; name: string }, detail?: Partial<Omit<HearthDetail, "card">>): HearthCard;
  /** Test : fusionne `patch` dans la carte d'un groupe et émet hearth://state. */
  setHearthCard(id: string, patch: Partial<HearthCard>): void;
  /** Test : fusionne `patch` dans la page d'un groupe (membres, versions, propositions, bannière, fourche, réglages). */
  setHearthDetail(id: string, patch: Partial<Omit<HearthDetail, "card">>): void;
  /** Test : reçoit une invitation de groupe (hearth://invites). */
  emitHearthInvite(i: Partial<HearthInvite> & { group_id: string }): HearthInvite;
  /** Test : émet hearth://handover. */
  emitHearthHandover(e: HearthHandover): void;
  /** Test : émet hearth://host_prompt. */
  emitHearthHostPrompt(e: HearthHostPrompt): void;
  /** Test : le contrat d'utilisation de Minecraft est déjà accepté (ou non). Défaut : non. */
  setHearthEula(accepted: boolean): void;
  /** Test : commandes Hearth reçues, dans l'ordre (nom sans préfixe `hearth`, arguments). */
  readonly hearthCalls: Array<{ kind: string; args: unknown[] }>;
}

/** Hash du manifeste simulé, renvoyé par `joinPlan` et exigé par `joinStart`. */
export const FAKE_MANIFEST_HASH = "f".repeat(64);

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
    overlay_enabled: true, overlay_hotkey: "Ctrl+Shift+C", embedded_game: true, open_to_all_friends: false,
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
  let joinRequests: JoinRequest[] = [];
  let joinAnswer: JoinAnswer | "hold" = "accepted";
  const joinHeld = new Map<string, (a: JoinAnswer) => void>();
  const joinRequestCalls: Array<{ kind: "ask" | "answer"; arg: string; accept?: boolean }> = [];
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
    joinRequest: new Set<(r: JoinRequest) => void>(),
    joinRequests: new Set<(l: JoinRequest[]) => void>(),
    update: new Set<(s: UpdateState) => void>(),
    hearthState: new Set<(l: HearthCard[]) => void>(),
    hearthInvites: new Set<(l: HearthInvite[]) => void>(),
    hearthHandover: new Set<(e: HearthHandover) => void>(),
    hearthHostPrompt: new Set<(e: HearthHostPrompt) => void>(),
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


  // --- serveurs de groupe (Hearth) simulés ---
  const ME: HearthMemberLite = { public_key: hex(9, 64), name: "Moi", online: true };
  const hearthGroups = new Map<string, HearthDetail>();
  let hearthInvs: HearthInvite[] = [];
  let hearthEula = false;
  /** Le faux service part avec Hearth activé (le vrai réglage est coupé par défaut) ; `hearthSetEnabled` le change. */
  let hearthOn = true;
  let hearthN = 0;
  const hearthCalls: Array<{ kind: string; args: unknown[] }> = [];
  const liteOf = (pk: string): HearthMemberLite => {
    const f = friends.find((x) => x.friend.public_key === pk);
    return { public_key: pk, name: f?.friend.name ?? "Ami", online: f?.presence === "online" };
  };
  const hearthCard = (id: string): HearthDetail => {
    const g = hearthGroups.get(id);
    if (!g) throw new Error("groupe introuvable");
    return g;
  };
  const hearthCards = () => [...hearthGroups.values()].map((g) => ({ ...g.card }));
  const hearthEmit = () => emit(listeners.hearthState, hearthCards());
  /** Recalcule les compteurs de la carte à partir des membres. */
  const hearthSync = (g: HearthDetail) => {
    g.card.members_total = g.members.length;
    g.card.members_online = g.members.filter((m) => !m.is_me && m.online).map((m) => ({ public_key: m.public_key, name: m.name, online: true }));
  };
  const hearthPatch = (id: string, patch: Partial<HearthCard>) => {
    const g = hearthCard(id);
    g.card = { ...g.card, ...patch };
    hearthEmit();
  };
  const newVersion = (g: HearthDetail, kind: "session" | "checkpoint" | "restore") => {
    const number = (g.versions[0]?.number ?? 0) + 1;
    g.versions = [
      { id: `v-${g.card.id}-${number}`, number, kind, author: ME.public_key, author_name: ME.name, created_at: Math.floor(Date.now() / 1000), played_secs: 1800, total_size: 410_000_000, is_head: true, side_branch: false },
      ...g.versions.map((v) => ({ ...v, is_head: false })),
    ];
    g.card.version = number;
  };
  const makeHearth = (p: Partial<HearthCard> & { id: string; name: string }, d: Partial<Omit<HearthDetail, "card">> = {}): HearthDetail => {
    const members: HearthMember[] = d.members ?? [
      { ...ME, is_me: true, is_creator: true, sync: "ok", version: 1, unseen_days: null },
      ...friends.map((f): HearthMember => ({
        public_key: f.friend.public_key, name: f.friend.name, online: f.presence === "online", is_me: false, is_creator: false,
        sync: "ok", version: 1, unseen_days: f.presence === "online" ? null : 2,
      })),
    ];
    const card: HearthCard = {
      minecraft: "1.21.1", loader: "NeoForge", state: "idle", host: null, players: 0, progress: null, step: null, step_index: null,
      step_count: 3, unclean: null, members_online: [], members_total: members.length, is_creator: true, can_host: true,
      instance_id: "inst-aeronautics", version: 1, ...p,
    };
    const g: HearthDetail = {
      card, members, proposals: [], blocker: null, fork: [],
      versions: [{ id: `v-${p.id}-1`, number: 1, kind: "session", author: ME.public_key, author_name: ME.name, created_at: NOW - 86400, played_secs: 5400, total_size: 380_000_000, is_head: true, side_branch: false }],
      settings: { keep_background: false, disk_bytes: 380_000_000, outbox_pending: 0, instance_name: "Create: Aeronautics" },
      ...d,
    };
    g.card = card;
    hearthSync(g);
    if (p.members_online) g.card.members_online = p.members_online;
    return g;
  };
  /** Déroule les étapes d'un lancement simulé, puis `done`. Sans minuterie (tickMs = 0) : immédiat. */
  const hearthLaunch = (id: string, steps: string[], done: Partial<HearthCard>) => {
    if (tickMs <= 0) { hearthPatch(id, { step: null, step_index: null, ...done }); return; }
    steps.forEach((label, i) => {
      setTimeout(() => {
        if (!hearthGroups.has(id)) return;
        hearthPatch(id, { state: "launching", step: label, step_index: i, step_count: steps.length });
      }, i * tickMs);
    });
    setTimeout(() => { if (hearthGroups.has(id)) hearthPatch(id, { step: null, step_index: null, ...done }); }, steps.length * tickMs);
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
    hearthList: async () => hearthCards(),
    hearthDetail: async (id) => structuredClone(hearthCard(id)),
    hearthWorlds: async (instanceId) => {
      need(instanceId);
      return instanceId === "inst-aeronautics"
        ? [{ name: "Monde de survie", size_bytes: 412_000_000 }, { name: "Test créatif", size_bytes: 38_000_000 }]
        : [];
    },
    hearthCreate: async (input) => {
      hearthCalls.push({ kind: "Create", args: [input] });
      const inst = need(input.instanceId);
      const name = input.name.trim();
      if (!name) throw new Error("Donne un nom au serveur.");
      hearthN += 1;
      const picked = input.members.filter((pk) => friends.some((f) => f.friend.public_key === pk));
      const members: HearthMember[] = [
        { ...ME, is_me: true, is_creator: true, sync: "ok", version: 1, unseen_days: null },
        ...picked.map((pk): HearthMember => ({ ...liteOf(pk), is_me: false, is_creator: false, sync: "unknown", version: null, unseen_days: null })),
      ];
      const g = makeHearth({ id: `grp-${hearthN}`, name, minecraft: inst.minecraft, loader: inst.loader.loader, instance_id: inst.id }, { members });
      hearthGroups.set(g.card.id, g);
      hearthEmit();
      return { ...g.card };
    },
    hearthInvites: async () => hearthInvs.map((i) => ({ ...i })),
    hearthInviteAccept: async (id) => {
      hearthCalls.push({ kind: "InviteAccept", args: [id] });
      const inv = hearthInvs.find((i) => i.group_id === id);
      if (!inv) throw new Error("Cette invitation n'est plus valable.");
      hearthInvs = hearthInvs.filter((i) => i.group_id !== id);
      emit(listeners.hearthInvites, hearthInvs.map((i) => ({ ...i })));
      const g = makeHearth({ id, name: inv.name, minecraft: inv.minecraft, loader: inv.loader, is_creator: false, can_host: false, instance_id: null, state: "pack_missing" });
      hearthGroups.set(id, g);
      hearthEmit();
    },
    hearthInviteDismiss: async (id) => {
      hearthCalls.push({ kind: "InviteDismiss", args: [id] });
      hearthInvs = hearthInvs.filter((i) => i.group_id !== id);
      emit(listeners.hearthInvites, hearthInvs.map((i) => ({ ...i })));
    },
    hearthPlay: async (id) => {
      hearthCalls.push({ kind: "Play", args: [id] });
      const g = hearthCard(id);
      const st = g.card.state;
      if (st === "idle" || st === "unclean") {
        if (!hearthEula) throw new Error(HEARTH_EULA_REQUIRED);
        hearthLaunch(id, ["Mise à jour du monde", "Démarrage du serveur", "Ouverture de Minecraft"], { state: "hosting", host: { ...ME }, players: 1, unclean: null });
      } else if (st === "hosted") {
        hearthLaunch(id, ["Connexion à l'hôte", "Ouverture de Minecraft"], { state: "hosted", players: g.card.players + 1 });
      } else {
        throw new Error("Ce serveur ne peut pas être lancé pour l'instant.");
      }
    },
    hearthStop: async (id) => {
      hearthCalls.push({ kind: "Stop", args: [id] });
      hearthCard(id);
      hearthPatch(id, { state: "idle", host: null, players: 0 });
    },
    hearthUncleanChoice: async (id, choice) => {
      hearthCalls.push({ kind: "UncleanChoice", args: [id, choice] });
      hearthCard(id);
      if (choice === "wait") return;
      if (!hearthEula) throw new Error(HEARTH_EULA_REQUIRED);
      hearthLaunch(id, ["Mise à jour du monde", "Démarrage du serveur", "Ouverture de Minecraft"], { state: "hosting", host: { ...ME }, players: 1, unclean: null });
    },
    hearthSetInstance: async (id, instanceId) => {
      hearthCalls.push({ kind: "SetInstance", args: [id, instanceId] });
      const inst = need(instanceId);
      const g = hearthCard(id);
      g.settings.instance_name = inst.name;
      hearthPatch(id, { instance_id: instanceId, can_host: true, state: g.card.state === "pack_missing" ? "idle" : g.card.state });
    },
    hearthAddMember: async (id, pk) => {
      hearthCalls.push({ kind: "AddMember", args: [id, pk] });
      const g = hearthCard(id);
      if (g.members.some((m) => m.public_key === pk)) throw new Error("Déjà dans le groupe.");
      if (!friends.some((f) => f.friend.public_key === pk)) throw new Error("Ami introuvable.");
      g.members = [...g.members, { ...liteOf(pk), is_me: false, is_creator: false, sync: "unknown", version: null, unseen_days: null }];
      hearthSync(g);
      hearthEmit();
    },
    hearthRemoveMember: async (id, pk) => {
      hearthCalls.push({ kind: "RemoveMember", args: [id, pk] });
      const g = hearthCard(id);
      g.members = g.members.filter((m) => m.public_key !== pk);
      hearthSync(g);
      hearthEmit();
    },
    hearthLeave: async (id) => {
      hearthCalls.push({ kind: "Leave", args: [id] });
      hearthCard(id);
      hearthGroups.delete(id);
      hearthEmit();
    },
    hearthVote: async (id, proposalId) => {
      hearthCalls.push({ kind: "Vote", args: [id, proposalId] });
      const g = hearthCard(id);
      const p = g.proposals.find((x) => x.id === proposalId);
      if (!p) throw new Error("Cette décision n'existe plus.");
      p.approvals += 1;
      p.i_voted = true;
      if (p.approvals >= p.needed) {
        g.proposals = g.proposals.filter((x) => x.id !== proposalId);
        if (p.kind === "continue_without") g.blocker = null;
      }
      hearthEmit();
    },
    hearthContinueWithout: async (id, pk) => {
      hearthCalls.push({ kind: "ContinueWithout", args: [id, pk] });
      const g = hearthCard(id);
      g.proposals = [...g.proposals, { id: `prop-${++hearthN}`, kind: "continue_without", subject: liteOf(pk), approvals: 1, needed: Math.floor(g.members.length / 2) + 1, i_voted: true, can_vote: true }];
      hearthEmit();
    },
    hearthPackUpdate: async (id, instanceId) => {
      hearthCalls.push({ kind: "PackUpdate", args: [id, instanceId] });
      need(instanceId);
      const g = hearthCard(id);
      g.proposals = [...g.proposals, { id: `prop-${++hearthN}`, kind: "pack_update", subject: null, approvals: 1, needed: Math.floor(g.members.length / 2) + 1, i_voted: true, can_vote: true }];
      hearthEmit();
    },
    hearthRestore: async (id, versionId) => {
      hearthCalls.push({ kind: "Restore", args: [id, versionId] });
      const g = hearthCard(id);
      if (!g.versions.some((v) => v.id === versionId)) throw new Error("Version introuvable.");
      newVersion(g, "restore");
      hearthEmit();
    },
    hearthResolveFork: async (id, versionId) => {
      hearthCalls.push({ kind: "ResolveFork", args: [id, versionId] });
      const g = hearthCard(id);
      if (!g.fork.some((b) => b.version_id === versionId)) throw new Error("Version introuvable.");
      g.fork = [];
      hearthPatch(id, { state: "idle" });
    },
    hearthSetBackground: async (id, on) => {
      hearthCalls.push({ kind: "SetBackground", args: [id, on] });
      hearthCard(id).settings.keep_background = on;
      hearthEmit();
    },
    hearthEula: async () => ({ accepted: hearthEula, url: "https://aka.ms/MinecraftEULA" }),
    hearthEulaAccept: async () => { hearthCalls.push({ kind: "EulaAccept", args: [] }); hearthEula = true; },
    hearthEnabled: async () => hearthOn,
    hearthSetEnabled: async (enabled) => { hearthCalls.push({ kind: "SetEnabled", args: [enabled] }); hearthOn = enabled; hearthEmit(); },
    onHearthState: (cb) => sub(listeners.hearthState, cb),
    onHearthInvites: (cb) => sub(listeners.hearthInvites, cb),
    onHearthHandover: (cb) => sub(listeners.hearthHandover, cb),
    onHearthHostPrompt: (cb) => sub(listeners.hearthHostPrompt, cb),
    seedHearth: (p, d) => {
      const g = makeHearth(p, d);
      hearthGroups.set(g.card.id, g);
      hearthEmit();
      return { ...g.card };
    },
    setHearthCard: (id, patch) => hearthPatch(id, patch),
    setHearthDetail: (id, patch) => {
      const g = hearthCard(id);
      Object.assign(g, patch);
      if (patch.members) hearthSync(g);
      hearthEmit();
    },
    emitHearthInvite: (p) => {
      const inv: HearthInvite = {
        name: "Serveur des potes", from: liteOf(friends[0]!.friend.public_key), members: 3, minecraft: "1.21.1", loader: "NeoForge",
        received_at: Math.floor(Date.now() / 1000), ...p,
      };
      hearthInvs = [...hearthInvs.filter((i) => i.group_id !== inv.group_id), inv];
      emit(listeners.hearthInvites, hearthInvs.map((i) => ({ ...i })));
      return { ...inv };
    },
    emitHearthHandover: (e) => emit(listeners.hearthHandover, e),
    emitHearthHostPrompt: (e) => emit(listeners.hearthHostPrompt, e),
    setHearthEula: (a) => { hearthEula = a; },
    hearthCalls,
    friendInvites: async () => invites.map((i) => ({ ...i })),
    friendInviteDismiss: async (id) => {
      invites = invites.filter((i) => i.id !== id);
      emit(listeners.invites, invites.map((i) => ({ ...i })));
    },
    friendJoinRequest: async (pk) => {
      if (!friends.some((f) => f.friend.public_key === pk)) throw new Error("Ami introuvable.");
      joinRequestCalls.push({ kind: "ask", arg: pk });
      if (joinAnswer !== "hold") return joinAnswer;
      return new Promise<JoinAnswer>((resolve) => { joinHeld.set(pk, resolve); });
    },
    friendJoinRequestAnswer: async (pk, accept) => {
      joinRequestCalls.push({ kind: "answer", arg: pk, accept });
      if (!joinRequests.some((r) => r.public_key === pk)) throw new Error("Cette demande n'est plus valable.");
      joinRequests = joinRequests.filter((r) => r.public_key !== pk);
      emit(listeners.joinRequests, joinRequests.map((r) => ({ ...r })));
    },
    friendJoinRequests: async () => joinRequests.map((r) => ({ ...r })),
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
      manifest: makeManifest(), manifest_hash: FAKE_MANIFEST_HASH, cached: 91, to_receive: 3, to_receive_bytes: bytes_total,
      unknown_mods: ["mods/mod-maison.jar"],
    }),
    joinPlay: async () => {},
    // Comme le backend : refuse un autre manifeste, et des mods inconnus sans la case de confiance.
    joinStart: async (_pk, manifestHash, acceptUnknown) => {
      if (manifestHash !== FAKE_MANIFEST_HASH) throw new Error("La partie de ton ami a changé depuis l'affichage du plan : vérifie le nouveau plan avant de rejoindre.");
      if (!acceptUnknown) throw new Error("Des mods inconnus ou non vérifiés demandent ton accord : ouvre « Rejoindre » dans Cubelink pour les voir et les accepter.");
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
    emitJoinRequest: (p) => {
      const f = friends.find((x) => x.friend.public_key === p.public_key);
      const now = Math.floor(Date.now() / 1000);
      const r: JoinRequest = { name: f?.friend.name ?? "Ami", received_at: now, expires_at: now + 120, ...p };
      joinRequests = [...joinRequests.filter((x) => x.public_key !== r.public_key), r];
      emit(listeners.joinRequest, { ...r });
      emit(listeners.joinRequests, joinRequests.map((x) => ({ ...x })));
      return { ...r };
    },
    setJoinAnswer: (a) => { joinAnswer = a; },
    resolveJoinRequest: (pk, a) => {
      const res = joinHeld.get(pk);
      joinHeld.delete(pk);
      res?.(a);
    },
    joinRequestCalls,
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
    onFriendJoinRequest: (cb) => sub(listeners.joinRequest, cb),
    onFriendJoinRequests: (cb) => sub(listeners.joinRequests, cb),
    onUnknownMods: (cb) => sub(listeners.unknown, cb),
    onTransferErrorDetails: () => () => {},
    onBrowseProgress: (cb) => sub(listeners.browse, cb),
    onModsFixProgress: (cb) => sub(listeners.fix, cb),
    onFilesDropped: (cb) => sub(listeners.drop, cb),
  };
}
