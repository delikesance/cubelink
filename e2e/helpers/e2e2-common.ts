// Helpers E2E-2 (T3 installation, T4 lancement) : amitie, hote volumineux, relance de l'app A, parcours UI.
import { chromium, expect, type Page } from '@playwright/test';
import { spawn, execFileSync } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { invoke, testState, loginOffline, waitForRoute, ROOT, type App, type World } from '../harness/fixtures';

export const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const FAKE_JAVA = path.join(ROOT, 'tests-fixtures', 'mojang', 'fake-java.cmd');

export const sha256File = (p: string) => crypto.createHash('sha256').update(fs.readFileSync(p)).digest('hex');

/** Amis deja acceptes : B envoie la demande avec le code de A, A accepte. Renvoie les cles publiques. */
export async function makeFriends(a: App, b: App): Promise<{ keyA: string; keyB: string }> {
  const code = await invoke<string>(a.page, 'friend_my_code');
  await invoke(b.page, 'friend_send_request', { code });
  let req: { public_key: string } | undefined;
  for (let i = 0; i < 50 && !req; i++) {
    const r = await invoke<{ items: { public_key: string }[] }>(a.page, 'friend_requests', { offset: 0, limit: 10 });
    req = r.items[0];
    if (!req) await sleep(200);
  }
  if (!req) throw new Error('A n\'a pas recu la demande d\'ami');
  await invoke(a.page, 'friend_accept', { public_key: req.public_key });
  return { keyA: '', keyB: req.public_key };
}

/** Cle publique de A telle que vue par B (liste d'amis de B). */
export async function friendKeyOnB(b: App): Promise<string> {
  for (let i = 0; i < 50; i++) {
    const l = await invoke<{ friend: { public_key: string } }[]>(b.page, 'friend_list');
    if (l[0]) return l[0].friend.public_key;
    await sleep(200);
  }
  throw new Error('B n\'a pas d\'ami');
}

export const browsers: { close(): Promise<void> }[] = [];

export interface BigFile { path: string; size: number; sha256: string }

/**
 * Instance hote a fichiers volumineux (mods/*.jar aleatoires, faux zip) pour avoir le temps d'agir.
 * `sizeMb` par fichier, `count` fichiers. Ecrit instance.json au format StoredInstance.
 */
export async function makeBigHost(app: App, opts: { count?: number; sizeMb?: number; id?: string } = {}) {
  const count = opts.count ?? 5;
  const sizeMb = opts.sizeMb ?? 8;
  const id = opts.id ?? 'host-world';
  const st = await testState(app.page);
  const dir = path.join(st.paths.instances, id);
  const gameDir = path.join(dir, 'minecraft');
  fs.mkdirSync(path.join(gameDir, 'mods'), { recursive: true });
  const files: BigFile[] = [];
  for (let i = 0; i < count; i++) {
    const rel = `mods/e2e-big-${i + 1}.jar`;
    const buf = crypto.randomBytes(sizeMb * 1024 * 1024 + i * 4096 + 17);
    fs.writeFileSync(path.join(gameDir, ...rel.split('/')), buf);
    files.push({ path: rel, size: buf.length, sha256: crypto.createHash('sha256').update(buf).digest('hex') });
  }
  files.sort((x, y) => (x.path < y.path ? -1 : 1));
  const meta = {
    id, name: 'Monde de test', minecraft: '1.21.1-test',
    loader: { loader: 'vanilla', version: '' }, ram_mb: 2048, mod_count: count,
    created_at: Math.floor(Date.now() / 1000), kind: 'own',
  };
  fs.writeFileSync(path.join(dir, 'instance.json'), JSON.stringify(meta, null, 2));
  app.hostInstance = { id, dir, gameDir };
  const list = await invoke<{ id: string }[]>(app.page, 'instances_list');
  if (!list.some((i) => i.id === id)) throw new Error(`instance ${id} non declaree`);
  return { id, dir, gameDir, files, total: files.reduce((s, f) => s + f.size, 0) };
}

