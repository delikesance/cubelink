import { useEffect, useState } from "react";
import type { HostGuest, Services } from "../services/types";

const TICK_MS = 1000;

/** Vrai tant que l'invité n'est ni entré ni échu : l'accès existe encore, ou n'a plus de limite de temps. */
const isLive = (g: HostGuest, nowSec: number) => g.entered || g.expires_at == null || g.expires_at > nowSec;

/** Clés des amis dont l'invitation attend encore d'être rejoignée (ni entrés, ni échus). */
export function pendingInviteKeys(guests: HostGuest[], nowSec: number): Set<string> {
  return new Set(guests.filter((g) => g.origin === "invite" && !g.entered && g.expires_at != null && g.expires_at > nowSec).map((g) => g.public_key));
}

/** Temps restant : minutes arrondies au-dessus (« 4 min »), « < 1 min » sous 60 s. */
export function remainingText(expiresAt: number, nowSec: number): string {
  const left = expiresAt - nowSec;
  return left < 60 ? "< 1 min" : `${Math.ceil(left / 60)} min`;
}

/** Libellé d'un bouton Inviter désactivé : « Invité · 4 min ». */
export function invitedLabel(expiresAt: number, nowSec: number): string {
  return `Invité · ${remainingText(expiresAt, nowSec)}`;
}

/**
 * Hôte : amis autorisés à rejoindre mon monde, tenus à jour par host://guests. Aucun événement n'est émis à l'échéance :
 * un minuteur d'une seconde masque les lignes échues et fait défiler le temps restant (`nowSec`).
 */
export function useHostGuests(services: Services): { guests: HostGuest[]; nowSec: number } {
  const [all, setAll] = useState<HostGuest[]>([]);
  const [nowSec, setNowSec] = useState(() => Math.floor(Date.now() / 1000));
  useEffect(() => {
    let alive = true;
    services.hostGuests().then((l) => { if (alive) setAll(l); }, () => {});
    const off = services.onHostGuests((l) => { if (alive) setAll(l); });
    return () => { alive = false; off(); };
  }, [services]);
  const ticking = all.some((g) => g.expires_at != null);
  useEffect(() => {
    if (!ticking) return;
    const id = setInterval(() => setNowSec(Math.floor(Date.now() / 1000)), TICK_MS);
    return () => clearInterval(id);
  }, [ticking]);
  const now = Math.max(nowSec, Math.floor(Date.now() / 1000));
  return { guests: all.filter((g) => isLive(g, now)), nowSec: now };
}
