import { useEffect, useState } from "react";

export const ROUTES = [
  "login", "main", "create", "install", "join", "friends", "settings", "error", "legal",
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
