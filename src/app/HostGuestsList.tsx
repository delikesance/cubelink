// Hôte : section « Peuvent rejoindre » du panneau en jeu (invités, demandes acceptées, monde ouvert à tous les amis).
import { Badge, Button } from "../ui";
import { Avatar } from "../screens/Friends/Avatar";
import { remainingText } from "./useHostGuests";
import type { FriendWithPresence, HostGuest, Services } from "../services/types";

const ORIGIN_LABEL: Record<HostGuest["origin"], string> = {
  invite: "Invité",
  request: "Demande acceptée",
  all_friends: "Via « tous mes amis »",
};

/** État d'un invité : « Invité · 4 min », « Demande acceptée · 4 min », « Via « tous mes amis » · 4 min », ou « A rejoint »
 * (il a utilisé son accès : manifeste, fichiers ou tunnel ; l'accès vaut alors jusqu'à la fin de la partie). */
export function guestStatus(g: HostGuest, nowSec: number): string {
  if (g.entered) return "A rejoint";
  const label = ORIGIN_LABEL[g.origin];
  return g.expires_at == null ? label : `${label} · ${remainingText(g.expires_at, nowSec)}`;
}

interface Props {
  guests: HostGuest[];
  friends: FriendWithPresence[];
  nowSec: number;
  openToAll: boolean;
  services: Services;
  /** Clé de l'invité dont le retrait est en cours. */
  busyKey: string | null;
  onRevoke: (publicKey: string, name: string) => void;
}

export function HostGuestsList({ guests, friends, nowSec, openToAll, services, busyKey, onRevoke }: Props) {
  if (guests.length === 0 && !openToAll) return null;
  const nameOf = (key: string) => friends.find((f) => f.friend.public_key === key)?.friend.name ?? "Ami";
  return (
    <section className="ov-sec" aria-labelledby="ov-guests">
      <h2 id="ov-guests">Peuvent rejoindre{guests.length > 0 ? ` · ${guests.length}` : ""}</h2>
      {openToAll && <p className="ov-empty"><Badge tone="accent">Monde ouvert à tous tes amis</Badge></p>}
      {guests.length > 0 && (
        <ul className="ov-list" aria-label="Peuvent rejoindre">
          {guests.map((g) => {
            const name = nameOf(g.public_key);
            return (
              <li key={g.public_key} className="ov-row">
                <Avatar name={name} size={36} services={services} />
                <div className="ov-who">
                  <span className="ov-name"><span>{name}</span></span>
                  <span className="ov-sub">{guestStatus(g, nowSec)}</span>
                </div>
                <div className="ov-actions">
                  <Button variant="secondary" aria-label={`Retirer l'accès de ${name}`} loading={busyKey === g.public_key} onClick={() => onRevoke(g.public_key, name)}>Retirer</Button>
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
