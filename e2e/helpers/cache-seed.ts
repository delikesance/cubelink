// Remplit le cache de mods sur le VRAI disque pour que « Vider le cache… » soit actif (profil neuf = cache vide).
// Tauri rend `window.__TAURI_INTERNALS__` et son `invoke` non modifiables : simuler `cache_size` en JS est impossible.
// On pose donc un objet du magasin qu'aucune instance n'utilise, assez vieux pour etre supprimable.
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { DatabaseSync } from 'node:sqlite';

const SEEDED_BYTES = 12 * 1024 * 1024;
const OLD_ENOUGH_SECONDS = 60 * 60;

/** A appeler une fois l'app lancee (l'index du magasin existe), avant d'ouvrir l'onglet Stockage. */
export function seedUnusedCacheObject(dataDir: string): void {
  const content = crypto.randomBytes(SEEDED_BYTES);
  const sha256 = crypto.createHash('sha256').update(content).digest('hex');
  const relPath = `${sha256.slice(0, 2)}/${sha256.slice(2, 4)}/${sha256}`;
  const storeDir = path.join(dataDir, 'store');
  const objectPath = path.join(storeDir, ...relPath.split('/'));
  fs.mkdirSync(path.dirname(objectPath), { recursive: true });
  fs.writeFileSync(objectPath, content);

  const old = Math.floor(Date.now() / 1000) - OLD_ENOUGH_SECONDS;
  const db = new DatabaseSync(path.join(storeDir, 'index.sqlite'));
  try {
    db.exec('PRAGMA busy_timeout = 5000');
    db.prepare('INSERT INTO objects (sha256, rel_path, size, refs, created_at, last_used) VALUES (?, ?, ?, 0, ?, ?)')
      .run(sha256, relPath, SEEDED_BYTES, old, old);
  } finally {
    db.close();
  }
}
