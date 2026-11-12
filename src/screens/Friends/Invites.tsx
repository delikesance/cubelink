import { useState } from "react";
import "./invites.css";
import { useServices, useShell } from "../../app/context";
import { useInvites } from "../../app/useInvites";
import { Button } from "../../ui";
import { Avatar } from "./Avatar";
import type { Invite } from "../../services/types";

/** Invitations à rejoindre un monde, en attente (même forme que « Demandes reçues »). Rien n'est rendu sans invitation. */
export default function Invites({ compact = false }: { compact?: boolean }) {
  const services = useServices();
  const { navigate } = useShell();
  const invites = useInvites(services);
  const [error, setError] = useState<string | null>(null);

  if (invites.length === 0) return null;

  const dismiss = async (i: Invite) => {
    setError(null);
    try { await services.friendInviteDismiss(i.id); }
    catch (e) { setError(e instanceof Error && e.message ? e.message : "Impossible d'ignorer l'invitation."); }
  };

  return (
    <section className={compact ? "inv inv-compact" : "inv"} aria-labelledby="inv-title">
      <h2 id="inv-title" className="inv-title">
        Invitations <span className="inv-badge" aria-label={`${invites.length} invitation${invites.length > 1 ? "s" : ""} en attente`}>{invites.length}</span>
      </h2>
      {error && <p className="inv-error" role="alert">{error}</p>}
      <ul className="inv-list">
        {invites.map((i) => (
          <li key={i.id} className="inv-item">
            <Avatar name={i.name} size={36} />
            <div className="inv-who">
              <span className="inv-name">{i.name} t&apos;invite</span>
              <span className="inv-sub">{i.world_name} · {i.minecraft}</span>
            </div>
            <Button aria-label={`Rejoindre ${i.name}`} onClick={() => navigate("join", { friend: i.public_key })}>Rejoindre</Button>
            <Button variant="secondary" aria-label={`Ignorer l'invitation de ${i.name}`} onClick={() => { void dismiss(i); }}>Ignorer</Button>
          </li>
        ))}
      </ul>
    </section>
  );
}
