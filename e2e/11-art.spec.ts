// T11 : illustrations propres au modpack (accueil, page du modpack) : recherche automatique, installation depuis Parcourir,
// image locale, galerie/visionneuse, captures a toutes les tailles. Faux serveur : tests-fixtures/browse/server.mjs.
// Navigation actuelle : une tuile de l'accueil ouvre la page du modpack (menu « Actions de … », captures, À propos) ;
// « Source : … » et le lien vers la page du projet sont sur la fiche de Parcourir (Browse/Detail.tsx), plus sur l'accueil.
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

interface Inst { id: string; name: string; pack?: { source: string; project_id: string; version_id: string } | null; art?: { color: string | null; icon: string | null; screenshots: string[]; author: string; source: string; project_id: string } | null }

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
  await page.waitForTimeout(500);
  await shot(page, 'home-with-art');
  // Fiche Parcourir du modpack deja installe : source et lien vers la page du projet.
  await page.evaluate((d) => { window.location.hash = `#/browse?kind=modpack&detail=${d}`; }, `modrinth:${me.art!.project_id}`);
  await waitForRoute(page, 'browse');
  await expect(page.getByText(/Source : Modrinth/)).toBeVisible();
  await expect(page.getByRole('link', { name: /Ouvrir la page sur Modrinth/ })).toHaveAttribute('rel', /noopener/);

  // --- sans correspondance : pas d'erreur, degrade genere, instance jouable ---
  const other = await makeHostInstance(appA, { id: 'zz-unknown', name: 'Zzqx introuvable', minecraft: '1.21.1-test' });
  await page.reload();
  await waitForRoute(page, 'main');
  await page.waitForTimeout(1500);
  expect((await invoke<Inst[]>(page, 'instances_list')).find((i) => i.id === other.id)?.art ?? null).toBeNull();
  // la tuile sans illustration garde l'avatar par defaut (pas de .has-img) ; sa page propose la recherche automatique
  await expect(page.locator('.home-tile', { hasText: 'Zzqx introuvable' }).locator('.has-img')).toHaveCount(0);
  await page.getByRole('button', { name: 'Zzqx introuvable', exact: true }).click();
  await waitForRoute(page, 'instance');
  await page.getByRole('button', { name: 'Actions de Zzqx introuvable' }).click();
  await expect(page.getByRole('menuitem', { name: 'Chercher automatiquement' })).toBeVisible();
  await page.keyboard.press('Escape');
  await shot(page, 'home-no-art');

  // --- visionneuse (miniature « Capture 1 » de la page du modpack) : ouverture, fleches, Echap ---
  await page.evaluate(() => { window.location.hash = '#/instance?id=host-world'; });
  await waitForRoute(page, 'instance');
  await page.getByRole('group', { name: "Captures d'écran" }).getByRole('button', { name: 'Capture 1' }).click();
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
  await page.getByRole('button', { name: /^Installer/ }).click();
  await expect(page.getByRole('button', { name: 'Ouvrir le modpack' })).toBeVisible({ timeout: 60_000 }); // la fiche reste affichee
  await expect.poll(async () => (await invoke<Inst[]>(page, 'instances_list')).find((i) => /Lithium/.test(i.name))?.art?.screenshots.length ?? 0, { timeout: 30_000 }).toBeGreaterThan(0);
  await page.waitForTimeout(800);
  await page.evaluate(() => { window.location.hash = '#/main'; });
  await waitForRoute(page, 'main');
  await shot(page, 'home-installed-modpack');
  // l'origine du modpack est enregistree dans instance.json (projet exact et version telechargee)
  const lith = (await invoke<Inst[]>(page, 'instances_list')).find((i) => /Lithium/.test(i.name))!;
  expect(lith.pack).toEqual({ source: 'modrinth', project_id: 'modpack-1', version_id: 'modpack-1-v1' });
  const lithJson = JSON.parse(fs.readFileSync(path.join(host.dir, '..', lith.id, 'instance.json'), 'utf8'));
  expect(lithJson.pack).toEqual({ source: 'modrinth', project_id: 'modpack-1', version_id: 'modpack-1-v1' });

  // --- import manuel d'un .mrpack du catalogue : reconnu par son SHA-1, origine et art du projet exact ---
  const manualDir = fs.mkdtempSync(path.join(os.tmpdir(), 'cubelink-mrpack-'));
  const manualFile = path.join(manualDir, 'pack-manuel.mrpack');
  fs.writeFileSync(manualFile, Buffer.from(await (await fetch(`${fakeBase}/files/modpack-5-1.0.0.mrpack`)).arrayBuffer()));
  const imported = await invoke<Inst>(page, 'instances_import_mrpack', { path: manualFile, name: null, minecraft: null, loader: null, ram_mb: null });
  expect(imported.pack).toEqual({ source: 'modrinth', project_id: 'modpack-5', version_id: 'modpack-5-v1' });
  fs.rmSync(manualDir, { recursive: true, force: true });

  // --- captures a toutes les tailles ---
  for (const [w, h] of [[960, 600], [1440, 900], [1920, 1080], [2560, 1440]] as const) {
    await setWindowSize(page, w, h);
    await page.waitForTimeout(500);
    const hscroll = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    expect(hscroll, `defilement horizontal a ${w}x${h}`).toBeLessThanOrEqual(1);
    await shot(page, `home-${w}x${h}`);
  }
});
