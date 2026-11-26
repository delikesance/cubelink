#!/usr/bin/env node
// Faux Modrinth (/v2) + faux CurseForge (/v1) sur un seul port : `node tests-fixtures/browse/server.mjs --port 0`
// Sert aussi les fichiers (/files/<nom>) avec des hashes exacts. Utilise par e2e/10-browse.spec.ts.
// Routes :
//   GET  /files/<nom>                      fichier du catalogue (hashes exacts)
//   GET  /img/<nom>.png                    vrai PNG 320x180 (degrade colore selon le nom)
//   GET  /v2/search                        recherche Modrinth (ids `<kind>-N`, ex. modpack-1)
//   GET  /v2/project/{id}                  projet : title, description, color (entier), icon_url, gallery (4 images)
//   GET  /v2/project/{id}/members          [{ role: 'Owner', user: { username } }]
//   GET  /v2/team/{id}/members             idem (id = id du projet)
//   GET  /v2/project/{id}/version, /v2/version/{id}
//   GET  /v2/version_file/{sha1}?algorithm=sha1   version d'un fichier du catalogue (404 si inconnu)
//   GET  /v1/mods/search                   recherche CurseForge (header x-api-key: test-key, sinon 403)
//   GET  /v1/mods/{cfId}                   detail CurseForge (x-api-key requis) : logo, screenshots, authors
//   GET  /v1/mods/{cfId}/files[/{fileId}], POST /v1/mods/files
//   Modpack CurseForge a deux versions (projet 5000, fichiers 5001 = 1.0.0 et 5002 = 1.1.0, plus un pack serveur 5003
//   a ignorer) avec un mod non redistribuable (6004, sans downloadUrl) : e2e/21-modpack-update-cf.spec.ts.
import http from 'node:http';
import zlib from 'node:zlib';
import { buildZip, sha1, sha512 } from '../modrinth/lib.mjs';

const i = process.argv.indexOf('--port');
const port = i >= 0 ? Number(process.argv[i + 1]) : 0;
let BASE = '';

const jar = (id) => buildZip([
  { name: 'fabric.mod.json', data: JSON.stringify({ schemaVersion: 1, id, version: '1.0.0', name: id }) },
  { name: 'META-INF/MANIFEST.MF', data: 'Manifest-Version: 1.0\n' },
]);
const mrpack = (name, versionId = '1.0.0', extra = []) => buildZip([
  { name: 'modrinth.index.json', data: JSON.stringify({ formatVersion: 1, game: 'minecraft', versionId, name, files: [], dependencies: { minecraft: '1.21.1-test', 'fabric-loader': '0.16.5' } }) },
  { name: 'overrides/config/hello.txt', data: versionId === '1.0.0' ? 'hello' : 'hello v2' },
  ...extra,
]);

// ---- PNG minimal (sans dependance) ----
const crcTable = Array.from({ length: 256 }, (_, n) => { let c = n; for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1; return c >>> 0; });
const crc32 = (buf) => { let c = 0xffffffff; for (const b of buf) c = crcTable[(c ^ b) & 255] ^ (c >>> 8); return (c ^ 0xffffffff) >>> 0; };
const chunk = (type, data) => {
  const t = Buffer.from(type, 'ascii');
  const len = Buffer.alloc(4); len.writeUInt32BE(data.length);
  const crc = Buffer.alloc(4); crc.writeUInt32BE(crc32(Buffer.concat([t, data])));
  return Buffer.concat([len, t, data, crc]);
};
const hash = (str) => { let h = 2166136261; for (const ch of str) h = Math.imul(h ^ ch.charCodeAt(0), 16777619) >>> 0; return h; };
const makePng = (name, W = 320, H = 180) => {
  const h = hash(name);
  const a = [h & 255, (h >> 8) & 255, (h >> 16) & 255];
  const b = [(h >> 5) & 255, (h >> 13) & 255, (h >> 21) & 255];
  const raw = Buffer.alloc((W * 3 + 1) * H);
  for (let y = 0; y < H; y++) {
    const o = y * (W * 3 + 1);
    raw[o] = 0;
    for (let x = 0; x < W; x++) {
      const t = (x / W + y / H) / 2;
      for (let c = 0; c < 3; c++) raw[o + 1 + x * 3 + c] = Math.round(a[c] * (1 - t) + b[c] * t);
    }
  }
  const ihdr = Buffer.alloc(13); ihdr.writeUInt32BE(W, 0); ihdr.writeUInt32BE(H, 4); ihdr[8] = 8; ihdr[9] = 2;
  return Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), chunk('IHDR', ihdr), chunk('IDAT', zlib.deflateSync(raw, { level: 9 })), chunk('IEND', Buffer.alloc(0))]);
};

