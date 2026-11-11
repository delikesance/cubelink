// Session conservee + comptes multiples avec le VRAI binaire : fermeture/relance sur le meme dossier de donnees,
// faux serveur d'authentification local, dossier temporaire (jamais %APPDATA%\app.cubelink.launcher).
//   npx playwright test -c e2e 17-accounts --workers=1
// Binaire : celui du harnais (target/E0.4/debug, ou $CUBELINK_E2E_TARGET_DIR/debug), il doit contenir le front et le Rust a jour.
import fs from 'node:fs';
import path from 'node:path';
import { test, expect } from '@playwright/test';
import type { Page } from '@playwright/test';
import { Sandbox, ONLINE_A, ONLINE_B, resolveBinary, writeAccounts, writeLegacyTokens } from './helpers/e2e17-accounts';

type Acc = { id: string; kind: 'online' | 'offline'; name: string; uuid: string };
type AccState = { accounts: Acc[]; active: string | null };

const inv = <T = any>(page: Page, cmd: string, args: Record<string, unknown> = {}): Promise<T> =>
  page.evaluate(([c, a]) => (window as any).__TAURI_INTERNALS__.invoke(c as string, a as Record<string, unknown>), [cmd, args] as [string, Record<string, unknown>]) as Promise<T>;
const accounts = (page: Page) => inv<AccState>(page, 'auth_accounts');
const names = (s: AccState) => s.accounts.map((a) => a.name);
const activeName = (s: AccState) => s.accounts.find((a) => a.id === s.active)?.name ?? null;

/** Route affichee une fois le demarrage tranche : `main[data-route=login]` existe aussi pendant le chargement (`.boot`), on attend qu'il disparaisse. */
async function firstRoute(page: Page): Promise<string> {
  const h = await page.waitForFunction(() => {
    const m = document.querySelector('main[data-route]');
    if (!m) return null;
    const r = m.getAttribute('data-route');
    return r === 'login' && m.querySelector('.boot') ? null : r;
  }, undefined, { timeout: 30_000 });
  return (await h.jsonValue()) as string;
}
const route = (page: Page, r: string, timeout = 15_000) => page.waitForSelector(`main[data-route="${r}"]`, { timeout });

let sb: Sandbox;
let bin: string;
test.beforeAll(async () => { bin = await resolveBinary(); test.skip(!bin || !fs.existsSync(bin), `binaire introuvable : ${bin}`); });
test.beforeEach(async () => { sb = new Sandbox(bin); await sb.startAuth(); });
test.afterEach(async () => { await sb.dispose(); });

test.describe.configure({ mode: 'parallel' });

test('1. session en ligne conservee : connexion (faux Microsoft) puis fermeture/relance -> main sans ecran Connexion', async () => {
  let app = await sb.launch();
  expect(await firstRoute(app.page)).toBe('login'); // premier lancement : rien a restaurer
  expect(await inv(app.page, 'auth_status')).toBeNull();

  await app.page.getByRole('button', { name: 'Se connecter avec Microsoft' }).click();
  await expect(app.page.getByText('FAKE1234')).toBeVisible({ timeout: 15_000 });
  await route(app.page, 'main', 20_000); // le faux serveur valide au 1er sondage ; accueil apres WELCOME_MS
  const profile = await inv(app.page, 'auth_status');
  expect(profile).toMatchObject({ kind: 'online', name: 'TestPlayer' });
  const before = await accounts(app.page);
  expect(names(before)).toEqual(['TestPlayer']);
  expect(before.active).toBe(before.accounts[0].id);
  expect(fs.existsSync(path.join(sb.authDir, 'accounts.bin'))).toBe(true);
  expect(fs.existsSync(path.join(sb.authDir, 'tokens.bin'))).toBe(false);

  expect(await sb.close()).toBe(true); // sortie propre (WM_CLOSE), sans arret force
  app = await sb.launch();
  expect(await firstRoute(app.page)).toBe('main');
  await expect(app.page.locator('main[data-route="login"]')).toHaveCount(0);
  expect(await inv(app.page, 'auth_status')).toEqual(profile);
  expect(await accounts(app.page)).toEqual(before);
  await expect(app.page.locator('.acc-trigger')).toContainText('TestPlayer');
});

