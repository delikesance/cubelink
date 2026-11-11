// T9 : tailles de fenetre 960x600 et 1280x720 (redimensionnement de la VRAIE fenetre WebView2 par CDP
// Browser.setWindowBounds). Pour chaque ecran/etat : pas de defilement horizontal de la page, tous les
// boutons/liens/champs sont visibles ou atteignables par defilement (rien de coupe), bandeau « Mon code ami »
// sans chevauchement avec les onglets ; capture dans e2e/artifacts/sizes/.
import fs from 'node:fs';
import path from 'node:path';
import type { Page } from '@playwright/test';
import { test, expect, ARTIFACTS, makeHostInstance, invoke, waitForRoute } from './harness/fixtures';
import { SCREENS, prepare, gotoScreen, setWindowSize, type Screen } from './helpers/e2e4-lib';

const SIZES: [number, number][] = [[960, 600], [1280, 720]];
const OUT = path.join(ARTIFACTS, 'sizes');
const byId = (id: string) => SCREENS.find((s) => s.id === id)!;
const progress = (p: Record<string, unknown>) => ({ phase: 'receiving', files_done: 12, files_total: 40, bytes_done: 340e6, bytes_total: 1.2e9, speed_bps: 800_000, via_relay: true, ...p });

interface Variant { id: string; screen: Screen; setup?: (page: Page) => Promise<void> }
const VARIANTS: Variant[] = [
  ...SCREENS.map((screen) => ({ id: screen.id, screen })),
  { id: 'create-advanced', screen: byId('create'), setup: async (p) => { await p.getByRole('button', { name: /Créer une instance vide/ }).click(); await p.getByRole('button', { name: 'Options avancées' }).click(); } },
  { id: 'friends-code-confirm', screen: byId('friends-requests'), setup: async (p) => { await p.getByRole('button', { name: /Générer un nouveau code/ }).click(); } },
  { id: 'install-relay', screen: byId('install'), setup: async (p) => { await invoke(p, 'plugin:event|emit', { event: 'transfer://progress', payload: progress({}) }); await p.locator('.inst-note').waitFor(); } },
  { id: 'install-done', screen: byId('install'), setup: async (p) => { await invoke(p, 'plugin:event|emit', { event: 'transfer://progress', payload: progress({ phase: 'done', files_done: 40, bytes_done: 1.2e9 }) }); await p.getByRole('button', { name: 'Jouer' }).or(p.getByRole('button', { name: 'Réessayer' })).first().waitFor(); } },
  { id: 'install-cancel-confirm', screen: byId('install'), setup: async (p) => { await p.getByRole('button', { name: 'Annuler' }).click(); await p.getByRole('group', { name: /Confirmer/ }).waitFor(); } },
  { id: 'settings-network', screen: byId('settings'), setup: async (p) => { await p.getByRole('tab', { name: 'Réseau' }).click(); } },
  { id: 'settings-java', screen: byId('settings'), setup: async (p) => { await p.getByRole('tab', { name: 'Java' }).click(); } },
  { id: 'settings-storage-confirm', screen: byId('settings'), setup: async (p) => { await p.getByRole('tab', { name: 'Stockage' }).click(); await p.getByRole('button', { name: /Vider le cache/ }).click(); } },
  { id: 'settings-about', screen: byId('settings'), setup: async (p) => { await p.getByRole('tab', { name: 'À propos' }).click(); } },
  { id: 'error-waiting', screen: { ...byId('error'), hash: '#/error?state=waiting' } },
  { id: 'error-no-license', screen: { ...byId('error'), hash: '#/error?state=no-license' } },
  { id: 'legal-last-section', screen: byId('legal'), setup: async (p) => { await p.locator('article section, main section').last().scrollIntoViewIfNeeded(); } },
];

