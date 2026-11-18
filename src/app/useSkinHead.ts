import { useEffect, useState } from "react";
import type { Services } from "../services/types";

/** Délais (ms) des nouvelles tentatives quand la tête n'est pas là : réseau pas encore prêt au démarrage de la session, Mojang qui limite le débit. */
export const SKIN_RETRY_MS = [4_000, 15_000, 60_000, 180_000];

/** Tête de skin (data URL) d'un joueur, retrouvée par pseudo ou par uuid de compte ; `null` tant qu'elle n'est pas là (ou sans réseau : on réessaie quelques fois). */
export function useSkinHead(services: Services | undefined, by: "name" | "uuid" | null, id: string): string | null {
  const [src, setSrc] = useState<string | null>(null);
  useEffect(() => {
    setSrc(null);
    if (!services || !by) return;
    let alive = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const load = (attempt: number) => {
      (by === "name" ? services.skinHeadByName(id) : services.authSkinHead(id)).then(
        (s) => {
          if (!alive) return;
          if (s) setSrc(s);
          else if (attempt < SKIN_RETRY_MS.length) timer = setTimeout(() => load(attempt + 1), SKIN_RETRY_MS[attempt]);
        },
        () => { if (alive && attempt < SKIN_RETRY_MS.length) timer = setTimeout(() => load(attempt + 1), SKIN_RETRY_MS[attempt]); },
      );
    };
    load(0);
    return () => { alive = false; if (timer) clearTimeout(timer); };
  }, [services, by, id]);
  return src;
}
