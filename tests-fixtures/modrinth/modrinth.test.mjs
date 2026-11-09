import test, { before, after } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { createServer, readZip, sha1, sha256, sha512, MRPACK_NAME } from './lib.mjs';
import { makeInstance } from './make-instance-fixture.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const srv = createServer();
let base;
const get = (p, init) => fetch(base + p, init);
const control = (cfg) => get('/__control', { method: 'POST', body: JSON.stringify(cfg) });

before(async () => {
  ({ base } = await srv.start(0));
});
after(() => srv.close());
test.beforeEach(() => control({ reset: true }));

test('catalogue : 5 mods, jars valides, hashes exacts', () => {
  assert.equal(srv.catalog.length, 5);
  for (const m of srv.catalog) {
    assert.equal(m.sha1, sha1(m.jar));
    assert.equal(m.sha512, sha512(m.jar));
    const z = readZip(m.jar);
    assert.ok(z['fabric.mod.json'] || z['META-INF/neoforge.mods.toml'], m.filename);
  }
  assert.ok(srv.catalog.some((m) => readZip(m.jar)['META-INF/neoforge.mods.toml']));
});

test('déterminisme : deux serveurs, mêmes hashes', () => {
  const other = createServer();
  assert.deepEqual(other.catalog.map((m) => m.sha1), srv.catalog.map((m) => m.sha1));
  assert.equal(other.home.sha256, srv.home.sha256);
});

test('search : facets ET/OU, query, pagination', async () => {
  const facets = encodeURIComponent('[["versions:1.21.1"],["categories:fabric"],["project_type:mod"]]');
  let r = await (await get(`/v2/search?query=&facets=${facets}&limit=2&offset=1`)).json();
  assert.equal(r.total_hits, 4);
  assert.equal(r.hits.length, 2);
  assert.equal(r.offset, 1);
  assert.ok(r.hits[0].project_id && r.hits[0].slug && r.hits[0].title);
  r = await (await get('/v2/search?query=delta')).json();
  assert.equal(r.hits.length, 1);
  assert.equal(r.hits[0].slug, 'cubelink-delta');
  r = await (await get('/v2/search?facets=' + encodeURIComponent('[["versions:1.20.1"]]'))).json();
  assert.equal(r.total_hits, 0);
});

test('project/{id}/version : filtres loaders/game_versions, slug, 404', async () => {
  const m = srv.catalog[0];
  let v = await (await get(`/v2/project/${m.projectId}/version?game_versions=${encodeURIComponent('["1.21.1"]')}&loaders=${encodeURIComponent('["fabric"]')}`)).json();
  assert.equal(v.length, 1);
  assert.equal(v[0].files[0].hashes.sha1, m.sha1);
  assert.equal(v[0].files[0].url, `${base}/cdn/${m.filename}`);
  assert.equal(v[0].files[0].primary, true);
  v = await (await get(`/v2/project/${m.slug}/version?loaders=${encodeURIComponent('["neoforge"]')}`)).json();
  assert.deepEqual(v, []);
  assert.equal((await get('/v2/project/nope/version')).status, 404);
  const dep = await (await get(`/v2/project/${srv.catalog[4].projectId}/version`)).json();
  assert.equal(dep[0].dependencies[0].dependency_type, 'required');
});

test('version_file : sha1 et sha512 ; 404 pour le mod maison', async () => {
  const m = srv.catalog[2];
  let r = await get(`/v2/version_file/${m.sha1}?algorithm=sha1`);
  assert.equal(r.status, 200);
  assert.equal((await r.json()).project_id, m.projectId);
  r = await get(`/v2/version_file/${m.sha512}?algorithm=sha512`);
  assert.equal((await r.json()).id, m.versionId);
  assert.equal((await get(`/v2/version_file/${srv.home.sha1}?algorithm=sha1`)).status, 404);
});

test('version_files (POST) : inconnus absents', async () => {
  const hashes = [srv.catalog[0].sha1, srv.home.sha1, srv.catalog[1].sha1];
  const r = await get('/v2/version_files', { method: 'POST', body: JSON.stringify({ hashes, algorithm: 'sha1' }) });
  const j = await r.json();
  assert.deepEqual(Object.keys(j).sort(), [srv.catalog[0].sha1, srv.catalog[1].sha1].sort());
  assert.ok(!(srv.home.sha1 in j));
  assert.equal((await get('/v2/version_files', { method: 'POST', body: '{oops' })).status, 400);
});

test('cdn : jars identiques aux hashes annoncés, mod maison non distribué', async () => {
  for (const m of srv.catalog) {
    const b = Buffer.from(await (await get(`/cdn/${m.filename}`)).arrayBuffer());
    assert.equal(sha1(b), m.sha1);
    assert.equal(sha512(b), m.sha512);
  }
  assert.equal((await get(`/cdn/${srv.home.filename}`)).status, 404);
  assert.equal((await get('/cdn/inconnu.jar')).status, 404);
});

