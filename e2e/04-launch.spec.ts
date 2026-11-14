// T4 : lancement du jeu par B (profil hors ligne, faux java), temoin de lancement, monde LAN, fermeture, echec.
import fs from 'node:fs';
import net from 'node:net';
import path from 'node:path';
import { test, expect, invoke, joinStart, testState, startFakeWorld } from './harness/fixtures';
import {
  setupHostAndGuest, sleep, recordEvents, events, waitProgress, guestInstance, walk,
} from './helpers/e2e2-common';

test.describe.configure({ mode: 'serial' });
test.setTimeout(180_000);

const WITNESS = '.cubelink-fake-launch.json';
const STOP = '.cubelink-fake-stop';

/** A heberge (petits fichiers), B rejoint par commande (l'ecran Installation est teste dans 03) et attend la fin. */
async function installed(world: any, env: Record<string, string> = {}) {
  const ctx = await setupHostAndGuest(world, { count: 5, sizeMb: 1 }, env);
  const { appB } = world;
  await recordEvents(appB.page, 'transfer://progress');
  await recordEvents(appB.page, 'launch://state');
  await joinStart(appB.page, ctx.keyA);
  await waitProgress(appB.page, (p) => p.phase === 'done', 90_000, 'installation terminee');
  const g = await guestInstance(appB);
  expect(g).not.toBeNull();
  // l'accueil relit les instances au montage : on le remonte
  await appB.page.evaluate(() => { window.location.hash = '#/friends'; });
  await sleep(300);
  await appB.page.evaluate(() => { window.location.hash = '#/main'; });
  await appB.page.waitForSelector('main[data-route="main"]');
  return { ...ctx, guest: g! };
}

const states = async (page: any) => (await events<{ id: string; state: string; message?: string }>(page, 'launch://state')).map((e) => e.state);

async function waitState(page: any, state: string, timeout = 30_000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    if ((await states(page)).includes(state)) return;
    await sleep(100);
  }
  throw new Error(`etat ${state} jamais atteint : ${JSON.stringify(await states(page))}`);
}

test('lancement : preparation puis en cours, temoin correct, jeton jamais en clair', async ({ appA, appB, servers, world }) => {
  const { guest } = await installed({ appA, appB, servers, binary: world.binary });
  const page = appB.page;

  // L'instance invitee n'est pas dans la liste de l'accueil (modpack d'un ami) : on la lance par la commande,
  // comme le fait « Rejoindre » ; l'etat se lit dans les evenements launch://state.
  await invoke(page, 'instances_launch', { id: guest.id });
  await waitState(page, 'running');
  const seq = await states(page);
  expect(seq.indexOf('preparing')).toBeGreaterThanOrEqual(0);
  expect(seq.indexOf('preparing')).toBeLessThan(seq.indexOf('running'));

  const witnessFile = path.join(guest.gameDir, WITNESS);
  for (let i = 0; i < 100 && !fs.existsSync(witnessFile); i++) await sleep(100);
  const w = JSON.parse(fs.readFileSync(witnessFile, 'utf8'));
  expect(w.done).toBe(true);
  // game_dir de l'instance invitee de B (pas celui de l'hote)
  expect(path.resolve(w.game_dir).toLowerCase()).toBe(path.resolve(guest.gameDir).toLowerCase());
  expect(path.resolve(w.cwd).toLowerCase()).toBe(path.resolve(guest.gameDir).toLowerCase());
  expect(w.game_dir.toLowerCase()).not.toContain('host-world');
  expect(w.version).toBe('1.21.1-test');
  expect(w.classpath.length).toBeGreaterThan(0);
  expect(w.classpath_missing).toEqual([]);
  for (const p of w.classpath) expect(fs.existsSync(path.resolve(w.cwd, p)), `classpath : ${p}`).toBe(true);
  expect(w.main_class).toBeTruthy();
  const arg = (flag: string) => { const i = w.args.indexOf(flag); return i >= 0 ? w.args[i + 1] : undefined; };
  expect(arg('--username')).toBe('Invite');
  // jeton : masque dans le temoin, absent des journaux
  const token = arg('--accessToken');
  expect(token === '***' || token === undefined || token === '0').toBe(true);
  const dump = JSON.stringify(w) + appB.log.join('');
  expect(dump).not.toMatch(/fake-(mc|access|token)[\w-]*/i);
  const logs = walk(path.join(appB.dataDir, 'logs'), (p) => p.endsWith('.log') || p.endsWith('.old'));
  for (const f of logs) expect(fs.readFileSync(f, 'utf8')).not.toMatch(/fake-(mc|access|token)[\w-]*/i);

  // fermeture du faux java (fichier d'arret) : etat ferme
  fs.writeFileSync(path.join(guest.gameDir, STOP), '');
  await waitState(page, 'exited');
  expect(await states(page)).not.toContain('failed');
});

