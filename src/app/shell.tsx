import { useCallback, useEffect, useMemo, useRef, useState, type ComponentType } from "react";
import "./shell.css";
import { createServices, type Services } from "../services";
import { ShellContext, type ShellContextValue } from "./context";
import { navigate, goBack, takePendingFocus, useCanGoBack, useLocation, type RouteName } from "./router";
import { TitleBar } from "./TitleBar";
import { UpdateBanner } from "./UpdateBanner";

export { useShell, useServices, useScreenTitle, useBack } from "./context";
export { navigate, goBack, canGoBack, useCanGoBack, ROUTES, parseHash, type RouteName } from "./router";

export const ROUTE_LABELS: Record<RouteName, string> = {
  login: "Connexion", main: "Accueil", create: "Créer une instance", install: "Installation",
  join: "Rejoindre", friends: "Amis", settings: "Paramètres", error: "Erreur", legal: "Mentions légales", browse: "Parcourir",
};

// Dossier d'écran par route. Les écrans sont chargés sans être modifiés.
const SCREEN_DIRS: Record<RouteName, string> = {
  login: "Login", main: "Main", create: "Create", install: "Install", join: "Join",
  friends: "Friends", settings: "Settings", error: "Error", legal: "Legal", browse: "Browse",
};
type ScreenModule = { default?: ComponentType };
const screenModules = import.meta.glob<ScreenModule>("../screens/*/index.tsx", { eager: true });

export function screenFor(route: RouteName): ComponentType | null {
  const mod = screenModules[`../screens/${SCREEN_DIRS[route]}/index.tsx`];
  return typeof mod?.default === "function" ? mod.default : null;
}

function Fallback({ route }: { route: RouteName }) {
  return (
    <div role="status" style={{ padding: 24, color: "var(--muted)" }}>
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
    // Retour : tente de redonner le focus à l'élément qui avait déclenché la navigation.
    const sel = takePendingFocus();
    if (main && sel) {
      try {
        const el = main.querySelector<HTMLElement>(sel);
        if (el) { el.focus(); if (document.activeElement === el) return; }
      } catch { /* sélecteur invalide : on ignore */ }
    }
    const active = document.activeElement;
    if (main && active !== main && main.contains(active)) return;
    main?.focus();
  }, [route]);

  // Raccourcis globaux : Alt+← et bouton « précédent » de la souris.
  useEffect(() => {
    const modalOpen = () => document.querySelector('[aria-modal="true"]') !== null;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "ArrowLeft" || !e.altKey || e.ctrlKey || e.metaKey || e.shiftKey) return;
      if (modalOpen()) return;
      const t = document.activeElement as HTMLElement | null;
      if (t && (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName))) return;
      e.preventDefault();
      goBack();
    };
    const onMouse = (e: MouseEvent) => {
      if (e.button !== 3 || modalOpen()) return;
      e.preventDefault();
      goBack();
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("mouseup", onMouse);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("mouseup", onMouse);
    };
  }, []);

  const canBack = useCanGoBack();
  const set = useCallback((t: string | null) => setTitle(t), []);
  const value = useMemo<ShellContextValue>(
    () => ({ services: svc, route, params, navigate, goBack, canGoBack: canBack, setTitle: set }),
    [svc, route, params, canBack, set],
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
        <UpdateBanner services={svc} />
        <main id="contenu" className="content" ref={mainRef} tabIndex={-1} data-route={route}>
          {Screen ? <Screen /> : <Fallback route={route} />}
        </main>
      </div>
    </ShellContext.Provider>
  );
}
