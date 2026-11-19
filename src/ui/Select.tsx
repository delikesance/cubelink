/* Select : liste deroulante maison (remplace <select>, dont le popup natif n'est pas stylable).
   <Select id aria-label aria-describedby value onChange={(valeur) => …} options={[{ value, label }]} disabled placeholder />
   Bouton role="combobox" + liste role="listbox". L'`id` va sur le bouton : un <label htmlFor> ou un Field le relie.
   Clavier : Entree/Espace/Bas ouvre, Haut/Bas/Debut/Fin deplacent, Entree choisit, Echap ferme, une lettre saute a l'option qui commence par elle. */
import { useEffect, useId, useRef, useState, type KeyboardEvent } from "react";
import "./ui.css";

export interface SelectOption { value: string; label: string; }

export interface SelectProps {
  id?: string;
  value: string;
  options: SelectOption[];
  onChange: (value: string) => void;
  disabled?: boolean;
  placeholder?: string;
  className?: string;
  "aria-label"?: string;
  "aria-describedby"?: string;
}

export function Select({ id, value, options, onChange, disabled, placeholder, className, ...aria }: SelectProps) {
  const uid = useId();
  const listId = `${uid}-list`;
  const root = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const selected = Math.max(0, options.findIndex((o) => o.value === value));
  const [active, setActive] = useState(selected);
  const current = options.find((o) => o.value === value);

  useEffect(() => {
    if (!open) return;
    const away = (e: MouseEvent) => { if (!root.current?.contains(e.target as Node)) setOpen(false); };
    document.addEventListener("mousedown", away);
    return () => document.removeEventListener("mousedown", away);
  }, [open]);

  useEffect(() => {
    if (open) document.getElementById(`${uid}-o${active}`)?.scrollIntoView?.({ block: "nearest" });
  }, [open, active, uid]);

  const show = () => { setActive(selected); setOpen(true); };
  const pick = (i: number) => { const o = options[i]; if (o) onChange(o.value); setOpen(false); };

  const onKey = (e: KeyboardEvent<HTMLButtonElement>) => {
    if (disabled) return;
    const last = options.length - 1;
    if (!open) {
      if (["ArrowDown", "ArrowUp", "Enter", " "].includes(e.key)) { e.preventDefault(); show(); }
      return;
    }
    switch (e.key) {
      case "ArrowDown": e.preventDefault(); setActive((a) => Math.min(last, a + 1)); break;
      case "ArrowUp": e.preventDefault(); setActive((a) => Math.max(0, a - 1)); break;
      case "Home": e.preventDefault(); setActive(0); break;
      case "End": e.preventDefault(); setActive(last); break;
      case "Enter": case " ": e.preventDefault(); pick(active); break;
      case "Escape": e.preventDefault(); e.stopPropagation(); setOpen(false); break;
      case "Tab": setOpen(false); break;
      default:
        if (e.key.length === 1) {
          const from = options.findIndex((o, i) => i > active && o.label.toLowerCase().startsWith(e.key.toLowerCase()));
          const at = from >= 0 ? from : options.findIndex((o) => o.label.toLowerCase().startsWith(e.key.toLowerCase()));
          if (at >= 0) setActive(at);
        }
    }
  };

  return (
    <div className={["cl-select", className].filter(Boolean).join(" ")} ref={root}>
      <button
        type="button" id={id} role="combobox" className="cl-input cl-select-btn"
        aria-haspopup="listbox" aria-expanded={open} aria-controls={open ? listId : undefined}
        aria-activedescendant={open ? `${uid}-o${active}` : undefined}
        disabled={disabled} onClick={() => (open ? setOpen(false) : show())} onKeyDown={onKey} {...aria}
      >
        <span className={current ? "cl-select-val" : "cl-select-val cl-select-ph"}>{current?.label ?? placeholder ?? ""}</span>
        <svg className="cl-select-chev" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M6 9l6 6 6-6" /></svg>
      </button>
      {open && (
        <ul id={listId} role="listbox" className="cl-select-list" tabIndex={-1}>
          {options.map((o, i) => (
            <li
              key={o.value} id={`${uid}-o${i}`} role="option" aria-selected={o.value === value}
              className={["cl-select-opt", i === active ? "is-active" : "", o.value === value ? "is-selected" : ""].filter(Boolean).join(" ")}
              onMouseEnter={() => setActive(i)} onMouseDown={(e) => e.preventDefault()} onClick={() => pick(i)}
            >{o.label}</li>
          ))}
        </ul>
      )}
    </div>
  );
}
