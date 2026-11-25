import { useEffect, useSyncExternalStore } from "react";
import type { Services } from "../services/types";

/**
 * Demande d'ami que j'ai envoyée et dont j'attends la réponse. Petit magasin partagé : la réponse (acceptation,
 * refus, expiration) peut arriver quand « Ajouter un ami » n'est plus à l'écran ; elle est alors retenue et
 * montrée au retour sur l'écran (avec le bouton « Annuler » tant que la demande attend).
 *
 * Une seule demande est suivie à la fois : l'interface ne connaît que le code envoyé, pas la clé publique portée par
 * les événements, et « Ajouter un ami » refuse d'en envoyer une autre tant que celle-ci attend (il faut l'annuler).
 * Les événements reçus sans demande en attente (annulée, ou déjà répondue) sont donc ignorés.
 */
export type Outgoing =
  | { step: "sent"; verify: string; code: string }
  | { step: "answered"; kind: "accepted" | "refused" | "expired"; name?: string };

let current: Outgoing | null = null;
const listeners = new Set<() => void>();

function set(next: Outgoing | null): void {
  current = next;
  listeners.forEach((l) => l());
}

/** Une demande vient d'être envoyée : elle attend la réponse. */
export function trackOutgoing(verify: string, code: string): void {
  set({ step: "sent", verify, code });
}

/** Oublie la demande suivie (annulée, ou message de réponse écarté). */
export function clearOutgoing(): void {
  if (current !== null) set(null);
}

function answer(kind: "accepted" | "refused" | "expired", name?: string): void {
  if (current?.step === "sent") set({ step: "answered", kind, name });
}

/** La demande suivie (re-rendu à chaque changement). */
export function useOutgoing(): Outgoing | null {
  return useSyncExternalStore(
    (cb) => { listeners.add(cb); return () => { listeners.delete(cb); }; },
    () => current,
    () => null,
  );
}

/**
 * Écoute les réponses du pair. Appelé par le Shell (toujours monté, donc aussi hors de « Ajouter un ami ») et par
 * l'écran lui-même : les deux abonnements sont sans danger, `answer` ne change rien une fois la réponse reçue.
 */
export function useOutgoingIntake(services: Services): void {
  useEffect(() => {
    const offs = [
      services.onFriendAccepted(({ name }) => answer("accepted", name)),
      services.onFriendRefused(() => answer("refused")),
      services.onFriendRequestExpired(() => answer("expired")),
    ];
    return () => offs.forEach((off) => off());
  }, [services]);
}

/** Test : oublie tout. */
export function resetOutgoing(): void {
  set(null);
}
