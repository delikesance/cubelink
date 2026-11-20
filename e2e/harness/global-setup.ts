// Resout le binaire UNE seule fois pour toute la suite (reconstruit par defaut, voir resolveBinary dans
// build.mjs), puis le publie aux workers via CUBELINK_E2E_BIN : sans cela, chaque worker reconstruirait.
// CUBELINK_E2E_SKIP_BUILD=1 reutilise le binaire existant.
// Windows seulement : le harnais pilote WebView2 par CDP (--remote-debugging-port, voir fixtures.ts), absent de
// WebKitGTK. Le garde est ici, pas dans playwright.config.ts, que `playwright test --list` charge aussi ; il passe
// avant toute compilation, pour ne pas bâtir un binaire de plusieurs Go qui ne servirait à rien.
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

export default async function globalSetup(): Promise<void> {
  if (process.platform !== 'win32') {
    console.error(
      "Les E2E Playwright pilotent WebView2 par CDP (--remote-debugging-port) : Windows seulement.\n" +
        "Sous Linux (WebKitGTK), utilise e2e/linux/ : `xvfb-run -a node e2e/linux/bindings-real.mjs` ou " +
        "`bash e2e/linux/background.sh` (voir le README, « Vérifier son travail »).",
    );
    process.exit(1);
  }
  const here = path.dirname(fileURLToPath(import.meta.url));
  const { resolveBinary } = await import(pathToFileURL(path.join(here, 'build.mjs')).href);
  process.env.CUBELINK_E2E_BIN = resolveBinary({}) as string;
}
