import { lazy, Suspense, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ComponentType } from "react";
import "./shell.css";
import { createServices, type Services } from "../services";
import { ShellContext, type ShellContextValue } from "./context";
import { navigate, goBack, parseHash, takePendingFocus, useCanGoBack, useLocation, type RouteName } from "./router";
import { TitleBar } from "./TitleBar";
import { UpdateBanner } from "./UpdateBanner";
import { StageNotice } from "./StageNotice";
import { useContextMenu, useSuppressNativeMenu } from "./useContextMenu";
import { openExternal } from "./externalLinks";
import { useStage, useStageFullscreen, useStageLayout } from "./stage";
import InGamePanel from "../screens/Overlay";
import "../screens/Overlay/overlay.css";

export { useShell, useServices, useScreenTitle, useBack } from "./context";
export { navigate, goBack, canGoBack, useCanGoBack, ROUTES, parseHash, type RouteName } from "./router";

export const ROUTE_LABELS: Record<RouteName, string> = {
  login: "Connexion", main: "Accueil", create: "Créer une instance", install: "Installation",
  instance: "Modpack", join: "Rejoindre", settings: "Paramètres", error: "Erreur", legal: "Mentions légales", browse: "Parcourir",
};

// Dossier d'écran par route. Les écrans sont chargés sans être modifiés.
const SCREEN_DIRS: Record<RouteName, string> = {
  login: "Login", main: "Main", create: "Create", install: "Install", join: "Join",
  instance: "Instance", settings: "Settings", error: "Error", legal: "Legal", browse: "Browse",
};
type ScreenModule = { default?: ComponentType };
// Accueil et connexion : chargés tout de suite ; les autres écrans le sont à la demande (code splitting).
const eagerModules = import.meta.glob<ScreenModule>("../screens/{Main,Login}/index.tsx", { eager: true });
const lazyModules = import.meta.glob<ScreenModule>(["../screens/*/index.tsx", "!../screens/Main/index.tsx", "!../screens/Login/index.tsx", "!../screens/Overlay/index.tsx"]);
const lazyCache = new Map<RouteName, ComponentType>();
const loaded = new Map<RouteName, ComponentType>();

/** Précharge tous les écrans (utilisé par les tests pour un rendu synchrone). */
export async function preloadScreens(): Promise<void> {
  await Promise.all((Object.keys(SCREEN_DIRS) as RouteName[]).map(async (route) => {
    const loader = lazyModules[`../screens/${SCREEN_DIRS[route]}/index.tsx`];
    if (!loader) return;
    const m = await loader();
    if (typeof m.default === "function") loaded.set(route, m.default);
  }));
}

export function screenFor(route: RouteName): ComponentType | null {
  const key = `../screens/${SCREEN_DIRS[route]}/index.tsx`;
  const mod = eagerModules[key];
  if (mod) return typeof mod.default === "function" ? mod.default : null;
  const loader = lazyModules[key];
  if (!loader) return null;
  const ready = loaded.get(route);
  if (ready) return ready;
  let Comp = lazyCache.get(route);
  if (!Comp) {
    const Lazy = lazy(async () => {
      const m = await loader();
      if (typeof m.default === "function") loaded.set(route, m.default);
      return { default: typeof m.default === "function" ? m.default : () => <Fallback route={route} /> };
    });
    Comp = function LazyScreen() {
      return <Suspense fallback={<div role="status" aria-busy="true" style={{ padding: 24, color: "var(--muted)" }}>Chargement…</div>}><Lazy /></Suspense>;
    };
    lazyCache.set(route, Comp);
  }
  return Comp;
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

  // Démarrage : arrivé sur « login » sans l'avoir demandé (hash vide ou par défaut), on regarde
  // d'abord si une session est conservée. Une arrivée volontaire (#/login, déconnexion) n'est pas contrôlée.
  const [booting, setBooting] = useState(
    () => parseHash(window.location.hash).route === "login" && !/^#\/?login/i.test(window.location.hash),
  );
  useEffect(() => {
    if (!booting) return;
    let on = true;
    void (async () => {
      let profile = null;
      try { profile = await svc.authStatus(); } catch { /* traité comme l'absence de session */ }
      if (!on) return;
      if (profile) { navigate("main"); setBooting(false); return; }
      let existed = false;
      try { existed = (await svc.authAccounts()).accounts.length > 0; } catch { /* sans liste : pas de message */ }
      if (!on) return;
      if (existed) navigate("login", { notice: "expired" });
      setBooting(false);
    })();
    return () => { on = false; };
  }, [booting, svc]);

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
  useSuppressNativeMenu();
  // Clic droit sur un lien web (descriptions, mentions légales…) quand l'écran ne propose pas son propre menu.
  const linkMenu = useContextMenu();
  const onLinkMenu = (e: React.MouseEvent<HTMLElement>) => {
    const a = (e.target as Element | null)?.closest?.("a[href]") as HTMLAnchorElement | null;
    if (!a || !/^https?:\/\//i.test(a.href)) return;
    const url = a.href;
    linkMenu.bind("Lien", [
      { key: "open", label: "Ouvrir dans le navigateur", onSelect: () => openExternal(url) },
      { key: "copy", label: "Copier le lien", onSelect: () => { void navigator.clipboard?.writeText(url).catch(() => {}); } },
    ])(e);
  };
  const set = useCallback((t: string | null) => setTitle(t), []);
  const value = useMemo<ShellContextValue>(
    () => ({ services: svc, route, params, navigate, goBack, canGoBack: canBack, setTitle: set }),
    [svc, route, params, canBack, set],
  );
  const Screen = screenFor(route);
  const stage = useStage(svc);
  // Zone de jeu : l'élément réservé est mesuré et son rectangle publié à Rust.
  const slotRef = useRef<HTMLDivElement>(null);
  useStageLayout(svc, slotRef);
  const fullscreen = useStageFullscreen(svc, stage);

  // Échap : ferme le panneau « en jeu » et rend le clavier au jeu (la confirmation du panneau gère sa propre touche Échap avant).
  useEffect(() => {
    if (!stage.panel) return;
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape" && !e.defaultPrevented) { e.preventDefault(); void svc.stagePanel(false).catch(() => {}); } };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [svc, stage.panel]);

  return (
    <ShellContext.Provider value={value}>
      <div className="app" data-fullscreen={fullscreen || undefined}>
        <a
          className="skip-link"
          href="#contenu"
          onClick={(e) => { e.preventDefault(); mainRef.current?.focus(); }}
        >
          Aller au contenu
        </a>
        <TitleBar context={title ?? ROUTE_LABELS[route]} stage={stage} />
        <main id="contenu" className="content" ref={mainRef} tabIndex={-1} data-route={route} onContextMenu={onLinkMenu}>
          {route === "login" && booting
            ? <div className="boot" role="status">Chargement…</div>
            : Screen ? <Screen /> : <Fallback route={route} />}
        </main>
        {linkMenu.menu}
        <UpdateBanner services={svc} />
        <StageNotice services={svc} />
        <div className="stage-slot" ref={slotRef} aria-hidden="true" />
        {/* Monté (et déjà dessiné) sous la fenêtre du jeu tant que le jeu est affiché : la découpe animée de la fenêtre du jeu
            le révèle, sans flash. Fermé : inerte (ni focus ni clic). */}
        {stage.attached && stage.game && (
          <div className="ov-dock" data-open={stage.panel || undefined} {...(stage.panel ? {} : { inert: "" })}>
            <InGamePanel open={stage.panel} />
          </div>
        )}
      </div>
    </ShellContext.Provider>
  );
}
