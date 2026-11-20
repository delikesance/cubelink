// #137 : mise à jour d'un modpack installé depuis le catalogue, sur l'application reelle, contre le faux Modrinth de
// tests-fixtures/browse/server.mjs (projet `modpack-7` : v1 1.0.0, puis 1.1.0 qui change hello.txt et ajoute added.txt).
// Parcours : installation de la v1, le joueur modifie un fichier du pack et pose les siens, badge « Mise à jour 1.1.0
// disponible », « Mettre à jour » (fichiers du joueur gardes), puis « Annuler la mise à jour » (retour exact).
import fs from 'node:fs';
import path from 'node:path';
import { spawn, type ChildProcess } from 'node:child_process';
import { test as base, expect, ARTIFACTS, ROOT, invoke, loginOffline, testState, waitForRoute, type AppOptions } from './harness/fixtures';

const OUT = path.join(ARTIFACTS, 'modpack-update');
fs.mkdirSync(OUT, { recursive: true });

const test = base.extend<{ appOptions: AppOptions }>({
  appOptions: [async ({}, use) => {
    const proc: ChildProcess = spawn(process.execPath, [path.join(ROOT, 'tests-fixtures', 'browse', 'server.mjs'), '--port', '0'], { stdio: ['ignore', 'pipe', 'inherit'], windowsHide: true });
    const info = await new Promise<{ base: string }>((resolve, reject) => {
      let buf = '';
      proc.stdout!.on('data', (d) => { buf += d; try { resolve(JSON.parse(buf.split('\n')[0])); } catch { /* incomplet */ } });
      proc.once('exit', () => reject(new Error('faux serveur browse arrete')));
    });
    await use({ env: { CUBELINK_TEST_CF_API: info.base, CUBELINK_TEST_MODRINTH_API: info.base } });
    proc.kill();
  }, { option: true }],
});

interface Inst { id: string; pack?: { version_id: string } | null }

test('Mise à jour d\'un modpack : badge, mise à jour sans toucher aux fichiers du joueur, annulation', async ({ appA }) => {
  const page = appA.page;
  await waitForRoute(page, 'login');
  await loginOffline(page, 'Alex');

  // Installation de la v1 (comme « Parcourir » avec une version choisie).
  const res = await invoke<{ instance: Inst }>(page, 'browse_install', { input: { source: 'modrinth', id: 'modpack-7', kind: 'modpack', version_id: 'modpack-7-v1' } });
  const id = res.instance.id;
  expect(res.instance.pack?.version_id).toBe('modpack-7-v1');
  const st = await testState(page);
  const game = path.join(st.paths.instances, id, 'minecraft');
  expect(fs.readFileSync(path.join(game, 'config', 'hello.txt'), 'utf8')).toBe('hello');

  // Le joueur : fichier du pack modifié, mod à lui, options et monde.
  fs.writeFileSync(path.join(game, 'config', 'hello.txt'), 'hello du joueur');
  fs.mkdirSync(path.join(game, 'mods'), { recursive: true });
  fs.writeFileSync(path.join(game, 'mods', 'perso.jar'), 'perso');
  fs.writeFileSync(path.join(game, 'options.txt'), 'fov:90');
  fs.mkdirSync(path.join(game, 'saves', 'Monde'), { recursive: true });
  fs.writeFileSync(path.join(game, 'saves', 'Monde', 'level.dat'), 'niveau');

  // La commande de détection voit la 1.1.0.
  const up = await invoke<{ version_id: string; version_number: string } | null>(page, 'modpack_update_check', { instance_id: id });
  expect(up).toMatchObject({ version_id: 'modpack-7-v2', version_number: '1.1.0' });

  // Interface : badge puis mise à jour.
  await page.evaluate((i) => { window.location.hash = `#/instance?id=${i}`; }, id);
  await waitForRoute(page, 'instance');
  await expect(page.getByText('Mise à jour 1.1.0 disponible')).toBeVisible();
  await page.screenshot({ path: path.join(OUT, 'badge.png') });
  await page.getByRole('button', { name: 'Mettre à jour', exact: true }).click();
  await expect(page.getByText('Modpack mis à jour')).toBeVisible({ timeout: 60_000 });
  await page.screenshot({ path: path.join(OUT, 'resume.png') });

  expect(fs.readFileSync(path.join(game, 'config', 'added.txt'), 'utf8')).toBe('ajoute en 1.1.0');
  // hello.txt modifié par le joueur : gardé tel quel et signalé.
  expect(fs.readFileSync(path.join(game, 'config', 'hello.txt'), 'utf8')).toBe('hello du joueur');
  await expect(page.getByText(/config\/hello\.txt/)).toBeVisible();
  expect(fs.readFileSync(path.join(game, 'mods', 'perso.jar'), 'utf8')).toBe('perso');
  expect(fs.readFileSync(path.join(game, 'options.txt'), 'utf8')).toBe('fov:90');
  expect(fs.readFileSync(path.join(game, 'saves', 'Monde', 'level.dat'), 'utf8')).toBe('niveau');
  const meta = JSON.parse(fs.readFileSync(path.join(st.paths.instances, id, 'instance.json'), 'utf8'));
  expect(meta.pack.version_id).toBe('modpack-7-v2');

  // Annulation : tout revient, la mise à jour est de nouveau proposée.
  await page.getByRole('button', { name: 'Annuler la mise à jour' }).click();
  await expect(page.getByText(/Mise à jour annulée/)).toBeVisible();
  expect(fs.existsSync(path.join(game, 'config', 'added.txt'))).toBe(false);
  expect(fs.readFileSync(path.join(game, 'config', 'hello.txt'), 'utf8')).toBe('hello du joueur');
  const back = JSON.parse(fs.readFileSync(path.join(st.paths.instances, id, 'instance.json'), 'utf8'));
  expect(back.pack.version_id).toBe('modpack-7-v1');
  await expect(page.getByText('Mise à jour 1.1.0 disponible')).toBeVisible();
});
