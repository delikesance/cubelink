/* StatusDot + Badge : etats jamais portes par la couleur seule (libelle texte obligatoire).
   <StatusDot tone="ok" label="En ligne" />  : pastille 10 px + libelle (`hideLabel` le reserve aux lecteurs d'ecran).
   <Badge tone="warn">Bientot</Badge>        : pastille texte arrondie.
   tone : "ok" | "warn" | "danger" | "accent" | "neutral" (defaut). */
import type { ReactNode } from "react";
import "./ui.css";

export type Tone = "ok" | "warn" | "danger" | "accent" | "neutral";

export function StatusDot({ tone = "neutral", label, hideLabel }: { tone?: Tone; label: string; hideLabel?: boolean }) {
  return (
    <span className="cl-status">
      <span className={`cl-dot cl-dot-${tone}`} aria-hidden="true" />
      <span className={hideLabel ? "cl-sr" : undefined}>{label}</span>
    </span>
  );
}

export function Badge({ tone = "neutral", children }: { tone?: Tone; children: ReactNode }) {
  return <span className={`cl-badge cl-badge-${tone}`}>{children}</span>;
}
