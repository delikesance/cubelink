// Multijoueur EN REEL : relais public n0, Internet requis. Active par E2E_REAL=1 (sinon ignore).
//   E2E_REAL=1 npx playwright test -c e2e 12-multiplayer-real --workers=1
// Deux apps A (hote) et B (invite), toutes deux en « toujours le relais » (relay_only : aucun transport IP,
// donc chemin relaye comme deux reseaux differents) et decouverte activee. Parcours complet :
// ajout d'ami (code, verification, acceptation), partage auto, join_plan, transfert (via_relay === true,
// hash verifies), tunnel TCP reel jusqu'au faux monde. Les temps mesures sont journalises (console + annotations).
import net from 'node:net';
import path from 'node:path';
import { test, expect, invoke, testState, makeHostInstance, startFakeWorld, loginOffline } from './harness/fixtures';
import { makeBigHost, recordEvents, waitProgress, lastProgress, guestInstance, sha256File, waitOnline, events } from './helpers/e2e2-common';
import { NAME_A, NAME_B, keyOfCode, makeFriends as makeFriendsUi, readMyCode } from './helpers/e2e1-common';

test.skip(process.env.E2E_REAL !== '1', 'Reserve a E2E_REAL=1 : Internet et relais public n0 requis.');

test.use({ appOptions: { relayMode: 'default', settings: { relay_only: true, allow_relay: true, discovery: true } } });

const timings: Record<string, number> = {};
async function timed<T>(label: string, f: () => Promise<T>): Promise<T> {
  const t = Date.now();
  try { return await f(); } finally { timings[label] = Date.now() - t; console.log(`[real] ${label} : ${timings[label]} ms`); }
}

/** Une poignee de main TCP (0x42 -> 0x24) a travers le tunnel ; renvoie la latence aller-retour en ms. */
function tunnelRoundTrip(port: number): Promise<number> {
  return new Promise((resolve, reject) => {
    const t0 = process.hrtime.bigint();
    const s = net.connect(port, '127.0.0.1', () => s.write(Buffer.from([0x42])));
    s.setTimeout(30_000, () => { s.destroy(); reject(new Error('tunnel : pas de reponse en 30 s')); });
    s.once('data', (d) => { s.destroy(); expect(d[0]).toBe(0x24); resolve(Number(process.hrtime.bigint() - t0) / 1e6); });
    s.once('error', reject);
  });
}

