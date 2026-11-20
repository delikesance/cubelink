import { useCallback, useEffect, useRef, useState } from "react";
import type { Services } from "../../services";
import type { TransferErrorDetails, TransferProgress } from "../../services/types";
import { smoothSpeed } from "./format";

/** Étape (0..4) correspondant à une phase ; en pause/échec on reste sur la dernière étape atteinte. */
export function stepIndex(phase: TransferProgress["phase"], resumeAt: number): number {
  switch (phase) {
    case "preparing": return 0;
    case "receiving": return 1;
    case "verifying": return 2;
    case "launching": return 3;
    case "done": return 4;
    default: return resumeAt;
  }
}

/**
 * État partagé d'un transfert (écran Rejoindre) : progression, vitesse lissée, détails d'erreur,
 * mods inconnus, et actions asynchrones protégées (occupé + message d'erreur).
 */
export function useTransfer(services: Services) {
  const [p, setP] = useState<TransferProgress | null>(null);
  const [speed, setSpeed] = useState<number | null>(null);
  const [errDetails, setErrDetails] = useState<TransferErrorDetails | null>(null);
  const [unknownMods, setUnknownMods] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const resumeAt = useRef(0);
  const cancelled = useRef(false);

  useEffect(() => {
    return services.onTransferProgress((next) => {
      if (cancelled.current) return;
      if (next.phase !== "paused" && next.phase !== "failed") {
        resumeAt.current = Math.min(stepIndex(next.phase, resumeAt.current), 3);
      }
      if (next.phase === "receiving") setSpeed((s) => smoothSpeed(s, next.speed_bps));
      setP(next);
    });
  }, [services]);

  useEffect(() => services.onTransferErrorDetails((e) => setErrDetails(e)), [services]);
  useEffect(() => services.onUnknownMods((e) => setUnknownMods(e.unknown_mods)), [services]);

  const run = useCallback(async (fn: () => Promise<void>, msg: string) => {
    setBusy(true);
    setActionError(null);
    try { await fn(); } catch { setActionError(msg); } finally { setBusy(false); }
  }, []);

  /** Annule le transfert (joinCancel) puis appelle `after` ; les événements tardifs sont ignorés. */
  const cancel = (after: () => void) => run(async () => {
    cancelled.current = true;
    try { await services.joinCancel(); } catch (e) { cancelled.current = false; throw e; }
    after();
  }, "Impossible d'annuler pour l'instant. Réessaie.");

  const reset = useCallback(() => {
    cancelled.current = false;
    resumeAt.current = 0;
    setErrDetails(null);
    setP(null);
    setSpeed(null);
  }, []);

  const phase = p?.phase ?? "preparing";
  return { p, phase, speed, errDetails, unknownMods, busy, actionError, run, cancel, reset, cur: stepIndex(phase, resumeAt.current) };
}
