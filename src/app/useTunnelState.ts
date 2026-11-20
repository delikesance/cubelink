import { useCallback, useEffect, useRef, useState } from "react";
import type { Services } from "../services/types";

/** Liaison à signaler au joueur : `ok` et `closed` n'ont rien à afficher. */
export type TunnelProblem = "reconnecting" | "lost";

export interface TunnelNoticeState {
  problem: TunnelProblem;
  /** Pseudo de l'hôte si on le connaît (liste d'amis), sinon null : le message reste générique. */
  name: string | null;
}

/** Phrase du bandeau ; sans nom, « ton ami » (jamais la clé publique). */
export function tunnelMessage(problem: TunnelProblem, name: string | null): string {
  const who = name ?? "ton ami";
  return problem === "reconnecting"
    ? `Connexion à ${who} perdue, reconnexion… Le jeu peut se reconnecter sans être relancé.`
    : `${name ?? "Ton ami"} a fermé son monde. Relance la partie pour le rejoindre.`;
}

/**
 * Invité : liaison du tunnel vers l'hôte, tenue à jour par tunnel://state. `reconnecting` se retire au retour de `ok`
 * ou à la fermeture du tunnel (`closed`) ; `lost` est terminal côté Rust (aucun `ok` ensuite) : il reste jusqu'à
 * `dismiss` ou `closed`. Pas de lecture initiale : l'événement n'est émis qu'aux changements.
 */
export function useTunnelState(services: Services): { notice: TunnelNoticeState | null; dismiss: () => void } {
  const [notice, setNotice] = useState<TunnelNoticeState | null>(null);
  // Pseudos déjà résolus par clé, et hôte du dernier événement : la résolution asynchrone ne doit pas écraser un état plus récent.
  const names = useRef(new Map<string, string>());
  const hostKey = useRef<string | null>(null);
  useEffect(() => {
    let alive = true;
    const off = services.onTunnelState((e) => {
      if (!alive) return;
      if (e.state !== "reconnecting" && e.state !== "lost") {
        hostKey.current = null;
        setNotice(null);
        return;
      }
      const key = e.friend_public_key ?? null;
      hostKey.current = key;
      setNotice({ problem: e.state, name: key ? names.current.get(key) ?? null : null });
      if (!key || names.current.has(key)) return;
      services.friendList().then((list) => {
        const name = list.find((f) => f.friend.public_key === key)?.friend.name;
        if (!alive || !name) return;
        names.current.set(key, name);
        if (hostKey.current === key) setNotice((n) => (n ? { ...n, name } : n));
      }, () => {});
    });
    return () => { alive = false; off(); };
  }, [services]);
  const dismiss = useCallback(() => { hostKey.current = null; setNotice(null); }, []);
  return { notice, dismiss };
}