// ---- catalogue ----
const NAMES = ['Sodium', 'Lithium', 'Iris', 'Create', 'Jade', 'Xaero Minimap', 'Mod Menu', 'Journey', 'Farmers Delight', 'Waystones', 'Cloth Config', 'Fabric API'];
const files = new Map(); // nom -> Buffer
const mkFile = (name, buf) => { files.set(name, buf); return buf; };

const projects = [];
for (const kind of ['mod', 'modpack', 'resourcepack', 'shader']) {
  for (let n = 0; n < 30; n++) {
    const base = NAMES[n % NAMES.length];
    const name = `${base} ${kind === 'mod' ? '' : kind + ' '}${Math.floor(n / NAMES.length) || ''}`.trim();
    const id = `${kind}-${n}`;
    const ext = kind === 'mod' ? 'jar' : kind === 'modpack' ? 'mrpack' : 'zip';
    const fname = `${id}-1.0.0.${ext}`;
    const buf = kind === 'mod' ? jar(`m_${n}`) : kind === 'modpack' ? mrpack(name) : buildZip([{ name: 'pack.mcmeta', data: '{"pack":{"pack_format":34,"description":"x"}}' }]);
    mkFile(fname, buf);
    projects.push({ id, kind, name, fname, ext, buf, downloads: 1_000_000 - n * 31_337, restricted: n % 6 === 5, n });
  }
}
// Modpack à deux versions (mise à jour d'un modpack installé) : la 1.1.0 change hello.txt et ajoute added.txt.
// Même Minecraft que le pack de base ('1.21.1-test'), pour que l'instance importée soit compatible avec la détection.
const UPD = projects.find((x) => x.id === 'modpack-7');
UPD.fname2 = 'modpack-7-1.1.0.mrpack';
UPD.buf2 = mkFile(UPD.fname2, mrpack(UPD.name, '1.1.0', [{ name: 'overrides/config/added.txt', data: 'ajoute en 1.1.0' }]));

// Modpack CurseForge a deux versions, hors du catalogue de recherche (id de projet 5000). La 1.1.0 remplace un mod
// (a-1 -> a-2), ajoute un mod non redistribuable (sans downloadUrl) et change deux configs. Le fichier 5003 est un
// pack serveur plus recent, que la detection doit ignorer.
const CF_PACK_ID = 5000;
const cfMod = (fileId, name, restricted = false) => ({ fileId, name, restricted, buf: mkFile(name, jar(name.replace(/\W/g, '_'))) });
const CFM = { a1: cfMod(6001, 'cf-mod-a-1.jar'), a2: cfMod(6002, 'cf-mod-a-2.jar'), b: cfMod(6003, 'cf-mod-b.jar'), r: cfMod(6004, 'cf-restreint.jar', true) };
const cfZip = (mods, overrides) => buildZip([
  { name: 'manifest.json', data: JSON.stringify({ minecraft: { version: '1.21.1-test', modLoaders: [{ id: 'fabric-0.16.5', primary: true }] }, manifestType: 'minecraftModpack', name: 'CF Pack', overrides: 'overrides', files: mods.map((m) => ({ projectID: 7000 + m.fileId, fileID: m.fileId, required: true })) }) },
  ...overrides.map(([name, data]) => ({ name: `overrides/${name}`, data })),
]);
const CFP = [
  { id: 5001, name: 'cfpack-1.0.0.zip', display: 'CF Pack 1.0.0', date: '2025-01-01T00:00:00Z', server: false,
    buf: cfZip([CFM.a1, CFM.b], [['config/cf-keep.txt', 'keep v1'], ['config/cf-upd.txt', 'upd v1']]) },
  { id: 5002, name: 'cfpack-1.1.0.zip', display: 'CF Pack 1.1.0', date: '2025-06-01T00:00:00Z', server: false,
    buf: cfZip([CFM.a2, CFM.b, CFM.r], [['config/cf-keep.txt', 'keep v2'], ['config/cf-upd.txt', 'upd v2'], ['config/added-cf.txt', 'ajoute en 1.1.0']]) },
  { id: 5003, name: 'cfpack-server-1.2.0.zip', display: 'CF Pack Server 1.2.0', date: '2025-09-01T00:00:00Z', server: true,
    buf: cfZip([CFM.a2], [['config/server.txt', 'serveur']]) },
];
for (const f of CFP) mkFile(f.name, f.buf);
const cfPackFile = (f) => ({
  id: f.id, modId: CF_PACK_ID, displayName: f.display, fileName: f.name, fileDate: f.date, fileLength: f.buf.length, releaseType: 1,
  isServerPack: f.server, downloadUrl: `${BASE}/files/${f.name}`, hashes: [{ value: sha1(f.buf), algo: 1 }], gameVersions: ['1.21.1-test', 'Fabric'], dependencies: [],
});
const cfModFile = (m) => ({
  id: m.fileId, modId: 7000 + m.fileId, displayName: m.name, fileName: m.name, fileDate: '2025-01-01T00:00:00Z', fileLength: m.buf.length,
  downloadUrl: m.restricted ? null : `${BASE}/files/${m.name}`, hashes: [{ value: sha1(m.buf), algo: 1 }], gameVersions: ['1.21.1-test', 'Fabric'], dependencies: [],
});
// dépendance obligatoire : le mod 0 dépend du mod 1
const DEP = { from: 'mod-0', to: 'mod-1' };
const cfId = (p) => 1000 + projects.indexOf(p);
const CLASS = { 6: 'mod', 4471: 'modpack', 12: 'resourcepack', 6552: 'shader' };