test('2. multi-comptes hors ligne : formulaire libre, ajout, 2 clics barre de titre, Reglages, retrait, relance, dernier retrait -> login', async () => {
  // Limite : le mode test est obligatoire pour isoler le dossier de donnees (CUBELINK_DATA_DIR n'est honore qu'avec CUBELINK_TEST_MODE=1) ;
  // lancer SANS mode test utiliserait le VRAI dossier de l'utilisateur, donc interdit. Le compte hors ligne n'a de toute facon plus aucune
  // condition de mode test (ni dans le front, ni dans la commande : voir les tests Rust offline_accepte_sans_drapeau_et_emet).
  let app = await sb.launch();
  expect(await firstRoute(app.page)).toBe('login');
  await expect(app.page.locator('.acc-trigger')).toHaveCount(0); // pas de selecteur sans compte

  // Formulaire hors ligne de l'ecran Connexion.
  await app.page.getByRole('button', { name: 'Jouer hors ligne' }).click();
  const field = app.page.getByLabel('Pseudo hors ligne');
  await expect(field).toBeVisible();
  await field.fill('ab'); // trop court
  await app.page.getByRole('button', { name: 'Jouer', exact: true }).click();
  await expect(app.page.locator('.login-offline [role="alert"], .login-offline .error, .login-offline [id$="-err"]').first()).toBeVisible();
  expect(await accounts(app.page)).toMatchObject({ accounts: [], active: null });
  await field.fill('Alice_1');
  await app.page.getByRole('button', { name: 'Jouer', exact: true }).click();
  await route(app.page, 'main');
  let st = await accounts(app.page);
  expect(names(st)).toEqual(['Alice_1']);
  expect(st.accounts[0].kind).toBe('offline');

  // 2e compte (hors ligne) via Reglages > Comptes.
  await app.page.evaluate(() => { window.location.hash = '#/settings'; });
  await route(app.page, 'settings');
  const card = app.page.getByRole('region', { name: 'Comptes' });
  await expect(card.locator('.acc-row')).toHaveCount(1);
  await card.getByRole('button', { name: 'Ajouter un compte hors ligne' }).click();
  await app.page.getByLabel('Pseudo hors ligne').fill('Bob_2');
  await card.getByRole('button', { name: 'Ajouter', exact: true }).click();
  await expect(card.locator('.acc-row')).toHaveCount(2);
  st = await accounts(app.page);
  expect(names(st)).toEqual(['Alice_1', 'Bob_2']);
  expect(activeName(st)).toBe('Bob_2'); // un compte qu'on vient d'ajouter devient le compte actif (idem Microsoft, test 3)

  // Changer de compte via Reglages ("Utiliser").
  await card.getByRole('button', { name: 'Utiliser Alice_1' }).click();
  await expect.poll(async () => activeName(await accounts(app.page))).toBe('Alice_1');
  await expect(card.locator('.acc-row[data-active] .name')).toHaveText('Alice_1');

  // Changer de compte via la barre de titre : 2 clics (ouvrir le menu, choisir).
  await expect(app.page.locator('.acc-trigger')).toContainText('Alice_1');
  await app.page.locator('.acc-trigger').click();
  await app.page.getByRole('menuitemradio', { name: /Bob_2/ }).click();
  await expect.poll(async () => activeName(await accounts(app.page))).toBe('Bob_2');
  await expect(app.page.locator('.acc-trigger')).toContainText('Bob_2');
  await app.page.locator('.acc-trigger').click();
  await app.page.getByRole('menuitemradio', { name: /Alice_1/ }).click();
  await expect.poll(async () => activeName(await accounts(app.page))).toBe('Alice_1');
  await expect(app.page.locator('.acc-trigger')).toContainText('Alice_1');
  expect(await inv(app.page, 'auth_status')).toMatchObject({ name: 'Alice_1', kind: 'offline' });

  // Persistance multi-comptes : fermeture propre puis relance.
  expect(await sb.close()).toBe(true);
  app = await sb.launch();
  expect(await firstRoute(app.page)).toBe('main');
  st = await accounts(app.page);
  expect(names(st)).toEqual(['Alice_1', 'Bob_2']);
  expect(activeName(st)).toBe('Alice_1');
  await app.page.locator('.acc-trigger').click();
  await expect(app.page.getByRole('menuitemradio')).toHaveCount(2);
  await expect(app.page.getByRole('menuitemradio', { name: /Alice_1/ })).toHaveAttribute('aria-checked', 'true');
  await app.page.keyboard.press('Escape');

  // Retrait avec confirmation : Annuler ne retire rien, Retirer retire.
  await app.page.evaluate(() => { window.location.hash = '#/settings'; });
  await route(app.page, 'settings');
  const card2 = app.page.getByRole('region', { name: 'Comptes' });
  await card2.getByRole('button', { name: 'Retirer Bob_2', exact: true }).click();
  await expect(card2.getByText('Retirer ce compte')).toBeVisible();
  await card2.getByRole('button', { name: 'Annuler' }).click();
  expect(names(await accounts(app.page))).toEqual(['Alice_1', 'Bob_2']);
  await card2.getByRole('button', { name: 'Retirer Bob_2', exact: true }).click();
  await card2.getByRole('button', { name: 'Confirmer le retrait de Bob_2' }).click();
  await expect(card2.locator('.acc-row')).toHaveCount(1);
  st = await accounts(app.page);
  expect(names(st)).toEqual(['Alice_1']);
  expect(activeName(st)).toBe('Alice_1');

  // Retrait du dernier compte -> route login ; et apres relance on reste sur login.
  await card2.getByRole('button', { name: 'Retirer Alice_1', exact: true }).click();
  await card2.getByRole('button', { name: 'Confirmer le retrait de Alice_1' }).click();
  await route(app.page, 'login');
  expect(await accounts(app.page)).toMatchObject({ accounts: [], active: null });
  expect(await inv(app.page, 'auth_status')).toBeNull();
  expect(await sb.close()).toBe(true);
  app = await sb.launch();
  expect(await firstRoute(app.page)).toBe('login');
});

