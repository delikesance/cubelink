// Met a jour packaging/nix/package.nix (version + hash) depuis latest.json et l'AppImage de la release.
//   node scripts/nix-bump.mjs <latest.json> <fichier .AppImage> [package.nix]
// Le hash est le SRI sha256 de l'AppImage (calcule localement, pas de reseau). Aucun git ici : artefact de release seulement.
import crypto from 'node:crypto';
import fs from 'node:fs';

const [latestFile, appimage, nixFile = 'packaging/nix/package.nix'] = process.argv.slice(2);
if (!latestFile || !appimage) {
  console.error('usage : nix-bump.mjs <latest.json> <AppImage> [package.nix]');
  process.exit(1);
}
const latest = JSON.parse(fs.readFileSync(latestFile, 'utf8'));
if (!latest.version || !latest.platforms?.['linux-x86_64']) {
  console.error('latest.json sans version ou sans entree linux-x86_64');
  process.exit(1);
}
const hash = 'sha256-' + crypto.createHash('sha256').update(fs.readFileSync(appimage)).digest('base64');
let nix = fs.readFileSync(nixFile, 'utf8');
nix = nix.replace(/version = "[^"]*"; # nix-bump:version/, `version = "${latest.version}"; # nix-bump:version`);
nix = nix.replace(/hash = "[^"]*"; # nix-bump:hash/, `hash = "${hash}"; # nix-bump:hash`);
fs.writeFileSync(nixFile, nix);
console.log(`package.nix : ${latest.version} ${hash}`);
