// Calcule la version d'une release : <majeur>.<mineur> lus dans src-tauri/tauri.conf.json + numéro de run CI comme patch.
// Majeur/mineur se changent à la main dans tauri.conf.json ; le patch est automatique (donc un nouveau tag à chaque merge dans main).
//   node scripts/release-version.mjs <numéro>            -> affiche la version (ex. 0.1.42)
//   node scripts/release-version.mjs <numéro> --write    -> l'écrit aussi dans src-tauri/tauri.conf.json (build CI uniquement, non commité)
import fs from 'node:fs';

const file = new URL('../src-tauri/tauri.conf.json', import.meta.url);
const raw = fs.readFileSync(file, 'utf8');
const conf = JSON.parse(raw);
const run = Number(process.argv[2]);
if (!Number.isInteger(run) || run < 1 || run > 65535) {
  console.error('numéro de run invalide (1..65535) : ' + process.argv[2]);
  process.exit(1);
}
const m = /^(\d+)\.(\d+)\.\d+/.exec(conf.version);
if (!m) {
  console.error('version invalide dans tauri.conf.json : ' + conf.version);
  process.exit(1);
}
const version = `${m[1]}.${m[2]}.${run}`;
if (process.argv.includes('--write')) {
  fs.writeFileSync(file, raw.replace(/("version"\s*:\s*")[^"]+(")/, `$1${version}$2`));
}
console.log(version);
