/* Tooltip : infobulle OPAQUE (surface #242029), au survol ET au focus clavier, Echap pour la masquer.
   <Tooltip text="Copier"><IconButton aria-label="Copier" .../></Tooltip>
   Props : text ; children (un seul element focalisable) ; side "top" (defaut) | "bottom".
   Le declencheur recoit aria-describedby vers l'infobulle (role="tooltip"). Ne remplace pas aria-label sur une icone. */
import { cloneElement, useId, useState, type ReactElement } from "react";
import "./ui.css";

export interface TooltipProps { text: string; children: ReactElement<Record<string, unknown>>; side?: "top" | "bottom" }

export function Tooltip({ text, children, side = "top" }: TooltipProps) {
  const [open, setOpen] = useState(false);
  const id = useId();
  return (
    <span
      className="cl-tip-wrap"
      onMouseEnter={() => setOpen(true)} onMouseLeave={() => setOpen(false)}
      onFocus={() => setOpen(true)} onBlur={() => setOpen(false)}
      onKeyDown={(e) => { if (e.key === "Escape" && open) setOpen(false); }}
    >
      {cloneElement(children, { "aria-describedby": open ? id : undefined })}
      {open && <span id={id} role="tooltip" className={`cl-tip cl-tip-${side}`}>{text}</span>}
    </span>
  );
}
