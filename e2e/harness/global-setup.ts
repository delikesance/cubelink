// Construit (si demande) le binaire UNE seule fois pour toute la suite, puis le publie aux workers
// via CUBELINK_E2E_BIN : sans cela, chaque worker reconstruirait le binaire avec CUBELINK_E2E_BUILD=1.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

export default async function globalSetup(): Promise<void> {
  const here = path.dirname(fileURLToPath(import.meta.url));
  const { binaryPath, build } = await import(pathToFileURL(path.join(here, 'build.mjs')).href);
  const bin: string = process.env.CUBELINK_E2E_BUILD === '1' || !fs.existsSync(binaryPath({})) ? build({}) : binaryPath({});
  process.env.CUBELINK_E2E_BIN = bin;
}
