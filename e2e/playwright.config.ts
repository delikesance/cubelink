import { defineConfig } from '@playwright/test';

// E2E Cubelink : l'application Tauri reelle est pilotee par CDP (voir e2e/harness/fixtures.ts).
export default defineConfig({
  testDir: '.',
  testMatch: /.*\.spec\.ts$/,
  outputDir: 'artifacts/playwright',
  timeout: 120_000,
  expect: { timeout: 10_000 },
  workers: 1, // deux instances WebView2 par test : pas de parallelisme
  fullyParallel: false,
  retries: 0,
  reporter: [['list']],
});
