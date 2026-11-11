/* Drawer : tiroir lateral droit (360 px, entree 280 ms) avec voile, role="dialog" aria-modal, Echap et clic sur le voile pour fermer,
   focus piege (Tab boucle) puis rendu a l'element precedemment focalise a la fermeture.
   Props : open ; onClose ; title (libelle accessible, affiche en en-tete) ; children ; width (px, defaut 360).
   Reste monte pendant la sortie (280 ms) ; sous prefers-reduced-motion la transition est quasi immediate. */
import { useEffect, useId, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import "./ui.css";

export interface DrawerProps { open: boolean; onClose: () => void; title: string; children?: ReactNode; width?: number }

const FOCUSABLE = 'a[href],button:not([disabled]),input:not([disabled]),select:not([disabled]),textarea:not([disabled]),[tabindex]:not([tabindex="-1"])';

export function Drawer({ open, onClose, title, children, width = 360 }: DrawerProps) {
  const [mounted, setMounted] = useState(open);
  const [shown, setShown] = useState(false);
  const panel = useRef<HTMLDivElement>(null);
  const prev = useRef<HTMLElement | null>(null);
  const tid = useId();

  useEffect(() => {
    if (open) {
      prev.current = document.activeElement as HTMLElement | null;
      setMounted(true);
      const r = requestAnimationFrame(() => setShown(true));
      return () => cancelAnimationFrame(r);
    }
    setShown(false);
    const t = setTimeout(() => setMounted(false), 280);
    return () => clearTimeout(t);
  }, [open]);

  useEffect(() => {
    if (!open || !mounted) return;
    const p = panel.current;
    if (p && !p.contains(document.activeElement)) (p.querySelector<HTMLElement>(FOCUSABLE) ?? p).focus();
    return () => { const el = prev.current; if (el && document.contains(el)) el.focus(); };
  }, [open, mounted]);

  if (!mounted) return null;
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") { e.stopPropagation(); onClose(); return; }
    if (e.key !== "Tab") return;
    const items = [...(panel.current?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? [])];
    if (items.length === 0) { e.preventDefault(); panel.current?.focus(); return; }
    const first = items[0]!, last = items[items.length - 1]!;
    const a = document.activeElement;
    if (e.shiftKey && (a === first || a === panel.current)) { e.preventDefault(); last.focus(); }
    else if (!e.shiftKey && a === last) { e.preventDefault(); first.focus(); }
  };
  return (
    <div className={`cl-drawer-root${shown && open ? " is-open" : ""}`} onKeyDown={onKeyDown}>
      <div className="cl-veil" data-testid="drawer-veil" onClick={onClose} />
      <div ref={panel} className="cl-drawer" role="dialog" aria-modal="true" aria-labelledby={tid} tabIndex={-1} style={{ width }}>
        <h2 id={tid} className="cl-drawer-title">{title}</h2>
        {children}
      </div>
    </div>
  );
}
