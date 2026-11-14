import { useCallback, useEffect, useState } from "react";
import type { HearthCard, HearthDetail, HearthInvite, Services } from "../services/types";

/** Cartes des serveurs de groupe, tenues à jour par hearth://state (liste complète à chaque changement). `null` : pas encore chargé. */
export function useHearthCards(services: Services): HearthCard[] | null {
  const [cards, setCards] = useState<HearthCard[] | null>(null);
  useEffect(() => {
    let alive = true;
    services.hearthList().then((l) => { if (alive) setCards(l); }, () => { if (alive) setCards([]); });
    const off = services.onHearthState((l) => { if (alive) setCards(l); });
    return () => { alive = false; off(); };
  }, [services]);
  return cards;
}

/** Invitations de groupe en attente (hearth://invites). */
export function useHearthInvites(services: Services): HearthInvite[] {
  const [list, setList] = useState<HearthInvite[]>([]);
  useEffect(() => {
    let alive = true;
    services.hearthInvites().then((l) => { if (alive) setList(l); }, () => {});
    const off = services.onHearthInvites((l) => { if (alive) setList(l); });
    return () => { alive = false; off(); };
  }, [services]);
  return list;
}

export interface HearthDetailState {
  detail: HearthDetail | null;
  /** Message d'erreur de chargement (groupe quitté ou introuvable). */
  error: string | null;
  reload: () => void;
}

/** Page d'un serveur : rechargée au montage, à chaque changement d'état du groupe et à la demande. */
export function useHearthDetail(services: Services, id: string | undefined): HearthDetailState {
  const [detail, setDetail] = useState<HearthDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [tick, setTick] = useState(0);
  const reload = useCallback(() => setTick((n) => n + 1), []);
  useEffect(() => {
    if (!id) return;
    let alive = true;
    services.hearthDetail(id).then(
      (d) => { if (alive) { setDetail(d); setError(null); } },
      (e: unknown) => { if (alive) { setDetail(null); setError(e instanceof Error ? e.message : String(e)); } },
    );
    return () => { alive = false; };
  }, [services, id, tick]);
  useEffect(() => (id ? services.onHearthState(() => reload()) : undefined), [services, id, reload]);
  return { detail, error, reload };
}
