// Helpers E2E-3 (T5 mod inconnu, T6 echecs) : amitie par commandes, partage hote, evenements, redemarrage de l'hote.
import { ROOT, expect, inviteAllFriends, invoke, loginOffline, makeHostInstance, startFakeWorld, testState, type App, type World } from '../harness/fixtures';
import { spawn } from 'node:child_process';
import { chromium } from '@playwright/test';
import { execFileSync } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';

export const NAME_A = 'Alice';
export const NAME_B = 'Bob';
export const HOME_MOD = 'mods/homebrew-mod-0.0.1.jar';
export const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
export const sha256File = (p: string) => crypto.createHash('sha256').update(fs.readFileSync(p)).digest('hex');
export const sha1File = (p: string) => crypto.createHash('sha1').update(fs.readFileSync(p)).digest('hex');

export async function gotoHash(app: App, hash: string, route: string): Promise<void> {
  await app.page.evaluate((h) => { window.location.hash = h; }, hash);
  await app.page.waitForSelector(`main[data-route="${route}"]`, { timeout: 15_000 });
}

/** Amitie A <-> B par commandes (B demande, A accepte). Renvoie les cles publiques. */
export async function befriend(world: Pick<World, 'appA' | 'appB' | 'servers' | 'binary'>): Promise<{ keyA: string; keyB: string }> {
  const { appA: a, appB: b } = world;
  await Promise.all([a.page.waitForSelector('main[data-route="login"]'), b.page.waitForSelector('main[data-route="login"]')]);
  await loginOffline(a.page, NAME_A);
  await loginOffline(b.page, NAME_B);
  // Le code ne porte les adresses directes qu'une fois le point d'acces lie : on reessaie un court instant.
  await expect.poll(async () => {
    try { await invoke(b.page, 'friend_send_request', { code: await invoke<string>(a.page, 'friend_my_code') }); return 'ok'; } catch (e) { return String(e); }
  }, { timeout: 30_000, intervals: [500, 1000] }).toBe('ok');
  let reqs: { items: { public_key: string }[] } = { items: [] };
  await expect.poll(async () => (reqs = await invoke(a.page, 'friend_requests', { offset: 0, limit: 5 })).items.length, { timeout: 30_000 }).toBe(1);
  const keyB = reqs.items[0].public_key;
  await invoke(a.page, 'friend_accept', { public_key: keyB });
  let fl: { friend: { public_key: string } }[] = [];
  await expect.poll(async () => (fl = await invoke(b.page, 'friend_list')).length, { timeout: 20_000 }).toBe(1);
  await expect.poll(async () => (await invoke<unknown[]>(a.page, 'friend_list')).length, { timeout: 20_000 }).toBe(1);
  return { keyA: fl[0].friend.public_key, keyB };
}

/** Pose l'instance hote (+ fichier lourd optionnel), lance le faux monde et partage. */
export async function hostShare(a: App, opts: { bigMb?: number } = {}) {
  const host = await makeHostInstance(a);
  if (opts.bigMb) {
    const dir = path.join(host.gameDir, 'config');
    fs.mkdirSync(dir, { recursive: true });
    fs.writeFileSync(path.join(dir, 'big.bin'), crypto.randomBytes(opts.bigMb * 1024 * 1024));
  }
  const world = await startFakeWorld(a);
  await invoke(a.page, 'host_start_sharing', { instance_id: host.id, world_port: world.lanPort });
  // Règle de jonction : les amis déjà ajoutés sont invités pour cette partie.
  await inviteAllFriends(a);
  return { host, world };
}

/** Enregistre les evenements Tauri `names` dans window.__e2e3 (liste de {name, payload}). */
export async function captureEvents(app: App, names: string[]): Promise<void> {
  await app.page.evaluate(async (evs) => {
    const w = window as any;
    w.__e2e3 = w.__e2e3 ?? [];
    for (const event of evs) {
      const handler = w.__TAURI_INTERNALS__.transformCallback((e: any) => { w.__e2e3.push({ name: event, payload: e.payload }); });
      await w.__TAURI_INTERNALS__.invoke('plugin:event|listen', { event, target: { kind: 'Any' }, handler });
    }
  }, names);
}
export const events = (app: App, name: string) =>
  app.page.evaluate((n) => ((window as any).__e2e3 ?? []).filter((e: any) => e.name === n).map((e: any) => e.payload), name);