test('3. comptes en ligne + hors ligne : bascule vers un compte en ligne, ajout Microsoft (faux serveur), restauration apres relance', async () => {
  writeAccounts(sb, [ONLINE_A, ONLINE_B], ONLINE_A.uuid); // deux comptes en ligne a jetons valides (le faux serveur ne sert qu'un seul profil)
  let app = await sb.launch();
  expect(await firstRoute(app.page)).toBe('main');
  expect(await accounts(app.page)).toMatchObject({ active: ONLINE_A.uuid });
  expect(names(await accounts(app.page))).toEqual([ONLINE_A.name, ONLINE_B.name]);

  // + hors ligne via la commande
  await inv(app.page, 'auth_offline_login', { name: 'Hors_Ligne' });
  let st = await accounts(app.page);
  expect(st.accounts.map((a) => a.kind)).toEqual(['online', 'online', 'offline']);

  // Bascule vers le 2e compte en ligne via la barre de titre (aucun reseau : jetons valides).
  await app.page.locator('.acc-trigger').click();
  await expect(app.page.locator('.acc-item .acc-tag')).toHaveCount(1); // seul le compte hors ligne porte l'etiquette
  await app.page.getByRole('menuitemradio', { name: new RegExp(ONLINE_B.name) }).click();
  await expect.poll(async () => (await accounts(app.page)).active).toBe(ONLINE_B.uuid);
  expect(await inv(app.page, 'auth_status')).toMatchObject({ kind: 'online', name: ONLINE_B.name });

  // Ajout d'un compte Microsoft depuis Reglages (faux serveur : profil TestPlayer).
  await app.page.evaluate(() => { window.location.hash = '#/settings'; });
  await route(app.page, 'settings');
  const card = app.page.getByRole('region', { name: 'Comptes' });
  await card.getByRole('button', { name: 'Ajouter un compte Microsoft' }).click();
  await expect(card.getByText('FAKE1234')).toBeVisible({ timeout: 15_000 });
  await expect(card.getByText('TestPlayer ajouté', { exact: false })).toBeVisible({ timeout: 20_000 });
  st = await accounts(app.page);
  expect(names(st)).toEqual([ONLINE_A.name, ONLINE_B.name, 'Hors_Ligne', 'TestPlayer']);
  expect(st.accounts[3].kind).toBe('online');
  const active = activeName(st);
  console.log(`[e2e17] compte actif apres ajout Microsoft : ${active}`);

  expect(await sb.close()).toBe(true);
  app = await sb.launch();
  expect(await firstRoute(app.page)).toBe('main');
  const after = await accounts(app.page);
  expect(after).toEqual(st); // liste (ordre compris) et compte actif restaures
  expect(await inv(app.page, 'auth_status')).toMatchObject({ name: active });

  // Retirer le compte actif : le premier restant devient actif ; la route reste main.
  const victim = activeName(after)!;
  await app.page.evaluate(() => { window.location.hash = '#/settings'; });
  await route(app.page, 'settings');
  const card2 = app.page.getByRole('region', { name: 'Comptes' });
  await card2.getByRole('button', { name: `Retirer ${victim}`, exact: true }).click();
  await card2.getByRole('button', { name: `Confirmer le retrait de ${victim}` }).click();
  await expect(card2.locator('.acc-row')).toHaveCount(3);
  const s3 = await accounts(app.page);
  expect(names(s3)).not.toContain(victim);
  expect(s3.active).toBe(s3.accounts[0].id);
  await route(app.page, 'settings');
});

