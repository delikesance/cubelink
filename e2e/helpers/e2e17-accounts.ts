// Helpers E2E-17 : une application reelle que l'on peut FERMER puis RELANCER sur le meme dossier de donnees
// (le harnais `world` ne sait que demarrer deux instances jetables). Tout vit dans un dossier temporaire :
// jamais le dossier de donnees reel, jamais de vrais identifiants ; faux serveur d'authentification local.
import { chromium, type Browser, type Page } from '@playwright/test';
import { spawn, execFileSync, type ChildProcess } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
export const ROOT = path.resolve(HERE, '..', '..');
const FIXTURES = path.join(ROOT, 'tests-fixtures');
export const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const s = net.createServer();
    s.once('error', reject);
    s.listen(0, '127.0.0.1', () => { const p = (s.address() as net.AddressInfo).port; s.close(() => resolve(p)); });
  });
}

function killTree(pid: number | undefined) {
  if (!pid) return;
  try { execFileSync('taskkill', ['/PID', String(pid), '/T', '/F'], { stdio: 'ignore' }); } catch { /* deja termine */ }
}

/** Tue les msedgewebview2 dont la ligne de commande reference ce dossier de profil de test (et rien d'autre). */
function killWebViewsOf(dir: string) {
  const needle = dir.replace(/'/g, "''");
  try {
    execFileSync('powershell', ['-NoProfile', '-Command',
      `Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" | Where-Object { $_.CommandLine -like '*${needle}*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`],
      { stdio: 'ignore', timeout: 20_000 });
  } catch { /* meilleur effort */ }
}

export interface RunningApp { proc: ChildProcess; page: Page; browser: Browser; log: string[] }

export class Sandbox {
  readonly root = fs.mkdtempSync(path.join(os.tmpdir(), 'cubelink-e2e17-'));
  readonly dataDir = path.join(this.root, 'data');
  readonly webview = path.join(this.root, 'webview');
  authBase = '';
  private authProc?: ChildProcess;
  private current?: RunningApp;

  constructor(readonly binary: string) {
    fs.mkdirSync(this.dataDir, { recursive: true });
    fs.mkdirSync(this.webview, { recursive: true });
  }

  /** Faux serveur Microsoft/Xbox/XSTS/Minecraft (tests-fixtures/auth/server.mjs). */
  async startAuth(): Promise<void> {
    const proc = spawn(process.execPath, [path.join(FIXTURES, 'auth', 'server.mjs'), '--port', '0'], { stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true });
    this.authProc = proc;
    this.authBase = await new Promise<string>((resolve, reject) => {
      const t = setTimeout(() => reject(new Error('faux serveur auth : pas de ligne ready en 15 s')), 15_000);
      let buf = '';
      proc.stdout!.on('data', (d) => {
        buf += d;
        for (const line of buf.split('\n')) { try { const j = JSON.parse(line); if (j.ready) { clearTimeout(t); resolve(j.base); return; } } catch { /* pas JSON */ } }
      });
      proc.once('exit', (c) => { clearTimeout(t); reject(new Error(`faux serveur auth termine (code ${c})`)); });
    });
  }

  /** Lance l'application sur ce dossier de donnees. Toujours en mode test : sans CUBELINK_TEST_MODE=1, CUBELINK_DATA_DIR est IGNORE
   *  et l'app ouvrirait le vrai dossier de l'utilisateur (incident deja survenu, ne jamais retirer ce garde-fou). */
  async launch(): Promise<RunningApp> {
    const testMode = true;
    const cdp = await freePort();
    const env: Record<string, string> = {
      ...(process.env as Record<string, string>),
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${cdp} --remote-allow-origins=*`,
      WEBVIEW2_USER_DATA_FOLDER: this.webview,
      CUBELINK_DATA_DIR: this.dataDir,
      CUBELINK_RELAY_MODE: 'disabled',
    };
    for (const k of Object.keys(env)) if (k === 'CUBELINK_TEST_MODE' || k.startsWith('CUBELINK_AUTH_')) delete env[k];
    if (testMode) {
      env.CUBELINK_TEST_MODE = '1';
      for (const k of ['MS', 'XBOX', 'XSTS', 'MC']) env[`CUBELINK_AUTH_${k}_URL`] = this.authBase;
    }
    const log: string[] = [];
    const proc = spawn(this.binary, [], { env, stdio: ['ignore', 'pipe', 'pipe'], windowsHide: false, cwd: ROOT });
    proc.stdout!.on('data', (d) => log.push(String(d)));
    proc.stderr!.on('data', (d) => log.push(String(d)));
    const deadline = Date.now() + 60_000;
    for (;;) {
      if (proc.exitCode !== null) throw new Error(`application arretee (code ${proc.exitCode}) :\n${log.join('')}`);
      try { if ((await fetch(`http://127.0.0.1:${cdp}/json/version`)).ok) break; } catch { /* pas encore */ }
      if (Date.now() > deadline) { killTree(proc.pid); throw new Error('CDP indisponible apres 60 s'); }
      await sleep(100);
    }
    const browser = await chromium.connectOverCDP(`http://127.0.0.1:${cdp}`);
    let page: Page | undefined;
    const d2 = Date.now() + 30_000;
    while (!page && Date.now() < d2) {
      page = browser.contexts().flatMap((c) => c.pages()).find((p) => !p.url().startsWith('devtools:'));
      if (!page) await sleep(100);
    }
    if (!page) throw new Error('aucune page WebView2');
    await page.waitForLoadState('domcontentloaded');
    // Garde-fou : l'app DOIT utiliser le dossier temporaire (jamais le dossier reel de l'utilisateur), sinon on arrete tout.
    let st: { test_mode?: boolean; data_dir?: string } | null = null;
    for (let i = 0; i < 100 && !st; i++) {
      st = await page.evaluate(() => (window as any).__TAURI_INTERNALS__.invoke('test_state', {})).catch(() => null) as typeof st;
      if (!st) await sleep(100);
    }
    const norm = (d: string) => { try { return fs.realpathSync.native(d).toLowerCase(); } catch { return d.toLowerCase(); } };
    if (!st || st.test_mode !== true || norm(String(st.data_dir)) !== norm(this.dataDir)) {
      killTree(proc.pid);
      killWebViewsOf(this.webview);
      throw new Error(`data-dir inattendu (${st?.data_dir}) : attendu ${this.dataDir}. Arret par securite.`);
    }
    this.current = { proc, page, browser, log };
    return this.current;
  }

  /** Ferme l'application : demande de fermeture de la fenetre (WM_CLOSE) d'abord, arret force seulement si elle ne part pas en 8 s. Renvoie true si la sortie a ete propre. */
  async close(): Promise<boolean> {
    const app = this.current;
    if (!app) return true;
    this.current = undefined;
    try { await Promise.race([app.browser.close(), sleep(3000)]); } catch { /* ignore */ }
    let clean = app.proc.exitCode !== null;
    if (!clean) {
      try { execFileSync('taskkill', ['/PID', String(app.proc.pid)], { stdio: 'ignore' }); } catch { /* ignore */ }
      const end = Date.now() + 8000;
      while (app.proc.exitCode === null && Date.now() < end) await sleep(100);
      clean = app.proc.exitCode !== null;
      if (!clean) killTree(app.proc.pid);
    }
    killWebViewsOf(this.webview);
    await sleep(500);
    return clean;
  }

  async dispose(): Promise<void> {
    await this.close();
    killTree(this.authProc?.pid);
    try { fs.rmSync(this.root, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 }); } catch (e) { console.warn(`[e2e17] nettoyage impossible : ${(e as Error).message}`); }
  }

  get authDir(): string { return path.join(this.dataDir, 'auth'); }
}

