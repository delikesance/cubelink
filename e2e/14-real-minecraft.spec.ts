// VRAI Minecraft (Fabric 26.3, JDK 25) avec l'agent LAN. Active par E2E_MC=1 (fenetre de jeu brievement visible).
//   E2E_MC=1 npx playwright test -c e2e 14-real-minecraft --workers=1
// Data-dir temporaire : copies de versions/loaders/libraries, jonction vers assets, COPIE de l'instance
// « fabulously-optimized » (monde « Nouveau monde » inclus). Les donnees reelles de l'utilisateur ne sont pas modifiees.
// Internet requis (version manifest Mojang, meta Fabric) ; seuls les java.exe dont la ligne de commande contient
// le dossier de test sont tues.
import fs from 'node:fs';
import path from 'node:path';
import { test, expect, invoke } from './harness/fixtures';
import { AGENT_JAR, findJava25, javaPidsOfDir, killProcsOfDir, prepareDataDir, readLog, sleep, startRealApp, tmpRoot, waitLog, type RealApp } from './helpers/e2e14-real';

test.skip(process.env.E2E_MC !== '1', 'Reserve a E2E_MC=1 : vrai Minecraft, Internet, JDK 25.');

const ID = 'fabulously-optimized';
const WORLD = 'Nouveau monde';

async function boot(binary: string, root: string, env: Record<string, string>) {
  const dataDir = path.join(root, 'data');
  prepareDataDir(dataDir, ID);
  const app = await startRealApp(binary, root, dataDir, { CUBELINK_JAVA_PATH: findJava25(), CUBELINK_QUICKPLAY_WORLD: WORLD, ...env });
  await app.page.waitForSelector('main[data-route="login"]', { timeout: 30_000 });
  await invoke(app.page, 'auth_offline_login', { name: 'Testeur' });
  return app;
}

test.describe.configure({ mode: 'serial' });

test('agent LAN dans le vrai Minecraft : monde ouvert au reseau local, port detecte par Cubelink', async ({ binary }) => {
  test.setTimeout(600_000);
  const root = tmpRoot('cubelink-mc-');
  let app: RealApp | undefined;
  try {
    app = await boot(binary, root, { CUBELINK_LAN_AGENT: AGENT_JAR });
    await invoke(app.page, 'instances_launch', { id: ID });
    const a = app;
    const dead = () => (a.proc.exitCode !== null ? `app arretee : ${a.log.join('').slice(-2000)}` : null);
    await waitLog(a.dataDir, ID, /\[Cubelink\] LAN: agent charge/, 240_000, dead);
    await waitLog(a.dataDir, ID, /\[Cubelink\] LAN: classe Minecraft trouvee/, 60_000, dead);
    await waitLog(a.dataDir, ID, /\[Cubelink\] LAN: serveur solo trouve/, 240_000, dead);
    await waitLog(a.dataDir, ID, /\[Cubelink\] LAN: ouverture du monde/, 120_000, dead);
    const l5 = await waitLog(a.dataDir, ID, /Started serving on|Local game hosted on port|Published LAN server on port/, 60_000, dead);
    const lines = readLog(a.dataDir, ID).split(/\r?\n/).filter((l) => /\[Cubelink\] LAN:|Started serving|Local game hosted|Published LAN server/.test(l));
    console.log('[mc] journal reel :\n' + lines.join('\n'));
    const port = Number(/(\d{4,5})\s*$/.exec(l5)?.[1]);
    expect(port).toBeGreaterThan(1023);
    await expect.poll(async () => (await invoke<{ world_port: number | null }>(a.page, 'host_state')).world_port, { timeout: 60_000 }).toBe(port);
    const st = await invoke<{ instance_id: string; sharing: boolean; world_port: number }>(a.page, 'host_state');
    console.log('[mc] host_state : ' + JSON.stringify(st));
    expect(st.instance_id).toBe(ID);
  } finally {
    await app?.close();
    killProcsOfDir(root);
    fs.rmSync(root, { recursive: true, force: true, maxRetries: 10, retryDelay: 300 });
  }
});

test('garde-fou : un jar d\'agent invalide ne doit pas empecher le vrai jeu de demarrer', async ({ binary }) => {
  test.setTimeout(600_000);
  const root = tmpRoot('cubelink-mc-');
  let app: RealApp | undefined;
  try {
    const bad = path.join(root, 'cubelink-lan-agent.jar');
    fs.writeFileSync(bad, 'pas un jar');
    app = await boot(binary, root, { CUBELINK_LAN_AGENT: bad });
    await invoke(app.page, 'instances_launch', { id: ID });
    const a = app;
    const l = await waitLog(a.dataDir, ID, /Setting user:|Backend library|Reloading ResourceManager/, 300_000);
    console.log('[mc] jeu demarre apres relance sans agent : ' + l);
    const out = a.log.join('');
    console.log('[mc] sortie launcher (extrait) : ' + out.split(/\r?\n/).filter((x) => /Cubelink\] LAN|javaagent/i.test(x)).join(' | '));
    expect(out).toMatch(/l'agent a fait planter la JVM, relance sans agent/);
    expect(readLog(a.dataDir, ID)).not.toMatch(/\[Cubelink\] LAN: agent charge/);
    expect(javaPidsOfDir(root).length).toBeGreaterThan(0);
    await sleep(500);
  } finally {
    await app?.close();
    killProcsOfDir(root);
    fs.rmSync(root, { recursive: true, force: true, maxRetries: 10, retryDelay: 300 });
  }
});
