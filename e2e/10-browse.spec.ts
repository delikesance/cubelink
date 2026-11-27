// T10 : « Parcourir » (recherche + installation CurseForge/Modrinth) et mods d'instance (dont mods perso),
// sur l'application reelle, contre tests-fixtures/browse/server.mjs (faux Modrinth /v2 + faux CurseForge /v1).
// Captures visuelles dans e2e/artifacts/browse/.
// La fiche d'un mod / modpack est une PAGE (« Retour aux résultats »), plus une boite de dialogue ; la liste des mods d'une
// instance est l'onglet Mods de la page de l'instance ; la version se choisit dans le composant Select (combobox), plus un <select>.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, type ChildProcess } from 'node:child_process';
import { test as base, expect, ARTIFACTS, ROOT, makeHostInstance, invoke, loginOffline, waitForRoute, type AppOptions } from './harness/fixtures';
import { setWindowSize } from './helpers/e2e4-lib';

const OUT = path.join(ARTIFACTS, 'browse');
fs.mkdirSync(OUT, { recursive: true });

let fake: { base: string; proc: ChildProcess } | null = null;

// Le faux serveur doit etre pret AVANT le lancement de l'app (variables d'environnement).
const test = base.extend<{ appOptions: AppOptions }>({
  appOptions: [async ({}, use) => {
    const proc = spawn(process.execPath, [path.join(ROOT, 'tests-fixtures', 'browse', 'server.mjs'), '--port', '0'], { stdio: ['ignore', 'pipe', 'inherit'], windowsHide: true });
    const info = await new Promise<{ base: string }>((resolve, reject) => {
      let buf = '';
      proc.stdout!.on('data', (d) => { buf += d; try { resolve(JSON.parse(buf.split('\n')[0])); } catch { /* incomplet */ } });
      proc.once('exit', () => reject(new Error('faux serveur browse arrete')));
    });
    fake = { base: info.base, proc };
    await use({ env: { CUBELINK_TEST_CF_API: info.base, CUBELINK_TEST_MODRINTH_API: info.base } });
    proc.kill();
  }, { option: true }],
});

const shot = (page: import('@playwright/test').Page, name: string) => page.screenshot({ path: path.join(OUT, `${name}.png`) });

