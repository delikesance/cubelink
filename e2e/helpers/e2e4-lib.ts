// Helpers E2E-4 (T7/T8/T9) : navigation, mesures DOM, arbre d'accessibilite CDP, contraste.
import type { Page } from '@playwright/test';
import { invoke, waitForRoute, loginOffline, makeHostInstance, type App } from '../harness/fixtures';

export type Screen = { id: string; hash: string; route: string; ready?: string };

const NO_FRIEND = '0000000000000000000000000000000000000000000000000000000000000000';

/** Ecrans testes. `ready` = selecteur CSS qui doit etre visible avant mesure. */
export const SCREENS: Screen[] = [
  { id: 'login', hash: '#/login', route: 'login', ready: 'button:has-text("Se connecter avec Microsoft")' },
  { id: 'main', hash: '#/main', route: 'main', ready: '.home-play' },
  { id: 'create', hash: '#/create', route: 'create', ready: 'main h1' },
  { id: 'friends-list', hash: '#/friends', route: 'friends', ready: '[role=tab]' },
  { id: 'friends-requests', hash: '#/friends', route: 'friends', ready: '[role=tab]' },
  { id: 'friends-add', hash: '#/friends', route: 'friends', ready: '[role=tab]' },
  { id: 'join', hash: `#/join?friend=${NO_FRIEND}`, route: 'join', ready: 'main h1' },
  { id: 'install', hash: '#/install?friend=abcd', route: 'install', ready: '.inst-card' },
  { id: 'settings', hash: '#/settings', route: 'settings', ready: '[role=tablist]' },
  { id: 'error', hash: '#/error?state=disk-full', route: 'error', ready: 'main h1' },
  { id: 'legal', hash: '#/legal', route: 'legal', ready: 'article' },
];

/** Connecte hors ligne et pose une instance hote (ecran d'accueil realiste). */
export async function prepare(app: App): Promise<void> {
  await waitForRoute(app.page, 'login');
  await loginOffline(app.page, 'Alex');
  await makeHostInstance(app);
  await app.page.reload();
  await waitForRoute(app.page, 'main');
}

export async function gotoScreen(page: Page, s: Screen): Promise<void> {
  await page.evaluate((h) => { window.location.hash = h; }, s.hash);
  await waitForRoute(page, s.route);
  if (s.ready) await page.locator(s.ready).first().waitFor({ state: 'visible', timeout: 15_000 });
  if (s.id === 'friends-requests') await page.getByRole('tab', { name: /Demandes/ }).click();
  await page.waitForTimeout(300); // fin des transitions/chargements
}

/** Redimensionne la VRAIE fenetre WebView2 (Browser.setWindowBounds) et attend innerWidth/innerHeight. */
export async function setWindowSize(page: Page, width: number, height: number): Promise<{ w: number; h: number }> {
  const cdp = await page.context().newCDPSession(page);
  try {
    const { windowId } = await cdp.send('Browser.getWindowForTarget');
    await cdp.send('Browser.setWindowBounds', { windowId, bounds: { windowState: 'normal', width, height } });
  } finally { await cdp.detach().catch(() => {}); }
  await page.waitForFunction(([w, h]) => innerWidth === w && innerHeight === h, [width, height], { timeout: 10_000 }).catch(() => {});
  await page.waitForTimeout(200);
  return page.evaluate(() => ({ w: innerWidth, h: innerHeight }));
}

/** Elements sans nom accessible, d'apres l'arbre d'accessibilite reel du navigateur. */
export async function unnamedControls(page: Page): Promise<string[]> {
  const cdp = await page.context().newCDPSession(page);
  try {
    await cdp.send('DOM.enable');
    await cdp.send('Accessibility.enable');
    const { nodes } = (await cdp.send('Accessibility.getFullAXTree')) as { nodes: any[] };
    const roles = new Set(['button', 'link', 'textbox', 'checkbox', 'radio', 'combobox', 'tab', 'switch', 'searchbox', 'spinbutton', 'slider', 'menuitem', 'listbox', 'MenuListPopup']);
    const out: string[] = [];
    for (const n of nodes) {
      if (n.ignored) continue;
      const role = n.role?.value;
      if (!roles.has(role)) continue;
      if ((n.name?.value ?? '').trim()) continue;
      let html = `backendDOMNodeId=${n.backendDOMNodeId}`;
      try {
        const { object } = await cdp.send('DOM.resolveNode', { backendNodeId: n.backendDOMNodeId });
        const r = await cdp.send('Runtime.callFunctionOn', { objectId: object.objectId, functionDeclaration: 'function(){return this.outerHTML.slice(0,140)}', returnByValue: true });
        html = String(r.result.value);
      } catch { /* garde l'id */ }
      out.push(`${role} sans nom : ${html}`);
    }
    return out;
  } finally { await cdp.detach().catch(() => {}); }
}

