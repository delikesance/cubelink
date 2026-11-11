import { createContext, useCallback, useContext, useEffect } from "react";
import type { Services } from "../services";
import { navigate, goBack, type RouteName } from "./router";

export interface ShellContextValue {
  services: Services;
  route: RouteName;
  params: Record<string, string>;
  navigate: typeof navigate;
  goBack: typeof goBack;
  canGoBack: boolean;
  /** Définit le contexte affiché après « Cubelink · » ; null revient au libellé de la route. */
  setTitle: (title: string | null) => void;
}

export const ShellContext = createContext<ShellContextValue | null>(null);

export function useShell(): ShellContextValue {
  const v = useContext(ShellContext);
  if (!v) throw new Error("useShell doit être utilisé dans <Shell>.");
  return v;
}

export function useServices(): Services {
  return useShell().services;
}

/** Callback stable qui appelle goBack(fallback) (typiquement pour Échap). */
export function useBack(fallback?: RouteName): () => void {
  const { goBack: back } = useShell();
  return useCallback(() => back(fallback), [back, fallback]);
}

/** Les écrans appellent ce hook pour fixer leur titre contextuel. */
export function useScreenTitle(title: string | null): void {
  const { setTitle } = useShell();
  useEffect(() => {
    setTitle(title);
    return () => setTitle(null);
  }, [title, setTitle]);
}
