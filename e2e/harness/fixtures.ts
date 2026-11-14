// Fixtures Playwright E2E Cubelink : deux instances reelles de l'application (profils A et B)
// pilotees par CDP (WebView2 = Chromium), contre de faux serveurs locaux.
import { test as base, chromium, expect, type Browser, type Page } from '@playwright/test';
import { spawn, execFileSync, type ChildProcess } from 'node:child_process';
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
export const ROOT = path.resolve(HERE, '..', '..');
export const ARTIFACTS = path.join(ROOT, 'e2e', 'artifacts');
const FIXTURES = path.join(ROOT, 'tests-fixtures');
const FAKE_JAVA = path.join(FIXTURES, 'mojang', 'fake-java.cmd');
const isWin = process.platform === 'win32';

// ---------- utilitaires ----------
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const s = net.createServer();
    s.once('error', reject);
    s.listen(0, '127.0.0.1', () => {
      const p = (s.address() as net.AddressInfo).port;
      s.close(() => resolve(p));
    });
  });
}

/** Tue un processus et tous ses descendants (WebView2 inclus). */
function killTree(pid: number | undefined) {
  if (!pid) return;
  try {
    if (isWin) execFileSync('taskkill', ['/PID', String(pid), '/T', '/F'], { stdio: 'ignore' });
    else process.kill(pid, 'SIGKILL');
  } catch { /* deja termine */ }
}

/** Filet de securite : tue les processus msedgewebview2 restants utilisant ce dossier de profil. */
function killWebViewsOf(userDataDirs: string[]) {
  if (!isWin || userDataDirs.length === 0) return;
  try {
    const cond = userDataDirs.map((d) => `$_.CommandLine -like '*${d.replace(/'/g, "''")}*'`).join(' -or ');
    execFileSync('powershell', ['-NoProfile', '-Command',
      `Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" | Where-Object { ${cond} } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`],
      { stdio: 'ignore', timeout: 20_000 });
  } catch { /* meilleur effort */ }
}

function rmDir(dir: string) {
  try { fs.rmSync(dir, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 }); } catch (e) {
    console.warn(`[e2e] nettoyage impossible de ${dir} : ${(e as Error).message}`);
  }
}

// ---------- faux serveurs ----------
export interface FakeServer { name: string; base: string; port: number; proc: ChildProcess; log: string[] }

async function startServer(name: string, script: string): Promise<FakeServer> {
  const proc = spawn(process.execPath, [path.join(FIXTURES, script), '--port', '0'], { stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true });
  const log: string[] = [];
  const ready = await new Promise<{ port: number; base: string }>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`faux serveur ${name} : pas de ligne ready en 15 s\n${log.join('')}`)), 15_000);
    let buf = '';
    proc.stdout!.on('data', (d) => {
      buf += d; log.push(String(d));
      for (const line of buf.split('\n')) {
        try { const j = JSON.parse(line); if (j.ready) { clearTimeout(timer); resolve(j); return; } } catch { /* pas JSON */ }
      }
    });
    proc.stderr!.on('data', (d) => log.push(String(d)));
    proc.once('exit', (c) => { clearTimeout(timer); reject(new Error(`faux serveur ${name} termine (code ${c})\n${log.join('')}`)); });
  });
  return { name, base: ready.base, port: ready.port, proc, log };
}

export interface Servers { auth: FakeServer; mojang: FakeServer; modrinth: FakeServer }

// ---------- application ----------
/** `settings` : champs de Settings ecrits dans settings.json AVANT le demarrage (relay_only, discovery... sont lus au demarrage du backend). */
export interface AppOptions { relayMode?: string; env?: Record<string, string>; settings?: Record<string, unknown> }

export interface App {
  label: 'A' | 'B';
  page: Page;
  dataDir: string;
  userDataDir: string;
  cdpPort: number;
  proc: ChildProcess;
  log: string[];
  extraProcs: ChildProcess[];
  /** Instance hote posee par makeHostInstance (si appelee). */
  hostInstance?: { id: string; dir: string; gameDir: string };
}

