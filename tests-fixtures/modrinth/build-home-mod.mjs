#!/usr/bin/env node
// Génère le mod maison (jar factice déterministe, absent du catalogue).
// Usage : node build-home-mod.mjs [<dossier-sortie>]  -> écrit le jar et imprime son sha256.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { buildHomeMod } from './lib.mjs';

export function writeHomeMod(dir) {
  const h = buildHomeMod();
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, h.filename), h.jar);
  return h;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const dir = process.argv[2];
  const h = dir ? writeHomeMod(dir) : buildHomeMod();
  console.log(JSON.stringify({ filename: h.filename, sha256: h.sha256, sha1: h.sha1, size: h.jar.length, written: !!dir }));
}
