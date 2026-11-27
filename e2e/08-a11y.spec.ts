// T8 : accessibilite au CLAVIER de tous les ecrans, sur la VRAIE application (CDP).
// Controles manuels via le DOM et l'arbre d'accessibilite du navigateur (@axe-core/playwright n'est pas installe).
// Refonte 2 : Parametres / Mentions sont des ecrans (plus des dialogues), Creer = choix puis formulaire.
// Refonte de l'accueil : Amis = colonne de l'accueil + onglet Amis des Parametres (plus d'ecran ni de tiroir) ; Mods et Logs
// = onglets de la page du modpack ; les dialogues modaux restants sont des confirmations (<Modal>).
import { test, expect, waitForRoute } from './harness/fixtures';
import { SCREENS, prepare, gotoScreen, tabThrough, smallTargets, contrastIssues, unnamedControls, type Screen } from './helpers/e2e4-lib';
import { seedUnusedCacheObject } from './helpers/cache-seed';

const byId = (id: string) => SCREENS.find((s) => s.id === id)!;
/** Boutons du bandeau de fenetre (46x36, taille native Windows), selecteur de compte du bandeau (36 px de haut comme lui) et lien d'evitement (visible au focus seulement). */
const CHROME = /^(button\.tb-btn|button\.acc-trigger|a\.skip-link)/;
/** Zones denses documentees (docs/ux-guidelines.md, « Compact ») : onglets de la colonne d'amis 32 px, comptes 36 px. */
const COMPACT: { desc: RegExp; min: number }[] = [
  { desc: /^button\. "(En partie|En ligne|Tous) · \d+"$/, min: 32 },
  { desc: /^button\.acc-(remove|addbtn) /, min: 36 },
];
const isTooSmall = (t: { desc: string; w: number; h: number }) => {
  const compact = COMPACT.find((c) => c.desc.test(t.desc));
  return !(compact && t.w >= compact.min && t.h >= compact.min);
};

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

  test('Amis : #/friends ouvre Parametres > Amis (onglet selectionne, demandes, ajout et code presents)', async ({ appA }) => {
    const page = appA.page;
    await prepare(appA);
    await gotoScreen(page, byId('friends-list'));
    await expect(page.getByRole('tab', { name: 'Amis', exact: true })).toHaveAttribute('aria-selected', 'true');
    await expect(page.getByRole('heading', { name: /Demandes reçues/ })).toBeVisible();
    await expect(page.getByLabel('Code ami de ton ami')).toBeVisible();
    await expect(page.getByLabel(/^Mon code ami : /)).toBeVisible();
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

  test('Mentions legales : toutes les sections a la suite, sans navigation par sections, Retour atteignable au clavier', async ({ appA }) => {
    const page = appA.page;
    await prepare(appA);
    await gotoScreen(page, byId('legal'));
    // pas de navigation « Sections des reglages » ici (src/screens/Legal/index.test.tsx le verifie aussi)
    await expect(page.getByRole('navigation', { name: 'Sections des réglages' })).toHaveCount(0);
    expect(await page.locator('article h3').count()).toBeGreaterThan(1);
    const back = page.locator('main').getByRole('button', { name: 'Retour' }); // l'autre « Retour » est celui de la barre de titre
    await back.focus();
    await expect(back).toBeFocused();
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
    seedUnusedCacheObject(appA.dataDir);
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

  test('dialogue modal de l\'accueil (Supprimer l\'instance) : alertdialog, focus piege, Echap ferme', async ({ appA }) => {
    const page = appA.page;
    await prepare(appA);
    await gotoScreen(page, byId('main'));
    await page.locator('.home-tile').first().click({ button: 'right' });
    await page.getByRole('menuitem', { name: 'Supprimer' }).click();
    const dlg = page.getByRole('alertdialog');
    await expect(dlg).toBeVisible();
    await expect(dlg).toHaveAttribute('aria-modal', 'true');
    expect(await dlg.getAttribute('aria-labelledby'), 'dialogue nomme').toBeTruthy();
    await expect.poll(() => page.evaluate(() => !!document.activeElement?.closest('[role=alertdialog]')), 'focus dans le dialogue').toBe(true);
    // Tab ne sort jamais du dialogue (focus piege)
    for (let i = 0; i < 25; i++) {
      await page.keyboard.press('Tab');
      expect(await page.evaluate(() => !!document.activeElement?.closest('[role=alertdialog]')), `focus piege (Tab ${i})`).toBe(true);
    }
    await page.keyboard.press('Escape');
    await expect(page.getByRole('alertdialog'), 'ferme par Echap').toHaveCount(0, { timeout: 3000 });
    await waitForRoute(page, 'main', 1000); // Echap n'a ferme que le dialogue, l'instance est toujours la
    await expect(page.locator('.home-tile').first()).toBeVisible();
  });

  test('focus place sur le titre h1 a l\'ouverture : Join, Error, Reglages', async ({ appA }) => {
    const page = appA.page;
    await prepare(appA);
    for (const id of ['join', 'error', 'settings']) {
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

  test('un seul repere <main> dans Rejoindre', async ({ appA }) => {
    const page = appA.page;
    await prepare(appA);
    await gotoScreen(page, byId('join'));
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
      main: ['aside[aria-label="Amis"]', 'section[aria-label],[role=region][aria-label]', '[role=tablist][aria-label]'],
      create: ['main h1', '[aria-live]'],
      'friends-list': ['[role=tablist]', '[role=tab][aria-selected]', '[role=tabpanel][aria-labelledby]'],
      'friends-requests': ['[role=tablist]', '[role=tab][aria-selected]', '[role=tabpanel][aria-labelledby]', '[aria-live]'],
      join: ['main h1', '[role=alert],[role=status]'],
      settings: ['main h1', '[role=tablist]', '[role=tabpanel]:not([hidden])', '[role=status][aria-live=polite]'],
      error: ['main h1', '[role=alert],[role=status]'],
      legal: ['main h1', 'article[aria-labelledby]'],
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
    await page.getByRole('checkbox', { name: /Laisser tous mes amis rejoindre/ }).check();
    await expect(page.locator('[role=status][aria-live=polite]', { hasText: 'Enregistré' })).toBeVisible();
  });

  test('cibles tactiles >= 44 px sur tous les ecrans', async ({ appA }) => {
    const page = appA.page;
    await prepare(appA);
    for (const s of SCREENS) {
      await gotoScreen(page, s);
      const small = (await smallTargets(page, 44)).filter((t) => !CHROME.test(t.desc) && isTooSmall(t));
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
    seedUnusedCacheObject(appA.dataDir);
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
