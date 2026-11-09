import { useCallback, useEffect, useMemo, useRef, useState, type ComponentType } from "react";
import "./shell.css";
import { createServices, type Services } from "../services";
import { ShellContext, type ShellContextValue } from "./context";
import { navigate, useLocation, type RouteName } from "./router";
import { TitleBar } from "./TitleBar";

export { useShell, useServices, useScreenTitle } from "./context";
export { navigate, ROUTES, parseHash, type RouteName } from "./router";

export const ROUTE_LABELS: Record<RouteName, string> = {
  login: "Connexion", main: "Accueil", create: "Créer une instance", install: "Installation",
  join: "Rejoindre", friends: "Amis", settings: "Paramètres", error: "Erreur", legal: "Mentions légales",
};

// Dossier d'écran par route. Les écrans sont chargés sans être modifiés.
const SCREEN_DIRS: Record<RouteName, string> = {
  login: "Login", main: "Main", create: "Create", install: "Install", join: "Join",
  friends: "Friends", settings: "Settings", error: "Error", legal: "Legal",
};
type ScreenModule = { default?: ComponentType };
const screenModules = import.meta.glob<ScreenModule>("../screens/*/index.tsx", { eager: true });

export function screenFor(route: RouteName): ComponentType | null {
  const mod = screenModules[`../screens/${SCREEN_DIRS[route]}/index.tsx`];
  return typeof mod?.default === "function" ? mod.default : null;
}

function Fallback({ route }: { route: RouteName }) {
  return (
    <div role="status" style={{ padding: 24, color: "var(--text-dim)" }}>
      L'écran « {ROUTE_LABELS[route]} » n'est pas encore disponible.
    </div>
  );
}

export function Shell({ services }: { services?: Services } = {}) {
  const svc = useMemo(() => services ?? createServices(), [services]);
  const { route, params } = useLocation();
  const [title, setTitle] = useState<string | null>(null);
  const mainRef = useRef<HTMLElement>(null);
  const first = useRef(true);

  // Focus sur le contenu à chaque changement de route (pas au premier rendu).
  useEffect(() => {
    if (first.current) { first.current = false; return; }
    // Les effets des enfants s'exécutent avant celui-ci : si l'écran a déjà placé le focus
    // dans le contenu (titre, champ...), on ne le lui reprend pas.
    const main = mainRef.current;
    const active = document.activeElement;
    if (main && active !== main && main.contains(active)) return;
    main?.focus();
  }, [route]);

  const set = useCallback((t: string | null) => setTitle(t), []);
  const value = useMemo<ShellContextValue>(
    () => ({ services: svc, route, params, navigate, setTitle: set }),
    [svc, route, params, set],
  );
  const Screen = screenFor(route);

  return (
    <ShellContext.Provider value={value}>
      <div className="app">
        <a
          className="skip-link"
          href="#contenu"
          onClick={(e) => { e.preventDefault(); mainRef.current?.focus(); }}
        >
          Aller au contenu
        </a>
        <TitleBar context={title ?? ROUTE_LABELS[route]} />
        <main id="contenu" className="content" ref={mainRef} tabIndex={-1} data-route={route}>
          {Screen ? <Screen /> : <Fallback route={route} />}
        </main>
      </div>
    </ShellContext.Provider>
  );
}
