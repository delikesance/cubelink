// Mise à jour d'un modpack CurseForge installé depuis le catalogue, sur l'application réelle, contre le faux CurseForge
// de tests-fixtures/browse/server.mjs (projet 5000 : fichier 5001 = 1.0.0, 5002 = 1.1.0, 5003 = pack serveur à ignorer).
// La 1.1.0 remplace un mod, change deux configs et ajoute un mod non redistribuable (sans URL) : il reste à poser à la main.
// Parcours : installation de la v1, le joueur modifie un fichier du pack et pose les siens, badge « Mise à jour … 1.1.0
// disponible » (le pack serveur n'est pas proposé), « Mettre à jour » (fichiers du joueur gardés, mod à télécharger
// soi-même listé), puis « Annuler la mise à jour » (retour exact). La clé `test-key` est fournie par l'application
// en mode test (`CUBELINK_TEST_CF_API`).
import fs from 'node:fs';
import path from 'node:path';
import { spawn, type ChildProcess } from 'node:child_process';
import { test as base, expect, ARTIFACTS, ROOT, invoke, loginOffline, testState, waitForRoute, type AppOptions } from './harness/fixtures';

const OUT = path.join(ARTIFACTS, 'modpack-update-cf');
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

interface Inst { id: string; pack?: { source: string; version_id: string } | null }

test('Mise à jour d\'un modpack CurseForge : badge, mise à jour sans toucher aux fichiers du joueur, mod à poser, annulation', async ({ appA }) => {
  const page = appA.page;
  await waitForRoute(page, 'login');
  await loginOffline(page, 'Alex');

  // Installation de la v1 (comme « Parcourir » avec une version choisie).
  const res = await invoke<{ instance: Inst }>(page, 'browse_install', { input: { source: 'curseforge', id: '5000', kind: 'modpack', version_id: '5001' } });
  const id = res.instance.id;
  expect(res.instance.pack).toMatchObject({ source: 'curseforge', version_id: '5001' });
  const st = await testState(page);
  const game = path.join(st.paths.instances, id, 'minecraft');
  expect(fs.existsSync(path.join(game, 'mods', 'cf-mod-a-1.jar'))).toBe(true);
  expect(fs.existsSync(path.join(game, 'mods', 'cf-mod-b.jar'))).toBe(true);
  expect(fs.readFileSync(path.join(game, 'config', 'cf-upd.txt'), 'utf8')).toBe('upd v1');

  // Le joueur : fichier du pack modifié, mod à lui, options et monde.
  fs.writeFileSync(path.join(game, 'config', 'cf-keep.txt'), 'keep du joueur');
  fs.writeFileSync(path.join(game, 'mods', 'perso.jar'), 'perso');
  fs.writeFileSync(path.join(game, 'options.txt'), 'fov:90');
  fs.mkdirSync(path.join(game, 'saves', 'Monde'), { recursive: true });
  fs.writeFileSync(path.join(game, 'saves', 'Monde', 'level.dat'), 'niveau');

  // La détection voit la 1.1.0 (fichier 5002), pas le pack serveur 5003 pourtant plus récent.
  const up = await invoke<{ version_id: string; name: string } | null>(page, 'modpack_update_check', { instance_id: id });
  expect(up).toMatchObject({ version_id: '5002', name: 'CF Pack 1.1.0' });

  // Interface : badge puis mise à jour.
  await page.evaluate((i) => { window.location.hash = `#/instance?id=${i}`; }, id);
  await waitForRoute(page, 'instance');
  await expect(page.getByText('Mise à jour CF Pack 1.1.0 disponible')).toBeVisible();
  await page.screenshot({ path: path.join(OUT, 'badge.png') });
  await page.getByRole('button', { name: 'Mettre à jour', exact: true }).click();
  await expect(page.getByText('Modpack mis à jour')).toBeVisible({ timeout: 60_000 });
  await page.screenshot({ path: path.join(OUT, 'resume.png') });

  // Mods : a-1 remplacé par a-2, b inchangé, le mod non redistribuable n'est pas posé mais signalé.
  expect(fs.existsSync(path.join(game, 'mods', 'cf-mod-a-1.jar'))).toBe(false);
  expect(fs.existsSync(path.join(game, 'mods', 'cf-mod-a-2.jar'))).toBe(true);
  expect(fs.existsSync(path.join(game, 'mods', 'cf-mod-b.jar'))).toBe(true);
  expect(fs.existsSync(path.join(game, 'mods', 'cf-restreint.jar'))).toBe(false);
  await expect(page.getByText(/1 mod à télécharger toi-même : mods\/cf-restreint\.jar/)).toBeVisible();
  // Configs : la non modifiée est mise à jour, celle du joueur est gardée (et signalée), la nouvelle est ajoutée.
  expect(fs.readFileSync(path.join(game, 'config', 'cf-upd.txt'), 'utf8')).toBe('upd v2');
  expect(fs.readFileSync(path.join(game, 'config', 'added-cf.txt'), 'utf8')).toBe('ajoute en 1.1.0');
  expect(fs.readFileSync(path.join(game, 'config', 'cf-keep.txt'), 'utf8')).toBe('keep du joueur');
  await expect(page.getByText(/config\/cf-keep\.txt/)).toBeVisible();
  // Données du joueur intactes.
  expect(fs.readFileSync(path.join(game, 'mods', 'perso.jar'), 'utf8')).toBe('perso');
  expect(fs.readFileSync(path.join(game, 'options.txt'), 'utf8')).toBe('fov:90');
  expect(fs.readFileSync(path.join(game, 'saves', 'Monde', 'level.dat'), 'utf8')).toBe('niveau');
  const meta = JSON.parse(fs.readFileSync(path.join(st.paths.instances, id, 'instance.json'), 'utf8'));
  expect(meta.pack).toMatchObject({ source: 'curseforge', version_id: '5002' });

  // Annulation : tout revient, la mise à jour est de nouveau proposée.
  await page.getByRole('button', { name: 'Annuler la mise à jour' }).click();
  await expect(page.getByText(/Mise à jour annulée/)).toBeVisible();
  expect(fs.existsSync(path.join(game, 'mods', 'cf-mod-a-1.jar'))).toBe(true);
  expect(fs.existsSync(path.join(game, 'mods', 'cf-mod-a-2.jar'))).toBe(false);
  expect(fs.existsSync(path.join(game, 'config', 'added-cf.txt'))).toBe(false);
  expect(fs.readFileSync(path.join(game, 'config', 'cf-upd.txt'), 'utf8')).toBe('upd v1');
  expect(fs.readFileSync(path.join(game, 'config', 'cf-keep.txt'), 'utf8')).toBe('keep du joueur');
  const back = JSON.parse(fs.readFileSync(path.join(st.paths.instances, id, 'instance.json'), 'utf8'));
  expect(back.pack.version_id).toBe('5001');
  await expect(page.getByText('Mise à jour CF Pack 1.1.0 disponible')).toBeVisible();
});
