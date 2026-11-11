/** Couleur d'avatar d'une instance (pastille du rail, initiale). L'accent de l'interface reste UN SEUL cyan (tokens.css). */

export type RGB = [number, number, number];

export const INK_DARK = "#06222E";
export const INK_LIGHT = "#FFFFFF";

export function parseHex(h: string | null | undefined): RGB | null {
  const m = /^#([0-9a-f]{6})$/i.exec((h ?? "").trim());
  if (!m) return null;
  const n = parseInt(m[1], 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

export function toHex([r, g, b]: RGB): string {
  const c = (v: number) => Math.max(0, Math.min(255, Math.round(v))).toString(16).padStart(2, "0");
  return `#${c(r)}${c(g)}${c(b)}`.toUpperCase();
}

const chan = (v: number) => { const s = v / 255; return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4; };
export function luminance(hex: string): number {
  const [r, g, b] = parseHex(hex) ?? [0, 0, 0];
  return 0.2126 * chan(r) + 0.7152 * chan(g) + 0.0722 * chan(b);
}

/** Rapport de contraste WCAG entre deux couleurs #rrggbb (1 à 21). */
export function contrastRatio(a: string, b: string): number {
  const la = luminance(a), lb = luminance(b);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

/** Mélange linéaire : t=0 -> a, t=1 -> b. */
export function mix(a: string, b: string, t: number): string {
  const x = parseHex(a) ?? [0, 0, 0], y = parseHex(b) ?? [0, 0, 0];
  return toHex([x[0] + (y[0] - x[0]) * t, x[1] + (y[1] - x[1]) * t, x[2] + (y[2] - x[2]) * t]);
}

/** Éloigne `bg` de `fg` (assombrit si fg est plus clair, éclaircit sinon) jusqu'à un contraste >= `min`. */
export function ensureContrast(bg: string, fg: string, min = 4.5): string {
  if (contrastRatio(bg, fg) >= min) return bg.toUpperCase();
  const target = luminance(fg) > luminance(bg) ? "#000000" : "#FFFFFF";
  for (let t = 0.02; t <= 1.0001; t += 0.02) {
    const c = mix(bg, target, t);
    if (contrastRatio(c, fg) >= min) return c;
  }
  return target;
}

/** Encre (texte) la plus lisible sur `bg`. */
export function pickInk(bg: string): string {
  return contrastRatio(bg, INK_DARK) >= contrastRatio(bg, INK_LIGHT) ? INK_DARK : INK_LIGHT;
}

/** Teinte 0-359 stable, dérivée du nom (même nom -> même teinte). */
export function hashHue(name: string): number {
  let h = 2166136261;
  for (let i = 0; i < name.length; i++) { h ^= name.charCodeAt(i); h = Math.imul(h, 16777619); }
  return (h >>> 0) % 360;
}

export function hslToHex(h: number, s: number, l: number): string {
  const a = s * Math.min(l, 1 - l);
  const f = (n: number) => { const k = (n + h / 30) % 12; return l - a * Math.max(-1, Math.min(k - 3, 9 - k, 1)); };
  return toHex([f(0) * 255, f(8) * 255, f(4) * 255]);
}

export interface AvatarColors {
  /** Fond de la pastille. */
  bg: string;
  /** Texte (initiale) sur `bg`, contraste >= 4.5. */
  ink: string;
}

/** Couleur de l'avatar : `color` du modpack, sinon teinte stable issue du nom. */
export function avatarColors(color: string | null | undefined, name: string): AvatarColors {
  const rgb = parseHex(color);
  const base = rgb ? toHex(rgb) : hslToHex(hashHue(name), 0.5, 0.45);
  const bg = ensureContrast(base, pickInk(base), 4.5);
  return { bg, ink: pickInk(bg) };
}
