import { useEffect, useState } from "react";
import type { Services } from "../services/types";

/** Tête de skin (data URL) d'un joueur, retrouvée par pseudo ou par uuid de compte ; `null` tant qu'elle n'est pas là (ou sans réseau). */
export function useSkinHead(services: Services | undefined, by: "name" | "uuid" | null, id: string): string | null {
  const [src, setSrc] = useState<string | null>(null);
  useEffect(() => {
    setSrc(null);
    if (!services || !by) return;
    let alive = true;
    (by === "name" ? services.skinHeadByName(id) : services.authSkinHead(id)).then((s) => { if (alive) setSrc(s); }, () => {});
    return () => { alive = false; };
  }, [services, by, id]);
  return src;
}
