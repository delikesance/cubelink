import { useEffect, useState } from "react";
import { Button } from "../ui";
import type { Services } from "../services/types";
import { useUpdateState } from "./useUpdateState";

/** Bannière discrète : visible uniquement quand une mise à jour est téléchargée et prête. */
export function UpdateBanner({ services }: { services: Services }) {
  const st = useUpdateState(services);
  const [dismissed, setDismissed] = useState<string | null>(null);
  const [error, setError] = useState("");
  const version = st?.version ?? "";
  useEffect(() => { setError(""); }, [version, st?.phase]);
  if (!st || st.phase !== "ready" || dismissed === version) return null;
  const install = () => {
    setError("");
    services.updateInstall().catch((e) => setError(e instanceof Error ? e.message : typeof e === "string" ? e : "L'installation a échoué."));
  };
  return (
    <div className="update-banner" role="region" aria-label="Mise à jour">
      <span className="update-banner-txt" role="status">
        {error || `Mise à jour${version ? ` ${version}` : ""} prête.`}
      </span>
      <Button onClick={install}>Redémarrer et installer</Button>
      <Button variant="secondary" onClick={() => setDismissed(version)}>Plus tard</Button>
    </div>
  );
}
