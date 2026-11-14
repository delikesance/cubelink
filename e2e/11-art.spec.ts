// T11 : illustrations propres au modpack (accueil) : recherche automatique, installation depuis Parcourir,
// image locale, galerie/visionneuse, captures a toutes les tailles. Faux serveur : tests-fixtures/browse/server.mjs.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, type ChildProcess } from 'node:child_process';
import { test as base, expect, ARTIFACTS, ROOT, makeHostInstance, invoke, loginOffline, waitForRoute, type AppOptions } from './harness/fixtures';
import { setWindowSize } from './helpers/e2e4-lib';

const OUT = path.join(ARTIFACTS, 'art');
fs.mkdirSync(OUT, { recursive: true });
let fakeBase = '';

const test = base.extend<{ appOptions: AppOptions }>({
  appOptions: [async ({}, use) => {
    const proc: ChildProcess = spawn(process.execPath, [path.join(ROOT, 'tests-fixtures', 'browse', 'server.mjs'), '--port', '0'], { stdio: ['ignore', 'pipe', 'inherit'], windowsHide: true });
    const info = await new Promise<{ base: string }>((resolve, reject) => {
      let buf = '';
      proc.stdout!.on('data', (d) => { buf += d; try { resolve(JSON.parse(buf.split('\n')[0])); } catch { /* incomplet */ } });
      proc.once('exit', () => reject(new Error('faux serveur browse arrete')));
    });
    fakeBase = info.base;
    await use({ env: { CUBELINK_TEST_CF_API: info.base, CUBELINK_TEST_MODRINTH_API: info.base } });
    proc.kill();
  }, { option: true }],
});

const shot = (page: import('@playwright/test').Page, name: string) => page.screenshot({ path: path.join(OUT, `${name}.png`) });

interface Inst { id: string; name: string; art?: { color: string | null; icon: string | null; screenshots: string[]; author: string; source: string } | null }