// ---------- fichiers de comptes (format de core/src/auth/tokens.rs : "CLT1" || nonce(12) || AES-256-GCM(aad="CLT1") || tag) ----------
const MAGIC = Buffer.from('CLT1');

function authKey(authDir: string): Buffer {
  fs.mkdirSync(authDir, { recursive: true });
  const f = path.join(authDir, 'tokens.key');
  if (!fs.existsSync(f)) fs.writeFileSync(f, crypto.randomBytes(32));
  return fs.readFileSync(f);
}

export function encryptBlob(authDir: string, plain: object): Buffer {
  const nonce = crypto.randomBytes(12);
  const c = crypto.createCipheriv('aes-256-gcm', authKey(authDir), nonce);
  c.setAAD(MAGIC);
  const ct = Buffer.concat([c.update(JSON.stringify(plain), 'utf8'), c.final()]);
  return Buffer.concat([MAGIC, nonce, ct, c.getAuthTag()]);
}

const FAR = 4_102_444_800; // an 2100
export const ONLINE_A = { name: 'AncienJoueur', uuid: '11111111111141118111111111111111' };
export const ONLINE_B = { name: 'SecondEnLigne', uuid: '22222222222242228222222222222222' };

/** Ancien format : auth/tokens.bin (un seul compte en ligne). `expired` : jetons perimes (refresh via le faux serveur au demarrage). */
export function writeLegacyTokens(sb: Sandbox, p = ONLINE_A, expired = false): void {
  const exp = expired ? 1 : FAR;
  fs.mkdirSync(sb.authDir, { recursive: true });
  fs.writeFileSync(path.join(sb.authDir, 'tokens.bin'), encryptBlob(sb.authDir, {
    ms_refresh_token: 'fake-ms-refresh-1', ms_expires_at: exp, minecraft_token: 'fake-mc-access-1', minecraft_expires_at: exp,
    profile: { kind: 'online', name: p.name, uuid: p.uuid },
  }));
}

/** Nouveau format : auth/accounts.bin avec des comptes en ligne (jetons valides) deja presents. */
export function writeAccounts(sb: Sandbox, ps: { name: string; uuid: string }[], active: string): void {
  fs.mkdirSync(sb.authDir, { recursive: true });
  fs.writeFileSync(path.join(sb.authDir, 'accounts.bin'), encryptBlob(sb.authDir, {
    version: 1, active,
    accounts: ps.map((p) => ({ id: p.uuid, profile: { kind: 'online', name: p.name, uuid: p.uuid },
      tokens: { ms_refresh_token: 'fake-ms-refresh-1', ms_expires_at: FAR, minecraft_token: 'fake-mc-access-1', minecraft_expires_at: FAR } })),
  }));
}

export async function resolveBinary(): Promise<string> {
  if (process.env.CUBELINK_E2E_BIN) return process.env.CUBELINK_E2E_BIN;
  const { binaryPath } = await import(pathToFileURL(path.join(HERE, '..', 'harness', 'build.mjs')).href);
  return binaryPath({});
}
