import { useCallback, useEffect, useState } from "react";
import type { JoinRequest, Services } from "../services/types";

/** Hôte : demandes à rejoindre en attente de décision, tenues à jour par friend://join_requests. */
export function useJoinRequests(services: Services): JoinRequest[] {
  const [list, setList] = useState<JoinRequest[]>([]);
  useEffect(() => {
    let alive = true;
    services.friendJoinRequests().then((l) => { if (alive) setList(l); }, () => {});
    const off = services.onFriendJoinRequests((l) => { if (alive) setList(l); });
    return () => { alive = false; off(); };
  }, [services]);
  return list;
}

/** État de ma demande à rejoindre auprès d'un ami. */
export type AskState = "pending" | "accepted" | "refused" | "expired";

/** Texte du bouton ou de la mention selon l'état de la demande. */
export const ASK_LABEL: Record<AskState, string> = {
  pending: "Demande envoyée…",
  accepted: "Demande acceptée",
  refused: "Demande refusée",
  expired: "Sans réponse",
};

/**
 * Invité : demandes envoyées aux amis (une par ami). Accepté : l'ami peut être rejoint ; l'état est oublié quand l'ami
 * se déconnecte (l'accès ne vaut que pour sa partie en cours). `ask` rejette avec le message de l'erreur.
 */
export function useJoinAsk(services: Services): { states: Record<string, AskState>; ask: (publicKey: string) => Promise<void> } {
  const [states, setStates] = useState<Record<string, AskState>>({});
  const set = useCallback((key: string, s: AskState | null) => {
    setStates((m) => {
      const next = { ...m };
      if (s) next[key] = s; else delete next[key];
      return next;
    });
  }, []);
  useEffect(() => services.onFriendPresence((e) => { if (e.presence !== "online") set(e.public_key, null); }), [services, set]);
  const ask = useCallback(async (key: string) => {
    set(key, "pending");
    try {
      set(key, await services.friendJoinRequest(key));
    } catch (e) {
      set(key, null);
      throw e;
    }
  }, [services, set]);
  return { states, ask };
}
