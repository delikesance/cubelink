// Faux Modrinth de test : bibliothèque (zip déterministe, catalogue, serveur). Aucune dépendance.
import http from 'node:http';
import crypto from 'node:crypto';
import zlib from 'node:zlib';

// ---------- ZIP déterministe ----------
const CRC_TABLE = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();
export function crc32(buf) {
  let c = 0xffffffff;
  for (const b of buf) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

/** entries : [{name, data: Buffer|string}] -> Buffer zip (deflate, horodatage fixe 2020-01-01). */
export function buildZip(entries) {
  const DOS_TIME = 0;
  const DOS_DATE = ((2020 - 1980) << 9) | (1 << 5) | 1;
  const locals = [];
  const centrals = [];
  let offset = 0;
  for (const e of entries) {
    const name = Buffer.from(e.name, 'utf8');
    const raw = Buffer.isBuffer(e.data) ? e.data : Buffer.from(e.data, 'utf8');
    const comp = zlib.deflateRawSync(raw, { level: 9 });
    const crc = crc32(raw);
    const lh = Buffer.alloc(30);
    lh.writeUInt32LE(0x04034b50, 0);
    lh.writeUInt16LE(20, 4);
    lh.writeUInt16LE(0x0800, 6); // UTF-8
    lh.writeUInt16LE(8, 8);
    lh.writeUInt16LE(DOS_TIME, 10);
    lh.writeUInt16LE(DOS_DATE, 12);
    lh.writeUInt32LE(crc, 14);
    lh.writeUInt32LE(comp.length, 18);
    lh.writeUInt32LE(raw.length, 22);
    lh.writeUInt16LE(name.length, 26);
    lh.writeUInt16LE(0, 28);
    locals.push(lh, name, comp);
    const ch = Buffer.alloc(46);
    ch.writeUInt32LE(0x02014b50, 0);
    ch.writeUInt16LE(20, 4);
    ch.writeUInt16LE(20, 6);
    ch.writeUInt16LE(0x0800, 8);
    ch.writeUInt16LE(8, 10);
    ch.writeUInt16LE(DOS_TIME, 12);
    ch.writeUInt16LE(DOS_DATE, 14);
    ch.writeUInt32LE(crc, 16);
    ch.writeUInt32LE(comp.length, 20);
    ch.writeUInt32LE(raw.length, 24);
    ch.writeUInt16LE(name.length, 28);
    ch.writeUInt32LE(offset, 42);
    centrals.push(ch, name);
    offset += lh.length + name.length + comp.length;
  }
  const cd = Buffer.concat(centrals);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(entries.length, 8);
  end.writeUInt16LE(entries.length, 10);
  end.writeUInt32LE(cd.length, 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...locals, cd, end]);
}

/** Lit les entrées d'un zip produit par buildZip (pour les tests). */
export function readZip(buf) {
  const eocd = buf.length - 22;
  if (buf.readUInt32LE(eocd) !== 0x06054b50) throw new Error('zip invalide');
  const n = buf.readUInt16LE(eocd + 10);
  let p = buf.readUInt32LE(eocd + 16);
  const out = {};
  for (let i = 0; i < n; i++) {
    const method = buf.readUInt16LE(p + 10);
    const crc = buf.readUInt32LE(p + 16);
    const csize = buf.readUInt32LE(p + 20);
    const nlen = buf.readUInt16LE(p + 28);
    const off = buf.readUInt32LE(p + 42);
    const name = buf.toString('utf8', p + 46, p + 46 + nlen);
    const lnlen = buf.readUInt16LE(off + 26);
    const lxlen = buf.readUInt16LE(off + 28);
    const start = off + 30 + lnlen + lxlen;
    const comp = buf.subarray(start, start + csize);
    const data = method === 8 ? zlib.inflateRawSync(comp) : comp;
    if (crc32(data) !== crc) throw new Error('crc invalide : ' + name);
    out[name] = data;
    p += 46 + nlen;
  }
  return out;
}

export const sha1 = (b) => crypto.createHash('sha1').update(b).digest('hex');
export const sha256 = (b) => crypto.createHash('sha256').update(b).digest('hex');
export const sha512 = (b) => crypto.createHash('sha512').update(b).digest('hex');

