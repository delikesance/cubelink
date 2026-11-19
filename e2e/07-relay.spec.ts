// T7 : plus aucun relais (n0 retire, #55). L'application n'utilise ni relais public, ni decouverte d'adresses : les
// reglages « relais » (allow_relay, relay_only) n'existent plus. Verifie SANS Internet : un ancien settings.json qui les
// contient se charge et ils disparaissent a la reecriture, l'ecran n'a plus de case « relais », le test de connexion ne
// parle plus de relais, et l'indication « via un relais » (evenement de transfert) reste coherente.
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

test.describe('T7 sans relais : reglages (sans Internet)', () => {
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

  test('« Tester ma connexion » : plus de relais, seulement IPv6 publique et port ouvert sur la box', async ({ appA }) => {
    const page = appA.page;
    await waitForRoute(page, 'login');
    await loginOffline(page, 'Alex');
    const r = await invoke<Record<string, unknown>>(page, 'net_test');
    expect(Object.keys(r).sort()).toEqual(['ipv6', 'port_mapped']);
    expect(r.port_mapped, 'mode test : rien n\'est ouvert sur la box').toBe(false);
    await openSettings(page);
    await page.getByRole('button', { name: 'Tester ma connexion' }).click();
    await expect(page.getByText(/Adresse IPv6 publique/)).toBeVisible();
    await expect(page.getByText(/Port ouvert sur la box/)).toBeVisible();
    await expect(page.getByText(/relais de secours/i)).toHaveCount(0);
  });

  test('indication « via un relais » dans Installation (evenement injecte) et absence sans relais', async ({ appA }) => {
    const page = appA.page;
    await waitForRoute(page, 'login');
    await loginOffline(page, 'Alex');
    await page.evaluate(() => { window.location.hash = '#/install?friend=abcd'; });
    await waitForRoute(page, 'install');
    const note = page.locator('.inst-note');
    await expect(note).toHaveCount(0); // pas de relais : pas d'indication
    const emit = (via_relay: boolean, speed_bps: number) => invoke(page, 'plugin:event|emit', {
      event: 'transfer://progress',
      payload: { phase: 'receiving', files_done: 1, files_total: 4, bytes_done: 1_000_000, bytes_total: 8_000_000, speed_bps, via_relay },
    });
    // L'abonnement de l'ecran (listen) est asynchrone : on reemet jusqu'a ce que l'ecran l'ait recu.
    await expect(async () => {
      await emit(false, 8_000_000);
      await expect(page.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '12', { timeout: 1_000 }); // l'evenement est bien recu
    }).toPass({ timeout: 15_000 });
    await expect(note).toHaveCount(0);
    await emit(true, 900_000);
    await expect(note).toBeVisible();
    await expect(note).toContainText('via un relais');
    await expect(note).toContainText('plus lent');
    await emit(false, 8_000_000);
    await expect(note).toHaveCount(0); // l'indication disparait quand la connexion redevient directe
    test.info().annotations.push({ type: 'note', description: 'Le message dedie « plus lentement » (SlowViaRelay, p2p/src/relay.rs SlowRelayAdvisor) n\'est pas branche : aucun evenement ni texte cote front/src-tauri (grep « lentement » : seulement la note statique de Install et le texte de Parametres). Seule la note « plus lent » liee a via_relay est testee.' });
  });
});
// Le parcours reel (ami, partage, join, transfert direct, tunnel TCP) est dans e2e/12-multiplayer-real.spec.ts (E2E_REAL=1).
