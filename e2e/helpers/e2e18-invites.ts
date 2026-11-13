// Helpers E2E-18 : invitations « rejoins mon monde » et panneau en jeu (Scene).
// Tout passe par le harnais `world` (deux applications reelles, dossiers de donnees temporaires, CUBELINK_TEST_MODE=1).
import type { Page } from '@playwright/test';
import { invoke, loginOffline, makeHostInstance, waitForRoute, type App, type World } from '../harness/fixtures';
import { makeFriends, friendKeyOnB, waitOnline, sleep } from './e2e2-common';

export { sleep };

export interface Invite { id: string; public_key: string; name: string; world_name: string; minecraft: string; received_at: number; expires_at: number }

/** Raccourci rare : evite d'occuper Ctrl+Shift+C (global a la machine) pendant les tests. */
export const TEST_HOTKEY = 'Ctrl+Alt+Shift+F11';
export const WORLD_NAME = 'Monde Invitation';

/** A (hote, instance posee, connecte) et B (ami, connecte), amis acceptes ; renvoie la cle de chacun vue par l'autre. */
export async function setupPair(world: Pick<World, 'appA' | 'appB'>) {
  const { appA, appB } = world;
  for (const app of [appA, appB]) {
    await waitForRoute(app.page, 'login');
    // Le raccourci par defaut est global a la machine : on le remplace des le depart.
    const s = await invoke<Record<string, unknown>>(app.page, 'settings_get');
    await invoke(app.page, 'settings_set', { ...s, close_on_launch: false, overlay_hotkey: TEST_HOTKEY });
  }
  await loginOffline(appA.page, 'Hote');
  await loginOffline(appB.page, 'Invite');
  const host = await makeHostInstance(appA, { name: WORLD_NAME });
  await makeFriends(appA, appB);
  const keyA = await friendKeyOnB(appB); // A vu par B
  const keyB = await friendKeyOnB(appA); // B vu par A
  await waitOnline(appA.page);
  await waitOnline(appB.page);
  return { host, keyA, keyB };
}

/** Ouvre le monde de A au reseau (comme les specs existantes : partage avec un port LAN). */
export async function openWorld(app: App, instanceId: string, port = 25599) {
  await invoke(app.page, 'host_start_sharing', { instance_id: instanceId, world_port: port });
}

export const errText = (e: unknown) => String((e as { message?: string })?.message ?? e);

/** Appelle une commande et renvoie le message d'erreur (null si elle reussit). */
export async function invokeErr(page: Page, cmd: string, args: Record<string, unknown> = {}): Promise<string | null> {
  return invoke(page, cmd, args).then(() => null, errText);
}

/** Enregistre la charge utile brute de chaque evenement `name` dans window.__rec18[name]. */
export async function record(page: Page, name: string) {
  await page.evaluate(async (ev) => {
    const w = window as any;
    w.__rec18 = w.__rec18 ?? {};
    w.__rec18[ev] = w.__rec18[ev] ?? [];
    const handler = w.__TAURI_INTERNALS__.transformCallback((e: { payload: unknown }) => { w.__rec18[ev].push(e.payload); });
    await w.__TAURI_INTERNALS__.invoke('plugin:event|listen', { event: ev, target: { kind: 'Any' }, handler });
  }, name);
}

export async function recorded<T = any>(page: Page, name: string): Promise<T[]> {
  return page.evaluate((ev) => ((window as any).__rec18?.[ev] ?? []) as any[], name);
}

/** Attend qu'une condition asynchrone soit vraie (sondage). */
export async function until<T>(what: string, fn: () => Promise<T | false | null | undefined>, timeout = 20_000, step = 150): Promise<T> {
  const end = Date.now() + timeout;
  for (;;) {
    const v = await fn();
    if (v) return v;
    if (Date.now() > end) throw new Error(`delai depasse : ${what}`);
    await sleep(step);
  }
}

/** Emet un evenement applicatif depuis la page (recu par les ecouteurs Rust, comme un vrai `launch://state`). */
export async function emitEvent(page: Page, event: string, payload: unknown) {
  await page.evaluate(([ev, p]) => (window as any).__TAURI_INTERNALS__.invoke('plugin:event|emit', { event: ev, payload: p }), [event, payload] as [string, unknown]);
}

export interface StageState { attached: boolean; game: boolean; panel: boolean }
export const stageState = (page: Page) => invoke<StageState>(page, 'stage_state');
