import { useState } from "react";
import { Button, Card, ProgressBar } from "../../ui";
import { useServices } from "../../app/context";
import { useUpdateState } from "../../app/useUpdateState";

function messageOf(e: unknown): string {
  if (e instanceof Error && e.message) return e.message;
  if (typeof e === "string" && e) return e;
  return "L'opération a échoué. Réessaie dans un instant.";
}

/** Section « Mise à jour » : version installée, vérification manuelle, état du téléchargement. */
export function UpdateSection() {
  const services = useServices();
  const st = useUpdateState(services);
  const [err, setErr] = useState("");
  const phase = st?.phase ?? "idle";
  const busy = phase === "checking" || phase === "downloading";
  const v = st?.version ?? "";
  const pct = st?.total ? (st.downloaded / st.total) * 100 : null;

  const check = () => { setErr(""); services.updateCheck().catch((e) => setErr(messageOf(e))); };
  const install = () => { setErr(""); services.updateInstall().catch((e) => setErr(messageOf(e))); };

  let text = "Aucune vérification effectuée pour l'instant.";
  if (phase === "checking") text = "Recherche en cours…";
  else if (phase === "up_to_date") text = "Cubelink est à jour.";
  else if (phase === "available" || phase === "downloading") text = `Version ${v} trouvée, téléchargement…`;
  else if (phase === "ready") text = `Version ${v} prête à installer.`;
  else if (phase === "error") text = st?.message || "La vérification a échoué.";

  return (
    <Card as="section" aria-label="Mise à jour" className="set-list">
      <div className="set-row">
        <div className="set-txt">
          <b>Mise à jour</b>
          <span>Version installée&nbsp;: {st?.current ?? "…"}</span>
          <span role="status" aria-live="polite" className={phase === "error" || err ? "set-error" : undefined}>{err || text}</span>
          {phase === "downloading" && <ProgressBar value={pct} label="Téléchargement de la mise à jour" showValue />}
        </div>
        {phase === "ready" && <Button onClick={install}>Redémarrer et installer</Button>}
        <Button variant="secondary" disabled={busy} onClick={check}>Chercher une mise à jour</Button>
      </div>
    </Card>
  );
}
