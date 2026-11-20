import { useEffect, useState } from "react";
import { Button } from "../ui";
import type { Services } from "../services/types";
import { useHearthCards } from "./useHearth";
import { useUpdateState } from "./useUpdateState";

/** Carte flottante discrète (en bas, ne pousse pas le contenu) : visible uniquement quand une mise à jour est téléchargée et prête. */
export function UpdateBanner({ services }: { services: Services }) {
  const st = useUpdateState(services);
  const [dismissed, setDismissed] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const cards = useHearthCards(services);
  const hosting = (cards ?? []).some((c) => c.state === "hosting");
  const version = st?.version ?? "";
  useEffect(() => { setError(""); setBusy(false); }, [version, st?.phase]);
  if (!st || st.phase !== "ready" || dismissed === version) return null;
  const install = () => {
    setError("");
    // L'arrêt d'un serveur de groupe peut durer : le bouton reste grisé jusqu'au redémarrage (ou jusqu'au refus).
    setBusy(true);
    services.updateInstall().catch((e) => {
      setError(e instanceof Error ? e.message : typeof e === "string" ? e : "L'installation a échoué.");
      setBusy(false);
    });
  };
  return (
    <div className="update-banner" role="region" aria-label="Mise à jour">
      <span className="update-banner-ico" aria-hidden="true">
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M12 3v12m0 0-4-4m4 4 4-4M5 21h14" /></svg>
      </span>
      <div className="update-banner-body">
        <strong className="update-banner-title">Mise à jour{version ? ` ${version}` : ""} prête</strong>
        <span className={`update-banner-txt${error ? " is-error" : ""}`} role="status">
          {error || (busy && hosting ? "Arrêt du serveur de groupe, monde publié…" : hosting
            ? "Redémarre pour l'installer. Le serveur de ton groupe sera arrêté proprement (monde publié) : rien n'est perdu."
            : "Redémarre pour l'installer. Rien n'est perdu.")}
        </span>
      </div>
      <div className="update-banner-actions">
        <Button variant="secondary" onClick={() => setDismissed(version)}>Plus tard</Button>
        <Button onClick={install} loading={busy}>Redémarrer et installer</Button>
      </div>
    </div>
  );
}