/** Suspend / reprend un processus Windows (NtSuspendProcess) : coupure reseau sans perdre l'adresse de l'hote. */
const SUSPEND_PS = (pid: number, fn: 'NtSuspendProcess' | 'NtResumeProcess') => `
$sig='[DllImport("ntdll.dll")] public static extern int ${fn}(IntPtr h);'
$t=Add-Type -MemberDefinition $sig -Name N${fn} -Namespace W -PassThru
$p=[System.Diagnostics.Process]::GetProcessById(${pid})
[void]$t::${fn}($p.Handle)`;
export const suspendApp = (app: App) => { execFileSync('powershell', ['-NoProfile', '-Command', SUSPEND_PS(app.proc.pid!, 'NtSuspendProcess')], { stdio: 'ignore' }); };
export const resumeApp = (app: App) => { execFileSync('powershell', ['-NoProfile', '-Command', SUSPEND_PS(app.proc.pid!, 'NtResumeProcess')], { stdio: 'ignore' }); };

export function killApp(app: App): void {
  try { execFileSync('taskkill', ['/PID', String(app.proc.pid), '/T', '/F'], { stdio: 'ignore' }); } catch { /* deja arrete */ }
}

/**
 * Relance l'application avec le MEME dossier de donnees (meme identite), profil WebView2 et port CDP.
 * Met a jour app.proc / app.page ; le nettoyage de la fixture tue le nouveau processus.
 */
export async function restartApp(world: Pick<World, 'servers' | 'binary'>, app: App, relayMode = 'disabled'): Promise<void> {
  const { servers, binary } = world;
  killApp(app);
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
    CUBELINK_AUTH_MS_URL: servers.auth.base, CUBELINK_AUTH_XBOX_URL: servers.auth.base, CUBELINK_AUTH_XSTS_URL: servers.auth.base, CUBELINK_AUTH_MC_URL: servers.auth.base,
    CUBELINK_MOJANG_META_URL: servers.mojang.base, CUBELINK_FABRIC_META_URL: servers.mojang.base, CUBELINK_ADOPTIUM_URL: servers.mojang.base,
    CUBELINK_MODRINTH_API_URL: servers.modrinth.base,
    CUBELINK_JAVA_PATH: path.join(ROOT, 'tests-fixtures', 'mojang', 'fake-java.cmd'),
    CUBELINK_SKIP_JAVA_PROBE: '1', CUBELINK_RELAY_MODE: relayMode,
  };
  const proc = spawn(binary, [], { env, stdio: ['ignore', 'pipe', 'pipe'], windowsHide: false, cwd: ROOT });
  proc.stdout!.on('data', (d) => app.log.push(String(d)));
  proc.stderr!.on('data', (d) => app.log.push(String(d)));
  app.proc = proc;
  const deadline = Date.now() + 60_000;
  while (Date.now() < deadline) {
    try { if ((await fetch(`http://127.0.0.1:${app.cdpPort}/json/version`)).ok) break; } catch { /* pas encore */ }
    await sleep(250);
  }
  const browser = await chromium.connectOverCDP(`http://127.0.0.1:${app.cdpPort}`);
  openBrowsers.push(browser);
  let page;
  while (!page && Date.now() < deadline + 30_000) {
    page = browser.contexts().flatMap((c) => c.pages()).find((p) => !p.url().startsWith('devtools:'));
    if (!page) await sleep(250);
  }
  if (!page) throw new Error('aucune page apres relance');
  await page.waitForLoadState('domcontentloaded');
  app.page = page;
  await page.waitForSelector('main[data-route="login"]', { timeout: 30_000 });
}
export const openBrowsers: { close(): Promise<void> }[] = [];

export { loginOffline, testState };