test('Illustrations : recherche auto, image locale, installation Parcourir, galerie, captures', async ({ appA }) => {
  const page = appA.page;
  await waitForRoute(page, 'login');
  await loginOffline(page, 'Alex');
  const host = await makeHostInstance(appA, { id: 'host-world', name: 'Sodium modpack', minecraft: '1.21.1-test' });
  const metaPath = path.join(host.dir, 'instance.json');
  const meta = JSON.parse(fs.readFileSync(metaPath, 'utf8'));
  meta.loader = { loader: 'fabric', version: '0.16.5' };
  fs.writeFileSync(metaPath, JSON.stringify(meta, null, 2));
  await page.reload();
  await waitForRoute(page, 'main');
  await page.waitForTimeout(500);

  // --- recherche automatique, sans aucun clic : l'illustration est appliquee par defaut ---
  // le décor plein écran a disparu de l'accueil : l'icône du modpack sur la tuile prouve l'illustration appliquée
  await expect(page.locator('.home-av.has-img img').first()).toBeVisible({ timeout: 30_000 });
  await expect.poll(async () => (await invoke<Inst[]>(page, 'instances_list')).find((i) => i.id === 'host-world')?.art?.screenshots.length ?? 0, { timeout: 30_000 }).toBe(4);
  const list = await invoke<Inst[]>(page, 'instances_list');
  const me = list.find((i) => i.id === 'host-world')!;
  expect(me.art?.source).toBe('modrinth');
  expect(me.art?.color).toMatch(/^#[0-9a-f]{6}$/);
  expect(me.art?.icon).toBeTruthy();
  expect(fs.readdirSync(path.join(host.dir, 'art')).sort()).toEqual(expect.arrayContaining(['icon.png', 'shot-1.png']));
  // l'art n'est PAS dans le dossier de jeu (donc pas partage comme des mods)
  expect(fs.existsSync(path.join(host.gameDir, 'art'))).toBe(false);
  await expect(page.getByText(/Source : Modrinth/)).toBeVisible();
  await expect(page.getByRole('link', { name: 'Page du modpack' })).toHaveAttribute('rel', /noopener/);
  await page.waitForTimeout(500);
  await shot(page, 'home-with-art');

  // --- sans correspondance : pas d'erreur, degrade genere, instance jouable ---
  const other = await makeHostInstance(appA, { id: 'zz-unknown', name: 'Zzqx introuvable', minecraft: '1.21.1-test' });
  await page.reload();
  await waitForRoute(page, 'main');
  await page.waitForTimeout(1500);
  expect((await invoke<Inst[]>(page, 'instances_list')).find((i) => i.id === other.id)?.art ?? null).toBeNull();
  const rail = page.getByRole('navigation', { name: 'Instances' });
  await rail.getByRole('button', { name: /Zzqx introuvable/ }).click();
  await expect(page.locator('svg[data-default-art]').first()).toBeAttached();
  await rail.getByRole('button', { name: /Zzqx introuvable/ }).click({ button: 'right' });
  await expect(page.getByRole('menuitem', { name: 'Chercher automatiquement' })).toBeVisible();
  await page.keyboard.press('Escape');
  await shot(page, 'home-no-art');
  await rail.getByRole('button', { name: /Sodium modpack/ }).click();

  // --- visionneuse (menu « Voir les captures ») : ouverture, fleches, Echap ---
  await rail.getByRole('button', { name: /Sodium modpack/ }).click({ button: 'right' });
  await page.getByRole('menuitem', { name: /^Voir les captures/ }).click();
  const viewer = page.getByRole('dialog');
  await expect(viewer).toBeVisible();
  await shot(page, 'viewer');
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('Escape');
  await expect(viewer).toHaveCount(0);

  // --- image locale : icone ; extension refusee ---
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'cubelink-art-'));
  const png = path.join(tmp, 'ma-icone.png');
  fs.writeFileSync(png, Buffer.from(await (await fetch(`${fakeBase}/img/custom.png`)).arrayBuffer()));
  const upd = await invoke<Inst>(page, 'instances_art_set', { instance_id: 'host-world', path: png, role: 'icon' });
  expect(upd.art?.icon).toBeTruthy();
  const bad = path.join(tmp, 'x.exe');
  fs.writeFileSync(bad, 'MZ');
  await expect(invoke(page, 'instances_art_set', { instance_id: 'host-world', path: bad, role: 'icon' })).rejects.toBeTruthy();
  // un fichier d'art hors liste est refuse
  await expect(invoke(page, 'instances_art_image', { instance_id: 'host-world', file: '../instance.json' })).rejects.toBeTruthy();
  const url = await invoke<string>(page, 'instances_art_image', { instance_id: 'host-world', file: me.art!.screenshots[0] });
  expect(url).toMatch(/^data:image\/png;base64,/);
  fs.rmSync(tmp, { recursive: true, force: true });

  // --- installation d'un modpack depuis Parcourir : l'art est attache ---
  await page.evaluate(() => { window.location.hash = '#/browse?kind=modpack'; });
  await waitForRoute(page, 'browse');
  await page.getByRole('searchbox').fill('Lithium');
  await page.locator('.br-card-btn', { hasText: 'Lithium modpack' }).first().click();
  await page.getByRole('dialog').getByRole('button', { name: /^Installer/ }).click();
  await waitForRoute(page, 'main', 60_000);
  await expect.poll(async () => (await invoke<Inst[]>(page, 'instances_list')).find((i) => /Lithium/.test(i.name))?.art?.screenshots.length ?? 0, { timeout: 30_000 }).toBeGreaterThan(0);
  await page.waitForTimeout(800);
  await shot(page, 'home-installed-modpack');

  // --- captures a toutes les tailles ---
  for (const [w, h] of [[960, 600], [1440, 900], [1920, 1080], [2560, 1440]] as const) {
    await setWindowSize(page, w, h);
    await page.waitForTimeout(500);
    const hscroll = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    expect(hscroll, `defilement horizontal a ${w}x${h}`).toBeLessThanOrEqual(1);
    await shot(page, `home-${w}x${h}`);
  }
});
