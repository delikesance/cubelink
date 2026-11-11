import { useEffect, useState } from "react";
import type { HostState, Invite, Services } from "../services/types";

/** Invitations reçues en attente, tenues à jour par friend://invites (liste complète à chaque changement). */
export function useInvites(services: Services): Invite[] {
  const [invites, setInvites] = useState<Invite[]>([]);
  useEffect(() => {
    let alive = true;
    services.friendInvites().then((l) => { if (alive) setInvites(l); }, () => {});
    const off = services.onFriendInvites((l) => { if (alive) setInvites(l); });
    return () => { alive = false; off(); };
  }, [services]);
  return invites;
}

/** Clés des amis qui « hébergent » : un ami qui a une invitation non expirée est dans un monde ouvert (le backend ne transporte pas l'hébergement). */
export function hostingKeys(invites: Invite[], nowSec = Math.floor(Date.now() / 1000)): Set<string> {
  return new Set(invites.filter((i) => i.expires_at > nowSec).map((i) => i.public_key));
}

/** Vrai quand un monde est ouvert au réseau et partagé : on peut alors inviter un ami. */
export function canInviteFrom(host: HostState | null): boolean {
  return !!host && host.sharing && !!host.world_port;
}

/** État du partage hôte (null si rien n'est partagé) : lecture au montage puis événement host://state. */
export function useHostState(services: Services): HostState | null {
  const [host, setHost] = useState<HostState | null>(null);
  useEffect(() => {
    let alive = true;
    services.hostState().then((h) => { if (alive) setHost(h); }, () => {});
    const off = services.onHostState((h) => { if (alive) setHost(h); });
    return () => { alive = false; off(); };
  }, [services]);
  return host;
}