test('Parcourir : recherche, installation d\'un mod avec dependance, modpack, non telechargeable, mods perso, captures', async ({ appA }) => {
  const page = appA.page;
  await waitForRoute(page, 'login');
  await loginOffline(page, 'Alex');
  const host = await makeHostInstance(appA, { minecraft: '1.21.1' });
  // instance fabric 1.21.1 (compatible avec le faux catalogue)
  const metaPath = path.join(host.dir, 'instance.json');
  const meta = JSON.parse(fs.readFileSync(metaPath, 'utf8'));
  meta.loader = { loader: 'fabric', version: '0.16.0' };
  fs.writeFileSync(metaPath, JSON.stringify(meta, null, 2));
  await page.reload();
  await waitForRoute(page, 'main');
  await shot(page, 'main-hero');

  // --- recherche Modrinth ---
  await page.evaluate(() => { window.location.hash = '#/browse?kind=mod&instance=host-world'; });
  await waitForRoute(page, 'browse');
  await expect(page.locator('.br-card').first()).toBeVisible();
  await page.getByRole('searchbox').fill('Sodium');
  await expect(page.getByText('3 résultats')).toBeVisible();
  await shot(page, 'browse-mods-modrinth');

  // --- installation du mod 0 (depend obligatoirement du mod 1) ---
  await page.locator('.br-card-btn', { hasText: 'Sodium' }).first().click();
  const version = page.getByRole('combobox', { name: 'Version' });
  await expect(version).toBeVisible();
  await version.click(); // le composant Select : la liste (listbox) s'ouvre sur ses options
  await expect(page.getByRole('listbox').getByRole('option').first()).toBeVisible();
  await version.click();
  await expect(page.getByRole('listbox')).toHaveCount(0);
  await shot(page, 'browse-detail');
  await page.getByRole('button', { name: /^Installer/ }).click();
  const mods = path.join(host.gameDir, 'mods');
  await expect.poll(() => fs.existsSync(mods) ? fs.readdirSync(mods).sort() : [], { timeout: 30_000 }).toEqual(expect.arrayContaining(['mod-0-1.0.0.jar', 'mod-1-1.0.0.jar']));
  await shot(page, 'browse-installed');
  await page.getByRole('button', { name: 'Retour aux résultats' }).click();

  // --- CurseForge : mod restreint (mod-5) non telechargeable ---
  await page.getByRole('button', { name: 'CurseForge' }).click();
  await page.getByRole('searchbox').fill('Xaero');
  await page.locator('.br-card-btn', { hasText: 'Xaero Minimap' }).first().click();
  await expect(page.getByRole('button', { name: /^Installer/ })).toBeDisabled();
  await expect(page.getByRole('link', { name: /Ouvrir sur CurseForge/ })).toBeVisible();
  await shot(page, 'browse-restricted');
  await page.getByRole('button', { name: 'Retour aux résultats' }).click();

  // --- CurseForge : mod autorise (mod-0 'Sodium') installe via la cle factice ---
  await page.getByRole('searchbox').fill('Lithium');
  await page.locator('.br-card-btn', { hasText: 'Lithium' }).first().click();
  // Meme jar que la dependance Modrinth posee plus haut : reconnu comme deja installe, pas de second exemplaire.
  await expect(page.getByText(/Déjà installé dans .*mod-1-1\.0\.0\.jar/)).toBeVisible({ timeout: 30_000 });
  await expect(page.getByRole('button', { name: 'Désinstaller' })).toBeVisible();
  await expect(page.getByRole('button', { name: /^Installer/ })).toHaveCount(0);
  expect(fs.readdirSync(mods).filter((f) => f.startsWith('mod-1-'))).toEqual(['mod-1-1.0.0.jar']);
  await page.getByRole('button', { name: 'Retour aux résultats' }).click();

  // --- modpack Modrinth : nouvelle instance ---
  await page.getByRole('button', { name: 'Modrinth' }).click();
  await page.getByRole('tab', { name: 'Modpacks' }).click();
  await page.getByRole('searchbox').fill('Sodium');
  await page.locator('.br-card-btn', { hasText: 'Sodium' }).first().click();
  await page.getByRole('button', { name: /^Installer/ }).click();
  // La fiche reste affichee une fois le modpack installe (plus de retour automatique a l'accueil) : « Ouvrir le modpack ».
  await expect(page.getByRole('button', { name: 'Ouvrir le modpack' })).toBeVisible({ timeout: 60_000 });
  const list = await invoke<{ name: string }[]>(page, 'instances_list');
  expect(list.some((i) => /Sodium/.test(i.name))).toBeTruthy();

  // --- mods personnalises : ajout, liste, bascule, suppression ---
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'cubelink-custom-'));
  const custom = path.join(tmp, 'mon-mod-perso.jar');
  const zipLib = await import('../tests-fixtures/modrinth/lib.mjs');
  fs.writeFileSync(custom, zipLib.buildZip([{ name: 'fabric.mod.json', data: '{"schemaVersion":1,"id":"perso","version":"1"}' }]));
  const added = await invoke<{ file: string; custom: boolean }>(page, 'mods_add', { instance_id: 'host-world', path: custom });
  expect(added.custom).toBe(true);
  expect(fs.existsSync(path.join(mods, 'mon-mod-perso.jar'))).toBe(true);
  // refus d'une extension non jar
  await expect(invoke(page, 'mods_add', { instance_id: 'host-world', path: path.join(tmp, 'x.exe') })).rejects.toBeTruthy();

  await page.evaluate(() => { window.location.hash = '#/instance?id=host-world&tab=mods'; });
  await page.reload();
  await waitForRoute(page, 'instance');
  const panel = page.locator('.dr-panel'); // onglet Mods de la page de l'instance
  await expect(panel.getByText('mon-mod-perso.jar')).toBeVisible();
  await expect(panel.getByText('Personnalisé').first()).toBeVisible();
  await shot(page, 'mods-panel');
  await panel.getByRole('switch', { name: /Désactiver mon-mod-perso\.jar/ }).click();
  await expect.poll(() => fs.existsSync(path.join(mods, 'mon-mod-perso.jar.disabled'))).toBeTruthy();
  await panel.getByRole('button', { name: 'Supprimer mon-mod-perso.jar' }).click();
  // Plus de confirmation : suppression annulable pendant 5 s, effective ensuite.
  await expect(panel.getByRole('status').filter({ hasText: /retiré/ })).toBeVisible();
  await expect.poll(() => fs.existsSync(path.join(mods, 'mon-mod-perso.jar.disabled')), { timeout: 20_000 }).toBeFalsy();
  fs.rmSync(tmp, { recursive: true, force: true });

  // --- captures a toutes les tailles ---
  for (const [w, h] of [[960, 600], [1440, 900], [1920, 1080], [2560, 1440]] as const) {
    await setWindowSize(page, w, h);
    await page.evaluate(() => { window.location.hash = '#/browse?kind=mod&instance=host-world'; });
    await waitForRoute(page, 'browse');
    await expect(page.locator('.br-card').first()).toBeVisible();
    await page.waitForTimeout(300);
    const hscroll = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    expect(hscroll, `defilement horizontal a ${w}x${h}`).toBeLessThanOrEqual(1);
    await shot(page, `browse-${w}x${h}`);
    await page.evaluate(() => { window.location.hash = '#/main'; });
    await waitForRoute(page, 'main');
    await shot(page, `main-${w}x${h}`);
  }
});