/** Prepare A (hote, instance volumineuse partagee) et B (ami accepte) ; renvoie la cle de A vue par B. */
export async function setupHostAndGuest(world: Pick<World, 'appA' | 'appB' | 'servers' | 'binary'>, bigOpts?: { count?: number; sizeMb?: number }, extraEnv: Record<string, string> = {}) {
  const { appA, appB } = world;
  // Le hook de test annonce les adresses directes (relais coupe) : un redemarrage n'est utile que pour l'environnement propre au test.
  for (const app of [appA, appB]) {
    await waitForRoute(app.page, 'login');
    const s = await invoke<Record<string, unknown>>(app.page, 'settings_get');
    await invoke(app.page, 'settings_set', { ...s, close_on_launch: false });
    if (Object.keys(extraEnv).length) browsers.push(await restartApp(app, world, 'disabled', extraEnv));
  }
  await waitForRoute(appA.page, 'login'); await waitForRoute(appB.page, 'login');
  await loginOffline(appA.page, 'Hote');
  await loginOffline(appB.page, 'Invite');
  const host = await makeBigHost(appA, bigOpts);
  await makeFriends(appA, appB);
  const keyA = await friendKeyOnB(appB);
  await invoke(appA.page, 'host_start_sharing', { instance_id: host.id, world_port: 25599 });
  return { host, keyA };
}

/** Parcours UI de B : Ecran Amis -> Rejoindre l'ami -> Rejoindre (coche la confiance si demandee) -> Installation. */
export async function waitOnline(page: Page, timeout = 60_000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    const l = await invoke<{ presence: string }[]>(page, 'friend_list');
    if (l[0]?.presence === 'online') return;
    await sleep(500);
  }
  throw new Error('ami jamais en ligne');
}

export async function startJoinViaUi(page: Page) {
  await waitOnline(page);
  await page.evaluate(() => { window.location.hash = '#/main'; });
  await sleep(300);
  await page.evaluate(() => { window.location.hash = '#/friends'; });
  const joinBtn = page.getByRole('button', { name: /^Rejoindre / });
  await expect(joinBtn).toBeVisible({ timeout: 30_000 });
  await joinBtn.click();
  const go = page.getByRole('button', { name: 'Rejoindre et jouer' });
  await expect(go).toBeVisible({ timeout: 30_000 });
  const trust = page.getByRole('checkbox');
  if (await trust.count()) await trust.check();
  await go.click();
  await expect(page.locator('#inst-title')).toBeVisible({ timeout: 15_000 });
}

export interface Progress { phase: string; files_done: number; files_total: number; bytes_done: number; bytes_total: number }

/** Lit la barre de progression : valuenow + valuetext. */
export async function barState(page: Page) {
  const bar = page.getByRole('progressbar');
  const now = Number(await bar.getAttribute('aria-valuenow'));
  const text = (await bar.getAttribute('aria-valuetext')) ?? '';
  return { now, text };
}

/** Suspend / reprend un processus Windows (NtSuspendProcess via PowerShell + P/Invoke). */
const SUSPEND_PS = (pid: number, fn: 'NtSuspendProcess' | 'NtResumeProcess') => `
$sig='[DllImport("ntdll.dll")] public static extern int ${fn}(IntPtr h);'
$t=Add-Type -MemberDefinition $sig -Name N${fn} -Namespace W -PassThru
$p=[System.Diagnostics.Process]::GetProcessById(${pid})
[void]$t::${fn}($p.Handle)`;
export function suspendProcess(pid: number) { execFileSync('powershell', ['-NoProfile', '-Command', SUSPEND_PS(pid, 'NtSuspendProcess')], { stdio: 'ignore' }); }
export function resumeProcess(pid: number) { execFileSync('powershell', ['-NoProfile', '-Command', SUSPEND_PS(pid, 'NtResumeProcess')], { stdio: 'ignore' }); }

function killTree(pid: number | undefined) {
  if (!pid) return;
  try { execFileSync('taskkill', ['/PID', String(pid), '/T', '/F'], { stdio: 'ignore' }); } catch { /* deja termine */ }
}

/**
 * Arrete l'app (arbre complet) puis la relance avec le MEME dossier de donnees (meme identite) et le meme profil WebView2.
 * Met a jour app.proc / app.page ; le nettoyage de la fixture tue le nouveau processus.
 */
