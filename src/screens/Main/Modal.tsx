import { useEffect, useId, useRef, type KeyboardEvent, type ReactNode } from "react";

const FOCUSABLE = 'a[href],button:not(:disabled),input:not(:disabled),select:not(:disabled),textarea:not(:disabled),[tabindex]:not([tabindex="-1"])';

export interface ModalProps {
  title: string;
  onClose: () => void;
  /** "alertdialog" pour une confirmation : le focus initial va sur [data-autofocus] s'il existe. */
  role?: "dialog" | "alertdialog";
  /** Plein écran sans en-tête visible (visionneuse) : le titre reste le nom accessible. */
  fullscreen?: boolean;
  children: ReactNode;
}

/** Boîte de dialogue modale : piège de focus, Échap, restauration du focus, clic sur le fond = fermer. */
export function Modal({ title, onClose, role = "dialog", fullscreen, children }: ModalProps) {
  const ref = useRef<HTMLDivElement>(null);
  const titleId = useId();
  const closeRef = useRef(onClose);
  closeRef.current = onClose;

  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    const el = ref.current;
    (el?.querySelector<HTMLElement>("[data-autofocus]") ?? el?.querySelector<HTMLElement>(FOCUSABLE))?.focus();
    return () => { previous?.focus?.(); };
  }, []);

  useEffect(() => {
    const onEsc = (e: globalThis.KeyboardEvent) => {
      if (e.key === "Escape" && !e.defaultPrevented) { e.preventDefault(); closeRef.current(); }
    };
    document.addEventListener("keydown", onEsc);
    return () => document.removeEventListener("keydown", onEsc);
  }, []);

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key !== "Tab") return;
    const items = Array.from(ref.current?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? []);
    if (!items.length) return;
    const first = items[0];
    const last = items[items.length - 1];
    const active = document.activeElement;
    if (e.shiftKey && (active === first || !ref.current?.contains(active))) { e.preventDefault(); last.focus(); }
    else if (!e.shiftKey && (active === last || !ref.current?.contains(active))) { e.preventDefault(); first.focus(); }
  };

  return (
    <div className={`home-overlay${fullscreen ? " is-full" : ""}`} onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }}>
      <div
        ref={ref} className={`home-modal${fullscreen ? " is-full" : ""}`} role={role} aria-modal="true"
        aria-labelledby={titleId} onKeyDown={onKeyDown}
      >
        {fullscreen ? <h2 id={titleId} className="home-sr">{title}</h2> : (
          <header className="home-modal-head">
            <h2 id={titleId}>{title}</h2>
          </header>
        )}
        {children}
      </div>
    </div>
  );
}
