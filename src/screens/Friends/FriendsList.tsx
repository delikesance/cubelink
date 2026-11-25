import { useEffect, useState } from "react";
import "./friendsList.css";
import { useServices } from "../../app/context";
import { Button } from "../../ui";
import { errMsg } from "../../app/useLaunch";
import type { FriendWithPresence } from "../../services/types";
import { Avatar } from "./Avatar";
import { lastSeenText } from "./lastSeen";
import { RemoveFriendDialog } from "./RemoveFriend";

/** Paramètres > Amis : « Mes amis », avec Retirer (confirmation). */
export default function FriendsList({ refreshKey = 0 }: { refreshKey?: number } = {}) {
  const services = useServices();
  const [friends, setFriends] = useState<FriendWithPresence[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [toRemove, setToRemove] = useState<FriendWithPresence | null>(null);
  const [busyTrust, setBusyTrust] = useState<string | null>(null);
  const [notice, setNotice] = useState("");

  useEffect(() => {
    let alive = true;
    services.friendList().then(
      (l) => { if (alive) setFriends(l); },
      (e: unknown) => { if (alive) { setFriends([]); setError(errMsg(e)); } },
    );
    const off = services.onFriendPresence((e) => {
      const now = Math.floor(Date.now() / 1000);
      setFriends((l) => l && l.map((f) => (f.friend.public_key === e.public_key ? { ...f, presence: e.presence, in_game: e.in_game, last_seen: now } : f)));
    });
    // Ma demande envoyée vient d'être acceptée : le nouvel ami apparaît sans changer d'écran.
    const offAccepted = services.onFriendAccepted(() => {
      services.friendList().then((l) => { if (alive) setFriends(l); }, () => {});
    });
    return () => { alive = false; off(); offAccepted(); };
  }, [services, refreshKey]);

  const toggleTrust = async (item: FriendWithPresence, checked: boolean) => {
    const next = checked ? "always" : "ask";
    setBusyTrust(item.friend.public_key);
    setError(null);
    try {
      await services.friendSetTrust(item.friend.public_key, next);
      setFriends((l) => l && l.map((f) => (
        f.friend.public_key === item.friend.public_key ? { ...f, friend: { ...f.friend, trust: next } } : f
      )));
      setNotice(checked
        ? `Les mods de ${item.friend.name} seront installés sans te demander (les scripts demandent toujours ton accord).`
        : `Cubelink te demandera à nouveau ton accord pour les mods de ${item.friend.name}.`);
    } catch (e) {
      setError(errMsg(e));
    } finally {
      setBusyTrust((cur) => (cur === item.friend.public_key ? null : cur));
    }
  };

  const nowSec = Math.floor(Date.now() / 1000);
  return (
    <section className="fl" aria-labelledby="fl-title">
      <h3 id="fl-title" className="fl-title">Mes amis{friends && friends.length > 0 ? ` (${friends.length})` : ""}</h3>
      {error && <p className="fl-error" role="alert">{error}</p>}
      {friends && friends.length === 0 && !error && <p className="fl-empty">Tu n'as pas encore d'ami. Ajoute-en un avec son code, ci-dessous.</p>}
      {friends && friends.length > 0 && (
        <ul className="fl-list">
          {friends.map((f) => {
            const on = f.presence === "online";
            return (
              <li key={f.friend.public_key} className="fl-item">
                <Avatar name={f.friend.name} size={40} presence={on ? "online" : "offline"} services={services} />
                <span className="fl-who">
                  <span className="fl-name">{f.friend.name}</span>
                  <span className="fl-sub">{on ? "En ligne" : (lastSeenText(f.last_seen, nowSec) ?? "Hors ligne")}</span>
                </span>
                <label className="fl-trust">
                  <input
                    type="checkbox"
                    checked={f.friend.trust === "always"}
                    disabled={busyTrust === f.friend.public_key}
                    aria-label={`Installer les mods de ${f.friend.name} sans me demander`}
                    onChange={(e) => { void toggleTrust(f, e.target.checked); }}
                  />
                  <span>Installer ses mods sans me demander</span>
                </label>
                <Button variant="secondary" aria-label={`Retirer ${f.friend.name} de mes amis`} onClick={() => setToRemove(f)}>Retirer</Button>
              </li>
            );
          })}
        </ul>
      )}
      <div className="fl-live" role="status" aria-live="polite">{notice}</div>
      {toRemove && (
        <RemoveFriendDialog
          services={services} friend={toRemove.friend}
          onClose={() => setToRemove(null)}
          onRemoved={(k) => {
            setFriends((l) => l && l.filter((f) => f.friend.public_key !== k));
            setNotice(`${toRemove.friend.name} n'est plus dans tes amis.`);
            setToRemove(null);
          }}
        />
      )}
    </section>
  );
}
