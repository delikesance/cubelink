#!/usr/bin/env node
// Faux miroir Mojang / Fabric meta / Adoptium pour les E2E. Node >= 18, aucune dépendance, 127.0.0.1 uniquement.
//   node tests-fixtures/mojang/server.mjs --port 0
// Imprime sur stdout : {"ready":true,"port":N,"base":"http://127.0.0.1:N"}
import http from 'node:http';
import crypto from 'node:crypto';
import { pathToFileURL } from 'node:url';
import { buildZip } from './zip.mjs';

const sha1 = (b) => crypto.createHash('sha1').update(b).digest('hex');
const sha256 = (b) => crypto.createHash('sha256').update(b).digest('hex');
const json = (o) => Buffer.from(JSON.stringify(o, null, 2));

export const VERSION_ID = '1.21.1-test';
export const OTHER_VERSION_ID = '1.20.4-test';
export const FABRIC_LOADERS = [
  { version: '0.16.5', stable: true },
  { version: '0.16.6-beta.1', stable: false },
];
export const JAVA_MAJORS = [8, 17, 21, 25];

const mavenPath = (name) => {
  const [g, a, v, c] = name.split(':');
  return `${g.replaceAll('.', '/')}/${a}/${v}/${a}-${v}${c ? '-' + c : ''}.jar`;
};
const jar = (label) =>
  buildZip([
    { name: 'META-INF/MANIFEST.MF', data: `Manifest-Version: 1.0\r\nImplementation-Title: ${label}\r\n\r\n` },
    { name: 'fake/Placeholder.class', data: Buffer.from(`cubelink-fake-class:${label}`) },
  ]);