export interface FocusStep { desc: string; visible: boolean; focusOk: boolean; outline: string; inDom: number }

/** Appuie sur Tab jusqu'au retour au premier element / sortie du document ; renvoie les etapes. */
export async function tabThrough(page: Page, max = 60): Promise<{ steps: FocusStep[]; focusables: string[]; positiveTabindex: number; cycled: boolean; trapped: boolean }> {
  // Point de depart : le lien d'evitement (premier element du document).
  await page.evaluate(() => { window.scrollTo(0, 0); (document.querySelector('a.skip-link') as HTMLElement | null)?.focus(); });
  const describe = () => page.evaluate(() => {
    const SEL = 'a[href],button,input,select,textarea,summary,[tabindex]';
    const vis = (e: Element) => { const cd = e.closest('details:not([open])'); if (cd && e.tagName !== 'SUMMARY') return false; const r = (e as HTMLElement).getBoundingClientRect(); const cs = getComputedStyle(e); return r.width > 0 && r.height > 0 && cs.visibility !== 'hidden' && cs.display !== 'none'; };
    const all = [...document.querySelectorAll(SEL)].filter((e) => vis(e) && !(e as HTMLInputElement).disabled && (e as HTMLElement).tabIndex >= 0);
    const el = document.activeElement as HTMLElement | null;
    const label = (e: Element) => { const h = e as HTMLElement; return `${e.tagName.toLowerCase()}${h.getAttribute('role') ? '[' + h.getAttribute('role') + ']' : ''} "${(h.getAttribute('aria-label') || h.innerText || (h as HTMLInputElement).value || h.id || h.getAttribute('href') || '').trim().replace(/\s+/g, ' ').slice(0, 40)}"`; };
    const pos = all.filter((e) => (e as HTMLElement).tabIndex > 0).length;
    if (!el || el === document.body || el === document.documentElement) return { body: true, focusables: all.map(label), pos };
    const cs = getComputedStyle(el);
    const outline = `${cs.outlineStyle} ${cs.outlineWidth} ${cs.outlineColor} | ${cs.boxShadow}`;
    const focusOk = el.matches(':focus-visible') && ((cs.outlineStyle !== 'none' && parseFloat(cs.outlineWidth) >= 2 && cs.outlineColor === 'rgb(111, 211, 240)') || cs.boxShadow.includes('111, 211, 240'));
    const r = el.getBoundingClientRect();
    return { body: false, desc: label(el), visible: r.width > 0 && r.height > 0, focusOk, outline, inDom: all.indexOf(el), focusables: all.map(label), pos };
  });
  const steps: FocusStep[] = [];
  let focusables: string[] = [];
  let positive = 0;
  let cycled = false;
  let trapped = false;
  const seen = new Set<string>();
  for (let i = 0; i < max; i++) {
    await page.keyboard.press('Tab');
    const d: any = await describe();
    focusables = d.focusables; positive = d.pos;
    if (d.body) { cycled = true; break; }
    if (d.inDom === 0 && String(d.desc).includes('Aller au contenu')) { cycled = true; break; } // retour au point de depart
    const key = `${d.inDom}|${d.desc}`;
    if (seen.has(key)) { trapped = true; break; } // le focus tourne dans un sous-ensemble (piege ou dialogue modal)
    seen.add(key);
    steps.push({ desc: d.desc, visible: d.visible, focusOk: d.focusOk, outline: d.outline, inDom: d.inDom });
  }
  return { steps, focusables, positiveTabindex: positive, cycled, trapped };
}

