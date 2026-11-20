import { useEffect, useState } from "react";
import { Button } from "../ui";
import type { Services } from "../services";
import { stageFallbackMessage, useStageFallback } from "./stage";

/** Durée d'affichage du message simple ; l'annonce de coupure de session reste jusqu'à ce qu'on la ferme ou qu'on réessaie. */
export const STAGE_NOTICE_MS = 10_000;

/** Quand « Réessayer » ne peut rien faire : la zone de jeu est refusée ailleurs que par la coupure de session. */
const RETRY_UNAVAILABLE = "La zone de jeu n'est pas disponible : vérifie que « Jeu intégré » est activé dans les réglages.";

/**
 * Message discret quand le jeu n'a pas pu être affiché dans la fenêtre de Cubelink : il tourne dans la sienne,
 * rien n'est perdu, le joueur doit seulement savoir où le trouver. Carte flottante, ne pousse pas le contenu.
 * Après la coupure de session (plusieurs replis de suite), « Réessayer dans Cubelink » redonne sa chance à la zone de jeu
 * sans relancer l'application : fermer la fenêtre ne fait que la masquer, le processus (et la coupure) resteraient.
 */
export function StageNotice({ services }: { services: Services }) {
  const { notice, dismiss } = useStageFallback(services);
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    if (!notice || notice.session_off) return;
    const t = window.setTimeout(dismiss, STAGE_NOTICE_MS);
    return () => window.clearTimeout(t);
  }, [notice, dismiss]);
  // Une nouvelle annonce repart d'un état propre.
  useEffect(() => { setFailed(false); }, [notice]);
  if (!notice) return null;

  const retry = async () => {
    if (busy) return;
    setBusy(true);
    setFailed(false);
    try {
      if (await services.stageRetry()) dismiss();
      else setFailed(true);
    } catch {
      setFailed(true);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="stage-notice" role="status">
      <span className="stage-notice-txt">{failed ? RETRY_UNAVAILABLE : stageFallbackMessage(notice)}</span>
      <span className="stage-notice-actions">
        {notice.session_off && <Button variant="primary" disabled={busy} onClick={() => void retry()}>Réessayer dans Cubelink</Button>}
        <Button variant="secondary" onClick={dismiss}>Fermer</Button>
      </span>
    </div>
  );
}