/** Construit tout le contenu (hash et tailles calculés ici) pour une URL de base donnée. */
export function buildFixtures(base) {
  /** @type {Map<string,{body:Buffer,type:string}>} */
  const files = new Map();
  const put = (path, body, type = 'application/octet-stream') => {
    files.set(path, { body, type });
    return { path, sha1: sha1(body), sha256: sha256(body), size: body.length };
  };

  // Assets
  const assetContents = {
    'icons/icon_16x16.png': Buffer.from('fake-png-16x16'),
    'minecraft/lang/en_us.json': Buffer.from('{"menu.singleplayer":"Singleplayer"}'),
    'minecraft/sounds/test.ogg': Buffer.from('OggS-fake-sound-data'),
  };
  const objects = {};
  for (const [name, body] of Object.entries(assetContents)) {
    const h = sha1(body);
    objects[name] = { hash: h, size: body.length };
    put(`/resources/${h.slice(0, 2)}/${h}`, body);
  }
  const assetIndex = put('/assets/indexes/17.json', json({ objects }), 'application/json');

  // Libraries
  const libA = put('/libraries/com/cubelink/test/lib-a/1.0.0/lib-a-1.0.0.jar', jar('lib-a'));
  const nativeSpec = {
    'natives-windows': ['fake-natives.dll'],
    'natives-linux': ['libfake-natives.so'],
    'natives-macos': ['libfake-natives.dylib'],
  };
  const nativeDl = {};
  for (const [cls, names] of Object.entries(nativeSpec)) {
    const p = `com/cubelink/test/lib-natives/1.0.0/lib-natives-1.0.0-${cls}.jar`;
    const z = buildZip([
      ...names.map((n) => ({ name: n, data: Buffer.from(`fake-native-binary:${n}`) })),
      { name: 'META-INF/MANIFEST.MF', data: 'Manifest-Version: 1.0\r\n\r\n' },
    ]);
    const f = put(`/libraries/${p}`, z);
    nativeDl[cls] = { path: p, sha1: f.sha1, size: f.size, url: base + f.path };
  }

  const libraries = [
    {
      name: 'com.cubelink.test:lib-a:1.0.0',
      downloads: {
        artifact: {
          path: 'com/cubelink/test/lib-a/1.0.0/lib-a-1.0.0.jar',
          sha1: libA.sha1, size: libA.size, url: base + libA.path,
        },
      },
    },
    {
      name: 'com.cubelink.test:lib-natives:1.0.0',
      natives: { windows: 'natives-windows', linux: 'natives-linux', osx: 'natives-macos' },
      extract: { exclude: ['META-INF/'] },
      downloads: { classifiers: nativeDl },
    },
  ];

  // Versions
  const versionMeta = {};
  const entries = [];
  for (const [id, type, time] of [
    [VERSION_ID, 'release', '2024-08-08T12:00:00+00:00'],
    [OTHER_VERSION_ID, 'release', '2024-02-01T12:00:00+00:00'],
  ]) {
    const client = put(`/downloads/${id}/client.jar`, buildZip([
      { name: 'META-INF/MANIFEST.MF', data: 'Manifest-Version: 1.0\r\nMain-Class: net.minecraft.client.main.Main\r\n\r\n' },
      { name: 'net/minecraft/client/main/Main.class', data: Buffer.from(`cubelink-fake-client:${id}`) },
      { name: 'assets/.mcassetsroot', data: '' },
    ]));
    const vj = {
      id, type, time, releaseTime: time,
      mainClass: 'net.minecraft.client.main.Main',
      minimumLauncherVersion: 21,
      javaVersion: { component: 'java-runtime-delta', majorVersion: 21 },
      assetIndex: {
        id: '17', url: base + assetIndex.path, sha1: assetIndex.sha1, size: assetIndex.size,
        totalSize: Object.values(objects).reduce((s, o) => s + o.size, 0),
      },
      assets: '17',
      downloads: { client: { url: base + client.path, sha1: client.sha1, size: client.size } },
      libraries,
      arguments: {
        game: [
          '--username', '${auth_player_name}', '--version', '${version_name}',
          '--gameDir', '${game_directory}', '--assetsDir', '${assets_root}',
          '--assetIndex', '${assets_index_name}', '--uuid', '${auth_uuid}',
          '--accessToken', '${auth_access_token}', '--xuid', '${auth_xuid}',
          '--userType', '${user_type}', '--versionType', '${version_type}',
          { rules: [{ action: 'allow', features: { is_demo_user: true } }], value: '--demo' },
          {
            rules: [{ action: 'allow', features: { has_custom_resolution: true } }],
            value: ['--width', '${resolution_width}', '--height', '${resolution_height}'],
          },
        ],
        jvm: [
          { rules: [{ action: 'allow', os: { name: 'windows' } }], value: '-XX:HeapDumpPath=fake.heapdump' },
          { rules: [{ action: 'allow', os: { name: 'osx' } }], value: ['-XstartOnFirstThread'] },
          { rules: [{ action: 'allow', os: { arch: 'x86' } }], value: '-Xss1M' },
          '-Djava.library.path=${natives_directory}',
          '-Dminecraft.launcher.brand=${launcher_name}',
          '-Dminecraft.launcher.version=${launcher_version}',
          '-cp', '${classpath}',
        ],
      },
    };
    const vf = put(`/v/${id}.json`, json(vj), 'application/json');
    versionMeta[id] = { type, time, vf };
    entries.push({ id, type, url: base + vf.path, time, releaseTime: time, sha1: vf.sha1, complianceLevel: 1 });
  }
  put('/mc/game/version_manifest_v2.json', json({
    latest: { release: VERSION_ID, snapshot: VERSION_ID },
    versions: entries,
  }), 'application/json');

  // Fabric meta + Maven
  const ensureLib = (name) => {
    const f = put(`/maven/${mavenPath(name)}`, jar(name));
    return { name, url: `${base}/maven/`, sha1: f.sha1, size: f.size };
  };
  for (const mc of [VERSION_ID, OTHER_VERSION_ID]) {
    put(`/v2/versions/loader/${mc}`, json(FABRIC_LOADERS.map((l) => ({
      loader: { separator: '.', build: 1, maven: `net.fabricmc:fabric-loader:${l.version}`, version: l.version, stable: l.stable },
      intermediary: { maven: `net.fabricmc:intermediary:${mc}`, version: mc, stable: true },
    }))), 'application/json');
    for (const l of FABRIC_LOADERS) {
      put(`/v2/versions/loader/${mc}/${l.version}/profile/json`, json({
        id: `fabric-loader-${l.version}-${mc}`,
        inheritsFrom: mc,
        releaseTime: '2024-08-08T12:00:00+0000',
        time: '2024-08-08T12:00:00+0000',
        type: 'release',
        mainClass: 'net.fabricmc.loader.impl.launch.knot.KnotClient',
        arguments: { game: [], jvm: ['-DFabricMcEmu= net.minecraft.client.main.Main '] },
        libraries: [
          ensureLib(`net.fabricmc:fabric-loader:${l.version}`),
          ensureLib(`net.fabricmc:intermediary:${mc}`),
          ensureLib('org.ow2.asm:asm:9.7'),
        ],
      }), 'application/json');
    }
  }

  // Adoptium + faux JRE
  for (const major of JAVA_MAJORS) {
    const zipName = `OpenJDK${major}U-jre_fake_hotspot_${major}.0.0.zip`;
    const root = `jdk-${major}.0.0+0-jre`;
    const script = `#!/bin/sh\necho 'openjdk version "${major}.0.0" 2024-01-01' >&2\necho 'OpenJDK Runtime Environment (fake)' >&2\n`;
    const z = buildZip([
      { name: `${root}/`, mode: 0o755 },
      { name: `${root}/bin/`, mode: 0o755 },
      { name: `${root}/bin/java.exe`, data: `FAKE-JAVA-EXE ${major}\r\n` },
      { name: `${root}/bin/java`, data: script, mode: 0o755 },
      { name: `${root}/release`, data: `JAVA_VERSION="${major}.0.0"\n` },
    ]);
    const f = put(`/jre/${zipName}`, z, 'application/zip');
    put(`/v3/assets/latest/${major}/hotspot`, json([{
      binary: { package: { link: base + f.path, size: f.size, checksum: f.sha256, name: zipName } },
      version: { major, semver: `${major}.0.0+0` },
    }]), 'application/json');
  }

  return { files, objects, versionMeta };
}

