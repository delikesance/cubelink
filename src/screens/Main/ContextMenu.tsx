import "./shared.css";
import { Fragment, useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";

export interface MenuItem {
  key: string;
  label: string;
  danger?: boolean;
  icon?: ReactNode;
  /** Filet de séparation au-dessus de l'entrée. */
  separator?: boolean;
  /** Visible mais inactive (ex. Jouer pendant qu'un autre jeu tourne). */
  disabled?: boolean;
  onSelect: () => void;
}

export interface ContextMenuProps {
  label: string;
  x: number;
  y: number;
  items: MenuItem[];
  /** Fermeture ; `restoreFocus` vrai quand la fermeture vient du clavier. */
  onClose: (restoreFocus: boolean) => void;
}

/** Menu contextuel : role=menu, flèches / Début / Fin, Échap, clic extérieur. */
export function ContextMenu({ label, x, y, items, onClose }: ContextMenuProps) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ left: x, top: y });
  const closeRef = useRef(onClose);
  closeRef.current = onClose;

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    setPos({
      left: Math.max(4, Math.min(x, window.innerWidth - r.width - 4)),
      top: Math.max(4, Math.min(y, window.innerHeight - r.height - 4)),
    });
    el.querySelector<HTMLElement>('[role="menuitem"]')?.focus();
  }, [x, y]);

  useEffect(() => {
    const away = (e: Event) => {
      if (!ref.current?.contains(e.target as Node)) closeRef.current(false);
    };
    const close = () => closeRef.current(false);
    document.addEventListener("mousedown", away, true);
    document.addEventListener("contextmenu", away, true);
    window.addEventListener("blur", close);
    window.addEventListener("resize", close);
    return () => {
      document.removeEventListener("mousedown", away, true);
      document.removeEventListener("contextmenu", away, true);
      window.removeEventListener("blur", close);
      window.removeEventListener("resize", close);
    };
  }, []);

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const els = Array.from(ref.current?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? []);
    const i = els.indexOf(document.activeElement as HTMLElement);
    const go = (n: number) => { e.preventDefault(); els[(n + els.length) % els.length]?.focus(); };
    switch (e.key) {
      case "ArrowDown": go(i + 1); break;
      case "ArrowUp": go(i < 0 ? -1 : i - 1); break;
      case "Home": go(0); break;
      case "End": go(-1); break;
      case "Escape": e.preventDefault(); e.stopPropagation(); onClose(true); break;
      case "Tab": e.preventDefault(); onClose(true); break;
    }
  };

  return (
    <div ref={ref} className="home-menu enter" role="menu" aria-label={label} style={pos} onKeyDown={onKeyDown}>
      {items.map((it) => (
        <Fragment key={it.key}>
          {it.separator && <div role="separator" className="home-menusep" />}
          <button
            type="button" role="menuitem" tabIndex={-1} disabled={it.disabled} aria-disabled={it.disabled || undefined}
            className={`home-menuitem${it.danger ? " is-danger" : ""}`}
            onClick={() => { onClose(false); it.onSelect(); }}
          >
            {it.icon && <span className="home-menuicon" aria-hidden="true">{it.icon}</span>}
            <span>{it.label}</span>
          </button>
        </Fragment>
      ))}
    </div>
  );
}