/** Mesures de mise en page dans la page. Chaque controle est amene dans la vue (defilement) avant mesure. */
async function layout(page: Page) {
  return page.evaluate(async () => {
    const vw = innerWidth, vh = innerHeight;
    const doc = document.documentElement;
    const hscroll = { doc: doc.scrollWidth - doc.clientWidth, body: document.body.scrollWidth - doc.clientWidth };
    const isVisible = (e: Element) => { const r = e.getBoundingClientRect(); const cs = getComputedStyle(e); return r.width > 0 && r.height > 0 && cs.visibility !== 'hidden' && cs.display !== 'none' && !e.closest('[hidden]') && !e.closest('.skip-link') && !(e.closest('details:not([open])') && !(e.tagName === 'SUMMARY' && e.parentElement === e.closest('details:not([open])'))); };
    const name = (e: Element) => { const h = e as HTMLElement; return `${e.tagName.toLowerCase()}${h.className ? '.' + String(h.className).split(' ')[0] : ''} "${(h.getAttribute('aria-label') || h.innerText || (h as HTMLInputElement).placeholder || '').trim().replace(/\s+/g, ' ').slice(0, 36)}"`; };
    const clipOf = (e: Element): DOMRect | null => {
      // intersection des zones de rognage (overflow != visible) des ancetres, viewport compris
      let L = 0, T = 0, R = vw, B = vh;
      for (let p = e.parentElement; p && p !== document.documentElement; p = p.parentElement) {
        const cs = getComputedStyle(p);
        if (/(hidden|clip|auto|scroll)/.test(cs.overflowX + cs.overflowY)) {
          const r = p.getBoundingClientRect();
          if (/(hidden|clip|auto|scroll)/.test(cs.overflowX)) { L = Math.max(L, r.left); R = Math.min(R, r.right); }
          if (/(hidden|clip|auto|scroll)/.test(cs.overflowY)) { T = Math.max(T, r.top); B = Math.min(B, r.bottom); }
        }
      }
      return new DOMRect(L, T, R - L, B - T);
    };
    const SEL = 'button,a[href],input:not([type=hidden]),select,textarea,summary,[role=tab],h1,h2,[role=progressbar]';
    const items = [...document.querySelectorAll(SEL)].filter(isVisible).filter((e) => !e.closest('.titlebar'));
    const unreachable: string[] = []; const sizeZero: string[] = [];
    let interactive = 0;
    for (const e of items) {
      const h = e as HTMLElement;
      const isCtl = !/^(H1|H2|DIV)$/.test(h.tagName);
      if (isCtl) interactive++;
      h.scrollIntoView({ block: 'nearest', inline: 'nearest' });
      await new Promise((r) => requestAnimationFrame(() => r(null)));
      const r = h.getBoundingClientRect();
      const c = clipOf(h)!;
      const tol = 1.5;
      const ok = r.left >= c.left - tol && r.right <= c.right + tol && r.top >= c.top - tol && r.bottom <= c.bottom + tol;
      if (!ok) unreachable.push(`${name(e)} rect=[${Math.round(r.left)},${Math.round(r.top)},${Math.round(r.right)},${Math.round(r.bottom)}] zone=[${Math.round(c.left)},${Math.round(c.top)},${Math.round(c.right)},${Math.round(c.bottom)}]`);
    }
    // sticky/fixed recouvrant un controle apres defilement : le controle doit rester non masque par le bandeau
    document.querySelectorAll('*').forEach((e) => { (e as HTMLElement).scrollTop = 0; });
    window.scrollTo(0, 0);
    return { vw, vh, hscroll, interactive, unreachable, sizeZero };
  });
}

/** Bandeau « Mon code ami » (sticky) vs onglets : rectangles disjoints, au repos et apres defilement. */
async function bannerVsTabs(page: Page) {
  return page.evaluate(async () => {
    const banner = document.querySelector('section.mycode') as HTMLElement | null;
    const tabs = document.querySelector('[role=tablist][aria-label="Amis"]') as HTMLElement | null;
    if (!banner || !tabs) return { found: false, overlaps: [] as string[] };
    const overlap = (a: DOMRect, b: DOMRect) => Math.min(a.right, b.right) - Math.max(a.left, b.left) > 0.5 && Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top) > 0.5;
    const out: string[] = [];
    const scrollers = [...document.querySelectorAll('*')].filter((e) => e.scrollHeight > e.clientHeight + 1 && /(auto|scroll)/.test(getComputedStyle(e).overflowY));
    const check = (tag: string) => {
      const b = banner.getBoundingClientRect(), t = tabs.getBoundingClientRect();
      if (overlap(b, t)) out.push(`${tag} : bandeau [${Math.round(b.top)},${Math.round(b.bottom)}] chevauche onglets [${Math.round(t.top)},${Math.round(t.bottom)}]`);
      const pn = document.querySelector('[role=tabpanel]')?.getBoundingClientRect();
      if (pn && overlap(b, pn)) out.push(`${tag} : bandeau chevauche le panneau [${Math.round(pn.top)},${Math.round(pn.bottom)}]`);
    };
    check('repos');
    for (const s of scrollers) { s.scrollTop = s.scrollHeight; await new Promise((r) => requestAnimationFrame(() => r(null))); check(`defile ${s.tagName.toLowerCase()}`); s.scrollTop = 0; }
    const b = banner.getBoundingClientRect();
    return { found: true, overlaps: out, banner: [Math.round(b.top), Math.round(b.bottom)], scrollers: scrollers.length };
  });
}

