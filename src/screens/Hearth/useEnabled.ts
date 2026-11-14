import { useEffect, useState } from "react";
import type { Services } from "../../services/types";

/** Interrupteur « Serveurs de groupe (expérimental) ». `false` tant que la réponse n'est pas arrivée : rien ne s'affiche avant. */
export function useHearthEnabled(services: Services): boolean {
  const [on, setOn] = useState(false);
  useEffect(() => {
    let alive = true;
    const read = () => { services.hearthEnabled().then((v) => { if (alive) setOn(v); }, () => { if (alive) setOn(false); }); };
    read();
    const off = services.onHearthState(read);
    return () => { alive = false; off(); };
  }, [services]);
  return on;
}
