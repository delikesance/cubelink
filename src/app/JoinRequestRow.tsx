import "../screens/Overlay/overlay.css";
import "../screens/Friends/avatar.css";
import { Button } from "../ui";
import { Avatar } from "../screens/Friends/Avatar";
import type { Services } from "../services";
import type { JoinRequest } from "../services/types";

/** Une demande à rejoindre (hôte) : qui, et la décision. Partagée par le panneau « en jeu » et le bandeau de la coque. */
export function JoinRequestRow({ request, services, busy, onAnswer }: {
  request: JoinRequest;
  services: Services;
  busy: boolean;
  onAnswer: (request: JoinRequest, accept: boolean) => void;
}) {
  const r = request;
  return (
    <li className="ov-row ov-row-inv">
      <Avatar name={r.name} size={36} services={services} />
      <div className="ov-who">
        <span className="ov-name"><span>{r.name}</span></span>
        <span className="ov-sub">Veut rejoindre ta partie</span>
      </div>
      <div className="ov-actions">
        <Button aria-label={`Accepter la demande de ${r.name}`} loading={busy} onClick={() => onAnswer(r, true)}>Accepter</Button>
        <Button variant="secondary" aria-label={`Refuser la demande de ${r.name}`} disabled={busy} onClick={() => onAnswer(r, false)}>Refuser</Button>
      </div>
    </li>
  );
}
