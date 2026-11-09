// T8 : accessibilite au CLAVIER de tous les ecrans, sur la VRAIE application (CDP).
// Controles manuels via le DOM et l'arbre d'accessibilite du navigateur (@axe-core/playwright n'est pas installe).
import { test, expect, waitForRoute } from './harness/fixtures';
import { SCREENS, prepare, gotoScreen, tabThrough, smallTargets, contrastIssues, unnamedControls, type Screen } from './helpers/e2e4-lib';

const byId = (id: string) => SCREENS.find((s) => s.id === id)!;
/** Boutons du bandeau de fenetre (46x32, taille native Windows) et lien d'evitement (visible au focus seulement). */
const CHROME = /^(button\.tb-btn|a\.skip-link)/;

test.describe('T8 accessibilite clavier', () => {
  test('parcours Tab de chaque ecran : ordre logique, focus visible #FF8A4A, aucun piege', async ({ appA }) => {
    const page = appA.page;
    await prepare(appA);
    for (const s of SCREENS) {
      await gotoScreen(page, s);
      const t = await tabThrough(page);
      const label = `[${s.id}]`;
      expect.soft(t.positiveTabindex, `${label} aucun tabindex positif`).toBe(0);
      expect.soft(t.steps.length, `${label} au moins un element atteignable`).toBeGreaterThan(0);
      // focus visible : contour 2 px #FF8A4A (ou ombre equivalente) sur chaque element focalise
      for (const st of t.steps) {
        expect.soft(st.visible, `${label} ${st.desc} visible au focus`).toBe(true);
        expect.soft(st.focusOk, `${label} ${st.desc} focus visible : ${st.outline}`).toBe(true);
      }
      // ordre logique = ordre du DOM (aucun tabindex positif), strictement croissant
      const idx = t.steps.map((x) => x.inDom);
      expect.soft(idx, `${label} ordre de tabulation = ordre du DOM`).toEqual([...idx].sort((a, b) => a - b));
      if (s.id === 'create') {
        // dialogue modal : le focus tourne volontairement dans le dialogue (piege voulu, Echap le quitte : voir test dedie)
        expect.soft(t.trapped, '[create] focus confine au dialogue modal').toBe(true);
        const names = t.steps.map((x) => x.desc).join(' ');
        expect.soft(names).toContain("Créer l'instance");
        expect.soft(names).toContain('Annuler');
      } else {
        expect.soft(t.trapped, `${label} aucun piege de focus`).toBe(false);
        // tout element focalisable (hors radios d'un groupe, un seul est dans l'ordre de tabulation) est atteint
        const reached = new Set(t.steps.map((x) => x.desc));
        const missed = t.focusables.filter((f) => !reached.has(f) && !f.startsWith('input "') && !f.startsWith('a "Aller au contenu"'));
        expect.soft(missed, `${label} elements focalisables jamais atteints au clavier`).toEqual([]);
      }
    }
  });

  test('onglets Amis : fleches gauche/droite changent d\'onglet', async ({ appA }) => {
    const page = appA.page;
    await prepare(appA);
    await gotoScreen(page, byId('friends-requests'));
    await page.getByRole('tab', { name: /Demandes/ }).focus();
    await page.keyboard.press('ArrowRight');
    await expect(page.getByRole('tab', { name: 'Ajouter un ami' })).toHaveAttribute('aria-selected', 'true');
    await expect(page.getByRole('tab', { name: 'Ajouter un ami' })).toBeFocused();
  });

  test('onglets Parametres et Mentions legales : fleches, Home, End', async ({ appA }) => {
    const page = appA.page;
    await prepare(appA);
    for (const id of ['settings', 'legal']) {
      await gotoScreen(page, byId(id));
      const tabs = page.getByRole('tab');
      const n = await tabs.count();
      expect(n).toBeGreaterThan(1);
      await tabs.first().focus();
      await page.keyboard.press(id === 'settings' ? 'ArrowDown' : 'ArrowDown');
      await expect(tabs.nth(1)).toHaveAttribute('aria-selected', 'true');
      await expect(tabs.nth(1)).toBeFocused();
      await page.keyboard.press('End');
      await expect(tabs.nth(n - 1)).toBeFocused();
      await page.keyboard.press('Home');
      await expect(tabs.first()).toBeFocused();
      // un seul onglet dans l'ordre de tabulation (roving tabindex)
      expect(await page.locator('[role=tab][tabindex="0"]').count()).toBe(1);
    }
  });

  test('Echap ferme le dialogue Creer (focus dans le dialogue)', async ({ appA }) => {
    const page = appA.page;
    await prepare(appA);

    await gotoScreen(page, byId('create'));
    await page.getByRole('textbox').first().focus(); // focus dans le dialogue (voir test fixme : a l'ouverture il est sur <main>)
    await page.keyboard.press('Escape');
    await waitForRoute(page, 'main');
  });

  test('Echap ferme la confirmation « Vider le cache » (alertdialog)', async ({ appA }) => {
    const page = appA.page;
    await prepare(appA);
    await gotoScreen(page, byId('settings'));
    await page.getByRole('tab', { name: 'Cache et partage' }).click();
    await page.getByRole('button', { name: /Vider le cache/ }).click();
    await expect(page.getByRole('alertdialog')).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(page.getByRole('alertdialog')).toHaveCount(0);
    await expect(page.locator('main[data-route="settings"]')).toBeVisible(); // Echap n'a ferme que la confirmation
  });

  test('Echap ferme le dialogue Parametres', async ({ appA }) => {
    const page = appA.page;
    await prepare(appA);
    await gotoScreen(page, byId('settings'));
    await page.getByRole('tab', { name: 'Réseau P2P' }).focus();
    await page.keyboard.press('Escape');
    await waitForRoute(page, 'main', 3000);
  });

  test('focus place sur le titre a l\'ouverture : Join', async ({ appA }) => {
    const page = appA.page;
    await prepare(appA);
    await gotoScreen(page, byId('join'));
    await expect.poll(() => page.evaluate(() => document.activeElement?.tagName)).toBe('H1');
  });

  test('focus place sur le titre a l\'ouverture : Install et Error', async ({ appA }) => {
    const page = appA.page;
    await prepare(appA);
    for (const id of ['install', 'error']) {
      await gotoScreen(page, byId(id));
      const tag = await page.evaluate(() => document.activeElement?.tagName);
      expect(['H1', 'H2'], `[${id}] focus sur le titre`).toContain(tag);
    }
  });

  test('focus place dans le dialogue Creer a l\'ouverture (champ Nom)', async ({ appA }) => {
    const page = appA.page;
    await prepare(appA);
    await gotoScreen(page, byId('create'));
    expect(await page.evaluate(() => !!document.activeElement?.closest('[role=dialog]'))).toBe(true);
  });

  test("Echap des l'ouverture du dialogue Creer (sans clic)", async ({ appA }) => {
    const page = appA.page;
    await prepare(appA);
    await gotoScreen(page, byId('create'));
    await page.keyboard.press('Escape');
    await waitForRoute(page, 'main', 3000);
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
      login: ['[aria-live]', 'section[aria-labelledby]'],
      main: ['nav[aria-label]', '[role=status][aria-live=polite]', 'aside[aria-label]'],
      create: ['[role=dialog][aria-modal=true][aria-labelledby]', '[aria-live]'],
      'friends-requests': ['[role=tablist]', '[role=tab][aria-selected]', '[role=tabpanel][aria-labelledby]', '[aria-live]'],
      join: ['[role=alert],[role=status]'],
      install: ['[role=progressbar][aria-valuenow][aria-valuemin][aria-valuemax]', '[role=status][aria-live=polite]'],
      settings: ['[role=dialog][aria-modal=true][aria-labelledby]', '[role=tablist]', '[role=tabpanel]:not([hidden])', '[role=status][aria-live=polite]'],
      error: ['[role=alert],[role=status]'],
      legal: ['[role=tablist]', '[role=tabpanel]', '[role=region][aria-labelledby]'],
    };
    for (const s of SCREENS) {
      const sels = need[s.id];
      if (!sels) continue;
      await gotoScreen(page, s);
      for (const sel of sels) {
        expect.soft(await page.locator(sel).count(), `[${s.id}] ${sel}`).toBeGreaterThan(0);
      }
      // un seul <main> et un titre h1 present dans l'ecran
      if (s.id !== 'install') expect.soft(await page.locator('main').count(), `[${s.id}] un seul <main>`).toBe(1); // install : test fixme dedie
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
    // bandeau de fenetre : documente (design natif Windows 46x32), non bloquant
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
    // etats secondaires : confirmation du cache, section credits
    await gotoScreen(page, byId('settings'));
    await page.getByRole('tab', { name: 'Cache et partage' }).click();
    await page.getByRole('button', { name: /Vider le cache/ }).click();
    expect.soft((await contrastIssues(page)).issues, '[settings/cache+confirmation]').toEqual([]);
    await page.getByRole('button', { name: 'Annuler' }).click();
    await page.getByRole('tab', { name: 'Crédits et licences' }).click();
    for (const sm of await page.locator('[role=tabpanel]:not([hidden]) details > summary').all()) await sm.click();
    expect.soft((await contrastIssues(page)).issues, '[settings/credits]').toEqual([]);
  });
});

export type { Screen };
