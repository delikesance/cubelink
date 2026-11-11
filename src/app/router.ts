import { useEffect, useState, useSyncExternalStore } from "react";

export const ROUTES = [
  "login", "main", "create", "install", "join", "friends", "settings", "error", "legal", "browse", "overlay",
] as const;
export type RouteName = (typeof ROUTES)[number];
export const DEFAULT_ROUTE: RouteName = "login";

export interface Location {
  route: RouteName;
  params: Record<string, string>;
}

/** Lit un hash du type `#/main?id=1` ; une route inconnue ou vide donne la route par défaut. */
export function parseHash(hash: string): Location {
  const raw = hash.replace(/^#\/?/, "");
  const [path, query = ""] = raw.split("?");
  const name = path.split("/")[0].toLowerCase();
  const route = (ROUTES as readonly string[]).includes(name) ? (name as RouteName) : DEFAULT_ROUTE;
  const params: Record<string, string> = {};
  new URLSearchParams(query).forEach((v, k) => { params[k] = v; });
  return { route, params };
}

export function buildHash(route: RouteName, params?: Record<string, string>): string {
  const q = params && Object.keys(params).length ? `?${new URLSearchParams(params).toString()}` : "";
  return `#/${route}${q}`;
}

export function navigate(route: RouteName, params?: Record<string, string>): void {
  const h = buildHash(route, params);
  if (window.location.hash === h) return;
  window.location.hash = h;
}

// ---------------------------------------------------------------------------
// Historique interne (pile en mémoire) : alimenté à chaque changement de hash.
// ---------------------------------------------------------------------------

const HISTORY_MAX = 50;
/** Routes qui ne sont jamais une cible de retour. */
const NO_BACK: readonly RouteName[] = ["login", "error", "overlay"];

interface Entry { hash: string; focus: string | null }

let stack: Entry[] = [];
let pendingFocus: string | null = null;
const listeners = new Set<() => void>();

function norm(hash: string): string {
  const l = parseHash(hash);
  return buildHash(l.route, l.params);
}

/** Sélecteur CSS permettant de retrouver l'élément focalisé (id ou aria-label), sinon null. */
function focusSelector(): string | null {
  if (typeof document === "undefined") return null;
  const el = document.activeElement as HTMLElement | null;
  if (!el || el === document.body || !el.closest("main")) return null;
  if (el.id) return `[id="${el.id.replace(/"/g, '\\"')}"]`;
  const label = el.getAttribute("aria-label");
  return label ? `[aria-label="${label.replace(/"/g, '\\"')}"]` : null;
}

function emit(): void { listeners.forEach((l) => l()); }

/** Enregistre la location courante dans la pile (idempotent). */
function sync(): void {
  if (typeof window === "undefined") return;
  const h = norm(window.location.hash);
  const top = stack[stack.length - 1];
  if (top && top.hash === h) return;
  if (stack.length >= 2 && stack[stack.length - 2].hash === h) {
    stack.pop(); // retour vers la route précédente : pop plutôt que push (pas de boucle)
  } else {
    if (top) top.focus = focusSelector();
    stack.push({ hash: h, focus: null });
    if (stack.length > HISTORY_MAX) stack = stack.slice(stack.length - HISTORY_MAX);
  }
  emit();
}

if (typeof window !== "undefined") {
  window.addEventListener("hashchange", sync);
  sync();
}

function backIndex(): number {
  sync();
  for (let i = stack.length - 2; i >= 0; i--) {
    if (!NO_BACK.includes(parseHash(stack[i].hash).route)) return i;
  }
  return -1;
}

export function canGoBack(): boolean { return backIndex() >= 0; }

export function goBack(fallback?: RouteName): void {
  const i = backIndex();
  if (i < 0) { navigate(fallback ?? "main"); return; }
  const target = stack[i];
  stack = stack.slice(0, i + 1);
  pendingFocus = target.focus;
  window.location.hash = target.hash;
  emit();
}

/** Sélecteur du focus à restaurer après un goBack (consommé une seule fois par le Shell). */
export function takePendingFocus(): string | null {
  const f = pendingFocus;
  pendingFocus = null;
  return f;
}

/** Réinitialise la pile (tests). */
export function resetHistory(): void {
  stack = [];
  pendingFocus = null;
  sync();
  emit();
}

export function useCanGoBack(): boolean {
  return useSyncExternalStore(
    (cb) => { listeners.add(cb); return () => { listeners.delete(cb); }; },
    canGoBack,
    () => false,
  );
}

export function useLocation(): Location {
  const [loc, setLoc] = useState<Location>(() => parseHash(window.location.hash));
  useEffect(() => {
    const on = () => setLoc(parseHash(window.location.hash));
    window.addEventListener("hashchange", on);
    on();
    return () => window.removeEventListener("hashchange", on);
  }, []);
  return loc;
}
