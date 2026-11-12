import { useEffect } from "react";
import { Button } from "../ui";
import { stageFallbackMessage, useStageFallback } from "./stage";

/** Durée d'affichage du message simple ; l'annonce « jusqu'au redémarrage » reste jusqu'à ce qu'on la ferme. */
export const STAGE_NOTICE_MS = 10_000;

/**
 * Message discret quand le jeu n'a pas pu être affiché dans la fenêtre de Cubelink : il tourne dans la sienne,
 * rien n'est perdu, le joueur doit seulement savoir où le trouver. Carte flottante, ne pousse pas le contenu.
 */
export function StageNotice() {
  const { notice, dismiss } = useStageFallback();
  useEffect(() => {
    if (!notice || notice.session_off) return;
    const t = window.setTimeout(dismiss, STAGE_NOTICE_MS);
    return () => window.clearTimeout(t);
  }, [notice, dismiss]);
  if (!notice) return null;
  return (
    <div className="stage-notice" role="status">
      <span className="stage-notice-txt">{stageFallbackMessage(notice)}</span>
      <Button variant="secondary" onClick={dismiss}>Fermer</Button>
    </div>
  );
}
