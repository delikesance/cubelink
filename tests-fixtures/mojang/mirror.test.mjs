import test from 'node:test';
import assert from 'node:assert/strict';
import crypto from 'node:crypto';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { startMirror, VERSION_ID, OTHER_VERSION_ID } from './server.mjs';
import { readZip } from './zip.mjs';

const sha1 = (b) => crypto.createHash('sha1').update(b).digest('hex');
const sha256 = (b) => crypto.createHash('sha256').update(b).digest('hex');

const m = await startMirror({ port: 0 });
test.after(() => m.close());
const get = async (p) => {
  const r = await fetch(m.base + p);
  return { status: r.status, buf: Buffer.from(await r.arrayBuffer()) };
};
const getJson = async (p) => JSON.parse((await get(p)).buf.toString());
const control = (body) =>
  fetch(m.base + '/__control', { method: 'POST', body: JSON.stringify(body) }).then((r) => r.json());
const path = (url) => new URL(url).pathname;

test('manifeste : 2 versions, sha1 du version.json cohérent', async () => {
  const man = await getJson('/mc/game/version_manifest_v2.json');
  assert.deepEqual(man.versions.map((v) => v.id).sort(), [OTHER_VERSION_ID, VERSION_ID].sort());
  for (const v of man.versions) {
    const r = await get(path(v.url));
    assert.equal(r.status, 200);
    assert.equal(sha1(r.buf), v.sha1);
    assert.equal(JSON.parse(r.buf).id, v.id);
  }
});

test('version.json : arguments avec règles, client.jar, assetIndex, libraries cohérents', async () => {
  const man = await getJson('/mc/game/version_manifest_v2.json');
  const vj = await getJson(path(man.versions.find((v) => v.id === VERSION_ID).url));
  assert.ok(vj.arguments.game.some((a) => a.rules) && vj.arguments.jvm.some((a) => a.rules));
  assert.equal(vj.javaVersion.majorVersion, 21);

  const c = await get(path(vj.downloads.client.url));
  assert.equal(c.buf.length, vj.downloads.client.size);
  assert.equal(sha1(c.buf), vj.downloads.client.sha1);
  const jar = readZip(c.buf);
  assert.ok(jar.has('META-INF/MANIFEST.MF'));

  const idx = await get(path(vj.assetIndex.url));
  assert.equal(sha1(idx.buf), vj.assetIndex.sha1);
  assert.equal(idx.buf.length, vj.assetIndex.size);

  const a = vj.libraries[0].downloads.artifact;
  const la = await get(path(a.url));
  assert.equal(sha1(la.buf), a.sha1);
  assert.equal(la.buf.length, a.size);
  readZip(la.buf);
  assert.equal(a.path, 'com/cubelink/test/lib-a/1.0.0/lib-a-1.0.0.jar');
});

test('natives : zip avec faux .dll/.so/.dylib et META-INF', async () => {
  const man = await getJson('/mc/game/version_manifest_v2.json');
  const vj = await getJson(path(man.versions[0].url));
  const lib = vj.libraries[1];
  assert.equal(lib.natives.windows, 'natives-windows');
  const expect = { 'natives-windows': 'fake-natives.dll', 'natives-linux': 'libfake-natives.so', 'natives-macos': 'libfake-natives.dylib' };
  for (const [cls, dl] of Object.entries(lib.downloads.classifiers)) {
    const r = await get(path(dl.url));
    assert.equal(sha1(r.buf), dl.sha1);
    assert.equal(r.buf.length, dl.size);
    const z = readZip(r.buf);
    assert.ok(z.has(expect[cls]), cls);
    assert.ok(z.has('META-INF/MANIFEST.MF'));
  }
});

test('assets : 3 objets, hash et taille corrects', async () => {
  const idx = await getJson('/assets/indexes/17.json');
  const entries = Object.values(idx.objects);
  assert.equal(entries.length, 3);
  for (const o of entries) {
    const r = await get(`/resources/${o.hash.slice(0, 2)}/${o.hash}`);
    assert.equal(r.status, 200);
    assert.equal(sha1(r.buf), o.hash);
    assert.equal(r.buf.length, o.size);
  }
});