test('monde LAN : A ouvre son faux monde et le partage (cote hote)', async ({ appA, appB, servers, world }) => {
  const { host } = await installed({ appA, appB, servers, binary: world.binary });
  const fw = await startFakeWorld(appA);
  await invoke(appA.page, 'host_start_sharing', { instance_id: host.id, world_port: fw.lanPort });
  expect((await testState(appA.page)).tunnel_port).toBe(fw.lanPort);
  // le faux monde repond a la poignee de main (verifie le montage : 0x42 -> 0x24)
  const reply = await new Promise<number>((resolve, reject) => {
    const s = net.connect(fw.lanPort, '127.0.0.1', () => s.write(Buffer.from([0x42])));
    s.once('data', (d) => { s.destroy(); resolve(d[0]); });
    s.once('error', reject);
  });
  expect(reply).toBe(0x24);
});

test('monde LAN : B ouvre le tunnel invite et obtient la poignee de main du faux monde', async ({ appA, appB, servers, world }) => {
  const { host, keyA } = await installed({ appA, appB, servers, binary: world.binary });
  const fw = await startFakeWorld(appA);
  await invoke(appA.page, 'host_start_sharing', { instance_id: host.id, world_port: fw.lanPort });
  const local = await invoke<number>(appB.page, 'join_open_tunnel', { friend_public_key: keyA });
  try {
    expect(local, 'B expose un port local de tunnel').toBeGreaterThan(0);
    expect(local).not.toBe(fw.lanPort);
    expect((await testState(appB.page)).guest_tunnel_port).toBe(local);
    const reply = await new Promise<number>((resolve, reject) => {
      const s = net.connect(local, '127.0.0.1', () => s.write(Buffer.from([0x42])));
      s.once('data', (d) => { s.destroy(); resolve(d[0]); });
      s.once('error', reject);
    });
    expect(reply).toBe(0x24);
  } finally {
    await invoke(appB.page, 'join_close_tunnel').catch(() => {});
  }
});

test.describe('echec de lancement', () => {
  // code de sortie 3 : le faux java s'arrete tout seul apres 1,5 s
  const FAIL_ENV = { FAKE_JAVA_EXIT_CODE: '3', FAKE_JAVA_CRASH_AFTER_MS: '1500' };

  test('code de sortie different de 0 : message d\'echec en francais', async ({ appA, appB, servers, world }) => {
    const { guest } = await installed({ appA, appB, servers, binary: world.binary }, FAIL_ENV);
    const page = appB.page;
    await invoke(page, 'instances_launch', { id: guest.id });
    await waitState(page, 'failed', 60_000);
    const failed = (await events<{ state: string; message?: string }>(page, 'launch://state')).find((e) => e.state === 'failed')!;
    expect(failed.message ?? '').toMatch(/[a-zàâçéèêëîïôûù]{3,}/i);
    expect(failed.message ?? '', 'message en francais (pas de texte technique anglais seul)').not.toMatch(/^(exit|error|failed)/i);
  });
});