export interface TargetIssue { desc: string; w: number; h: number }
/** Cibles interactives visibles de moins de `min` px (hors liens en ligne dans un texte). */
export async function smallTargets(page: Page, min = 44): Promise<TargetIssue[]> {
  return page.evaluate((m) => {
    const out: { desc: string; w: number; h: number }[] = [];
    const SEL = 'button,a[href],input:not([type=hidden]),select,textarea,summary,[role=tab],[role=button]';
    for (const e of document.querySelectorAll(SEL)) {
      const h = e as HTMLElement;
      const cs = getComputedStyle(h);
      let r = h.getBoundingClientRect();
      if (!r.width || !r.height || cs.visibility === 'hidden' || (h as HTMLInputElement).disabled) continue;
      const lab = h.closest('label');
      if (lab && h.tagName === 'INPUT') r = lab.getBoundingClientRect();
      if (h.tagName === 'A') {
        const p = h.parentElement;
        if (cs.display === 'inline' && p && (p.textContent ?? '').trim().length > (h.textContent ?? '').trim().length + 3) continue;
      }
      if (r.height < m - 0.5 || r.width < m - 0.5) {
        out.push({ desc: `${h.tagName.toLowerCase()}.${(h.className || '').toString().slice(0, 30)} "${(h.getAttribute('aria-label') || h.innerText || '').trim().slice(0, 30)}"`, w: Math.round(r.width), h: Math.round(r.height) });
      }
    }
    return out;
  }, min);
}

export interface ContrastIssue { desc: string; ratio: number; need: number; fg: string; bg: string }
/** Contraste texte/fond sur les elements-texte visibles (fonds image ignores et comptes dans `skipped`). */
export async function contrastIssues(page: Page, limit = 80): Promise<{ checked: number; skipped: number; issues: ContrastIssue[] }> {
  return page.evaluate((lim) => {
    const cv = document.createElement('canvas'); cv.width = cv.height = 1;
    const ctx = cv.getContext('2d', { willReadFrequently: true })!;
    const rgba = (c: string): number[] => {
      ctx.clearRect(0, 0, 1, 1); ctx.fillStyle = '#000'; ctx.fillStyle = c; ctx.fillRect(0, 0, 1, 1);
      const d = ctx.getImageData(0, 0, 1, 1).data; return [d[0], d[1], d[2], d[3] / 255];
    };
    const over = (f: number[], b: number[]) => { const a = f[3]; return [f[0] * a + b[0] * (1 - a), f[1] * a + b[1] * (1 - a), f[2] * a + b[2] * (1 - a), 1]; };
    const lum = (c: number[]) => { const f = (v: number) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; }; return 0.2126 * f(c[0]) + 0.7152 * f(c[1]) + 0.0722 * f(c[2]); };
    const issues: any[] = []; let checked = 0, skipped = 0;
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    const seen = new Set<Element>();
    for (let n = walker.nextNode(); n; n = walker.nextNode()) {
      if (!(n.textContent ?? '').trim()) continue;
      const el = n.parentElement!; if (seen.has(el)) continue; seen.add(el);
      const cs = getComputedStyle(el); const r = el.getBoundingClientRect();
      if (!r.width || !r.height || cs.visibility === 'hidden' || +cs.opacity === 0) continue;
      if (el.closest('[hidden],[aria-hidden=true]')) continue;
      if (r.bottom < 0 || r.top > innerHeight || r.right < 0 || r.left > innerWidth) continue;
      const layers: number[][] = []; let indeterminate = false;
      for (let p: Element | null = el; p; p = p.parentElement) {
        const pcs = getComputedStyle(p);
        if (pcs.backgroundImage !== 'none') { indeterminate = true; break; }
        const c = rgba(pcs.backgroundColor); if (c[3] > 0) { layers.push(c); if (c[3] >= 1) break; }
      }
      const mid = document.elementsFromPoint(r.left + r.width / 2, r.top + r.height / 2);
      if (mid.some((m) => m.tagName === 'IMG' || m.tagName === 'VIDEO' || m.tagName === 'CANVAS')) indeterminate = true;
      if (indeterminate) { skipped++; continue; }
      let bg: number[] = [10, 10, 10, 1];
      for (let i = layers.length - 1; i >= 0; i--) bg = over(layers[i], bg);
      const fg = over(rgba(cs.color), bg);
      const l1 = lum(fg), l2 = lum(bg); const ratio = (Math.max(l1, l2) + 0.05) / (Math.min(l1, l2) + 0.05);
      const size = parseFloat(cs.fontSize); const bold = parseInt(cs.fontWeight) >= 700;
      const need = size >= 24 || (bold && size >= 18.66) ? 3 : 4.5;
      checked++;
      if (ratio < need) issues.push({ desc: `${el.tagName.toLowerCase()}.${(el.className || '').toString().slice(0, 30)} "${(n.textContent ?? '').trim().slice(0, 30)}"`, ratio: Math.round(ratio * 100) / 100, need, fg: `rgb(${fg.slice(0, 3).map(Math.round)})`, bg: `rgb(${bg.slice(0, 3).map(Math.round)})` });
      if (checked >= lim) break;
    }
    return { checked, skipped, issues };
  }, limit);
}

export { invoke };