test('Fabric meta : liste, profil, libraries Maven avec sha1', async () => {
  const list = await getJson(`/v2/versions/loader/${VERSION_ID}`);
  assert.ok(list[0].loader.stable);
  assert.ok(list.some((e) => !e.loader.stable));
  const lv = list[0].loader.version;
  const prof = await getJson(`/v2/versions/loader/${VERSION_ID}/${lv}/profile/json`);
  assert.equal(prof.id, `fabric-loader-${lv}-${VERSION_ID}`);
  assert.equal(prof.inheritsFrom, VERSION_ID);
  assert.ok(prof.libraries.length >= 3);
  for (const l of prof.libraries) {
    const [g, a, v] = l.name.split(':');
    const rel = `${g.replaceAll('.', '/')}/${a}/${v}/${a}-${v}.jar`;
    const r = await get(path(l.url) + rel);
    assert.equal(r.status, 200);
    assert.equal(sha1(r.buf), l.sha1);
  }
  assert.equal((await get('/v2/versions/loader/9.9.9')).status, 404);
});

test('Adoptium : paquet, sha256 et taille, zip avec bin/java.exe et bin/java', async () => {
  const pkgs = await getJson('/v3/assets/latest/21/hotspot?architecture=x64&image_type=jre&os=windows');
  const p = pkgs[0].binary.package;
  const r = await get(path(p.link));
  assert.equal(r.buf.length, p.size);
  assert.equal(sha256(r.buf), p.checksum);
  const z = readZip(r.buf);
  const names = [...z.keys()];
  assert.ok(names.some((n) => n.endsWith('/bin/java.exe')));
  assert.ok(names.some((n) => n.endsWith('/bin/java')));
  assert.equal((await get('/v3/assets/latest/9/hotspot')).status, 404);
});

test('fautes : 404, 500 puis ok, hash faux, lenteur, reset, /__state', async () => {
  await control({ reset: true });
  const idxPath = '/assets/indexes/17.json';
  const good = (await get(idxPath)).buf;

  await control({ faults: [{ match: idxPath, mode: '404', times: 1 }] });
  assert.equal((await get(idxPath)).status, 404);
  assert.equal((await get(idxPath)).status, 200);

  await control({ faults: [{ match: idxPath, mode: '500', times: 2 }] });
  assert.equal((await get(idxPath)).status, 500);
  assert.equal((await get(idxPath)).status, 500);
  assert.equal((await get(idxPath)).status, 200);

  await control({ faults: [{ match: idxPath, mode: 'bad_hash', times: 1 }] });
  const bad = (await get(idxPath)).buf;
  assert.notEqual(sha1(bad), sha1(good));
  assert.deepEqual((await get(idxPath)).buf, good);

  const jarP = '/libraries/com/cubelink/test/lib-a/1.0.0/lib-a-1.0.0.jar';
  const goodJar = (await get(jarP)).buf;
  await control({ faults: [{ match: 'lib-a', mode: 'bad_hash', times: 1 }] });
  const badJar = (await get(jarP)).buf;
  assert.equal(badJar.length, goodJar.length);
  assert.notEqual(sha1(badJar), sha1(goodJar));

  await control({ faults: [{ match: idxPath, mode: 'slow', times: 1, delayMs: 300 }] });
  const t0 = Date.now();
  assert.equal((await get(idxPath)).status, 200);
  assert.ok(Date.now() - t0 >= 250);

  const st = await getJson('/__state');
  assert.ok(st.requests.some((r) => r.fault === '500'));
  assert.ok(st.counts[idxPath] >= 8);

  await control({ faults: [{ match: idxPath, mode: '500' }] });
  assert.equal((await get(idxPath)).status, 500);
  await control({ reset: true });
  assert.equal((await get(idxPath)).status, 200);
  assert.equal((await getJson('/__state')).requests.length, 1);

  const bad2 = await fetch(m.base + '/__control', { method: 'POST', body: '{"faults":[{"mode":"x"}]}' });
  assert.equal(bad2.status, 400);
});

test('CLI : --port 0 imprime la ligne ready et écoute sur 127.0.0.1', async () => {
  const child = spawn(process.execPath, [fileURLToPath(new URL('./server.mjs', import.meta.url)), '--port', '0'], { stdio: ['ignore', 'pipe', 'inherit'] });
  try {
    const line = await new Promise((res, rej) => {
      let acc = '';
      child.stdout.on('data', (d) => { acc += d; if (acc.includes('\n')) res(acc.split('\n')[0]); });
      child.on('error', rej);
      setTimeout(() => rej(new Error('timeout')), 10000);
    });
    const info = JSON.parse(line);
    assert.equal(info.ready, true);
    assert.equal(info.base, `http://127.0.0.1:${info.port}`);
    assert.equal((await fetch(info.base + '/mc/game/version_manifest_v2.json')).status, 200);
  } finally {
    child.kill();
  }
});
