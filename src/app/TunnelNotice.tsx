import { Button } from "../ui";
import type { Services } from "../services";
import { tunnelMessage, useTunnelState } from "./useTunnelState";

/**
 * Invité : la liaison avec l'hôte est coupée (reconnexion en cours) ou perdue. Carte flottante non bloquante, au-dessus
 * de la carte du repli de la zone de jeu. `reconnecting` disparaît seul ; `lost` reste jusqu'à ce qu'on le ferme.
 */
export function TunnelNotice({ services }: { services: Services }) {
  const { notice, dismiss } = useTunnelState(services);
  if (!notice) return null;
  const lost = notice.problem === "lost";
  return (
    <div className="tunnel-notice" role={lost ? "alert" : "status"} data-state={notice.problem}>
      <span className="tunnel-notice-txt">{tunnelMessage(notice.problem, notice.name)}</span>
      {lost && <Button variant="secondary" onClick={dismiss}>Fermer</Button>}
    </div>
  );
}