test('4a. migration : ancien tokens.bin valide -> main direct, accounts.bin cree, tokens.bin supprime', async () => {
  writeLegacyTokens(sb, ONLINE_A);
  expect(fs.existsSync(path.join(sb.authDir, 'tokens.bin'))).toBe(true);
  let app = await sb.launch();
  expect(await firstRoute(app.page)).toBe('main');
  expect(await inv(app.page, 'auth_status')).toMatchObject({ kind: 'online', name: ONLINE_A.name, uuid: ONLINE_A.uuid });
  expect(await accounts(app.page)).toMatchObject({ active: ONLINE_A.uuid, accounts: [{ kind: 'online', name: ONLINE_A.name }] });
  await expect.poll(() => fs.existsSync(path.join(sb.authDir, 'accounts.bin'))).toBe(true);
  expect(fs.existsSync(path.join(sb.authDir, 'tokens.bin'))).toBe(false);
  // Persiste apres relance (maintenant lu depuis accounts.bin).
  expect(await sb.close()).toBe(true);
  app = await sb.launch();
  expect(await firstRoute(app.page)).toBe('main');
  expect(names(await accounts(app.page))).toEqual([ONLINE_A.name]);
});

test('4b. migration : ancien tokens.bin aux jetons EXPIRES -> renouvellement via le faux serveur, main direct', async () => {
  writeLegacyTokens(sb, ONLINE_A, true);
  const app = await sb.launch();
  const r = await firstRoute(app.page);
  const st = await accounts(app.page);
  console.log(`[e2e17] 4b : route=${r} comptes=${JSON.stringify(names(st))} actif=${activeName(st)}`);
  expect(r).toBe('main');
  expect(names(st)).toHaveLength(1);
  // Le renouvellement a remplace le profil par celui du faux serveur (TestPlayer) pour ce compte.
  const req = await (await fetch(`${sb.authBase}/__state`)).json() as { requests: { path: string }[] };
  expect(req.requests.some((x) => x.path.endsWith('/token'))).toBe(true);
});

test('5. formulaire hors ligne visible et fonctionnel sur l ecran Connexion (mode test uniquement, voir limite test 2)', async () => {
  const app = await sb.launch();
  expect(await firstRoute(app.page)).toBe('login');
  expect((await inv(app.page, 'test_state')).test_mode).toBe(true);
  await app.page.getByRole('button', { name: 'Jouer hors ligne' }).click();
  await app.page.getByLabel('Pseudo hors ligne').fill('Charlie');
  await app.page.getByRole('button', { name: 'Jouer', exact: true }).click();
  await route(app.page, 'main');
  expect(await inv(app.page, 'auth_status')).toMatchObject({ kind: 'offline', name: 'Charlie' });
});