test('multijoueur reel via le relais public : ami, partage, join, transfert relaye, tunnel', async ({ appA, appB }) => {
  test.setTimeout(600_000);
  for (const [app, name] of [[appA, NAME_A], [appB, NAME_B]] as const) {
    await app.page.waitForSelector('main[data-route="login"]');
    await loginOffline(app.page, name);
    const s = await invoke<{ relay_only: boolean; discovery: boolean }>(app.page, 'settings_get');
    expect(s.relay_only, 'relay_only lu depuis settings.json au demarrage').toBe(true);
    expect(s.discovery).toBe(true);
    expect(JSON.stringify((await testState(app.page)).relay_mode)).toMatch(/default/i);
  }

  // 1. Ajout d'ami complet par l'interface (code, verification, acceptation) via le relais.
  await timed('ajout_ami', async () => {
    const codeA = await readMyCode(appA);
    // Le code de A ne contient que le relais (pas d'IP : relay_only) : on le verifie via sa cle seulement.
    expect(codeA).toMatch(/^mcl1-/);
    await makeFriendsUi(appA, appB);
  });
  const keyA = keyOfCode(await invoke<string>(appA.page, 'friend_my_code'));
  const friends = await invoke<{ friend: { public_key: string } }[]>(appB.page, 'friend_list');
  expect(friends[0].friend.public_key).toBe(keyA);

  // 2. Presence : B voit A en ligne.
  await timed('presence_en_ligne', () => waitOnline(appB.page, 90_000));

  // 3. A prepare un monde (2 fichiers de 3 Mo), lance le faux monde LAN et partage.
  const host = await makeBigHost(appA, { count: 2, sizeMb: 3 });
  const fake = await startFakeWorld(appA);
  await invoke(appA.page, 'host_start_sharing', { instance_id: host.id, world_port: fake.lanPort });

  // 4. B : plan puis transfert.
  await recordEvents(appB.page, 'transfer://progress');
  const plan = await timed('join_plan', () => invoke<{ to_receive: number; manifest: { files: unknown[] } }>(appB.page, 'join_plan', { friend_public_key: keyA }));
  expect(plan.manifest.files).toHaveLength(2);
  expect(plan.to_receive).toBe(2);
  await timed('transfert', async () => {
    await invoke(appB.page, 'join_start', { friend_public_key: keyA });
    await waitProgress(appB.page, (p) => p.phase === 'done' || p.phase === 'failed', 300_000, 'fin du transfert');
  });
  const last = await lastProgress(appB.page) as { phase: string; error?: string; via_relay?: boolean; bytes_done: number; speed_bps?: number };
  expect(last.phase, last.error).toBe('done');
  const prog = await (await import('./helpers/e2e2-common')).events<{ phase: string; via_relay: boolean; speed_bps: number }>(appB.page, 'transfer://progress');
  const rcv = prog.filter((p) => p.phase === 'receiving');
  expect(rcv.length).toBeGreaterThan(0);
  expect(rcv.every((p) => p.via_relay), 'tous les evenements de reception passent par le relais').toBe(true);
  const maxSpeed = Math.max(...prog.map((p) => p.speed_bps ?? 0));
  console.log(`[real] debit max observe : ${(maxSpeed / 1e6).toFixed(2)} Mo/s ; octets : ${last.bytes_done}`);
  timings['debit_max_Mo_s'] = Math.round(maxSpeed / 1e4) / 100;

  // Hash des fichiers recus identiques a l'hote.
  const g = await guestInstance(appB);
  expect(g, 'instance invitee creee').not.toBeNull();
  for (const f of host.files) expect(sha256File(path.join(g!.gameDir, ...f.path.split('/')))).toBe(f.sha256);

  // 5. Tunnel : echange TCP reel (poignee de main du faux monde) a travers le relais.
  const port = await timed('ouverture_tunnel', () => invoke<number>(appB.page, 'join_open_tunnel', { friend_public_key: keyA }));
  expect(port).toBeGreaterThan(0);
  try {
    const lat: number[] = [];
    lat.push(await timed('tunnel_premier_aller_retour', () => tunnelRoundTrip(port)));
    for (let i = 0; i < 5; i++) lat.push(await tunnelRoundTrip(port));
    const sorted = [...lat.slice(1)].sort((a, b) => a - b);
    timings['tunnel_latence_mediane_ms'] = Math.round(sorted[Math.floor(sorted.length / 2)]);
    console.log(`[real] latences tunnel (ms) : ${lat.map((x) => x.toFixed(0)).join(', ')}`);

    // 5b. Coupure de la connexion iroh invite -> hote (hook de test) : reconnexion via le relais, MEME port local.
    await recordEvents(appB.page, 'tunnel://state');
    expect(await invoke<boolean>(appB.page, 'test_cut_guest_connection'), 'un tunnel est ouvert').toBe(true);
    const tCut = Date.now();
    let again = -1;
    for (let i = 0; i < 4 && again < 0; i++) {
      try { again = await tunnelRoundTrip(port); } catch { await new Promise((r) => setTimeout(r, 1000)); }
    }
    timings['tunnel_reprise_apres_coupure_ms'] = Date.now() - tCut;
    console.log(`[real] reprise du tunnel apres coupure : ${timings['tunnel_reprise_apres_coupure_ms']} ms (aller-retour ${again.toFixed(0)} ms)`);
    expect(again, 'echo via le meme port local apres reconnexion').toBeGreaterThan(0);
    expect(timings['tunnel_reprise_apres_coupure_ms']).toBeLessThan(15_000);
    const states = (await events<{ state: string }>(appB.page, 'tunnel://state')).map((e) => e.state);
    console.log(`[real] tunnel://state : ${states.join(' -> ') || '(aucun)'}`);
    expect(states, 'jamais perdu').not.toContain('lost');
    expect(await testState(appB.page).then((t) => t.guest_tunnel_port), 'meme port').toBe(port);
  } finally {
    await invoke(appB.page, 'join_close_tunnel').catch(() => {});
  }

  // 6. Apres l'arret du partage chez A : message distinct « n'a pas encore ouvert son monde ».
  await invoke(appA.page, 'host_stop_sharing');
  const err = await invoke(appB.page, 'join_plan', { friend_public_key: keyA }).then(() => '', (e) => String(e));
  expect(err).toMatch(/pas encore ouvert son monde/);

  console.log('[real] RESUME ' + JSON.stringify(timings));
  test.info().annotations.push({ type: 'temps', description: JSON.stringify(timings) });
});
