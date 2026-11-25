// Versions d'une pré-version canary, calculées UNE fois (job `version` de canary.yml) puis partagées par Linux et Windows.
//   node scripts/canary-version.mjs <dernière release, ex. v0.1.17> <n° de run ci> <n° d'essai>
// Affiche trois lignes `clé=valeur` (prêtes pour $GITHUB_OUTPUT) :
//   version  0.1.18-canary.12      version de l'app, nom du tag et des fichiers (une relance ajoute l'essai : 0.1.18-canary.12.2)
//   base     0.1.18                x.y.z nu : .rpm, .deb (qui n'acceptent pas le suffixe ou exigent du semver pur)
//   msi      0.1.18.12             ProductVersion du .msi : WiX n'accepte pas `-canary.12`, 4 composants numériques,
//                                  le dernier est le n° de run (<= 65535, même limite que release-version.mjs)
import { fileURLToPath } from 'node:url';

export function canaryVersions(last, run, attempt) {
  const m = /^v?(\d+)\.(\d+)\.(\d+)$/.exec(String(last ?? '').trim());
  if (!m) throw new Error('dernière release invalide (attendu x.y.z) : ' + last);
  const n = Number(run);
  if (!Number.isInteger(n) || n < 1 || n > 65535) throw new Error('numéro de run invalide (1..65535) : ' + run);
  const a = Number(attempt);
  if (!Number.isInteger(a) || a < 1) throw new Error("numéro d'essai invalide : " + attempt);
  const base = `${m[1]}.${m[2]}.${Number(m[3]) + 1}`;
  // Une relance du même run ajoute son numéro d'essai (sinon le tag existerait déjà).
  const version = `${base}-canary.${n}` + (a === 1 ? '' : `.${a}`);
  return { version, base, msi: `${base}.${n}` };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  try {
    const [last, run, attempt = '1'] = process.argv.slice(2);
    for (const [k, v] of Object.entries(canaryVersions(last, run, attempt))) console.log(`${k}=${v}`);
  } catch (e) {
    console.error(e.message);
    process.exit(1);
  }
}