test('mrpack : index valide, téléchargements pointent sur le faux CDN avec hashes exacts', async () => {
  const b = Buffer.from(await (await get(`/cdn/${MRPACK_NAME}`)).arrayBuffer());
  const z = readZip(b);
  const idx = JSON.parse(z['modrinth.index.json']);
  assert.equal(idx.formatVersion, 1);
  assert.equal(idx.game, 'minecraft');
  assert.equal(idx.dependencies.minecraft, '1.21.1');
  assert.ok(z['overrides/config/testpack.toml']);
  assert.equal(idx.files.length, 2);
  for (const f of idx.files) {
    assert.ok(f.downloads[0].startsWith(base + '/cdn/'));
    const jar = Buffer.from(await (await fetch(f.downloads[0])).arrayBuffer());
    assert.equal(sha1(jar), f.hashes.sha1);
    assert.equal(sha512(jar), f.hashes.sha512);
    assert.equal(jar.length, f.fileSize);
  }
});

test('fautes : 429 + Retry-After, 500 à compteur, lenteur, hash altéré, reset', async () => {
  await control({ rateLimit: { count: 1, retryAfter: 7 } });
  let r = await get('/v2/search');
  assert.equal(r.status, 429);
  assert.equal(r.headers.get('retry-after'), '7');
  assert.equal((await get('/v2/search')).status, 200); // compteur épuisé

  await control({ error: { count: 1, status: 503 } });
  assert.equal((await get('/v2/search')).status, 503);
  assert.equal((await get('/v2/search')).status, 200);

  await control({ slowMs: 150 });
  const t = Date.now();
  await get('/v2/search');
  assert.ok(Date.now() - t >= 140);
  await control({ slowMs: 0 });

  const m = srv.catalog[0];
  await control({ tamper: true });
  const b = Buffer.from(await (await get(`/cdn/${m.filename}`)).arrayBuffer());
  assert.notEqual(sha1(b), m.sha1);
  assert.equal(b.length, m.jar.length);
  // l'API continue d'annoncer le hash correct
  assert.equal((await (await get(`/v2/version_file/${m.sha1}`)).json()).files[0].hashes.sha1, m.sha1);

  await control({ reset: true });
  const ok = Buffer.from(await (await get(`/cdn/${m.filename}`)).arrayBuffer());
  assert.equal(sha1(ok), m.sha1);
});

test('__state : journal des requêtes et fautes', async () => {
  await get('/v2/search?query=alpha', { headers: { 'User-Agent': 'cubelink/test' } });
  await control({ error: { status: 500 } });
  const s = await (await get('/__state')).json();
  assert.equal(s.requests.length, 1);
  assert.equal(s.requests[0].path, '/v2/search');
  assert.equal(s.requests[0].ua, 'cubelink/test');
  assert.equal(s.faults.error.status, 500);
  assert.equal(s.catalog.length, 5);
});

test('instance fixture : manifeste attendu, exclusions, mod maison inconnu du catalogue', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'cubelink-inst-'));
  try {
    const { files } = makeInstance(dir);
    const paths = files.map((f) => f.path);
    assert.equal(paths.filter((p) => p.startsWith('mods/')).length, 5);
    assert.ok(paths.includes('config/cubelink-test.toml'));
    assert.ok(!paths.some((p) => p.startsWith('saves') || p === 'options.txt'));
    for (const f of files) assert.equal(sha256(fs.readFileSync(path.join(dir, ...f.path.split('/')))), f.sha256);
    assert.ok(fs.existsSync(path.join(dir, 'options.txt')) && fs.existsSync(path.join(dir, 'saves')));
    const known = new Set(srv.catalog.map((m) => `mods/${m.filename}`));
    assert.equal(paths.filter((p) => p.startsWith('mods/') && !known.has(p)).length, 1);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('CLI : server.mjs --port 0 imprime la ligne ready ; build-home-mod imprime le sha256', async () => {
  const child = spawn(process.execPath, [path.join(here, 'server.mjs'), '--port', '0'], { stdio: ['ignore', 'pipe', 'inherit'] });
  try {
    const line = await new Promise((res, rej) => {
      child.stdout.once('data', (d) => res(d.toString().split('\n')[0]));
      child.once('error', rej);
    });
    const j = JSON.parse(line);
    assert.equal(j.ready, true);
    assert.equal(j.base, `http://127.0.0.1:${j.port}`);
    assert.equal((await fetch(j.base + '/v2/search')).status, 200);
  } finally {
    child.kill();
  }
  const out = await new Promise((res) => {
    const c = spawn(process.execPath, [path.join(here, 'build-home-mod.mjs')], { stdio: ['ignore', 'pipe', 'inherit'] });
    let s = '';
    c.stdout.on('data', (d) => (s += d));
    c.on('close', () => res(s));
  });
  assert.equal(JSON.parse(out).sha256, srv.home.sha256);
});
