// Purge des anciennes pré-versions canary (étape finale du job `publish` de canary.yml).
//   gh release list --limit 200 --json tagName,isPrerelease,isDraft,createdAt | node scripts/canary-prune.mjs <à garder>
// Lit la liste sur stdin et affiche, un par ligne, les tags à supprimer : les pré-versions canary au-delà des `keep` plus
// récentes (tri par date de création, pas par n° de run : une relance `.N` ou un changement de x.y.z fausserait l'ordre).
// Ne touche JAMAIS une release stable, un brouillon ni une pré-version hors motif (publiée à la main, par exemple).
// `keep` invalide (non entier, < 1) : erreur, jamais de purge totale par accident.
import { fileURLToPath } from 'node:url';

export const KEEP_DEFAULT = 10;
// Même forme que `version` dans canary-version.mjs : v<x.y.z>-canary.<run>, plus `.<essai>` pour une relance.
export const CANARY_TAG = /^v\d+\.\d+\.\d+-canary\.\d+(\.\d+)?$/;

export function selectStale(releases, keep = KEEP_DEFAULT) {
  if (!Number.isInteger(keep) || keep < 1) throw new Error('nombre à garder invalide (entier >= 1) : ' + keep);
  if (!Array.isArray(releases)) throw new Error('liste de releases invalide (tableau JSON attendu)');
  return releases
    .filter((r) => r && r.isPrerelease === true && r.isDraft !== true && CANARY_TAG.test(String(r.tagName)))
    .sort((a, b) => {
      const d = Date.parse(b.createdAt) - Date.parse(a.createdAt);
      if (Number.isNaN(d)) throw new Error('date de création invalide pour ' + (Number.isNaN(Date.parse(a.createdAt)) ? a.tagName : b.tagName));
      return d || (a.tagName < b.tagName ? 1 : a.tagName > b.tagName ? -1 : 0);
    })
    .slice(keep)
    .map((r) => r.tagName);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  try {
    const raw = process.argv[2] ?? String(KEEP_DEFAULT);
    if (!/^\d+$/.test(raw)) throw new Error('nombre à garder invalide (entier >= 1) : ' + raw);
    let input = '';
    process.stdin.setEncoding('utf8');
    for await (const chunk of process.stdin) input += chunk;
    for (const tag of selectStale(JSON.parse(input), Number(raw))) console.log(tag);
  } catch (e) {
    console.error(e.message);
    process.exit(1);
  }
}
