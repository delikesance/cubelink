// T8 : accessibilite au CLAVIER de tous les ecrans, sur la VRAIE application (CDP).
// Controles manuels via le DOM et l'arbre d'accessibilite du navigateur (@axe-core/playwright n'est pas installe).
// Refonte 2 : Parametres / Mentions sont des ecrans (plus des dialogues), Creer = choix puis formulaire,
// Mods / Parametres d'instance / Logs / Amis sont des tiroirs (<Drawer>) de l'accueil.
import { test, expect, waitForRoute } from './harness/fixtures';
import { SCREENS, prepare, gotoScreen, tabThrough, smallTargets, contrastIssues, unnamedControls, type Screen } from './helpers/e2e4-lib';

const byId = (id: string) => SCREENS.find((s) => s.id === id)!;
/** Boutons du bandeau de fenetre (46x36, taille native Windows), selecteur de compte du bandeau (36 px de haut comme lui) et lien d'evitement (visible au focus seulement). */
const CHROME = /^(button\.tb-btn|button\.acc-trigger|a\.skip-link)/;

test.describe('T8 accessibilite clavier', () => {
  test('parcours Tab de chaque ecran : ordre logique, focus visible #6FD3F0, aucun piege', async ({ appA }) => {
    const page = appA.page;
    await prepare(appA);
    for (const s of SCREENS) {
      await gotoScreen(page, s);
      const t = await tabThrough(page, 150);
      const label = `[${s.id}]`;
      expect.soft(t.positiveTabindex, `${label} aucun tabindex positif`).toBe(0);
      expect.soft(t.steps.length, `${label} au moins un element atteignable`).toBeGreaterThan(0);
      // focus visible : contour 2 px #6FD3F0 (ou ombre equivalente) sur chaque element focalise
      for (const st of t.steps) {
        expect.soft(st.visible, `${label} ${st.desc} visible au focus`).toBe(true);
        expect.soft(st.focusOk, `${label} ${st.desc} focus visible : ${st.outline}`).toBe(true);
      }
      // ordre logique = ordre du DOM (aucun tabindex positif), strictement croissant
      const idx = t.steps.map((x) => x.inDom);
      expect.soft(idx, `${label} ordre de tabulation = ordre du DOM : ${t.steps.map((x) => x.inDom + ':' + x.desc).join(' > ')}`).toEqual([...idx].sort((a, b) => a - b));
      expect.soft(t.trapped, `${label} aucun piege de focus`).toBe(false);
      // tout element focalisable (hors radios d'un groupe, un seul est dans l'ordre de tabulation) est atteint
      const reached = new Set(t.steps.map((x) => x.desc));
      const missed = t.focusables.filter((f) => !reached.has(f) && !f.startsWith('input "') && !f.startsWith('a "Aller au contenu"'));
      expect.soft(missed, `${label} elements focalisables jamais atteints au clavier`).toEqual([]);
    }
  });

  test('onglets Amis : fleches gauche/droite changent d\'onglet', async ({ appA }) => {
    const page = appA.page;
    await prepare(appA);
    await gotoScreen(page, byId('friends-requests'));
    await page.getByRole('tab', { name: /Demandes/ }).focus();
    await page.keyboard.press('ArrowRight');
    await expect(page.getByRole('tab', { name: 'Tes amis' })).toHaveAttribute('aria-selected', 'true');
    await expect(page.getByRole('tab', { name: 'Tes amis' })).toBeFocused();
  });

  test('onglets Reglages : fleches, Home, End, roving tabindex', async ({ appA }) => {
    const page = appA.page;
    await prepare(appA);
    await gotoScreen(page, byId('settings'));
    const tabs = page.getByRole('tab');
    const n = await tabs.count();
    expect(n).toBeGreaterThan(1);
    await tabs.first().focus();
    await page.keyboard.press('ArrowRight');
    await expect(tabs.nth(1)).toHaveAttribute('aria-selected', 'true');
    await expect(tabs.nth(1)).toBeFocused();
    await page.keyboard.press('End');
    await expect(tabs.nth(n - 1)).toBeFocused();
    await page.keyboard.press('Home');
    await expect(tabs.first()).toBeFocused();
    // un seul onglet dans l'ordre de tabulation (roving tabindex)
    expect(await page.locator('[role=tab][tabindex="0"]').count()).toBe(1);
  });

  test('Mentions legales : navigation par liens vers les sections des Reglages', async ({ appA }) => {
    const page = appA.page;
    await prepare(appA);
    await gotoScreen(page, byId('legal'));
    const links = page.getByRole('navigation', { name: 'Sections des réglages' }).getByRole('link');
    expect(await links.count()).toBeGreaterThan(1);
    await links.first().focus();
    await expect(links.first()).toBeFocused();
  });

  test('Echap quitte Creer (choix puis formulaire) ; focus dans la vue a l\'ouverture', async ({ appA }) => {
    const page = appA.page;
    await prepare(appA);
    await gotoScreen(page, byId('create'));
    expect(await page.evaluate(() => !!document.activeElement?.closest('.create'))).toBe(true);
    await page.keyboard.press('Escape'); // des l'ouverture, sans clic
    await waitForRoute(page, 'main', 3000);
    await gotoScreen(page, byId('create'));
    await page.getByRole('button', { name: /Créer une instance vide/ }).click();
    // champ Nom focalise en entrant dans le formulaire
    await expect.poll(() => page.evaluate(() => (document.activeElement as HTMLInputElement | null)?.type)).toBe('text');
    await page.keyboard.press('Escape');
    await waitForRoute(page, 'main', 3000);
  });

  test('Echap ferme la confirmation « Vider le cache » (alertdialog) puis quitte les Reglages', async ({ appA }) => {
    const page = appA.page;
    await prepare(appA);
    await gotoScreen(page, byId('settings'));
    await page.getByRole('tab', { name: 'Stockage' }).click();
    await page.getByRole('button', { name: /Vider le cache/ }).click();
    await expect(page.getByRole('alertdialog')).toBeVisible();
    expect(await page.evaluate(() => !!document.activeElement?.closest('[role=alertdialog]'))).toBe(true);
    await page.keyboard.press('Escape');
    await expect(page.getByRole('alertdialog')).toHaveCount(0);
    await expect(page.locator('main[data-route="settings"]')).toBeVisible(); // Echap n'a ferme que la confirmation
    await page.getByRole('tab', { name: 'Réseau' }).focus();
    await page.keyboard.press('Escape');
    await waitForRoute(page, 'main', 3000);
  });

  test('Echap quitte Mentions legales et Rejoindre (vers accueil)', async ({ appA }) => {
    const page = appA.page;
    await prepare(appA);
    await gotoScreen(page, byId('legal'));
    await page.keyboard.press('Escape');
    // retour a l'ecran precedent de l'historique (accueil ici, Reglages en repli) : on quitte en tout cas Mentions legales
    await expect(page.locator('main[data-route="legal"]')).toHaveCount(0, { timeout: 3000 });
    await gotoScreen(page, byId('join'));
    await page.keyboard.press('Escape');
    await waitForRoute(page, 'main', 3000);
  });

  test('tiroirs de l\'accueil (Mods, Parametres, Logs, Amis) : dialogue modal, focus piege, Echap, focus rendu', async ({ appA }) => {
    const page = appA.page;
    await prepare(appA);
    await gotoScreen(page, byId('main'));
    const openers: [string, RegExp][] = [
      ['Mods', /^Mods$/],
      ['Paramètres', /^Paramètres$/],
      ['Logs', /^Logs$/],
      ['Amis', /^Amis/],
    ];
    for (const [name, re] of openers) {
      const opener = page.getByRole('button', { name: re }).first();
      await opener.focus();
      await page.keyboard.press('Enter');
      const dlg = page.getByRole('dialog').first();
      await expect(dlg, `[${name}] tiroir ouvert`).toBeVisible();
      await expect(dlg).toHaveAttribute('aria-modal', 'true');
      expect(await dlg.getAttribute('aria-labelledby'), `[${name}] dialogue nomme`).toBeTruthy();
      await expect.poll(() => page.evaluate(() => !!document.activeElement?.closest('[role=dialog]')), `[${name}] focus dans le tiroir`).toBe(true);
      // Tab ne sort jamais du tiroir (focus piege)
      for (let i = 0; i < 25; i++) {
        await page.keyboard.press('Tab');
        expect(await page.evaluate(() => !!document.activeElement?.closest('[role=dialog]')), `[${name}] focus piege (Tab ${i})`).toBe(true);
      }
      await page.keyboard.press('Escape');
      await expect(page.getByRole('dialog'), `[${name}] ferme par Echap`).toHaveCount(0, { timeout: 3000 });
      await expect.poll(() => page.evaluate(() => document.activeElement?.tagName), `[${name}] focus rendu`).not.toBe('BODY');
      await waitForRoute(page, 'main', 1000);
    }
  });

  test('focus place sur le titre h1 a l\'ouverture : Join, Install, Error, Reglages', async ({ appA }) => {
    const page = appA.page;
    await prepare(appA);
    for (const id of ['join', 'install', 'error', 'settings']) {
      await gotoScreen(page, byId(id));
      await expect.poll(() => page.evaluate(() => document.activeElement?.tagName), `[${id}] focus sur le titre`).toBe('H1');
    }
  });

  test('focus place sur le titre a l\'ouverture : Mentions legales', async ({ appA }) => {
    const page = appA.page;
    await prepare(appA);
    await gotoScreen(page, byId('legal'));
    await expect.poll(() => page.evaluate(() => /^H[12]$/.test(document.activeElement?.tagName ?? ''))).toBe(true);
  });

  test('un seul repere <main> dans Install', async ({ appA }) => {
    const page = appA.page;
    await prepare(appA);
    await gotoScreen(page, byId('install'));
    expect(await page.locator('main').count()).toBe(1);
  });

  test('chaque bouton / lien / champ a un nom accessible (arbre d\'accessibilite)', async ({ appA }) => {
    const page = appA.page;
    await prepare(appA);
    for (const s of SCREENS) {
      await gotoScreen(page, s);
      const bad = (await unnamedControls(page)).filter((b) => !b.startsWith('MenuListPopup')); // popup natif d'un <select>
      expect.soft(bad, `[${s.id}] controles sans nom`).toEqual([]);
    }
  });

  test('roles et regions aria-live presents sur chaque ecran', async ({ appA }) => {
    const page = appA.page;
    await prepare(appA);
    const need: Record<string, string[]> = {
      login: ['section[aria-labelledby]', 'main h1'],
      main: ['nav[aria-label]', 'section[aria-label],[role=region][aria-label]', 'button[aria-label^="Amis"]'],
      create: ['main h1', '[aria-live]'],
      'friends-list': ['[role=tablist]', '[role=tab][aria-selected]', '[role=tabpanel][aria-labelledby]'],
      'friends-requests': ['[role=tablist]', '[role=tab][aria-selected]', '[role=tabpanel][aria-labelledby]', '[aria-live]'],
      join: ['main h1', '[role=alert],[role=status]'],
      install: ['main h1', '[role=progressbar][aria-valuenow][aria-valuemin][aria-valuemax]', '[role=status][aria-live=polite]'],
      settings: ['main h1', '[role=tablist]', '[role=tabpanel]:not([hidden])', '[role=status][aria-live=polite]'],
      error: ['main h1', '[role=alert],[role=status]'],
      legal: ['main h1', 'nav[aria-label]', 'article[aria-labelledby]'],
    };
    for (const s of SCREENS) {
      const sels = need[s.id];
      if (!sels) continue;
      await gotoScreen(page, s);
      for (const sel of sels) {
        expect.soft(await page.locator(sel).count(), `[${s.id}] ${sel}`).toBeGreaterThan(0);
      }
      expect.soft(await page.locator('main').count(), `[${s.id}] un seul <main>`).toBe(1);
    }
    // alertes dynamiques
    await gotoScreen(page, byId('friends-add'));
    const field = page.getByLabel('Code ami de ton ami');
    await field.fill('pas-un-code');
    await field.blur();
    await expect(field).toHaveAttribute('aria-invalid', 'true');
    await expect(page.locator('#add-error')).toBeVisible();
    expect(await field.getAttribute('aria-describedby')).toContain('add-error');
    // code bien forme mais ami injoignable : l'erreur du service est annoncee (role=alert)
    await field.fill('mcl1-AAAAAAAAAAAAAAAAAAAAAAAA');
    await page.getByRole('button', { name: 'Envoyer la demande' }).click();
    await expect(page.locator('[role=alert]').filter({ hasText: /\S/ }).first()).toBeVisible({ timeout: 20_000 });
    await gotoScreen(page, byId('settings'));
    await page.getByRole('tab', { name: 'Réseau' }).click();
    await page.getByRole('checkbox', { name: /Passer toujours par le relais/ }).check();
    await expect(page.locator('[role=status][aria-live=polite]', { hasText: 'Enregistré' })).toBeVisible();
  });

  test('cibles tactiles >= 44 px sur tous les ecrans', async ({ appA }) => {
    const page = appA.page;
    await prepare(appA);
    for (const s of SCREENS) {
      await gotoScreen(page, s);
      const small = (await smallTargets(page, 44)).filter((t) => !CHROME.test(t.desc));
      expect.soft(small, `[${s.id}] cibles < 44 px`).toEqual([]);
    }
    // bandeau de fenetre : documente (design natif Windows 46x36), non bloquant
    const tb = await page.locator('.tb-btn').first().boundingBox();
    test.info().annotations.push({ type: 'note', description: `boutons du bandeau de fenetre ${Math.round(tb!.width)}x${Math.round(tb!.height)} px (exception design, non testee)` });
  });

  test('contraste texte/fond >= 4,5:1 (3:1 grand texte) sur les elements principaux', async ({ appA }) => {
    const page = appA.page;
    await prepare(appA);
    let total = 0;
    for (const s of SCREENS) {
      await gotoScreen(page, s);
      const r = await contrastIssues(page);
      total += r.checked;
      expect.soft(r.checked, `[${s.id}] echantillon non vide (ignores car fond image : ${r.skipped})`).toBeGreaterThan(0);
      expect.soft(r.issues, `[${s.id}] contrastes insuffisants`).toEqual([]);
    }
    expect(total).toBeGreaterThan(60);
    // etats secondaires : confirmation du cache, onglet A propos
    await gotoScreen(page, byId('settings'));
    await page.getByRole('tab', { name: 'Stockage' }).click();
    await page.getByRole('button', { name: /Vider le cache/ }).click();
    expect.soft((await contrastIssues(page)).issues, '[settings/stockage+confirmation]').toEqual([]);
    await page.getByRole('button', { name: 'Annuler' }).click();
    await page.getByRole('tab', { name: 'À propos' }).click();
    for (const sm of await page.locator('[role=tabpanel]:not([hidden]) details > summary').all()) await sm.click();
    expect.soft((await contrastIssues(page)).issues, '[settings/a-propos]').toEqual([]);
  });
});

export type { Screen };
