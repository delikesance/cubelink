// Helpers E2E-14 : VRAI Minecraft (E2E_MC=1). Un data-dir temporaire reutilise (copie/jonction) les bibliotheques,
// versions et assets deja telecharges par l'utilisateur ; ses vraies donnees ne sont JAMAIS modifiees.
import { chromium, type Browser, type Page } from '@playwright/test';
import { spawn, execFileSync, type ChildProcess } from 'node:child_process';
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
export const REAL_DATA = path.join(process.env.APPDATA ?? '', 'app.cubelink.launcher');
export const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');
export const AGENT_JAR = path.join(ROOT, 'core', 'resources', 'cubelink-lan-agent.jar');

export function findJava25(): string {
  if (process.env.E2E_JAVA) return process.env.E2E_JAVA;
  const base = 'C:\\Program Files\\Microsoft';
  const d = fs.existsSync(base) ? fs.readdirSync(base).filter((n) => /^jdk-2[5-9]/.test(n)).sort().pop() : undefined;
  if (!d) throw new Error('JDK 25 introuvable : definis E2E_JAVA');
  return path.join(base, d, 'bin', 'java.exe');
}

async function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const s = net.createServer();
    s.once('error', reject);
    s.listen(0, '127.0.0.1', () => { const p = (s.address() as net.AddressInfo).port; s.close(() => resolve(p)); });
  });
}

/** Copie (versions, loaders, libraries) ou jonction (assets, 1+ Go) depuis les donnees reelles + copie de l'instance. */
export function prepareDataDir(dataDir: string, instanceId: string | null): void {
  fs.mkdirSync(dataDir, { recursive: true });
  for (const d of ['versions', 'loaders', 'libraries']) fs.cpSync(path.join(REAL_DATA, d), path.join(dataDir, d), { recursive: true });
  fs.symlinkSync(path.join(REAL_DATA, 'assets'), path.join(dataDir, 'assets'), 'junction');
  fs.mkdirSync(path.join(dataDir, 'instances'), { recursive: true });
  if (instanceId) fs.cpSync(path.join(REAL_DATA, 'instances', instanceId),path.join(dataDir, 'instances', instanceId), {
    recursive: true,
    filter: (src) => !/[\\/](logs|crash-reports)([\\/]|$)/.test(src),
  });
}

export interface RealApp { proc: ChildProcess; page: Page; log: string[]; dataDir: string; userDataDir: string; browser: Browser; close(): Promise<void> }

export function killTree(pid: number | undefined) {
  if (!pid) return;
  try { execFileSync('taskkill', ['/PID', String(pid), '/T', '/F'], { stdio: 'ignore' }); } catch { /* deja termine */ }
}

/** Tue les processus (java.exe, msedgewebview2.exe) dont la ligne de commande contient CE dossier de test, et rien d'autre. */
export function killProcsOfDir(dir: string) {
  const needle = dir.replace(/'/g, "''");
  try {
    execFileSync('powershell', ['-NoProfile', '-Command',
      `Get-CimInstance Win32_Process | Where-Object { ($_.Name -eq 'java.exe' -or $_.Name -eq 'msedgewebview2.exe') -and $_.CommandLine -like '*${needle}*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`],
      { stdio: 'ignore', timeout: 30_000 });
  } catch { /* meilleur effort */ }
}

export function javaPidsOfDir(dir: string): number[] {
  const needle = dir.replace(/'/g, "''");
  try {
    const out = execFileSync('powershell', ['-NoProfile', '-Command',
      `Get-CimInstance Win32_Process -Filter "Name='java.exe'" | Where-Object { $_.CommandLine -like '*${needle}*' } | ForEach-Object { $_.ProcessId }`],
      { encoding: 'utf8', timeout: 30_000 });
    return out.split(/\s+/).filter(Boolean).map(Number);
  } catch { return []; }
}

/** Demarre l'app (mode test, hors ligne, aucun faux serveur : vraies URLs Mojang/Fabric) et connecte la page. */
export async function startRealApp(binary: string, tmp: string, dataDir: string, env: Record<string, string>): Promise<RealApp> {
  const userDataDir = path.join(tmp, 'webview');
  fs.mkdirSync(userDataDir, { recursive: true });
  const cdpPort = await freePort();
  const log: string[] = [];
  const proc = spawn(binary, [], {
    cwd: ROOT, stdio: ['ignore', 'pipe', 'pipe'], windowsHide: false,
    env: {
      ...(process.env as Record<string, string>),
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${cdpPort} --remote-allow-origins=*`,
      WEBVIEW2_USER_DATA_FOLDER: userDataDir,
      CUBELINK_TEST_MODE: '1',
      CUBELINK_DATA_DIR: dataDir,
      CUBELINK_RELAY_MODE: 'disabled',
      ...env,
    },
  });
  proc.stdout!.on('data', (d) => log.push(String(d)));
  proc.stderr!.on('data', (d) => log.push(String(d)));
  const deadline = Date.now() + 60_000;
  for (;;) {
    if (proc.exitCode !== null) throw new Error(`app arretee (code ${proc.exitCode}) :\n${log.join('')}`);
    try { if ((await fetch(`http://127.0.0.1:${cdpPort}/json/version`)).ok) break; } catch { /* pas encore */ }
    if (Date.now() > deadline) throw new Error('CDP indisponible apres 60 s');
    await sleep(100);
  }
  const browser = await chromium.connectOverCDP(`http://127.0.0.1:${cdpPort}`);
  let page: Page | undefined;
  const d2 = Date.now() + 30_000;
  while (!page && Date.now() < d2) {
    page = browser.contexts().flatMap((c) => c.pages()).find((p) => !p.url().startsWith('devtools:'));
    if (!page) await sleep(100);
  }
  if (!page) throw new Error('aucune page WebView2');
  await page.waitForLoadState('domcontentloaded');
  return {
    proc, page, log, dataDir, userDataDir, browser,
    async close() {
      try { await Promise.race([browser.close(), sleep(3000)]); } catch { /* ignore */ }
      killProcsOfDir(dataDir);
      killTree(proc.pid);
      killProcsOfDir(userDataDir);
    },
  };
}

export function readLog(dataDir: string, id: string): string {
  try { return fs.readFileSync(path.join(dataDir, 'logs', id, 'latest.log'), 'utf8'); } catch { return ''; }
}

/** Attend qu'une regex apparaisse dans latest.log de l'instance ; renvoie la ligne. */
export async function waitLog(dataDir: string, id: string, re: RegExp, timeoutMs: number, abort?: () => string | null): Promise<string> {
  const end = Date.now() + timeoutMs;
  while (Date.now() < end) {
    const m = readLog(dataDir, id).split(/\r?\n/).find((l) => re.test(l));
    if (m) return m;
    const a = abort?.();
    if (a) throw new Error(a);
    await sleep(1000);
  }
  throw new Error(`delai depasse (${timeoutMs} ms) : ${re}\n--- fin de latest.log ---\n${readLog(dataDir, id).split(/\r?\n/).slice(-40).join('\n')}`);
}

export function tmpRoot(prefix: string): string { return fs.mkdtempSync(path.join(os.tmpdir(), prefix)); }
