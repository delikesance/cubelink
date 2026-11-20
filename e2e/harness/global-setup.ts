// Resout le binaire UNE seule fois pour toute la suite (reconstruit par defaut, voir resolveBinary dans
// build.mjs), puis le publie aux workers via CUBELINK_E2E_BIN : sans cela, chaque worker reconstruirait.
// CUBELINK_E2E_SKIP_BUILD=1 reutilise le binaire existant.
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

export default async function globalSetup(): Promise<void> {
  const here = path.dirname(fileURLToPath(import.meta.url));
  const { resolveBinary } = await import(pathToFileURL(path.join(here, 'build.mjs')).href);
  process.env.CUBELINK_E2E_BIN = resolveBinary({}) as string;
}
