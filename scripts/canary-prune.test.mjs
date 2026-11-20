// Tests de selectStale (node --test scripts/canary-prune.test.mjs) : lancés par le job `version` de canary.yml.
import test from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { selectStale, CANARY_TAG } from './canary-prune.mjs';
import { canaryVersions } from './canary-version.mjs';

const script = fileURLToPath(new URL('./canary-prune.mjs', import.meta.url));
const day = (n) => new Date(Date.UTC(2026, 0, n)).toISOString();
const canary = (run, d, extra = {}) => ({ tagName: `v0.1.18-canary.${run}`, isPrerelease: true, isDraft: false, createdAt: day(d), ...extra });

test('moins de N pré-versions : rien à supprimer', () => {
  assert.deepEqual(selectStale([canary(1, 1), canary(2, 2)], 10), []);
  assert.deepEqual(selectStale([], 10), []);
});

test('exactement N : rien à supprimer', () => {
  assert.deepEqual(selectStale([canary(1, 1), canary(2, 2), canary(3, 3)], 3), []);
});

test('N+3 : seules les 3 plus anciennes sortent, de la plus récente à la plus ancienne', () => {
  const all = Array.from({ length: 13 }, (_, i) => canary(i + 1, i + 1));
  assert.deepEqual(selectStale(all, 10), ['v0.1.18-canary.3', 'v0.1.18-canary.2', 'v0.1.18-canary.1']);
});

test('le tri suit la date, pas le n° de run ni l\'ordre d\'entrée', () => {
  const all = [canary(50, 5), canary(7, 9), canary(99, 1), canary(20, 7)];
  assert.deepEqual(selectStale(all, 2), ['v0.1.18-canary.50', 'v0.1.18-canary.99']);
});

test('même date : départage stable par tag', () => {
  const all = [canary(1, 1), canary(2, 1), canary(3, 1)];
  assert.deepEqual(selectStale(all, 1), selectStale([...all].reverse(), 1));
});

test('releases stables, pré-versions hors motif et brouillons ne sont jamais supprimés', () => {
  const all = [
    canary(1, 1),
    { tagName: 'v0.1.17', isPrerelease: false, isDraft: false, createdAt: day(1) },
    { tagName: 'v0.1.17-canary.4', isPrerelease: false, isDraft: false, createdAt: day(1) },
    { tagName: 'v0.2.0-beta.1', isPrerelease: true, isDraft: false, createdAt: day(1) },
    { tagName: 'canary-latest', isPrerelease: true, isDraft: false, createdAt: day(1) },
    { tagName: 'v0.1.18-canary.9-fix', isPrerelease: true, isDraft: false, createdAt: day(1) },
    canary(8, 1, { isDraft: true }),
    canary(2, 2),
  ];
  assert.deepEqual(selectStale(all, 1), ['v0.1.18-canary.1']);
});

test('le suffixe de relance .N est reconnu (v0.1.18-canary.12.2)', () => {
  const all = [{ ...canary(12, 1), tagName: 'v0.1.18-canary.12.2' }, canary(13, 2)];
  assert.deepEqual(selectStale(all, 1), ['v0.1.18-canary.12.2']);
});

test('le motif accepte exactement ce que canary-version.mjs produit', () => {
  for (const attempt of [1, 2, 7]) {
    assert.match('v' + canaryVersions('v0.1.17', 12, attempt).version, CANARY_TAG);
  }
});

test('keep invalide (0, négatif, non entier, texte) : erreur, jamais de purge totale', () => {
  const all = [canary(1, 1)];
  for (const k of [0, -1, 1.5, NaN, '3', null]) assert.throws(() => selectStale(all, k), /invalide/);
});

test('entrée qui n\'est pas un tableau, ou date illisible : erreur', () => {
  assert.throws(() => selectStale({}, 10), /invalide/);
  assert.throws(() => selectStale([canary(1, 1, { createdAt: 'hier' }), canary(2, 2)], 1), /invalide/);
});

test('CLI : stdin JSON, un tag par ligne, comme dans canary.yml', () => {
  const all = Array.from({ length: 15 }, (_, i) => canary(i + 1, i + 1));
  all.push({ tagName: 'v0.1.17', isPrerelease: false, isDraft: false, createdAt: day(30) });
  const r = spawnSync(process.execPath, [script, '10'], { input: JSON.stringify(all), encoding: 'utf8' });
  assert.equal(r.status, 0);
  assert.equal(r.stdout, ['v0.1.18-canary.5', 'v0.1.18-canary.4', 'v0.1.18-canary.3', 'v0.1.18-canary.2', 'v0.1.18-canary.1'].join('\n') + '\n');
});

test('CLI : keep invalide ou JSON cassé => code 1, rien sur stdout', () => {
  for (const [arg, input] of [['0', '[]'], ['abc', '[]'], ['-2', '[]'], ['10', 'pas du json']]) {
    const r = spawnSync(process.execPath, [script, arg], { input, encoding: 'utf8' });
    assert.equal(r.status, 1, arg);
    assert.equal(r.stdout, '');
  }
});
