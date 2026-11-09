// Helpers E2E-1 (T1 ajout d'ami, T2 rejoindre) : pilotent la vraie UI via des selecteurs accessibles.
import { expect, invoke, loginOffline, type App, type World } from '../harness/fixtures';
import fs from 'node:fs';
import path from 'node:path';

export const NAME_A = 'Alice';
export const NAME_B = 'Bob';

export async function gotoRoute(app: App, route: string): Promise<void> {
  await app.page.evaluate((r) => { window.location.hash = `#/${r}`; }, route);
  await app.page.waitForSelector(`main[data-route="${route.split('?')[0]}"]`, { timeout: 15_000 });
}

const opened: { close(): Promise<void> }[] = [];
export async function closeOpened(): Promise<void> {
  for (const b of opened.splice(0)) { try { await Promise.race([b.close(), new Promise((r) => setTimeout(r, 3000))]); } catch { /* ignore */ } }
}

/** Connexion hors ligne des deux apps (le hook de test annonce les adresses directes : plus de redemarrage). */
export async function loginBoth(world: Pick<World, 'appA' | 'appB' | 'servers' | 'binary'>): Promise<void> {
  const { appA: a, appB: b } = world;
  await Promise.all([a.page.waitForSelector('main[data-route="login"]'), b.page.waitForSelector('main[data-route="login"]')]);
  await loginOffline(a.page, NAME_A);
  await loginOffline(b.page, NAME_B);
}

/** Code ami affiche sur l'ecran Amis (avec clic sur Copier, sans exiger le presse-papiers). */
export async function readMyCode(app: App): Promise<string> {
  await gotoRoute(app, 'friends');
  const code = app.page.getByLabel(/^Mon code ami : /);
  await expect(code).toBeVisible();
  const text = ((await code.textContent()) ?? '').trim();
  expect(text).toMatch(/^mcl1-/);
  await app.page.getByRole('button', { name: 'Copier', exact: true }).click();
  return text;
}

export async function openAddTab(app: App) {
  await gotoRoute(app, 'friends');
  await app.page.getByRole('tab', { name: 'Ajouter un ami' }).click();
  return app.page.getByLabel('Code ami de ton ami');
}

/** Colle le code dans « Ajouter un ami » et envoie la demande. */
export async function sendRequest(app: App, code: string): Promise<void> {
  const input = await openAddTab(app);
  await input.fill(code);
  await app.page.getByRole('button', { name: 'Envoyer la demande' }).click();
}

/** Etablit l'amitie A <-> B (B demande, A accepte). */
export async function makeFriends(a: App, b: App): Promise<void> {
  const codeA = await readMyCode(a);
  await sendRequest(b, codeA);
  await expect(b.page.getByLabel(/^Code de vérification : /)).toBeVisible({ timeout: 30_000 });
  await gotoRoute(a, 'friends');
  await a.page.getByRole('tab', { name: /Demandes/ }).click();
  await a.page.getByRole('button', { name: `Accepter ${NAME_B}` }).click({ timeout: 30_000 });
  await expect.poll(async () => (await invoke<unknown[]>(a.page, 'friend_list')).length, { timeout: 20_000 }).toBe(1);
  await expect.poll(async () => (await invoke<unknown[]>(b.page, 'friend_list')).length, { timeout: 20_000 }).toBe(1);
}

const B32 = 'abcdefghijklmnopqrstuvwxyz234567';
/** Cle publique (hex) contenue dans un code ami : version(1) epoch(4) cle(32), base32 sans remplissage. */
export function keyOfCode(code: string): string {
  const raw = code.replace(/^mcl1-/, '').replace(/-/g, '');
  let bits = '';
  for (const ch of raw) bits += B32.indexOf(ch).toString(2).padStart(5, '0');
  const bytes: number[] = [];
  for (let i = 0; i + 8 <= bits.length; i += 8) bytes.push(parseInt(bits.slice(i, i + 8), 2));
  return Buffer.from(bytes.slice(5, 37)).toString('hex');
}

/** Nombre de fichiers (recursif) sous un dossier, 0 s'il n'existe pas. */
export function countFiles(dir: string): number {
  if (!fs.existsSync(dir)) return 0;
  let n = 0;
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) n += e.isDirectory() ? countFiles(path.join(dir, e.name)) : 1;
  return n;
}
