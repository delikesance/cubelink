// Tests de canaryVersions (node --test scripts/canary-version.test.mjs) : lancés par le job `version` de canary.yml.
import test from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { canaryVersions } from './canary-version.mjs';

test('première tentative : patch suivant, suffixe canary, ProductVersion à 4 composants', () => {
  assert.deepEqual(canaryVersions('v0.1.17', 12, 1), { version: '0.1.18-canary.12', base: '0.1.18', msi: '0.1.18.12' });
});

test('relance : l\'essai s\'ajoute à la version mais pas au ProductVersion', () => {
  assert.deepEqual(canaryVersions('0.1.17', '12', '2'), { version: '0.1.18-canary.12.2', base: '0.1.18', msi: '0.1.18.12' });
});

test('le patch passe bien en numérique (0.1.9 -> 0.1.10)', () => {
  assert.equal(canaryVersions('v0.1.9', 3, 1).base, '0.1.10');
});

test('n° de run hors 1..65535 refusé (limite du 4e composant MSI)', () => {
  assert.throws(() => canaryVersions('v0.1.17', 0, 1), /run invalide/);
  assert.throws(() => canaryVersions('v0.1.17', 65536, 1), /run invalide/);
  assert.equal(canaryVersions('v0.1.17', 65535, 1).msi, '0.1.18.65535');
});

test('dernière release absente ou invalide refusée', () => {
  assert.throws(() => canaryVersions('', 1, 1), /release invalide/);
  assert.throws(() => canaryVersions('v0.1.18-canary.3', 1, 1), /release invalide/);
});

test('CLI : lignes clé=valeur pour $GITHUB_OUTPUT, code 1 sur erreur', () => {
  const script = fileURLToPath(new URL('./canary-version.mjs', import.meta.url));
  assert.equal(execFileSync('node', [script, 'v0.1.17', '12', '1'], { encoding: 'utf8' }),
    'version=0.1.18-canary.12\nbase=0.1.18\nmsi=0.1.18.12\n');
  assert.throws(() => execFileSync('node', [script, 'v0.1.17', '0', '1'], { stdio: 'pipe' }));
});