async function waitCdp(port: number, proc: ChildProcess, log: string[]): Promise<void> {
  const deadline = Date.now() + 60_000;
  while (Date.now() < deadline) {
    if (proc.exitCode !== null) throw new Error(`l'application s'est arretee (code ${proc.exitCode}) :\n${log.join('')}`);
    try {
      const r = await fetch(`http://127.0.0.1:${port}/json/version`);
      if (r.ok) return;
    } catch { /* pas encore */ }
    await sleep(100);
  }
  throw new Error(`CDP indisponible sur ${port} apres 60 s :\n${log.join('')}`);
}

async function startApp(label: 'A' | 'B', bin: string, servers: Servers, tmpRoot: string, opts: AppOptions, apps: App[], browsers: Browser[]): Promise<void> {
  const dataDir = path.join(tmpRoot, `data-${label}`);
  const userDataDir = path.join(tmpRoot, `webview-${label}`);
  fs.mkdirSync(dataDir, { recursive: true });
  fs.mkdirSync(userDataDir, { recursive: true });
  if (opts.settings) {
    const base = { language: 'fr', default_ram_mb: 4096, java_path: null, trust_default: 'ask', allow_relay: true, close_on_launch: false, relay_only: false, share_mods: true, discovery: true };
    fs.writeFileSync(path.join(dataDir, 'settings.json'), JSON.stringify({ ...base, ...opts.settings }));
  }
  const cdpPort = await freePort();
  const env: Record<string, string> = {
    ...(process.env as Record<string, string>),
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${cdpPort} --remote-allow-origins=*`,
    WEBVIEW2_USER_DATA_FOLDER: userDataDir,
    CUBELINK_TEST_MODE: '1',
    CUBELINK_DATA_DIR: dataDir,
    CUBELINK_AUTH_MS_URL: servers.auth.base,
    CUBELINK_AUTH_XBOX_URL: servers.auth.base,
    CUBELINK_AUTH_XSTS_URL: servers.auth.base,
    CUBELINK_AUTH_MC_URL: servers.auth.base,
    CUBELINK_MOJANG_META_URL: servers.mojang.base,
    CUBELINK_FABRIC_META_URL: servers.mojang.base,
    CUBELINK_ADOPTIUM_URL: servers.mojang.base,
    CUBELINK_MODRINTH_API_URL: servers.modrinth.base,
    CUBELINK_JAVA_PATH: FAKE_JAVA,
    CUBELINK_SKIP_JAVA_PROBE: '1',
    CUBELINK_RELAY_MODE: opts.relayMode ?? 'disabled',
    ...opts.env,
  };
  const log: string[] = [];
  const proc = spawn(bin, [], { env, stdio: ['ignore', 'pipe', 'pipe'], windowsHide: false, cwd: ROOT });
  proc.stdout!.on('data', (d) => log.push(String(d)));
  proc.stderr!.on('data', (d) => log.push(String(d)));
  const app = { label, dataDir, userDataDir, cdpPort, proc, log, extraProcs: [] as ChildProcess[] } as App;
  apps.push(app); // enregistre tout de suite : nettoye meme si la suite echoue
  await waitCdp(cdpPort, proc, log);
  const browser = await chromium.connectOverCDP(`http://127.0.0.1:${cdpPort}`);
  browsers.push(browser);
  const deadline = Date.now() + 30_000;
  let page: Page | undefined;
  while (!page && Date.now() < deadline) {
    page = browser.contexts().flatMap((c) => c.pages()).find((p) => !p.url().startsWith('devtools:'));
    if (!page) await sleep(100);
  }
  if (!page) throw new Error(`aucune page WebView2 pour ${label}`);
  await page.waitForLoadState('domcontentloaded');
  app.page = page;
}

// ---------- helpers ----------
/** Appelle une commande Tauri depuis la page (hors UI). */
export async function invoke<T = unknown>(page: Page, cmd: string, args: Record<string, unknown> = {}): Promise<T> {
  return page.evaluate(
    ([c, a]) => (window as any).__TAURI_INTERNALS__.invoke(c as string, a as Record<string, unknown>),
    [cmd, args] as [string, Record<string, unknown>],
  ) as Promise<T>;
}

/** Rejoindre comme l'écran « Rejoindre » : plan, puis `join_start` avec le hash du manifeste montré et la case de
 *  confiance cochée (exigée par le backend s'il y a des mods inconnus ou non vérifiés). */
export async function joinStart(page: Page, friendPublicKey: string): Promise<void> {
  const plan = await invoke<{ manifest_hash: string }>(page, 'join_plan', { friend_public_key: friendPublicKey });
  await invoke(page, 'join_start', { friend_public_key: friendPublicKey, manifest_hash: plan.manifest_hash, accept_unknown: true });
}

export async function testState(page: Page): Promise<any> { return invoke(page, 'test_state'); }

/** Attend que le routeur (hash) affiche la route : `<main data-route=...>`. */
export async function waitForRoute(page: Page, route: string, timeout = 15_000): Promise<void> {
  await page.waitForSelector(`main[data-route="${route}"]`, { timeout });
}

/** Connexion hors ligne via la commande (le mode test l'autorise), puis navigue vers `main` si demande. */
export async function loginOffline(page: Page, name: string, goMain = true): Promise<unknown> {
  const profile = await invoke(page, 'auth_offline_login', { name });
  if (goMain) {
    await page.evaluate(() => { window.location.hash = '#/main'; });
    await waitForRoute(page, 'main');
  }
  return profile;
}

/**
 * Pose l'instance hote dans le dossier de donnees de l'app : instance.json (lu par InstanceStore :
 * `<instances>/<id>/instance.json`, champs Instance + created_at + kind) et fichiers de
 * make-instance-fixture.mjs dans `<id>/minecraft/`. L'app la voit via `instances_list` (lecture disque a chaque appel).
 */
export async function makeHostInstance(app: App, opts: { id?: string; name?: string; minecraft?: string } = {}) {
  const id = opts.id ?? 'host-world';
  const st = await testState(app.page);
  const dir = path.join(st.paths.instances, id);
  const gameDir = path.join(dir, 'minecraft');
  fs.mkdirSync(gameDir, { recursive: true });
  const mod = await import(pathToFileURL(path.join(FIXTURES, 'modrinth', 'make-instance-fixture.mjs')).href);
  const manifest = mod.makeInstance(gameDir) as { files: { path: string; size: number; sha256: string }[]; excluded: string[] };
  const meta = {
    id, name: opts.name ?? 'Monde de test', minecraft: opts.minecraft ?? '1.21.1-test',
    loader: { loader: 'vanilla', version: '' }, ram_mb: 2048, mod_count: 0,
    created_at: Math.floor(Date.now() / 1000), kind: 'own',
  };
  fs.writeFileSync(path.join(dir, 'instance.json'), JSON.stringify(meta, null, 2));
  app.hostInstance = { id, dir, gameDir };
  const list = await invoke<{ id: string }[]>(app.page, 'instances_list');
  if (!list.some((i) => i.id === id)) throw new Error(`instance ${id} non declaree : ${JSON.stringify(list)}`);
  return { ...app.hostInstance, manifest };
}

/** Lance le faux java (monde LAN) dans le dossier de jeu de l'instance hote ; renvoie le port LAN. */
export async function startFakeWorld(app: App, env: Record<string, string> = {}): Promise<{ lanPort: number; proc: ChildProcess; witness: any }> {
  if (!app.hostInstance) throw new Error('makeHostInstance doit etre appele avant startFakeWorld');
  const gameDir = app.hostInstance.gameDir;
  const witnessFile = path.join(gameDir, '.cubelink-fake-launch.json');
  fs.rmSync(witnessFile, { force: true });
  const proc = spawn('cmd.exe', ['/c', FAKE_JAVA, '--gameDir', gameDir], {
    env: { ...process.env, FAKE_JAVA_LAN_PORT: '0', ...env }, stdio: 'ignore', windowsHide: true,
  });
  app.extraProcs.push(proc);
  const deadline = Date.now() + 15_000;
  while (Date.now() < deadline) {
    if (fs.existsSync(witnessFile)) {
      try { const w = JSON.parse(fs.readFileSync(witnessFile, 'utf8')); if (w.lan_port) return { lanPort: w.lan_port, proc, witness: w }; } catch { /* ecriture en cours */ }
    }
    await sleep(100);
  }
  throw new Error('faux java : temoin absent apres 15 s');
}

// ---------- fixtures ----------
export interface World { appA: App; appB: App; servers: Servers; binary: string }

async function dumpFailure(app: App, dir: string) {
  fs.mkdirSync(dir, { recursive: true });
  const tag = app.label;
  if (app.page) {
    try { await app.page.screenshot({ path: path.join(dir, `${tag}-screenshot.png`), timeout: 5000 }); } catch { /* page morte */ }
    try { fs.writeFileSync(path.join(dir, `${tag}-test_state.json`), JSON.stringify(await testState(app.page), null, 2)); } catch { /* idem */ }
  }
  fs.writeFileSync(path.join(dir, `${tag}-app.log`), app.log.join(''));
  try { fs.cpSync(path.join(app.dataDir, 'logs'), path.join(dir, `${tag}-data-logs`), { recursive: true }); } catch { /* pas de logs */ }
}

/** Option par test : `test.use({ appOptions: { relayMode: 'default' } })`. */
export const test = base.extend<{ appOptions: AppOptions; world: World; appA: App; appB: App; servers: Servers }, { binary: string }>({
  appOptions: [{}, { option: true }],

  binary: [async ({}, use) => {
    let bin = process.env.CUBELINK_E2E_BIN; // construit une seule fois par e2e/harness/global-setup.ts
    if (!bin) {
      const { binaryPath, build } = await import(pathToFileURL(path.join(HERE, 'build.mjs')).href);
      bin = process.env.CUBELINK_E2E_BUILD === '1' || !fs.existsSync(binaryPath({})) ? build({}) : binaryPath({});
    }
    await use(bin);
  }, { scope: 'worker', timeout: 900_000 }],

  world: async ({ binary, appOptions }, use, testInfo) => {
    const started: FakeServer[] = [];
    const apps: App[] = [];
    const browsers: Browser[] = [];
    const tmpRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'cubelink-e2e-'));
    try {
      const track = (s: FakeServer) => (started.push(s), s);
      const [auth, mojang, modrinth] = await Promise.all([
        startServer('auth', 'auth/server.mjs').then(track),
        startServer('mojang', 'mojang/server.mjs').then(track),
        startServer('modrinth', 'modrinth/server.mjs').then(track),
      ]);
      const servers = { auth, mojang, modrinth };
      await Promise.all([
        startApp('A', binary, servers, tmpRoot, appOptions, apps, browsers),
        startApp('B', binary, servers, tmpRoot, appOptions, apps, browsers),
      ]);
      apps.sort((x, y) => x.label.localeCompare(y.label)); // l'ordre d'enregistrement n'est plus garanti
      await use({ appA: apps[0], appB: apps[1], servers, binary });
    } finally {
      if (testInfo.status !== testInfo.expectedStatus || apps.some((a) => !a.page)) {
        const dir = path.join(ARTIFACTS, testInfo.title.replace(/[^\w.-]+/g, '_').slice(0, 60) + '-' + Date.now());
        for (const a of apps) await dumpFailure(a, dir);
        for (const s of started) fs.writeFileSync(path.join(dir, `server-${s.name}.log`), s.log.join(''));
        console.log(`[e2e] artefacts d'echec : ${dir}`);
      }
      for (const b of browsers) { try { await Promise.race([b.close(), sleep(3000)]); } catch { /* ignore */ } }
      for (const a of apps) {
        for (const p of a.extraProcs) killTree(p.pid);
        killTree(a.proc.pid);
      }
      killWebViewsOf(apps.map((a) => a.userDataDir));
      for (const s of started) killTree(s.proc.pid);
      rmDir(tmpRoot); // maxRetries gere les verrous residuels : pas de sleep fixe
    }
  },
  appA: async ({ world }, use) => { await use(world.appA); },
  appB: async ({ world }, use) => { await use(world.appB); },
  servers: async ({ world }, use) => { await use(world.servers); },
});

export { expect };
