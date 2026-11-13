// État de lancement partagé entre l'accueil et la page d'un modpack : un seul jeu à la fois, suivi par launch://state.
// Le magasin vit au niveau du module (un par `services`), il survit donc aux changements d'écran.
import { useCallback, useSyncExternalStore } from "react";
import type { LaunchState, Services } from "../services/types";

export interface LaunchSnapshot {
  /** Dernier état connu (préparation, en jeu, échec, fermé), null avant tout lancement. */
  launch: LaunchState | null;
  /** Échec de l'appel de lancement lui-même (instance introuvable…), effacé au prochain événement. */
  error: string | null;
  /** Début de la session en cours (ms), par instance en jeu. */
  since: Record<string, number>;
}

interface Store {
  snap: LaunchSnapshot;
  listeners: Set<() => void>;
}

const stores = new WeakMap<Services, Store>();

/** Retire le préfixe technique du backend (« introuvable : », « entrée invalide : »…). */
export const errMsg = (e: unknown) => (e instanceof Error ? e.message : String(e)).replace(/^[a-zéèêàçùîô' -]{3,24}\s:\s+/, "");

function storeFor(services: Services): Store {
  let s = stores.get(services);
  if (s) return s;
  const store: Store = { snap: { launch: null, error: null, since: {} }, listeners: new Set() };
  stores.set(services, store);
  services.onLaunchState((l) => {
    const since = { ...store.snap.since };
    if (l.state === "running") { if (!since[l.id]) since[l.id] = Date.now(); } else delete since[l.id];
    set(store, { launch: l, error: null, since });
  });
  s = store;
  return s;
}

function set(store: Store, snap: LaunchSnapshot) {
  store.snap = snap;
  store.listeners.forEach((f) => f());
}

/** Vrai si `id` est en préparation ou en jeu. */
export function isBusy(launch: LaunchState | null, id: string): boolean {
  return launch?.id === id && (launch.state === "preparing" || launch.state === "running");
}

export function useLaunch(services: Services) {
  const store = storeFor(services);
  const snap = useSyncExternalStore(
    (f) => { store.listeners.add(f); return () => { store.listeners.delete(f); }; },
    () => store.snap,
  );

  const play = useCallback(async (id: string) => {
    set(store, { ...store.snap, launch: { id, state: "preparing" }, error: null });
    try {
      await services.instancesLaunch(id);
    } catch (e) {
      set(store, { ...store.snap, launch: null, error: errMsg(e) });
    }
  }, [services, store]);

  /** Arrête le jeu (ou annule la préparation) ; renvoie le message d'erreur éventuel. */
  const kill = useCallback(async (id: string): Promise<string | null> => {
    try { await services.instancesKill(id); return null; } catch (e) { return errMsg(e); }
  }, [services]);

  /** Oublie l'état d'une instance supprimée. */
  const forget = useCallback((id: string) => {
    if (store.snap.launch?.id === id) set(store, { ...store.snap, launch: null, error: null });
  }, [store]);

  const clearError = useCallback(() => { if (store.snap.error) set(store, { ...store.snap, error: null }); }, [store]);

  return { ...snap, play, kill, forget, clearError, isBusy: (id: string) => isBusy(snap.launch, id) };
}