const json = (res, code, body) => { res.writeHead(code, { 'content-type': 'application/json' }); res.end(JSON.stringify(body)); };
const page = (list, q) => {
  const off = Number(q.get('offset') ?? q.get('index') ?? 0);
  const lim = Math.min(50, Number(q.get('limit') ?? q.get('pageSize') ?? 20));
  return [list.slice(off, off + lim), list.length];
};
const mrVersion = (p, v = 'v1') => {
  const two = v === 'v2';
  const [fname, buf] = two ? [p.fname2, p.buf2] : [p.fname, p.buf];
  return {
    id: `${p.id}-${v}`, project_id: p.id, name: `${p.name} ${two ? '1.1.0' : '1.0.0'}`, version_number: two ? '1.1.0' : '1.0.0',
    date_published: two ? '2025-06-01T00:00:00Z' : '2025-01-01T00:00:00Z', version_type: 'release',
    game_versions: [p === UPD ? '1.21.1-test' : '1.21.1'], loaders: p.kind === 'mod' || p.kind === 'modpack' ? ['fabric'] : ['minecraft'],
    files: [{ url: `${BASE}/files/${fname}`, filename: fname, primary: true, size: buf.length, hashes: { sha1: sha1(buf), sha512: sha512(buf) } }],
    dependencies: p.id === DEP.from ? [{ project_id: DEP.to, version_id: null, dependency_type: 'required' }] : [],
  };
};
const cfFile = (p) => ({
  id: cfId(p) * 10, modId: cfId(p), displayName: `${p.name} 1.0.0`, fileName: p.fname, fileDate: '2025-01-01T00:00:00Z',
  fileLength: p.buf.length, downloadUrl: p.restricted ? null : `${BASE}/files/${p.fname}`, hashes: [{ value: sha1(p.buf), algo: 1 }],
  gameVersions: ['1.21.1', 'Fabric'], dependencies: [],
});

