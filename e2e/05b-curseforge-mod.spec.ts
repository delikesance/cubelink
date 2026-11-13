// T5b : un mod absent de Modrinth mais publie sur CurseForge (reconnu par empreinte murmur2) n'est PAS « hors
// catalogue » : ni avertissement ni case de confiance, avant comme apres reception. Faux CurseForge local (/v1/fingerprints).
import crypto from 'node:crypto';
import fs from 'node:fs';
import http from 'node:http';
import path from 'node:path';
import type { AddressInfo } from 'node:net';
import { test as base, expect, invoke, type AppOptions } from './harness/fixtures';
import { HOME_MOD, befriend, captureEvents, events, gotoHash, hostShare } from './helpers/e2e3-common';

/** Empreinte CurseForge : MurmurHash2 32 bits, graine 1, octets blancs (9, 10, 13, 32) retires. */
function murmur2(buf: Buffer): number {
  const data = buf.filter((b) => b !== 9 && b !== 10 && b !== 13 && b !== 32);
  const M = 0x5bd1e995;
  let h = (1 ^ data.length) >>> 0;
  let i = 0;
  for (; i + 4 <= data.length; i += 4) {
    let k = (data[i] | (data[i + 1] << 8) | (data[i + 2] << 16) | (data[i + 3] << 24)) >>> 0;
    k = Math.imul(k, M) >>> 0;
    k = (k ^ (k >>> 24)) >>> 0;
    k = Math.imul(k, M) >>> 0;
    h = (Math.imul(h, M) ^ k) >>> 0;
  }
  const rest = data.length - i;
  if (rest > 0) {
    if (rest >= 3) h = (h ^ (data[i + 2] << 16)) >>> 0;
    if (rest >= 2) h = (h ^ (data[i + 1] << 8)) >>> 0;
    h = (h ^ data[i]) >>> 0;
    h = Math.imul(h, M) >>> 0;
  }
  h = (h ^ (h >>> 13)) >>> 0;
  h = Math.imul(h, M) >>> 0;
  return (h ^ (h >>> 15)) >>> 0;
}

/** Faux CurseForge : `known` empreinte -> sha1 publie ; compte les requetes recues. */
const cf = { known: new Map<number, string>(), asked: [] as number[][], base: '' };

const test = base.extend<{ appOptions: AppOptions }>({
  appOptions: [async ({}, use) => {
    const srv = http.createServer((req, res) => {
      let body = '';
      req.on('data', (d) => { body += d; });
      req.on('end', () => {
        if (req.method === 'POST' && req.url === '/v1/fingerprints/432' && req.headers['x-api-key']) {
          const fps: number[] = JSON.parse(body).fingerprints;
          cf.asked.push(fps);
          const exactMatches = fps.filter((f) => cf.known.has(f))
            .map((f, i) => ({ id: 900 + i, file: { id: 7000 + i, fileFingerprint: f, hashes: [{ value: cf.known.get(f), algo: 1 }] } }));
          res.writeHead(200, { 'content-type': 'application/json' }).end(JSON.stringify({ data: { exactMatches } }));
        } else {
          res.writeHead(404).end();
        }
      });
    });
    await new Promise<void>((r) => srv.listen(0, '127.0.0.1', r));
    cf.base = `http://127.0.0.1:${(srv.address() as AddressInfo).port}`;
    await use({ env: { CUBELINK_TEST_CF_API: cf.base } });
    srv.close();
  }, { option: true }],
});

test('T5b : mod CurseForge (inconnu de Modrinth) reconnu par empreinte, sans avertissement ni confiance', async ({ appA, appB, servers, binary }) => {
  const { keyA } = await befriend({ appA, appB, servers, binary });
  const { host } = await hostShare(appA);
  await captureEvents(appB, ['transfer://unknown_mods', 'transfer://progress']);

  // Le mod « maison » des autres tests est ici publie sur CurseForge.
  const jar = fs.readFileSync(path.join(host.gameDir, ...HOME_MOD.split('/')));
  const fp = murmur2(jar);
  cf.known.set(fp, crypto.createHash('sha1').update(jar).digest('hex'));

  const plan = await invoke<{ manifest: { files: { path: string; cf_fingerprint?: number }[] }; unknown_mods: string[]; unknown_unchecked: string[] }>(
    appB.page, 'join_plan', { friend_public_key: keyA });
  // L'hote annonce l'empreinte (calculee comme CurseForge), l'invite l'a envoyee au catalogue.
  expect(plan.manifest.files.find((f) => f.path === HOME_MOD)!.cf_fingerprint).toBe(fp);
  expect(cf.asked.flat()).toContain(fp);
  expect(plan.unknown_mods).toEqual([]);
  expect(plan.unknown_unchecked).toEqual([]);

  // Ecran Rejoindre : pas d'avertissement, bouton actif sans case de confiance.
  await gotoHash(appB, `#/join?friend=${keyA}`, 'join');
  const page = appB.page;
  const join = page.getByRole('button', { name: 'Rejoindre et jouer' });
  await expect(join).toBeEnabled({ timeout: 30_000 });
  await expect(page.getByRole('heading', { name: /mod inconnu va s'exécuter/ })).toHaveCount(0);
  await expect(page.getByLabel(/^Je fais confiance à/)).toHaveCount(0);

  // Reception puis re-verification locale (empreinte recalculee sur les octets recus) : toujours rien d'inconnu.
  const askedBefore = cf.asked.length;
  await join.click();
  await expect.poll(async () => (await events(appB, 'transfer://unknown_mods')).length, { timeout: 90_000 }).toBeGreaterThan(0);
  const ev = (await events(appB, 'transfer://unknown_mods')) as { unknown_mods: string[] }[];
  expect(ev[0].unknown_mods).toEqual([]);
  expect(cf.asked.length).toBeGreaterThan(askedBefore);
  expect(cf.asked.at(-1)).toContain(fp);
});
