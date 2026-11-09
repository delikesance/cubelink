// Construit le front (dist) puis le binaire Tauri qui EMBARQUE ce dist, et affiche/renvoie son chemin.
//
// Decision : un `cargo build` nu (debug ou release) n'active PAS la feature `tauri/custom-protocol` ;
// sans elle Tauri charge `devUrl` (http://localhost:5173) et non `dist`. Seule la CLI `tauri build`
// l'ajoute normalement. On l'active donc explicitement. Profil debug par defaut (compilation bien plus
// rapide, stderr/stdout visibles pour les journaux) ; `--release` possible.
// Usage : node e2e/harness/build.mjs [--release] [--skip-front] [--skip-cargo]
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');
const isWin = process.platform === 'win32';

function run(cmd, args, env = {}) {
  const r = spawnSync(cmd, args, { cwd: ROOT, stdio: ['ignore', 2, 2], shell: isWin && cmd === 'npm', env: { ...process.env, ...env } });
  if (r.status !== 0) throw new Error(`${cmd} ${args.join(' ')} a echoue (code ${r.status})`);
}

export function binaryPath({ release = false, targetDir } = {}) {
  const dir = targetDir ?? process.env.CUBELINK_E2E_TARGET_DIR ?? path.join(ROOT, 'target', 'E0.4');
  return path.join(dir, release ? 'release' : 'debug', isWin ? 'cubelink.exe' : 'cubelink');
}

export function build({ release = false, skipFront = false, skipCargo = false } = {}) {
  const targetDir = process.env.CUBELINK_E2E_TARGET_DIR ?? path.join(ROOT, 'target', 'E0.4');
  if (!skipFront) run('npm', ['run', 'build']);
  if (!skipCargo) {
    const args = ['build', '-p', 'cubelink', '--features', 'tauri/custom-protocol'];
    if (release) args.push('--release');
    run('cargo', args, { CARGO_TARGET_DIR: targetDir });
  }
  const bin = binaryPath({ release, targetDir });
  if (!fs.existsSync(bin)) throw new Error(`binaire introuvable : ${bin}`);
  return bin;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const a = process.argv.slice(2);
  console.log(build({ release: a.includes('--release'), skipFront: a.includes('--skip-front'), skipCargo: a.includes('--skip-cargo') }));
}
