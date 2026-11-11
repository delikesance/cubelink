import { defineConfig } from '@playwright/test';

// E2E Cubelink : l'application Tauri reelle est pilotee par CDP (voir e2e/harness/fixtures.ts).
export default defineConfig({
  testDir: '.',
  testMatch: /.*\.spec\.ts$/,
  outputDir: 'artifacts/playwright',
  timeout: 120_000,
  expect: { timeout: 10_000 },
  // Chaque test a ses propres dossiers, ports (aleatoires) et profils WebView2 : on parallelise.
  // E2E_WORKERS=1 pour revenir au mode sequentiel (diagnostic).
  workers: Number(process.env.E2E_WORKERS ?? 4),
  fullyParallel: true, // les describe 'serial' (03, 04) restent sequentiels
  globalSetup: './harness/global-setup.ts',
  retries: 0,
  reporter: [['list']],
});
