import { useEffect, useId, useState } from "react";

/** RAM « Par défaut » : l'instance suit la valeur globale des paramètres (lue au lancement). */
export const RAM_DEFAULT = 0;
const GB = 1024;
const MIN_GB = 2;

/** « 8 Go », « 1,5 Go ». */
export const ramLabel = (mb: number) => `${Math.round((mb / GB) * 10) / 10} Go`.replace(".", ",");

/** Valeur globale de la RAM (paramètres), `null` tant qu'elle n'est pas lue. */
export function useDefaultRam(services: { settingsGet: () => Promise<{ default_ram_mb: number }> }): number | null {
  const [mb, setMb] = useState<number | null>(null);
  useEffect(() => {
    let alive = true;
    services.settingsGet().then((s) => { if (alive) setMb(s.default_ram_mb); }, () => {});
    return () => { alive = false; };
  }, [services]);
  return mb;
}

export interface RamPickerProps {
  /** Mo, ou `RAM_DEFAULT` (0) pour suivre la valeur globale. */
  value: number;
  onChange: (mb: number) => void;
  /** Valeur globale des paramètres (`null` : pas encore connue). */
  defaultMb: number | null;
  label?: string;
  /** Conseil sous le réglage (ex. besoin d'un gros modpack). */
  hint?: string;
  disabled?: boolean;
}

/**
 * Mémoire allouée : « Valeur des paramètres » (cochée par défaut, l'instance suit le réglage global, même s'il change
 * plus tard) ou une valeur propre à l'instance, choisie au curseur.
 */
export function RamPicker({ value, onChange, defaultMb, label = "Mémoire allouée", hint, disabled }: RamPickerProps) {
  const uid = useId();
  const inherit = value === RAM_DEFAULT;
  const shown = inherit ? defaultMb : value;
  const custom = inherit ? (defaultMb ?? 4 * GB) : value;
  const maxGb = Math.max(16, Math.ceil(custom / GB), Math.ceil((defaultMb ?? 0) / GB));
  const gb = Math.min(maxGb, Math.max(MIN_GB, Math.round(custom / GB)));
  const defLabel = defaultMb ? ramLabel(defaultMb) : "…";

  return (
    <div className="cl-ram" role="group" aria-labelledby={`${uid}-l`}>
      <div className="cl-ram-head">
        <span id={`${uid}-l`} className="cl-label">{label}</span>
        <output className="cl-ram-val" aria-live="polite">{shown ? ramLabel(shown) : "…"}</output>
      </div>
      <label className="cl-ram-default">
        <input
          type="checkbox" checked={inherit} disabled={disabled}
          onChange={(e) => onChange(e.target.checked ? RAM_DEFAULT : Math.round(custom / GB) * GB)}
        />
        <span>Valeur des paramètres <span className="cl-ram-muted">({defLabel})</span></span>
      </label>
      {!inherit && (
        <>
          <input
            className="cl-ram-range" type="range" min={MIN_GB} max={maxGb} step={1} value={gb} disabled={disabled}
            aria-label={`${label} (Go)`} aria-valuetext={ramLabel(gb * GB)}
            onChange={(e) => onChange(Number(e.target.value) * GB)}
          />
          <div className="cl-ram-scale" aria-hidden="true"><span>{MIN_GB}&nbsp;Go</span><span>{maxGb}&nbsp;Go</span></div>
        </>
      )}
      {hint && <p className="cl-ram-hint">{hint}</p>}
    </div>
  );
}
