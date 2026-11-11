// Génère latest.json (manifeste lu par l'updater de Tauri) à partir des installeurs et de leurs signatures .sig.
//   node scripts/make-latest-json.mjs <dossier des artefacts> <version> <tag> <fichier de notes> <dépôt owner/nom>
// Cherche (récursivement) : *.msi (+ .sig) pour Windows, *.AppImage (+ .sig) pour Linux. Écrit <dossier>/latest.json.
import fs from 'node:fs';
import path from 'node:path';

const [dir, version, tag, notesFile, repo] = process.argv.slice(2);
if (!dir || !version || !tag || !repo) {
  console.error('usage : make-latest-json.mjs <dossier> <version> <tag> <notes> <owner/repo>');
  process.exit(1);
}

const all = [];
const walk = (d) => {
  for (const e of fs.readdirSync(d, { withFileTypes: true })) {
    const p = path.join(d, e.name);
    if (e.isDirectory()) walk(p);
    else all.push(p);
  }
};
walk(dir);

const platforms = {};
const add = (keys, file) => {
  const sig = file + '.sig';
  if (!fs.existsSync(sig)) {
    console.error('signature manquante : ' + sig);
    process.exit(1);
  }
  const entry = {
    signature: fs.readFileSync(sig, 'utf8').trim(),
    url: `https://github.com/${repo}/releases/download/${tag}/${encodeURIComponent(path.basename(file))}`,
  };
  for (const k of keys) platforms[k] = entry;
};

const msi = all.find((f) => f.endsWith('.msi'));
const appimage = all.find((f) => f.endsWith('.AppImage'));
if (msi) add(['windows-x86_64', 'windows-x86_64-msi'], msi);
if (appimage) add(['linux-x86_64', 'linux-x86_64-appimage'], appimage);
if (!Object.keys(platforms).length) {
  console.error('aucun installeur trouvé dans ' + dir);
  process.exit(1);
}

const notes = notesFile && fs.existsSync(notesFile) ? fs.readFileSync(notesFile, 'utf8').trim() : '';
const latest = { version, notes, pub_date: new Date().toISOString(), platforms };
fs.writeFileSync(path.join(dir, 'latest.json'), JSON.stringify(latest, null, 2));
console.log(`latest.json : ${version}, plateformes : ${Object.keys(platforms).join(', ')}`);