export async function restartApp(app: App, world: Pick<World, 'servers' | 'binary'>, relayMode = 'disabled', extraEnv: Record<string, string> = {}) {
  const { servers, binary } = world;
  killTree(app.proc.pid);
  await sleep(1500);
  try {
    execFileSync('powershell', ['-NoProfile', '-Command',
      `Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" | Where-Object { $_.CommandLine -like '*${app.userDataDir.replace(/'/g, "''")}*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`],
      { stdio: 'ignore', timeout: 20_000 });
  } catch { /* meilleur effort */ }
  await sleep(500);
  const env: Record<string, string> = {
    ...(process.env as Record<string, string>),
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${app.cdpPort} --remote-allow-origins=*`,
    WEBVIEW2_USER_DATA_FOLDER: app.userDataDir,
    CUBELINK_TEST_MODE: '1', CUBELINK_DATA_DIR: app.dataDir,
    CUBELINK_AUTH_MS_URL: servers.auth.base, CUBELINK_AUTH_XBOX_URL: servers.auth.base,
    CUBELINK_AUTH_XSTS_URL: servers.auth.base, CUBELINK_AUTH_MC_URL: servers.auth.base,
    CUBELINK_MOJANG_META_URL: servers.mojang.base, CUBELINK_FABRIC_META_URL: servers.mojang.base,
    CUBELINK_ADOPTIUM_URL: servers.mojang.base, CUBELINK_MODRINTH_API_URL: servers.modrinth.base,
    CUBELINK_JAVA_PATH: FAKE_JAVA, CUBELINK_SKIP_JAVA_PROBE: '1', CUBELINK_RELAY_MODE: relayMode, ...extraEnv,
  };
  const log: string[] = [];
  const proc = spawn(binary, [], { env, stdio: ['ignore', 'pipe', 'pipe'], windowsHide: false, cwd: ROOT });
  proc.stdout!.on('data', (d) => log.push(String(d)));
  proc.stderr!.on('data', (d) => log.push(String(d)));
  app.proc = proc;
  app.log = log;
  const deadline = Date.now() + 60_000;
  while (Date.now() < deadline) {
    try { if ((await fetch(`http://127.0.0.1:${app.cdpPort}/json/version`)).ok) break; } catch { /* pas encore */ }
    await sleep(250);
  }
  const browser = await chromium.connectOverCDP(`http://127.0.0.1:${app.cdpPort}`);
  let page: Page | undefined;
  while (!page && Date.now() < deadline + 30_000) {
    page = browser.contexts().flatMap((c) => c.pages()).find((p) => !p.url().startsWith('devtools:'));
    if (!page) await sleep(250);
  }
  if (!page) throw new Error('aucune page apres relance');
  await page.waitForLoadState('domcontentloaded');
  app.page = page;
  return browser;
}

/** Enregistre tous les evenements d'un nom donne dans window.__rec[name] (ecoute Tauri brute, sans passer par l'UI). */
export async function recordEvents(page: Page, event: string) {
  await page.evaluate(async (ev) => {
    const w = window as any;
    w.__rec = w.__rec ?? {};
    w.__rec[ev] = w.__rec[ev] ?? [];
    const handler = w.__TAURI_INTERNALS__.transformCallback((e: { payload: unknown }) => { w.__rec[ev].push({ t: Date.now(), ...(e.payload as object) }); });
    await w.__TAURI_INTERNALS__.invoke('plugin:event|listen', { event: ev, target: { kind: 'Any' }, handler });
  }, event);
}
export async function events<T = any>(page: Page, event: string): Promise<T[]> {
  return page.evaluate((ev) => ((window as any).__rec?.[ev] ?? []) as any[], event);
}
export async function lastProgress(page: Page): Promise<Progress & { error?: string }> {
  const l = await events<Progress>(page, 'transfer://progress');
  return l[l.length - 1] as Progress;
}
export async function waitProgress(page: Page, pred: (p: Progress) => boolean, timeout = 60_000, what = 'progression') {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    const l = await events<Progress>(page, 'transfer://progress');
    const hit = l.find(pred);
    if (hit) return hit;
    await sleep(50);
  }
  throw new Error(`delai depasse : ${what} ; dernier = ${JSON.stringify(await lastProgress(page))}`);
}

/** Dossier de jeu de l'instance invitee de B (la seule instance de kind guest) : { id, gameDir }. */
export async function guestInstance(app: App): Promise<{ id: string; gameDir: string } | null> {
  const st = await testState(app.page);
  // Les instances d'invite sont volontairement absentes de instances_list : on les cherche sur disque.
  const id = fs.existsSync(st.paths.instances) ? fs.readdirSync(st.paths.instances).find((d) => fs.existsSync(path.join(st.paths.instances, d, 'guest.json'))) : undefined;
  return id ? { id, gameDir: path.join(st.paths.instances, id, 'minecraft') } : null;
}

/** Liste recursive de fichiers dont le chemin satisfait `pred`. */
export function walk(dir: string, pred: (p: string) => boolean = () => true): string[] {
  const out: string[] = [];
  const rec = (d: string) => {
    let ents: fs.Dirent[] = [];
    try { ents = fs.readdirSync(d, { withFileTypes: true }); } catch { return; }
    for (const e of ents) { const p = path.join(d, e.name); if (e.isDirectory()) rec(p); else if (pred(p)) out.push(p); }
  };
  rec(dir);
  return out;
}