// ---------- Jars factices ----------
function fabricJar({ id, name, version, description }) {
  return buildZip([
    { name: 'META-INF/MANIFEST.MF', data: 'Manifest-Version: 1.0\r\n\r\n' },
    {
      name: 'fabric.mod.json',
      data: JSON.stringify({ schemaVersion: 1, id, version, name, description, environment: '*', depends: { fabricloader: '*', minecraft: '1.21.1' } }, null, 2),
    },
    { name: `${id}/Fake.class`, data: Buffer.from(`FAKE-CLASS:${id}:${version}`) },
  ]);
}
function forgeJar({ id, name, version, description }) {
  const toml = [
    'modLoader="javafml"',
    'loaderVersion="[1,)"',
    'license="MIT"',
    '',
    '[[mods]]',
    `modId="${id}"`,
    `version="${version}"`,
    `displayName="${name}"`,
    `description='''${description}'''`,
    '',
  ].join('\n');
  return buildZip([
    { name: 'META-INF/MANIFEST.MF', data: 'Manifest-Version: 1.0\r\n\r\n' },
    { name: 'META-INF/neoforge.mods.toml', data: toml },
    { name: `${id}/Fake.class`, data: Buffer.from(`FAKE-CLASS:${id}:${version}`) },
  ]);
}

const MC = '1.21.1';
const DEFS = [
  { n: 1, slug: 'cubelink-alpha', title: 'Alpha Lib', loader: 'fabric', desc: 'Bibliothèque factice alpha.' },
  { n: 2, slug: 'cubelink-beta', title: 'Beta Tweaks', loader: 'fabric', desc: 'Ajustements factices beta.' },
  { n: 3, slug: 'cubelink-gamma', title: 'Gamma Render', loader: 'fabric', desc: 'Rendu factice gamma.' },
  { n: 4, slug: 'cubelink-delta', title: 'Delta Storage', loader: 'neoforge', desc: 'Stockage factice delta.' },
  { n: 5, slug: 'cubelink-epsilon', title: 'Epsilon Extras', loader: 'fabric', desc: 'Extras factices epsilon.' },
];

function pad(n) {
  return String(n).padStart(8, '0');
}

/** Catalogue : 5 mods (jar + sha1/sha512 calculés dynamiquement). Contenu 100 % déterministe. */
export function buildCatalog() {
  return DEFS.map((d) => {
    const id = d.slug.replace(/-/g, '_');
    const version = `1.${d.n}.0`;
    const info = { id, name: d.title, version, description: d.desc };
    const jar = d.loader === 'fabric' ? fabricJar(info) : forgeJar(info);
    const filename = `${d.slug}-${version}.jar`;
    return {
      projectId: `PRJ${pad(d.n)}`,
      versionId: `VER${pad(d.n)}`,
      slug: d.slug,
      title: d.title,
      description: d.desc,
      author: 'cubelink-fixtures',
      loader: d.loader,
      gameVersion: MC,
      version,
      filename,
      jar,
      sha1: sha1(jar),
      sha512: sha512(jar),
      sha256: sha256(jar),
      downloads: 1000 * (6 - d.n),
      // le mod 5 dépend (requis) du mod 1 : utile pour les tests de dépendances
      deps: d.n === 5 ? ['PRJ' + pad(1)] : [],
    };
  });
}

/** Mod maison : jar valide ABSENT du catalogue (inconnu de Modrinth). Déterministe. */
export function buildHomeMod() {
  const jar = fabricJar({ id: 'homebrew_mod', name: 'Homebrew Mod', version: '0.0.1', description: 'Mod maison factice, absent de Modrinth.' });
  return { filename: 'homebrew-mod-0.0.1.jar', jar, sha1: sha1(jar), sha256: sha256(jar), sha512: sha512(jar) };
}

/** Modpack .mrpack de test (mods 4 et 5 via /cdn + overrides). `base` : URL du serveur. */
export function buildMrpack(catalog, base) {
  const picks = [catalog[3], catalog[4]];
  const index = {
    formatVersion: 1,
    game: 'minecraft',
    versionId: '1.0.0',
    name: 'Cubelink Test Pack',
    summary: 'Pack factice pour les E2E',
    files: picks.map((m) => ({
      path: `mods/${m.filename}`,
      hashes: { sha1: m.sha1, sha512: m.sha512 },
      env: { client: 'required', server: 'required' },
      downloads: [`${base}/cdn/${m.filename}`],
      fileSize: m.jar.length,
    })),
    dependencies: { minecraft: MC, 'fabric-loader': '0.16.5' },
  };
  return buildZip([
    { name: 'modrinth.index.json', data: JSON.stringify(index, null, 2) },
    { name: 'overrides/config/testpack.toml', data: 'greeting = "bonjour"\n' },
    { name: 'overrides/options.txt', data: 'lang:fr_fr\n' },
  ]);
}

