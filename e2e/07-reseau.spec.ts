// T7 : reseau sans relais (n0 retire, #55). L'application n'utilise ni relais public, ni decouverte d'adresses : les
// reglages « relais » (allow_relay, relay_only) n'existent plus. Verifie SANS Internet : un ancien settings.json qui les
// contient se charge et ils disparaissent a la reecriture, l'ecran n'a plus de case « relais », et le test de connexion
// ne parle plus de relais.
import fs from 'node:fs';
import { test, expect, invoke, testState, waitForRoute, loginOffline } from './harness/fixtures';

type Settings = Record<string, unknown>;
const lanCheckbox = (page: import('@playwright/test').Page) => page.getByRole('checkbox', { name: /Ouvrir mon monde au réseau local automatiquement/ });

async function openSettings(page: import('@playwright/test').Page) {
  await page.evaluate(() => { window.location.hash = '#/settings?tab=reseau'; });
  await waitForRoute(page, 'settings');
  await lanCheckbox(page).waitFor({ state: 'visible' });
  await expect(lanCheckbox(page)).toBeEnabled(); // reglages charges
}

test.describe('T7 reseau sans relais (sans Internet)', () => {
  // Ancien fichier de reglages d'une version avec relais n0 : champs contradictoires, ignores.
  test.use({ appOptions: { settings: { allow_relay: false, relay_only: true, discovery: true, default_ram_mb: 6144 } } });

  test('un ancien settings.json avec allow_relay/relay_only se charge, et ces champs disparaissent a la reecriture', async ({ appA }) => {
    const page = appA.page;
    await waitForRoute(page, 'login');
    await loginOffline(page, 'Alex');
    const st = await testState(page);
    expect(st).not.toHaveProperty('relay_mode');
    const before = await invoke<Settings>(page, 'settings_get');
    expect(before.default_ram_mb, "l'ancien fichier est lu, pas remplace par les defauts").toBe(6144);
    for (const k of ['allow_relay', 'relay_only', 'discovery']) expect(before).not.toHaveProperty(k);

    await openSettings(page);
    // plus aucune case relais ni decouverte publique
    await expect(page.getByRole('checkbox', { name: /relais/i })).toHaveCount(0);
    await expect(page.getByRole('checkbox', { name: /Retrouver mes amis même si leur adresse change/ })).toHaveCount(0);

    // un enregistrement reecrit le fichier sans les anciens champs, les autres sont gardes
    const lan = lanCheckbox(page);
    await expect(lan).toBeChecked();
    await lan.uncheck();
    await expect(page.locator('[role=status]', { hasText: 'Enregistré' })).toBeVisible();
    const file = JSON.parse(fs.readFileSync(st.paths.settings_file, 'utf8'));
    expect(file.lan_auto).toBe(false);
    expect(file.default_ram_mb).toBe(6144);
    for (const k of ['allow_relay', 'relay_only', 'discovery']) expect(file).not.toHaveProperty(k);

    // persistance : rechargement de la fenetre
    await page.reload();
    await waitForRoute(page, 'settings');
    await openSettings(page);
    await expect(lanCheckbox(page)).not.toBeChecked();
    await lanCheckbox(page).check();
    await expect(page.locator('[role=status]', { hasText: 'Enregistré' })).toBeVisible();
  });

  test('« Tester ma connexion » : plus de relais, seulement adresses publiques et port ouvert sur la box', async ({ appA }) => {
    const page = appA.page;
    await waitForRoute(page, 'login');
    await loginOffline(page, 'Alex');
    const r = await invoke<Record<string, unknown>>(page, 'net_test');
    expect(Object.keys(r).sort()).toEqual(['ipv4_public', 'ipv6', 'port_mapped']);
    expect(r.port_mapped, 'mode test : rien n\'est ouvert sur la box').toBe(false);
    await openSettings(page);
    await page.getByRole('button', { name: 'Tester ma connexion' }).click();
    await expect(page.getByText(/Adresse IPv4 publique/)).toBeVisible();
    await expect(page.getByText(/Port ouvert sur la box/)).toBeVisible();
    await expect(page.getByText(/relais de secours/i)).toHaveCount(0);
  });
});
// Le parcours reel (ami, partage, join, transfert direct, tunnel TCP) est dans e2e/12-multiplayer-real.spec.ts (E2E_REAL=1).
