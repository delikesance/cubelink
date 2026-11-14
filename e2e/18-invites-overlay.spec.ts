// E2E-18 : invitations « rejoins mon monde » entre deux vraies instances (A hote, B ami) et panneau en jeu (Scene).
// Deux applications simultanees, dossiers de donnees temporaires, CUBELINK_TEST_MODE=1 (harnais `world`).
import fs from 'node:fs';
import path from 'node:path';
import { test, expect, invoke, makeHostInstance } from './harness/fixtures';
import {
  setupPair, openWorld, invokeErr, record, recorded, until, sleep, emitEvent, stageState,
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

test('regle de jonction : un ami non invite est refuse ; sa demande refusee est limitee, acceptee elle ouvre l\'acces ; l\'arret du partage le retire', async ({ appA, appB }) => {
  const { host, keyA, keyB } = await setupPair({ appA, appB });
  await record(appA.page, 'friend://join_request');
  await record(appA.page, 'friend://join_requests');
  await openWorld(appA, host.id);

  // Ami non invite : ni plan, ni (donc) tunnel ; l'UI de B propose « Demander » et non « Rejoindre ».
  expect(await invokeErr(appB.page, 'join_plan', { friend_public_key: keyA }), 'plan refuse sans invitation').not.toBeNull();
  await appB.page.evaluate(() => { window.location.hash = '#/friends'; });
  await appB.page.waitForSelector('main[data-route="main"]');
  await expect(appB.page.getByRole('button', { name: /^Demander à rejoindre / })).toBeVisible({ timeout: 15_000 });
  await expect(appB.page.getByRole('button', { name: /^Rejoindre / })).toHaveCount(0);

  // Demande refusee : A la voit (evenement + liste), la refuse ; B recoit « refused ».
  const asked = invoke<string>(appB.page, 'friend_join_request', { public_key: keyA });
  const ev = await until('evenement friend://join_request sur A', async () => (await recorded<{ public_key: string; name: string; expires_at: number; received_at: number }>(appA.page, 'friend://join_request'))[0]);
  expect(ev.public_key).toBe(keyB);
  expect(ev.name).toBeTruthy();
  expect(ev.expires_at - ev.received_at).toBe(120);
  expect(await invoke<unknown[]>(appA.page, 'friend_join_requests')).toHaveLength(1);
  expect(await invokeErr(appB.page, 'friend_join_request', { public_key: keyA }), 'une seule demande en attente').toMatch(/déjà/);
  await invoke(appA.page, 'friend_join_request_answer', { public_key: keyB, accept: false });
  expect(await asked).toBe('refused');
  await until('file des demandes videe chez A', async () => (await invoke<unknown[]>(appA.page, 'friend_join_requests')).length === 0);
  expect(await invokeErr(appB.page, 'join_plan', { friend_public_key: keyA }), 'toujours refuse apres un refus').not.toBeNull();
  // Redemande avant 10 min : refus immediat, A n'est pas derange.
  expect(await invoke<string>(appB.page, 'friend_join_request', { public_key: keyA })).toBe('refused');
  expect(await recorded(appA.page, 'friend://join_request')).toHaveLength(1);
  // Decider d'une demande qui n'existe plus : erreur claire.
  expect(await invokeErr(appA.page, 'friend_join_request_answer', { public_key: keyB, accept: true })).toMatch(/plus valable/);

  // Une invitation n'est pas limitee par le refus : elle ouvre l'acces, et l'UI de B propose « Rejoindre ».
  expect(await invokeErr(appA.page, 'friend_invite', { public_key: keyB })).toBeNull();
  expect(await invokeErr(appB.page, 'join_plan', { friend_public_key: keyA }), 'plan accorde a l\'invite').toBeNull();
  await invoke(appA.page, 'host_stop_sharing');

  // L'arret du partage retire l'acces de l'invite.
  expect(await invokeErr(appB.page, 'join_plan', { friend_public_key: keyA })).not.toBeNull();
});

test('regle de jonction : demande acceptee (monde ouvert) ouvre l\'acces de la partie en cours seulement', async ({ appA, appB }) => {
  const { host, keyA, keyB } = await setupPair({ appA, appB });
  await openWorld(appA, host.id);
  const asked = invoke<string>(appB.page, 'friend_join_request', { public_key: keyA });
  await until('demande chez A', async () => (await invoke<unknown[]>(appA.page, 'friend_join_requests')).length === 1);
  expect(await invokeErr(appB.page, 'join_plan', { friend_public_key: keyA }), 'pas d\'acces avant la reponse').not.toBeNull();
  await invoke(appA.page, 'friend_join_request_answer', { public_key: keyB, accept: true });
  expect(await asked).toBe('accepted');
  expect(await invokeErr(appB.page, 'join_plan', { friend_public_key: keyA }), 'plan accorde apres acceptation').toBeNull();
  await appB.page.evaluate(() => { window.location.hash = '#/friends'; });
  await appB.page.waitForSelector('main[data-route="main"]');
  await expect(appB.page.getByRole('button', { name: /^Rejoindre / })).toBeVisible({ timeout: 15_000 });
  // Fin de partie : l'acces tombe ; la partie suivante repart de zero.
  await invoke(appA.page, 'host_stop_sharing');
  expect(await invokeErr(appB.page, 'join_plan', { friend_public_key: keyA })).not.toBeNull();
  await openWorld(appA, host.id);
  expect(await invokeErr(appB.page, 'join_plan', { friend_public_key: keyA }), 'acces non reconduit').not.toBeNull();
});

test('panneau en jeu : raccourci valide a l\'enregistrement ; hors Scene (mode test) stage_state reste au repos et stage_panel ne fait rien, meme avec un jeu simule', async ({ appA, appB }) => {
  // Le panneau « en jeu » est la bande de droite de la Scene (fenetre du jeu adoptee) : il n'existe pas de fenetre #/overlay.
  // Le mode test n'active pas la Scene (CUBELINK_STAGE_TEST), on verifie donc le contrat hors Scene ; l'incrustation reelle
  // est couverte par les tests d'integration de `host/` et l'essai manuel.
  await setupPair({ appA, appB });
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

  // sans Scene : rien n'est rattache, ouvrir le panneau ne change rien
  const idle = await stageState(page);
  expect(idle).toEqual({ attached: false, game: false, panel: false });
  await invoke(page, 'stage_panel', { open: true });
  expect(await stageState(page)).toEqual(idle);

  // jeu simule en cours / fini : l'etat de la Scene ne bouge pas (aucune fenetre adoptee)
  await emitEvent(page, 'launch://state', { id: 'fake-instance', state: 'running', message: '' });
  await sleep(500);
  await invoke(page, 'stage_panel', { open: true });
  expect(await stageState(page)).toEqual(idle);
  await emitEvent(page, 'launch://state', { id: 'fake-instance', state: 'exited', message: '' });
  await sleep(300);
  expect(await stageState(page)).toEqual(idle);
  expect(appA.page.url()).not.toMatch(/overlay/);
  await expect(appA.page.locator('header.titlebar')).toBeVisible();
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