test.describe('T9 tailles de fenetre', () => {
  test('960x600 et 1280x720 : aucun defilement horizontal, controles atteignables, rien de coupe', async ({ appA }) => {
    test.setTimeout(480_000);
    const page = appA.page;
    await prepare(appA);
    for (const n of [2, 3, 4, 5, 6]) await makeHostInstance(appA, { id: `monde-${n}`, name: `Monde numero ${n}` }); // rail et grille remplis
    await page.reload();
    fs.mkdirSync(OUT, { recursive: true });
    for (const f of fs.readdirSync(OUT)) fs.rmSync(path.join(OUT, f), { force: true });

    for (const v of VARIANTS) {
      for (const [w, h] of SIZES) {
        const t0 = Date.now();
        await setWindowSize(page, w, h);
        await page.evaluate(() => { window.location.hash = '#/main'; }); // remontage complet de l'ecran (etat propre)
        await waitForRoute(page, 'main');
        await gotoScreen(page, v.screen);
        if (v.setup) { await v.setup(page); await page.waitForTimeout(250); }
        const size = await page.evaluate(() => ({ w: innerWidth, h: innerHeight }));
        expect.soft(size, `[${v.id}] taille de fenetre obtenue`).toEqual({ w, h });
        await page.screenshot({ path: path.join(OUT, `${v.id}-${w}x${h}.png`), timeout: 10_000 }).catch(() => {});
        const l = await layout(page);
        const tag = `[${v.id} ${w}x${h}]`;
        expect.soft(l.hscroll.doc, `${tag} defilement horizontal de la page (documentElement)`).toBeLessThanOrEqual(0);
        expect.soft(l.hscroll.body, `${tag} defilement horizontal de la page (body)`).toBeLessThanOrEqual(0);
        expect.soft(l.interactive, `${tag} controles trouves`).toBeGreaterThan(0);
        expect.soft(l.unreachable, `${tag} elements coupes / hors zone meme apres defilement`).toEqual([]);
        if (process.env.E2E_VERBOSE) console.log(`${tag} ${Date.now() - t0} ms, ${l.interactive} controles`);
        if (v.id.startsWith('friends')) {
          const bt = await bannerVsTabs(page);
          expect.soft(bt.found, `${tag} bandeau et onglets presents`).toBe(true);
          expect.soft(bt.overlaps, `${tag} chevauchement bandeau « Mon code ami » / onglets`).toEqual([]);
        }
      }
    }
    expect(fs.readdirSync(OUT).length).toBe(VARIANTS.length * SIZES.length);
  });

  test('Amis a 960x600 : cartes en haut, onglets dessous, la page defile verticalement sans defilement horizontal', async ({ appA }) => {
    const page = appA.page;
    await prepare(appA);
    await setWindowSize(page, 960, 600);
    await gotoScreen(page, byId('friends-add'));
    const pos = await page.evaluate(() => {
      const r = (q: string) => document.querySelector(q)!.getBoundingClientRect();
      const cards = [...document.querySelectorAll('.fr-card')].map((c) => c.getBoundingClientRect());
      const t = r('[role=tablist][aria-label="Amis"]');
      const p = r('[role=tabpanel]');
      const doc = document.documentElement;
      const scrollers = [...document.querySelectorAll('*')].filter((e) => /(auto|scroll)/.test(getComputedStyle(e).overflowX) && e.scrollWidth > e.clientWidth + 1).map((e) => e.className || e.tagName);
      return {
        cards: cards.map((c) => ({ top: c.top, bottom: c.bottom, left: c.left, right: c.right })),
        tabsTop: t.top, tabsBottom: t.bottom, panelTop: p.top, vw: innerWidth,
        hDoc: doc.scrollWidth - doc.clientWidth, hScrollers: scrollers,
      };
    });
    expect(pos.cards).toHaveLength(2);
    // deux cartes cote a cote, dans la fenetre
    expect(Math.abs(pos.cards[0].top - pos.cards[1].top)).toBeLessThan(2);
    expect(pos.cards[0].right).toBeLessThanOrEqual(pos.cards[1].left + 1);
    expect(pos.cards[1].right).toBeLessThanOrEqual(pos.vw + 1);
    // onglets sous les cartes, panneau sous les onglets
    expect(pos.tabsTop).toBeGreaterThanOrEqual(Math.max(pos.cards[0].bottom, pos.cards[1].bottom) - 0.5);
    expect(pos.panelTop).toBeGreaterThanOrEqual(pos.tabsBottom - 0.5);
    // aucun defilement horizontal (page ni conteneur)
    expect(pos.hDoc).toBeLessThanOrEqual(0);
    expect(pos.hScrollers).toEqual([]);
    // le contenu defile verticalement : le panneau devient atteignable
    await page.evaluate(() => { const c = document.querySelector('.content') as HTMLElement | null; if (c) c.scrollTop = c.scrollHeight; });
    const bottom = await page.evaluate(() => document.querySelector('[role=tabpanel]')!.getBoundingClientRect().top < innerHeight);
    expect(bottom).toBe(true);
  });
});
