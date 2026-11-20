import { useEffect, useSyncExternalStore } from "react";
import type { Services } from "../services/types";

/**
 * Lien ami `cubelink://add/…` ouvert depuis le système, en attente d'être repris par « Ajouter un ami ».
 * Petit magasin partagé : le lien arrive dans le Shell, mais « Ajouter un ami » (Paramètres > Amis) peut ne pas
 * exister encore (autre écran, autre onglet). Il n'est jamais envoyé tout seul : il remplit seulement le champ.
 */
let pending: string | null = null;
const listeners = new Set<() => void>();

function notify(): void { listeners.forEach((l) => l()); }

/** Met un lien en attente (remplace le précédent). */
export function offerFriendLink(link: string): void {
  pending = link;
  notify();
}

/** Reprend le lien en attente (une seule fois) ; `null` si rien n'attend. */
export function takeFriendLink(): string | null {
  const l = pending;
  if (l !== null) { pending = null; notify(); }
  return l;
}

/** Test : oublie le lien en attente. */
export function resetFriendLink(): void {
  pending = null;
  notify();
}

/** Le lien en attente (re-rendu à chaque changement). */
export function usePendingFriendLink(): string | null {
  return useSyncExternalStore(
    (cb) => { listeners.add(cb); return () => { listeners.delete(cb); }; },
    () => pending,
    () => null,
  );
}

/**
 * Shell : reprend les liens du système, une fois au montage (lien passé au lancement, avant que l'écran écoute) puis à
 * chaque événement `friend://link` (Cubelink déjà ouvert).
 */
export function useFriendLinkIntake(services: Services): void {
  useEffect(() => {
    let alive = true;
    const pull = async () => {
      try {
        const link = await services.friendTakePendingLink();
        if (alive && link) offerFriendLink(link);
      } catch { /* sans lien : rien à faire */ }
    };
    void pull();
    const off = services.onFriendLink(() => { void pull(); });
    return () => { alive = false; off(); };
  }, [services]);
}
