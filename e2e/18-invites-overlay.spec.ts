// E2E-18 : invitations « rejoins mon monde » entre deux vraies instances (A hote, B ami) et panneau en jeu (overlay).
// Deux applications simultanees, dossiers de donnees temporaires, CUBELINK_TEST_MODE=1 (harnais `world`).
import fs from 'node:fs';
import path from 'node:path';
import { test, expect, invoke, makeHostInstance } from './harness/fixtures';
import {
  setupPair, openWorld, invokeErr, record, recorded, until, sleep, emitEvent, overlayState, overlayPage,
  TEST_HOTKEY, WORLD_NAME, type Invite,
} from './helpers/e2e18-invites';

test.setTimeout(180_000);

const invitesOf = (page: any) => invoke<Invite[]>(page, 'friend_invites');

test('invitation : A ouvre son monde et invite B ; B la recoit (evenement, file, UI) ; elle disparait quand A arrete le partage', async ({ appA, appB }) => {
  const { host, keyA, keyB } = await setupPair({ appA, appB });
  await record(appB.page, 'friend://invite');
  await record(appB.page, 'friend://invites');
  expect(await invitesOf(appB.page)).toEqual([]);

  await openWorld(appA, host.id);
  expect(await invokeErr(appA.page, 'friend_invite', { public_key: keyB }), 'invitation acceptee par le backend').toBeNull();

  // B : evenement friend://invite (charge utile Invite) puis file friend_invites
  const ev = await until('evenement friend://invite sur B', async () => (await recorded<Invite>(appB.page, 'friend://invite'))[0]).catch(async (e) => {
    throw new Error(`${e.message} ; file : ${JSON.stringify(await invitesOf(appB.page))} ; listes : ${JSON.stringify(await recorded(appB.page, 'friend://invites'))}`);
  });
  expect(ev.public_key).toBe(keyA);
  expect(ev.world_name).toBe(WORLD_NAME);
  expect(ev.minecraft).toBe('1.21.1-test');
  expect(ev.name).toBeTruthy();
  expect(ev.expires_at).toBeGreaterThan(ev.received_at);
  expect(ev.expires_at - ev.received_at).toBeLessThanOrEqual(5 * 60);
  const queue = await invitesOf(appB.page);
  expect(queue).toHaveLength(1);
  expect(queue[0].id).toBe(ev.id);
  const lists = await recorded<Invite[]>(appB.page, 'friend://invites');
  expect(lists.at(-1)).toHaveLength(1);

  // A n'a rien recu de B ; un second envoi immediat est refuse (limite de debit), message francais
  expect(await invitesOf(appA.page)).toEqual([]);
  expect(await invokeErr(appA.page, 'friend_invite', { public_key: keyB })).toMatch(/patientez/);

  // UI de B : ecran Amis -> bloc Invitations + badge « Heberge » sur l'ami
  await appB.page.evaluate(() => { window.location.hash = '#/friends'; });
  await expect(appB.page.getByRole('heading', { name: /Invitations/ })).toBeVisible({ timeout: 15_000 });
  await expect(appB.page.getByText(/t'invite/)).toBeVisible();
  await expect(appB.page.getByRole('button', { name: /Ignorer l'invitation de/ })).toBeVisible();
  await expect(appB.page.getByText('Héberge', { exact: true })).toBeVisible();

  // A arrete le partage : l'invitation est annulee chez B (message `revoke`)
  await invoke(appA.page, 'host_stop_sharing');
  await until('invitation retiree chez B', async () => (await invitesOf(appB.page)).length === 0);
  expect((await recorded<Invite[]>(appB.page, 'friend://invites')).at(-1)).toEqual([]);
  await expect(appB.page.getByRole('heading', { name: /Invitations/ })).toHaveCount(0, { timeout: 10_000 });
  await expect(appB.page.getByText('Héberge', { exact: true })).toHaveCount(0);
});

test('invitation : refusee sans monde ouvert (francais) ; ignorer supprime l\'invitation de la file et de l\'UI', async ({ appA, appB }) => {
  const { host, keyB } = await setupPair({ appA, appB });

  // aucun partage
  const noShare = await invokeErr(appA.page, 'friend_invite', { public_key: keyB });
  expect(noShare, 'refus sans partage').not.toBeNull();
  expect(noShare!).toMatch(/monde/i);
  expect(noShare!).toMatch(/ouvrez/i);
  expect(await invitesOf(appB.page)).toEqual([]);

  // partage sans monde ouvert au reseau local (port 0) : meme refus
  await invoke(appA.page, 'host_start_sharing', { instance_id: host.id, world_port: 0 });
  const noWorld = await invokeErr(appA.page, 'friend_invite', { public_key: keyB });
  expect(noWorld!).toMatch(/monde/i);
  expect(await invitesOf(appB.page)).toEqual([]);
  await invoke(appA.page, 'host_stop_sharing');

  // monde ouvert : cle inconnue refusee, ami valide accepte
  await openWorld(appA, host.id);
  expect(await invokeErr(appA.page, 'friend_invite', { public_key: 'ab'.repeat(32) })).toMatch(/inconnu/i);
  expect(await invokeErr(appA.page, 'friend_invite', { public_key: keyB })).toBeNull();
  const inv = await until('invitation recue', async () => (await invitesOf(appB.page))[0]);

  // ignorer : retire de la file, y compris pour un id inconnu sans erreur
  expect(await invokeErr(appB.page, 'friend_invite_dismiss', { id: 'inv-inexistante' })).toBeNull();
  expect(await invitesOf(appB.page)).toHaveLength(1);
  await appB.page.evaluate(() => { window.location.hash = '#/friends'; });
  const ignore = appB.page.getByRole('button', { name: /Ignorer l'invitation de/ });
  await expect(ignore).toBeVisible({ timeout: 15_000 });
  await ignore.click();
  await expect(ignore).toHaveCount(0, { timeout: 10_000 });
  expect(await invitesOf(appB.page)).toEqual([]);
  expect(inv.id).toBeTruthy();

  // par la commande
  await sleep(10_500); // limite de debit : 10 s entre deux invitations au meme ami
  expect(await invokeErr(appA.page, 'friend_invite', { public_key: keyB })).toBeNull();
  const again = await until('seconde invitation', async () => (await invitesOf(appB.page))[0]);
  await invoke(appB.page, 'friend_invite_dismiss', { id: again.id });
  expect(await invitesOf(appB.page)).toEqual([]);
});

test('overlay : indisponible sans jeu ; apres launch://state running la fenetre #/overlay existe (sans barre de titre) ; exited la rend indisponible', async ({ appA, appB }) => {
  const { keyA } = await setupPair({ appA, appB });
  const page = appA.page;

  // reglage : raccourci invalide refuse, valide accepte
  const s = await invoke<Record<string, any>>(page, 'settings_get');
  expect(s.overlay_enabled).toBe(true);
  for (const bad of ['Ctrl+Shift+', 'Ctrl+Shift+Banane', 'C', '']) {
    const e = await invokeErr(page, 'settings_set', { ...s, overlay_hotkey: bad });
    expect(e, `raccourci « ${bad} » refuse`).not.toBeNull();
    expect(e!).toMatch(/raccourci/i);
  }
  expect((await invoke<Record<string, any>>(page, 'settings_get')).overlay_hotkey, 'le raccourci invalide n\'a pas ete enregistre').toBe(TEST_HOTKEY);
  const saved = await invoke<Record<string, any>>(page, 'settings_set', { ...s, overlay_hotkey: 'Ctrl+Alt+Shift+F10' });
  expect(saved.overlay_hotkey).toBe('Ctrl+Alt+Shift+F10');
  expect((await invoke<Record<string, any>>(page, 'settings_get')).overlay_hotkey).toBe('Ctrl+Alt+Shift+F10');
  expect((await invoke<Record<string, any>>(page, 'settings_set', { ...s, overlay_hotkey: 'F9' })).overlay_hotkey).toBe('F9');
  await invoke(page, 'settings_set', { ...s, overlay_hotkey: TEST_HOTKEY });

  // sans jeu : indisponible, bascule refusee (francais)
  const idle = await overlayState(page);
  expect(idle).toMatchObject({ available: false, game_running: false, hotkey: TEST_HOTKEY });
  const err = await invokeErr(page, 'overlay_toggle');
  expect(err).toMatch(/aucun jeu/i);
  expect(overlayPage(appA)).toBeUndefined();

  // une invitation en attente chez A pour que le panneau l'affiche (B ouvre un monde et invite A)
  const hostB = await makeHostInstance(appB, { name: 'Monde de B' });
  await openWorld(appB, hostB.id);
  expect(await invokeErr(appB.page, 'friend_invite', { public_key: keyA })).toBeNull();
  await until('invitation de B chez A', async () => (await invoke<Invite[]>(page, 'friend_invites')).length === 1);

  // jeu simule en cours
  await emitEvent(page, 'launch://state', { id: 'fake-instance', state: 'running', message: '' });
  await until('overlay disponible', async () => (await overlayState(page)).available);
  expect(await overlayState(page)).toMatchObject({ available: true, game_running: true, hotkey: TEST_HOTKEY });

  expect(await invokeErr(page, 'overlay_toggle'), 'bascule acceptee pendant une partie').toBeNull();
  const ov = await until('page CDP de la fenetre overlay', async () => overlayPage(appA), 30_000);
  await ov.waitForSelector('main[data-route="overlay"]', { timeout: 20_000 });
  expect(ov.url()).toMatch(/#\/overlay$/);

  // contenu : amis, invitations, modpacks ; pas de barre de titre ; ni bandeau de mise a jour
  await expect(ov.getByRole('heading', { name: /^Invitations/ })).toBeVisible({ timeout: 15_000 });
  await expect(ov.getByRole('heading', { name: /^Amis/ })).toBeVisible();
  await expect(ov.getByRole('heading', { name: 'Modpacks' })).toBeVisible();
  await expect(ov.locator('header.titlebar')).toHaveCount(0);
  await expect(ov.getByRole('button', { name: 'Réduire' })).toHaveCount(0);
  await expect(ov.getByText('Monde de B')).toBeVisible({ timeout: 10_000 }); // invitation de B
  await expect(ov.getByRole('button', { name: /Ignorer l'invitation de/ })).toBeVisible();
  await expect(ov.getByRole('list', { name: 'Amis' }).getByRole('listitem')).toHaveCount(1);
  await expect(ov.getByText(WORLD_NAME)).toBeVisible(); // modpack de A (instance posee)
  await expect(ov.getByRole('button', { name: /^Lancer / })).toBeVisible();
  await expect(ov.getByText(TEST_HOTKEY)).toBeVisible();
  expect(await ov.evaluate(() => document.documentElement.hasAttribute('data-overlay'))).toBe(true);

  // la fenetre principale n'est pas affectee
  expect(appA.page.url()).not.toMatch(/overlay/);
  await expect(appA.page.locator('header.titlebar')).toBeVisible();

  // fin de partie : indisponible, fenetre masquee, bascule refusee
  await emitEvent(page, 'launch://state', { id: 'fake-instance', state: 'exited', message: '' });
  await until('overlay indisponible', async () => !(await overlayState(page)).available);
  expect(await overlayState(page)).toMatchObject({ available: false, game_running: false });
  expect(await invokeErr(page, 'overlay_toggle')).toMatch(/aucun jeu/i);
  // une preparation seule ne change rien
  await emitEvent(page, 'launch://state', { id: 'fake-instance', state: 'preparing', message: '' });
  await sleep(500);
  expect((await overlayState(page)).available).toBe(false);

  // desactive dans les reglages : indisponible meme avec un jeu
  await emitEvent(page, 'launch://state', { id: 'fake-instance', state: 'running', message: '' });
  await until('overlay disponible (2)', async () => (await overlayState(page)).available);
  await invoke(page, 'settings_set', { ...s, overlay_hotkey: TEST_HOTKEY, overlay_enabled: false });
  await until('overlay desactive', async () => !(await overlayState(page)).available);
  expect(await invokeErr(page, 'overlay_toggle')).toMatch(/aucun jeu/i);
  await emitEvent(page, 'launch://state', { id: 'fake-instance', state: 'exited', message: '' });
});

test('game_switch : sans jeu lance l\'instance ; avec un jeu le ferme puis relance (faux java)', async ({ appA, appB }) => {
  const { host } = await setupPair({ appA, appB });
  const page = appA.page;
  await record(page, 'launch://state');
  const states = async () => (await recorded<{ id: string; state: string }>(page, 'launch://state')).map((e) => e.state);
  const witness = path.join(host.gameDir, '.cubelink-fake-launch.json');

  // identifiant invalide ou inconnu : refus sans toucher a rien
  expect(await invokeErr(page, 'game_switch', { instance_id: '../x' })).not.toBeNull();
  expect(await invokeErr(page, 'game_switch', { instance_id: 'inconnue' })).not.toBeNull();
  expect(await states()).toEqual([]);

  const first = await invokeErr(page, 'game_switch', { instance_id: host.id });
  expect(first, 'game_switch sans jeu').toBeNull();
  await until('etat running', async () => (await states()).includes('running'), 60_000).catch(async (e) => {
    throw new Error(`${e.message} ; evenements : ${JSON.stringify(await recorded(page, 'launch://state'))}`);
  });
  await until('temoin de lancement', async () => fs.existsSync(witness) && JSON.parse(fs.readFileSync(witness, 'utf8')).done, 30_000);
  const w1 = JSON.parse(fs.readFileSync(witness, 'utf8'));
  fs.rmSync(witness, { force: true });

  // second appel pendant que le jeu tourne : arret propre puis nouveau lancement
  const before = (await states()).filter((s) => s === 'running').length;
  const second = await invokeErr(page, 'game_switch', { instance_id: host.id });
  expect(second, 'game_switch avec jeu en cours').toBeNull();
  await until('exited puis running', async () => {
    const st = await states();
    return st.includes('exited') && st.filter((s) => s === 'running').length > before;
  }, 90_000);
  await until('second temoin', async () => fs.existsSync(witness), 30_000);
  expect(JSON.parse(fs.readFileSync(witness, 'utf8')).version).toBe(w1.version);

  // nettoyage : arret du faux java
  fs.writeFileSync(path.join(host.gameDir, '.cubelink-fake-stop'), '');
  await until('exited final', async () => (await states()).filter((s) => s === 'exited').length >= 2, 30_000);
});
