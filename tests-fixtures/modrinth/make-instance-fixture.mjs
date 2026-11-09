#!/usr/bin/env node
// Crée un dossier d'instance hôte de test dans <dir> et imprime le manifeste attendu (JSON).
// Usage : node make-instance-fixture.mjs <dir>
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { buildCatalog, buildHomeMod, sha256 } from './lib.mjs';

export const CONFIG_FILE = { path: 'config/cubelink-test.toml', data: 'option = true\nname = "fixture"\n' };

/** Écrit l'instance ; renvoie {files (partagés, triés), excluded}. */
export function makeInstance(dir) {
  const catalog = buildCatalog().slice(0, 4);
  const home = buildHomeMod();
  const put = (rel, data) => {
    const abs = path.join(dir, ...rel.split('/'));
    fs.mkdirSync(path.dirname(abs), { recursive: true });
    fs.writeFileSync(abs, data);
    return Buffer.isBuffer(data) ? data : Buffer.from(data);
  };
  const files = [];
  const share = (rel, data) => {
    const b = put(rel, data);
    files.push({ path: rel, size: b.length, sha256: sha256(b) });
  };
  for (const m of catalog) share(`mods/${m.filename}`, m.jar);
  share(`mods/${home.filename}`, home.jar);
  share(CONFIG_FILE.path, CONFIG_FILE.data);
  // jamais partagés
  put('options.txt', 'lang:fr_fr\nfov:70\n');
  put('saves/TestWorld/level.dat', 'FAKE-LEVEL-DAT');
  put('saves/TestWorld/region/r.0.0.mca', 'FAKE-REGION');
  put('servers.dat', 'FAKE');
  files.sort((a, b) => (a.path < b.path ? -1 : 1));
  return { files, excluded: ['options.txt', 'saves/', 'servers.dat'] };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const dir = process.argv[2];
  if (!dir) {
    console.error('usage : node make-instance-fixture.mjs <dir>');
    process.exit(2);
  }
  fs.mkdirSync(dir, { recursive: true });
  console.log(JSON.stringify({ dir: path.resolve(dir), ...makeInstance(dir) }, null, 2));
}
