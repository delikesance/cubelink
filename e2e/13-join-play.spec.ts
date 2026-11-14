// T1 : « Rejoindre et jouer » (join_play) : tunnel ouvert, jeu lance avec --quickPlayMultiplayer, tunnel referme a la fin.
import fs from 'node:fs';
import path from 'node:path';
import { test, expect, grantByRequest, invoke, joinStart, testState, startFakeWorld } from './harness/fixtures';
import { setupHostAndGuest, sleep, recordEvents, events, waitProgress, guestInstance } from './helpers/e2e2-common';

test.describe.configure({ mode: 'serial' });
test.setTimeout(180_000);

const WITNESS = '.cubelink-fake-launch.json';
const STOP = '.cubelink-fake-stop';

async function installed(world: any) {
  const ctx = await setupHostAndGuest(world, { count: 3, sizeMb: 1 });
  const { appB } = world;
  await recordEvents(appB.page, 'transfer://progress');
  await recordEvents(appB.page, 'launch://state');
  await joinStart(appB.page, ctx.keyA);
  await waitProgress(appB.page, (p) => p.phase === 'done', 90_000, 'installation terminee');
  const guest = await guestInstance(appB);
  expect(guest).not.toBeNull();
  return { ...ctx, guest: guest! };
}

const states = async (page: any) => (await events<{ state: string }>(page, 'launch://state')).map((e) => e.state);

async function waitState(page: any, state: string, timeout = 30_000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    if ((await states(page)).includes(state)) return;
    await sleep(100);
  }
  throw new Error(`etat ${state} jamais atteint : ${JSON.stringify(await states(page))}`);
}

test('join_play : tunnel ouvert pendant le jeu, quickPlayMultiplayer vers le tunnel, tunnel referme a la fin', async ({ appA, appB, servers, world }) => {
  const { host, keyA, guest } = await installed({ appA, appB, servers, binary: world.binary });
  const fw = await startFakeWorld(appA);
  await invoke(appA.page, 'host_start_sharing', { instance_id: host.id, world_port: fw.lanPort });

  await invoke(appB.page, 'join_play', { friend_public_key: keyA, instance_id: guest.id });
  await waitState(appB.page, 'running');
  const port = (await testState(appB.page)).guest_tunnel_port;
  expect(port, 'tunnel invite ouvert pendant le jeu').toBeGreaterThan(0);

  const witnessFile = path.join(guest.gameDir, WITNESS);
  for (let i = 0; i < 100 && !fs.existsSync(witnessFile); i++) await sleep(100);
  const w = JSON.parse(fs.readFileSync(witnessFile, 'utf8'));
  const i = w.args.indexOf('--quickPlayMultiplayer');
  expect(i, 'argument de connexion automatique').toBeGreaterThanOrEqual(0);
  expect(w.args[i + 1]).toBe(`127.0.0.1:${port}`);
  expect(w.args).not.toContain('--server');

  fs.writeFileSync(path.join(guest.gameDir, STOP), '');
  await waitState(appB.page, 'exited');
  for (let k = 0; k < 50 && (await testState(appB.page)).guest_tunnel_port !== 0; k++) await sleep(100);
  expect((await testState(appB.page)).guest_tunnel_port, 'tunnel referme a la fin du jeu').toBe(0);
});

// Hote sans monde ouvert (port 0) : join_open_tunnel refuse en francais (refus explicite de l'hote, p2p/tunnel.rs).
test('join_play : hote sans monde ouvert, message francais clair et aucun tunnel ni jeu', async ({ appA, appB, servers, world }) => {
  const { host, keyA, guest } = await installed({ appA, appB, servers, binary: world.binary });
  // A partage ses fichiers mais n'a pas ouvert son monde au reseau local (port 0).
  // (setupHostAndGuest a partage avec un port : on arrete d'abord le partage, ce qui referme le monde.)
  await invoke(appA.page, 'host_stop_sharing');
  await invoke(appA.page, 'host_start_sharing', { instance_id: host.id, world_port: 0 });
  // L'arret du partage a retire l'acces de B (regle de jonction) : sa demande a rejoindre est acceptee par A.
  const keyB = (await invoke<{ friend: { public_key: string } }[]>(appA.page, 'friend_list'))[0].friend.public_key;
  await grantByRequest(appA, appB, keyA, keyB);

  const err = await invoke(appB.page, 'join_play', { friend_public_key: keyA, instance_id: guest.id }).then(
    () => null,
    (e: unknown) => String((e as any)?.message ?? e),
  );
  expect(err, 'join_play doit echouer').not.toBeNull();
  expect(err!).toMatch(/monde|r[ée]seau local|ouvr|hors ligne|joindre/i);
  expect(err!).not.toMatch(/^(error|failed|connection)/i);
  expect((await testState(appB.page)).guest_tunnel_port).toBe(0);
  expect(await states(appB.page)).not.toContain('running');
});
