// Tests de resolveBinary (node --test) : reconstruit par defaut, SKIP_BUILD reutilise, BIN fourni tel quel.
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { binaryPath, resolveBinary } from './build.mjs';

function sandbox() {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'cubelink-resolve-'));
  const prev = process.env.CUBELINK_E2E_TARGET_DIR;
  process.env.CUBELINK_E2E_TARGET_DIR = dir;
  const bin = binaryPath({});
  const make = () => { fs.mkdirSync(path.dirname(bin), { recursive: true }); fs.writeFileSync(bin, 'x'); };
  const done = () => { fs.rmSync(dir, { recursive: true, force: true }); if (prev === undefined) delete process.env.CUBELINK_E2E_TARGET_DIR; else process.env.CUBELINK_E2E_TARGET_DIR = prev; };
  return { bin, make, done };
}

function harness(env, bin) {
  const logs = [];
  let builds = 0;
  const buildFn = () => { builds++; fs.mkdirSync(path.dirname(bin), { recursive: true }); fs.writeFileSync(bin, 'y'); return bin; };
  return { opts: { env, buildFn, log: (m) => logs.push(m) }, logs, builds: () => builds };
}

test('par defaut : reconstruit meme si le binaire existe', () => {
  const s = sandbox();
  try {
    s.make();
    const h = harness({}, s.bin);
    assert.equal(resolveBinary(h.opts), s.bin);
    assert.equal(h.builds(), 1);
    assert.match(h.logs[0], /\[e2e\] binaire : .*modifie \d{4}-\d\d-\d\dT.*reconstruit/);
  } finally { s.done(); }
});

test('CUBELINK_E2E_BUILD=1 se comporte comme le defaut', () => {
  const s = sandbox();
  try {
    s.make();
    const h = harness({ CUBELINK_E2E_BUILD: '1' }, s.bin);
    resolveBinary(h.opts);
    assert.equal(h.builds(), 1);
  } finally { s.done(); }
});

test('SKIP_BUILD=1 avec binaire present : reutilise sans construire, journalise chemin et date', () => {
  const s = sandbox();
  try {
    s.make();
    const h = harness({ CUBELINK_E2E_SKIP_BUILD: '1' }, s.bin);
    assert.equal(resolveBinary(h.opts), s.bin);
    assert.equal(h.builds(), 0);
    assert.ok(h.logs[0].includes(path.resolve(s.bin)));
    assert.match(h.logs[0], /reutilise/);
  } finally { s.done(); }
});

test('SKIP_BUILD=1 avec binaire absent : construit quand meme', () => {
  const s = sandbox();
  try {
    const h = harness({ CUBELINK_E2E_SKIP_BUILD: '1' }, s.bin);
    resolveBinary(h.opts);
    assert.equal(h.builds(), 1);
    assert.match(h.logs[0], /binaire absent/);
  } finally { s.done(); }
});

test('CUBELINK_E2E_BIN : utilise tel quel sans construire ; erreur claire si absent', () => {
  const s = sandbox();
  try {
    s.make();
    const h = harness({ CUBELINK_E2E_BIN: s.bin }, s.bin);
    assert.equal(resolveBinary(h.opts), s.bin);
    assert.equal(h.builds(), 0);
    assert.match(h.logs[0], /CUBELINK_E2E_BIN/);
    const h2 = harness({ CUBELINK_E2E_BIN: s.bin + '.absent' }, s.bin);
    assert.throws(() => resolveBinary(h2.opts), /absent/);
    assert.equal(h2.builds(), 0);
  } finally { s.done(); }
});
