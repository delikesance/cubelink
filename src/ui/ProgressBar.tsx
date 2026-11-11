/* ProgressBar : barre de progression (seule animation continue du design : rayures en defilement, coupees sous prefers-reduced-motion).
   Props : value (0..100 ; undefined/null = indetermine) ; label (libelle accessible, obligatoire) ; showValue (affiche « 42 % ») ;
   tone "accent" (defaut) | "ok" | "warn" | "danger" ; className.
   role="progressbar" avec aria-valuemin/max/now (aria-valuenow absent si indetermine). */
import "./ui.css";

export interface ProgressBarProps {
  value?: number | null;
  label: string;
  showValue?: boolean;
  tone?: "accent" | "ok" | "warn" | "danger";
  className?: string;
}

export function ProgressBar({ value, label, showValue, tone = "accent", className }: ProgressBarProps) {
  const det = typeof value === "number" && Number.isFinite(value);
  const v = det ? Math.min(100, Math.max(0, value)) : 0;
  return (
    <div className={["cl-progress", className].filter(Boolean).join(" ")}>
      <div
        className={`cl-bar cl-tone-${tone}${det ? "" : " is-indeterminate"}`}
        role="progressbar" aria-label={label} aria-valuemin={0} aria-valuemax={100}
        aria-valuenow={det ? Math.round(v) : undefined}
      >
        <div className="cl-bar-fill" style={det ? { width: `${v}%` } : undefined} />
      </div>
      {showValue && det && <span className="cl-bar-val" aria-hidden="true">{Math.round(v)} %</span>}
    </div>
  );
}