/** Démarre le miroir. Renvoie { server, port, base, fixtures, state(), close() }. */
export async function startMirror({ port = 0 } = {}) {
  const requests = [];
  let faults = [];
  let fixtures = null;
  let base = '';

  const server = http.createServer((req, res) => {
    const url = new URL(req.url, 'http://127.0.0.1');
    const path = url.pathname;
    const finish = (status, body, type = 'application/json', fault) => {
      requests.push({ method: req.method, path, status, ...(fault ? { fault } : {}) });
      res.writeHead(status, { 'content-type': type, 'content-length': body.length, 'cache-control': 'no-store' });
      res.end(req.method === 'HEAD' ? undefined : body);
    };

    if (path === '/__state' && req.method === 'GET') {
      const counts = {};
      for (const r of requests) counts[r.path] = (counts[r.path] ?? 0) + 1;
      const b = json({ base, requests, counts, faults: faults.map((f) => ({ ...f })) });
      res.writeHead(200, { 'content-type': 'application/json', 'content-length': b.length });
      return res.end(b);
    }
    if (path === '/__control' && (req.method === 'POST' || req.method === 'PUT')) {
      const chunks = [];
      req.on('data', (c) => chunks.push(c));
      req.on('end', () => {
        let cfg;
        try { cfg = JSON.parse(Buffer.concat(chunks).toString() || '{}'); } catch { return finish(400, json({ error: 'JSON invalide' })); }
        if (cfg.reset) { faults = []; requests.length = 0; }
        for (const f of cfg.faults ?? []) {
          if (!f.match || !['bad_hash', '404', '500', 'slow'].includes(f.mode)) {
            return finish(400, json({ error: 'faute invalide : {match, mode: bad_hash|404|500|slow, times?, delayMs?}' }));
          }
          faults.push({ match: String(f.match), mode: f.mode, remaining: f.times ?? null, delayMs: f.delayMs ?? 1500 });
        }
        const b = json({ ok: true, faults });
        res.writeHead(200, { 'content-type': 'application/json', 'content-length': b.length });
        res.end(b);
      });
      return;
    }
    if (req.method !== 'GET' && req.method !== 'HEAD') return finish(405, json({ error: 'méthode refusée' }));

    const file = fixtures.files.get(path);
    const fault = faults.find((f) => path.includes(f.match) && (f.remaining === null || f.remaining > 0));
    const serve = (mode) => {
      if (!file) return finish(404, json({ error: 'introuvable', path }), 'application/json', mode);
      let body = file.body;
      if (mode === 'bad_hash') {
        if (path.endsWith('.json')) body = Buffer.concat([body, Buffer.from(' ')]);
        else { body = Buffer.from(body); body[0] ^= 0xff; }
      }
      finish(200, body, file.type, mode);
    };
    if (!fault) return serve();
    if (fault.remaining !== null) fault.remaining -= 1;
    if (fault.mode === '404') return finish(404, json({ error: 'faute injectée' }), 'application/json', '404');
    if (fault.mode === '500') return finish(500, json({ error: 'faute injectée' }), 'application/json', '500');
    if (fault.mode === 'slow') return void setTimeout(() => serve('slow'), fault.delayMs);
    return serve('bad_hash');
  });

  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(port, '127.0.0.1', resolve);
  });
  const actualPort = server.address().port;
  base = `http://127.0.0.1:${actualPort}`;
  fixtures = buildFixtures(base);

  return {
    server, port: actualPort, base, fixtures,
    state: () => ({ requests, faults }),
    close: () => new Promise((r) => { server.closeAllConnections?.(); server.close(() => r()); }),
  };
}

const isMain = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href;
if (isMain) {
  const i = process.argv.indexOf('--port');
  const port = i >= 0 ? Number(process.argv[i + 1]) : 0;
  const m = await startMirror({ port });
  console.log(JSON.stringify({ ready: true, port: m.port, base: m.base }));
  const stop = () => m.close().then(() => process.exit(0));
  process.on('SIGTERM', stop);
  process.on('SIGINT', stop);
}
