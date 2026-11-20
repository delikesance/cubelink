// T20 : onglet « Captures » de la page modpack : captures F2 du joueur (`screenshots/` du dossier de jeu) en lecture
// seule : etat vide, liste, miniature, visionneuse, « Comme bannière » (copie dans art/), noms dangereux refuses.
// Aucune entree reelle : tout passe par l'app. L'ouverture du dossier est un no-op en mode test.
import fs from 'node:fs';
import path from 'node:path';
import { test, expect, ARTIFACTS, makeHostInstance, invoke, loginOffline, waitForRoute } from './harness/fixtures';

const OUT = path.join(ARTIFACTS, 'captures');
fs.mkdirSync(OUT, { recursive: true });

// PNG 2x2 valide (le contenu est verifie par octets magiques cote Rust).
const PNG = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEklEQVR4nGM4oaGhcUKDAUIBAB/uBGEdvUokAAAAAElFTkSuQmCC', 'base64');

interface Shot { file: string; modified_ms: number; size: number }
interface Inst { id: string; art?: { screenshots: string[]; source: string } | null }

test('Captures du joueur : etat vide, liste, visionneuse, banniere, refus des noms dangereux', async ({ appA }) => {
  const page = appA.page;
  await waitForRoute(page, 'login');
  await loginOffline(page, 'Alex');
  const host = await makeHostInstance(appA, { id: 'host-world', name: 'Mon pack', minecraft: '1.21.1-test' });
  await page.reload();
  await waitForRoute(page, 'main');

  // --- sans dossier screenshots/ : liste vide, pas d'erreur ---
  expect(await invoke<Shot[]>(page, 'instances_screenshots_list', { instance_id: host.id })).toEqual([]);
  await page.evaluate(() => { window.location.hash = '#/instance?id=host-world&tab=captures'; });
  await waitForRoute(page, 'instance');
  await expect(page.getByRole('tab', { name: 'Captures' })).toHaveAttribute('aria-selected', 'true');
  await expect(page.getByText(/Aucune capture pour le moment/)).toBeVisible();
  await page.getByRole('button', { name: 'Ouvrir le dossier' }).click();
  await expect(page.getByRole('alert')).toHaveCount(0);
  expect(fs.existsSync(path.join(host.gameDir, 'screenshots')), 'le dossier est cree a la demande').toBe(true);

  // --- une capture du jeu et des intrus : seule l'image est listee ---
  const dir = path.join(host.gameDir, 'screenshots');
  fs.writeFileSync(path.join(dir, '2026-11-20_21.14.03.png'), PNG);
  fs.writeFileSync(path.join(dir, 'notes.txt'), 'pas une image');
  fs.writeFileSync(path.join(dir, 'faux.png'), '<html>pas une image</html>');
  const list = await invoke<Shot[]>(page, 'instances_screenshots_list', { instance_id: host.id });
  expect(list.map((s) => s.file).sort()).toEqual(['2026-11-20_21.14.03.png', 'faux.png']);

  // La fenetre reprend le focus (capture prise en jeu) : l'onglet se recharge sans clic.
  await page.evaluate(() => window.dispatchEvent(new Event('focus')));
  const grid = page.getByRole('list', { name: 'Captures' });
  await expect(grid.getByRole('listitem')).toHaveCount(2);
  await expect(grid.locator('img').first()).toBeVisible();
  await page.screenshot({ path: path.join(OUT, 'onglet-captures.png') });

  // --- visionneuse : ouverture puis Echap ---
  await grid.getByRole('button', { name: /^Agrandir la capture/ }).first().click();
  await expect(page.getByRole('dialog')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog')).toHaveCount(0);

  // --- noms dangereux et faux contenu refuses ---
  for (const file of ['../instance.json', '..\\instance.json', 'a/b.png', path.join(host.dir, 'instance.json'), 'notes.txt', 'faux.png']) {
    await expect(invoke(page, 'instances_screenshot_image', { instance_id: host.id, file }), file).rejects.toBeTruthy();
    await expect(invoke(page, 'instances_screenshot_set_banner', { instance_id: host.id, file }), file).rejects.toBeTruthy();
  }
  await expect(invoke(page, 'instances_screenshots_list', { instance_id: '../x' })).rejects.toBeTruthy();
  const url = await invoke<string>(page, 'instances_screenshot_image', { instance_id: host.id, file: '2026-11-20_21.14.03.png' });
  expect(url).toMatch(/^data:image\/png;base64,/);

  // --- « Comme bannière » : copie dans art/, la capture d'origine reste ---
  await grid.getByRole('button', { name: /comme bannière$/ }).first().click();
  await expect(page.getByText('Bannière définie.')).toBeVisible();
  const inst = (await invoke<Inst[]>(page, 'instances_list')).find((i) => i.id === host.id)!;
  expect(inst.art?.screenshots[0]).toBe('shot-1.png');
  expect(fs.readFileSync(path.join(host.dir, 'art', 'shot-1.png')).equals(PNG)).toBe(true);
  expect(fs.existsSync(path.join(dir, '2026-11-20_21.14.03.png'))).toBe(true);
});
