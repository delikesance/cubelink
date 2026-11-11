import { useEffect, useState } from "react";
import type { Services, UpdateState } from "../services/types";

/** État de la mise à jour automatique : lecture initiale puis événements update://state. */
export function useUpdateState(services: Services): UpdateState | null {
  const [state, setState] = useState<UpdateState | null>(null);
  useEffect(() => {
    let alive = true;
    const off = services.onUpdateState((s) => { if (alive) setState(s); });
    services.updateState().then((s) => { if (alive) setState((cur) => cur ?? s); }, () => {});
    return () => { alive = false; off(); };
  }, [services]);
  return state;
}
