import { useEffect, useState } from "react";
import "./friendslist.css";
import { useServices, useShell } from "../../app/context";
import { IconButton, Tooltip } from "../../ui";
import { Avatar } from "./Avatar";
import type { FriendWithPresence } from "../../services/types";

const Play = () => (
  <svg width="20" height="20" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M8 5.5v13l11-6.5z" /></svg>
);

/** Liste « Tes amis » : lignes de 64 px, nom tronqué avec infobulle, présence réelle, bouton Rejoindre. */
export default function FriendsList() {
  const services = useServices();
  const { navigate } = useShell();
  const [list, setList] = useState<FriendWithPresence[] | null>(null);
  const [error, setError] = useState(false);

  useEffect(() => {
    let alive = true;
    services.friendList().then((l) => { if (alive) setList(l); }, () => { if (alive) setError(true); });
    const off = services.onFriendPresence((e) => {
      setList((l) => l && l.map((f) => (f.friend.public_key === e.public_key ? { ...f, presence: e.presence } : f)));
    });
    return () => { alive = false; off(); };
  }, [services]);

  if (error) return <p className="fl-info" role="alert">Impossible de charger tes amis pour l'instant.</p>;
  if (!list) return <p className="fl-info" role="status">Chargement de tes amis…</p>;
  if (list.length === 0) return <p className="fl-info">Aucun ami pour l'instant. Ajoute-en un avec son code, dans l'onglet « Ajouter un ami ».</p>;

  return (
    <section aria-labelledby="fl-title" className="fl">
      <h2 id="fl-title" className="fl-title">Tes amis · {list.length}</h2>
      <ul className="fl-list">
        {list.map(({ friend, presence }, i) => {
          const online = presence === "online";
          return (
            <li key={friend.public_key} className="fl-row enter" style={{ "--i": Math.min(i, 8) } as React.CSSProperties}>
              <Avatar name={friend.name} size={44} presence={online ? "online" : "offline"} />
              <div className="fl-who">
                <Tooltip text={friend.name}><span className="fl-name" tabIndex={0}>{friend.name}</span></Tooltip>
                <span className={online ? "fl-state is-on" : "fl-state"}>{online ? "En ligne" : "Hors ligne"}</span>
                {!online && <span id={`fl-why-${i}`} className="fl-state">Demande-lui d'ouvrir Cubelink pour le rejoindre.</span>}
              </div>
              <IconButton variant="secondary" aria-label={`Rejoindre ${friend.name}`} icon={<Play />} disabled={!online} aria-describedby={online ? undefined : `fl-why-${i}`}
                onClick={() => navigate("join", { friend: friend.public_key })} />
            </li>
          );
        })}
      </ul>
    </section>
  );
}