const server = http.createServer((req, res) => {
  const u = new URL(req.url, 'http://x');
  const p = u.pathname;
  const q = u.searchParams;
  let m;
  if ((m = p.match(/^\/files\/(.+)$/))) {
    const b = files.get(decodeURIComponent(m[1]));
    if (!b) return json(res, 404, {});
    res.writeHead(200, { 'content-type': 'application/octet-stream', 'content-length': b.length });
    return res.end(b);
  }
  if ((m = p.match(/^\/img\/([^/]+)\.png$/))) {
    const b = makePng(decodeURIComponent(m[1]));
    res.writeHead(200, { 'content-type': 'image/png', 'content-length': b.length });
    return res.end(b);
  }
  // ---- Modrinth ----
  if ((m = p.match(/^\/v2\/(?:project|team)\/([^/]+)\/members$/)) || (m = p.match(/^\/v2\/team\/([^/]+)\/members$/))) {
    const x = projects.find((y) => y.id === m[1]);
    return x ? json(res, 200, [{ role: 'Owner', user: { username: 'auteur' + (x.n % 3) } }]) : json(res, 404, {});
  }
  if ((m = p.match(/^\/v2\/project\/([^/]+)$/))) {
    const x = projects.find((y) => y.id === m[1]);
    if (!x) return json(res, 404, {});
    return json(res, 200, {
      id: x.id, slug: x.id, title: x.name, description: `Description de ${x.name}.`, team: x.id, color: 3043309 + x.n * 1000,
      icon_url: `${BASE}/img/${x.id}-icon.png`,
      gallery: [1, 2, 3, 4].map((k) => ({ url: `${BASE}/img/${x.id}-${k}.png`, featured: k === 1, ordering: k - 1 })),
    });
  }
  if (p === '/v2/search') {
    const facets = JSON.parse(q.get('facets') || '[]').flat();
    const kind = (facets.find((f) => f.startsWith('project_type:')) || 'project_type:mod').split(':')[1];
    const text = (q.get('query') || '').toLowerCase();
    let list = projects.filter((x) => x.kind === kind && x.name.toLowerCase().includes(text));
    const idx = q.get('index');
    if (idx === 'downloads') list = [...list].sort((a, b) => b.downloads - a.downloads);
    const [hits, total] = page(list, q);
    return json(res, 200, { total_hits: total, hits: hits.map((x) => ({ project_id: x.id, slug: x.id, title: x.name, description: `Description de ${x.name}.`, downloads: x.downloads, icon_url: null, author: 'auteur' + (x.n % 3), project_type: x.kind })) });
  }
  if ((m = p.match(/^\/v2\/project\/([^/]+)\/version$/))) {
    const x = projects.find((y) => y.id === m[1]);
    return x ? json(res, 200, x === UPD ? [mrVersion(x, 'v2'), mrVersion(x)] : [mrVersion(x)]) : json(res, 404, {});
  }
  if ((m = p.match(/^\/v2\/version\/(.+)$/))) {
    const x = projects.find((y) => `${y.id}-v1` === m[1]);
    if (x) return json(res, 200, mrVersion(x));
    const y = projects.find((z) => z === UPD && `${z.id}-v2` === m[1]);
    return y ? json(res, 200, mrVersion(y, 'v2')) : json(res, 404, {});
  }
  if ((m = p.match(/^\/v2\/version_file\/([0-9a-f]{40})$/)) && q.get('algorithm') === 'sha1') {
    const x = projects.find((y) => sha1(y.buf) === m[1]);
    return x ? json(res, 200, mrVersion(x)) : json(res, 404, {});
  }
  // ---- CurseForge ----
  if (p === '/v1/mods/search') {
    if (req.headers['x-api-key'] !== 'test-key') return json(res, 403, {});
    const kind = CLASS[q.get('classId')] || 'mod';
    const text = (q.get('searchFilter') || '').toLowerCase();
    const [hits, total] = page(projects.filter((x) => x.kind === kind && x.name.toLowerCase().includes(text)), q);
    return json(res, 200, { pagination: { totalCount: total }, data: hits.map((x) => ({ id: cfId(x), name: x.name, summary: `Résumé CurseForge de ${x.name}.`, slug: x.id, links: { websiteUrl: `https://www.curseforge.com/minecraft/x/${x.id}` }, authors: [{ name: 'cfauteur' }], logo: null, downloadCount: x.downloads, allowModDistribution: !x.restricted })) });
  }
  if ((m = p.match(/^\/v1\/mods\/(\d+)$/))) {
    if (req.headers['x-api-key'] !== 'test-key') return json(res, 403, {});
    const x = projects[Number(m[1]) - 1000];
    if (!x) return json(res, 404, {});
    return json(res, 200, { data: { id: cfId(x), name: x.name, summary: `Résumé CurseForge de ${x.name}.`, slug: x.id, links: { websiteUrl: `https://www.curseforge.com/minecraft/x/${x.id}` }, authors: [{ name: 'cfauteur' }], logo: { url: `${BASE}/img/${x.id}-icon.png`, thumbnailUrl: `${BASE}/img/${x.id}-icon.png` }, screenshots: [1, 2, 3, 4].map((k) => ({ url: `${BASE}/img/${x.id}-${k}.png` })), downloadCount: x.downloads, allowModDistribution: !x.restricted } });
  }
  if ((m = p.match(/^\/v1\/mods\/(\d+)\/files(?:\/(\d+))?$/)) && Number(m[1]) === CF_PACK_ID) {
    if (req.headers['x-api-key'] !== 'test-key') return json(res, 403, {});
    const f = m[2] && CFP.find((x) => x.id === Number(m[2]));
    if (m[2]) return f ? json(res, 200, { data: cfPackFile(f) }) : json(res, 404, {});
    return json(res, 200, { data: [...CFP].reverse().map(cfPackFile) });
  }
  if ((m = p.match(/^\/v1\/mods\/(\d+)\/files(?:\/(\d+))?$/))) {
    const x = projects[Number(m[1]) - 1000];
    if (!x) return json(res, 404, {});
    return json(res, 200, { data: m[2] ? cfFile(x) : [cfFile(x)] });
  }
  if (p === '/v1/mods/files' && req.method === 'POST') {
    let body = ''; req.on('data', (d) => (body += d)); req.on('end', () => {
      const ids = JSON.parse(body).fileIds || [];
      const mods = Object.values(CFM).filter((x) => ids.includes(x.fileId)).map(cfModFile);
      json(res, 200, { data: [...mods, ...ids.map((id) => projects[Math.floor(id / 10) - 1000]).filter(Boolean).map(cfFile)] });
    });
    return;
  }
  json(res, 404, { error: 'not found', path: p });
});

server.listen(port, '127.0.0.1', () => {
  const pt = server.address().port;
  BASE = `http://127.0.0.1:${pt}`;
  console.log(JSON.stringify({ ready: true, port: pt, base: BASE }));
});
const stop = () => server.close(() => process.exit(0));
process.on('SIGINT', stop);
process.on('SIGTERM', stop);
