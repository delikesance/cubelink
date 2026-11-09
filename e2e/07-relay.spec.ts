// T7 : mode relais. Le relais public n0 exige Internet (et ~1 Mo/s) : aucun relais local n'est disponible dans le
// harnais (feature iroh `test-utils` non activee, voir N17). Le chemin reel « direct impossible -> relais » est donc
// reserve a E2E_RELAY=1 ; le reste (reglages, contradiction, indication « via un relais ») est verifie SANS Internet.
import fs from 'node:fs';
import { test, expect, invoke, testState, waitForRoute, loginOffline } from './harness/fixtures';

type Settings = { allow_relay: boolean; relay_only?: boolean; [k: string]: unknown };
const relayCheckbox = (page: import('@playwright/test').Page) => page.getByRole('checkbox', { name: /Passer toujours par le relais/ });

async function openSettings(page: import('@playwright/test').Page) {
  await page.evaluate(() => { window.location.hash = '#/settings'; });
  await waitForRoute(page, 'settings');
  await relayCheckbox(page).waitFor({ state: 'visible' });
  await expect(relayCheckbox(page)).toBeEnabled(); // reglages charges
}

test.describe('T7 relais : reglages (sans Internet)', () => {
  test.use({ appOptions: { relayMode: 'disabled' } });

  test('« Passer toujours par le relais » : persiste (ecran, commande, fichier) et survit au rechargement', async ({ appA }) => {
    const page = appA.page;
    await waitForRoute(page, 'login');
    await loginOffline(page, 'Alex');
    const st = await testState(page);
    expect(st.relay_mode).toBeTruthy(); // mode relais impose par CUBELINK_RELAY_MODE, visible dans test_state
    const before = await invoke<Settings>(page, 'settings_get');
    expect(before.relay_only).toBe(false);
    expect(before.allow_relay).toBe(true);

    await openSettings(page);
    await expect(relayCheckbox(page)).not.toBeChecked();
    await relayCheckbox(page).check();
    await expect(page.locator('[role=status]', { hasText: 'Enregistré' })).toBeVisible();

    // pris en compte cote Rust : commande + fichier de reglages du dossier de donnees
    expect((await invoke<Settings>(page, 'settings_get')).relay_only).toBe(true);
    const file = JSON.parse(fs.readFileSync(st.paths.settings_file, 'utf8'));
    expect(file.relay_only).toBe(true);
    expect(file.allow_relay).toBe(true); // l'autre reglage n'est pas modifie

    // persistance : rechargement de la fenetre
    await page.reload();
    await waitForRoute(page, 'settings'); // le hash est conserve
    await openSettings(page);
    await expect(relayCheckbox(page)).toBeChecked();

    // retour arriere
    await relayCheckbox(page).uncheck();
    await expect(page.locator('[role=status]', { hasText: 'Enregistré' })).toBeVisible();
    expect((await invoke<Settings>(page, 'settings_get')).relay_only).toBe(false);
    expect(JSON.parse(fs.readFileSync(st.paths.settings_file, 'utf8')).relay_only).toBe(false);

    // le mode de relais du harnais (hors reglages utilisateur) n'est pas modifie par l'ecran
    expect((await testState(page)).relay_mode).toEqual(st.relay_mode);
    test.info().annotations.push({ type: 'note', description: 'allow_relay/relay_only sont lus au demarrage du backend P2P (iroh_backend.rs) : leur effet reseau n\'est pas observable sans redemarrage ni relais.' });
  });

  test('l\'ecran n\'expose pas allow_relay : l\'interface ne peut pas creer la contradiction', async ({ appA }) => {
    const page = appA.page;
    await waitForRoute(page, 'login');
    await loginOffline(page, 'Alex');
    await openSettings(page);
    await relayCheckbox(page).check();
    await expect(page.locator('[role=status]', { hasText: 'Enregistré' })).toBeVisible();
    // cocher « toujours le relais » ne coupe jamais allow_relay
    const s = await invoke<Settings>(page, 'settings_get');
    expect(s.relay_only).toBe(true);
    expect(s.allow_relay).toBe(true);
    expect(await page.getByRole('checkbox', { name: /allow_relay|autoriser le relais|relais public/i }).count()).toBe(0);
  });

  test('contradiction « toujours le relais » + relais desactive refusee avec un message clair', async ({ appA }) => {
    test.fixme(!process.env.E2E_RUN_FIXME, 'BOGUE : settings_set accepte relay_only=true avec allow_relay=false. src-tauri/src/commands_settings.rs:30 (validate_with) ne verifie pas la coherence ; la contradiction n\'est detectee qu\'au demarrage du backend (p2p/src/relay.rs:103, « contradiction : … ») et fait alors echouer le P2P au lieu d\'etre refusee a l\'enregistrement. Mesure : settings_set({relay_only:true, allow_relay:false}) renvoie les reglages (OK), settings_get les relit, le fichier est ecrit.');
    const page = appA.page;
    await waitForRoute(page, 'login');
    await loginOffline(page, 'Alex');
    const cur = await invoke<Settings>(page, 'settings_get');
    const err = await invoke(page, 'settings_set', { ...cur, allow_relay: false, relay_only: true }).then(() => null, (e) => String(e));
    expect(err, 'la contradiction doit etre refusee').not.toBeNull();
    expect(err).toMatch(/relais/i);
    expect(err).toMatch(/contradict|incompatible|désactiv|desactiv/i);
    expect(await invoke<Settings>(page, 'settings_get')).toEqual(cur); // rien n'est enregistre
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
    await emit(false, 8_000_000);
    await expect(page.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '12'); // l'evenement est bien recu
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

test.describe('T7 relais : mode relais personnalise du harnais', () => {
  test.use({ appOptions: { relayMode: 'custom:http://127.0.0.1:9' } });
  test('CUBELINK_RELAY_MODE custom est expose dans test_state et l\'app demarre hors Internet', async ({ appA }) => {
    await waitForRoute(appA.page, 'login');
    const st = await testState(appA.page);
    expect(JSON.stringify(st.relay_mode)).toContain('127.0.0.1:9');
  });
});

test.describe('T7 relais : chemin reel via le relais public n0 (E2E_RELAY=1, Internet requis)', () => {
  test.use({ appOptions: { relayMode: 'default' } });
  test.skip(process.env.E2E_RELAY !== '1', 'Reserve a E2E_RELAY=1 : le relais public n0 exige Internet (debit ~1 Mo/s) et aucun relais local de test n\'est disponible (iroh `test-utils` non active).');

  test('direct impossible : les deux profils en « toujours le relais », mode relais par defaut', async ({ appA, appB }) => {
    test.setTimeout(300_000);
    for (const app of [appA, appB]) {
      await waitForRoute(app.page, 'login');
      await loginOffline(app.page, app.label === 'A' ? 'Alice' : 'Bob');
      expect(JSON.stringify((await testState(app.page)).relay_mode)).toMatch(/default/i);
      await openSettings(app.page);
      await relayCheckbox(app.page).check();
      await expect(app.page.locator('[role=status]', { hasText: 'Enregistré' })).toBeVisible();
      expect((await invoke<Settings>(app.page, 'settings_get')).relay_only).toBe(true);
    }
    // TODO (necessite Internet + redemarrage des apps pour relire relay_only) : demande d'ami, join, puis
    // verifier `transfer://progress.via_relay === true` et la note « via un relais » sur #/install.
  });
});