// ---------- Serveur ----------
const TAMPER = (buf) => {
  const b = Buffer.from(buf);
  b[b.length - 1] ^= 0xff;
  return b;
};

export const MRPACK_NAME = 'cubelink-test-pack-1.0.0.mrpack';

/** Crée le serveur (non démarré). `server.state` : état de test. */
export function createServer() {
  const catalog = buildCatalog();
  const home = buildHomeMod();
  let base = '';
  let mrpack = null;
  const log = [];
  const newFaults = () => ({ rateLimit: null, error: null, slowMs: 0, tamper: false });
  let faults = newFaults();

  const versionJson = (m) => ({
    id: m.versionId,
    project_id: m.projectId,
    name: `${m.title} ${m.version}`,
    version_number: m.version,
    game_versions: [m.gameVersion],
    version_type: 'release',
    loaders: [m.loader],
    featured: true,
    status: 'listed',
    date_published: '2024-01-01T00:00:00Z',
    downloads: m.downloads,
    files: [
      {
        hashes: { sha1: m.sha1, sha512: m.sha512 },
        url: `${base}/cdn/${m.filename}`,
        filename: m.filename,
        primary: true,
        size: m.jar.length,
      },
    ],
    dependencies: m.deps.map((p) => ({ version_id: null, project_id: p, file_name: null, dependency_type: 'required' })),
  });

  const findProject = (key) => catalog.find((m) => m.projectId === key || m.slug === key);
  const findByHash = (h, algo) => catalog.find((m) => (algo === 'sha512' ? m.sha512 : m.sha1) === String(h).toLowerCase());

  const send = (res, status, body, headers = {}) => {
    const isBuf = Buffer.isBuffer(body);
    const data = isBuf ? body : Buffer.from(typeof body === 'string' ? body : JSON.stringify(body));
    res.writeHead(status, {
      'Content-Type': isBuf ? 'application/octet-stream' : 'application/json; charset=utf-8',
      'Content-Length': data.length,
      ...headers,
    });
    res.end(data);
  };

  // consomme une faute à compteur ; renvoie la config si elle s'applique
  const take = (key) => {
    const f = faults[key];
    if (!f) return null;
    if (f.count != null) {
      if (f.count <= 0) return null;
      f.count -= 1;
    }
    return f;
  };

  const readBody = (req) =>
    new Promise((resolve, reject) => {
      const chunks = [];
      req.on('data', (c) => chunks.push(c));
      req.on('end', () => resolve(Buffer.concat(chunks)));
      req.on('error', reject);
    });

  const handler = async (req, res) => {
    const url = new URL(req.url, 'http://127.0.0.1');
    const p = url.pathname;
    const entry = { method: req.method, path: p, query: url.search, ua: req.headers['user-agent'] || '' };

    if (p === '/__state' && req.method === 'GET') {
      return send(res, 200, { requests: log, faults, catalog: catalog.map((m) => ({ projectId: m.projectId, slug: m.slug, filename: m.filename, sha1: m.sha1, sha256: m.sha256 })), home: { filename: home.filename, sha1: home.sha1, sha256: home.sha256 }, mrpack: MRPACK_NAME });
    }
    if (p === '/__control' && req.method === 'POST') {
      let cfg;
      try {
        cfg = JSON.parse((await readBody(req)).toString() || '{}');
      } catch {
        return send(res, 400, { error: 'json invalide' });
      }
      if (cfg.reset) {
        faults = newFaults();
        log.length = 0;
      }
      // rateLimit: {count?, retryAfter?} | null ; error: {count?, status?} | null ; slowMs: n ; tamper: bool
      for (const k of ['rateLimit', 'error']) {
        if (k in cfg) faults[k] = cfg[k] ? { count: null, ...cfg[k] } : null;
      }
      if ('slowMs' in cfg) faults.slowMs = Number(cfg.slowMs) || 0;
      if ('tamper' in cfg) faults.tamper = !!cfg.tamper;
      return send(res, 200, { ok: true, faults });
    }

    log.push(entry);
    const isCdn = p.startsWith('/cdn/');
    if (!isCdn && !p.startsWith('/v2/')) return send(res, 404, { error: 'not_found' });

    if (faults.slowMs) await new Promise((r) => setTimeout(r, faults.slowMs));
    const rl = take('rateLimit');
    if (rl) {
      entry.fault = 'rate_limit';
      return send(res, 429, { error: 'ratelimit' }, { 'Retry-After': String(rl.retryAfter ?? 1), 'X-Ratelimit-Remaining': '0' });
    }
    const er = take('error');
    if (er) {
      entry.fault = 'error';
      return send(res, er.status ?? 500, { error: 'fault_injected' });
    }

    if (isCdn) {
      const name = decodeURIComponent(p.slice(5));
      let body = null;
      const m = catalog.find((c) => c.filename === name);
      if (m) body = m.jar;
      else if (name === MRPACK_NAME) body = mrpack;
      else if (name === home.filename) body = null; // le mod maison n'est pas distribué
      if (!body) return send(res, 404, { error: 'not_found' });
      if (faults.tamper && !name.endsWith('.mrpack')) {
        entry.fault = 'tamper';
        body = TAMPER(body);
      }
      return send(res, 200, body);
    }

    const route = p.slice(3); // retire /v2
    if (req.method === 'GET' && route === '/search') {
      const q = (url.searchParams.get('query') || '').toLowerCase();
      let groups = [];
      try {
        groups = JSON.parse(url.searchParams.get('facets') || '[]');
      } catch {
        return send(res, 400, { error: 'invalid_input', description: 'facets invalides' });
      }
      const limit = Math.min(Number(url.searchParams.get('limit') ?? 10) || 10, 100);
      const offset = Number(url.searchParams.get('offset') ?? 0) || 0;
      const match = (m, f) => {
        const [k, v] = String(f).split(':');
        if (k === 'versions') return m.gameVersion === v;
        if (k === 'categories') return m.loader === v;
        if (k === 'project_type') return v === 'mod';
        return false;
      };
      const all = catalog.filter(
        (m) => (!q || `${m.slug} ${m.title} ${m.description}`.toLowerCase().includes(q)) && groups.every((g) => g.some((f) => match(m, f))),
      );
      const hits = all.slice(offset, offset + limit).map((m) => ({
        project_id: m.projectId,
        project_type: 'mod',
        slug: m.slug,
        author: m.author,
        title: m.title,
        description: m.description,
        categories: [m.loader],
        versions: [m.gameVersion],
        downloads: m.downloads,
        icon_url: null,
        latest_version: m.gameVersion,
      }));
      return send(res, 200, { hits, offset, limit, total_hits: all.length });
    }
    let mt = route.match(/^\/project\/([^/]+)\/version$/);
    if (req.method === 'GET' && mt) {
      const m = findProject(decodeURIComponent(mt[1]));
      if (!m) return send(res, 404, { error: 'not_found' });
      const arr = (k) => {
        const v = url.searchParams.get(k);
        return v ? JSON.parse(v) : null;
      };
      let gv, ld;
      try {
        gv = arr('game_versions');
        ld = arr('loaders');
      } catch {
        return send(res, 400, { error: 'invalid_input' });
      }
      const ok = (!gv || gv.includes(m.gameVersion)) && (!ld || ld.includes(m.loader));
      return send(res, 200, ok ? [versionJson(m)] : []);
    }
    mt = route.match(/^\/version_file\/([^/]+)$/);
    if (req.method === 'GET' && mt) {
      const m = findByHash(decodeURIComponent(mt[1]), url.searchParams.get('algorithm') || 'sha1');
      return m ? send(res, 200, versionJson(m)) : send(res, 404, { error: 'not_found', description: 'Specified version file does not exist' });
    }
    if (req.method === 'POST' && route === '/version_files') {
      let b;
      try {
        b = JSON.parse((await readBody(req)).toString());
      } catch {
        return send(res, 400, { error: 'invalid_input' });
      }
      const out = {};
      for (const h of b.hashes || []) {
        const m = findByHash(h, b.algorithm || 'sha1');
        if (m) out[h] = versionJson(m);
      }
      return send(res, 200, out);
    }
    return send(res, 404, { error: 'not_found' });
  };

  const server = http.createServer((req, res) => {
    handler(req, res).catch((e) => send(res, 500, { error: String(e) }));
  });
  server.catalog = catalog;
  server.home = home;

  /** Démarre sur 127.0.0.1:port (0 = libre) ; renvoie {port, base}. */
  server.start = (port = 0) =>
    new Promise((resolve) => {
      server.listen(port, '127.0.0.1', () => {
        const pt = server.address().port;
        base = `http://127.0.0.1:${pt}`;
        mrpack = buildMrpack(catalog, base);
        resolve({ port: pt, base });
      });
    });
  return server;
}
