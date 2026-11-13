import { useCallback, useEffect, useState, type MouseEvent as ReactMouseEvent, type ReactNode } from "react";
import { ContextMenu, type MenuItem } from "../screens/Main/ContextMenu";

export type { MenuItem };

/** Champ où le menu natif (couper, copier, coller) reste utile ; idem quand du texte est sélectionné. */
export function keepsNativeMenu(target: EventTarget | null): boolean {
  if (target instanceof HTMLElement && target.closest("input, textarea, [contenteditable=''], [contenteditable='true']")) return true;
  const sel = typeof window !== "undefined" ? window.getSelection() : null;
  return !!sel && !sel.isCollapsed && sel.toString().trim() !== "";
}

/**
 * Menu natif du navigateur (Précédent, Actualiser, Inspecter…) supprimé partout dans le launcher,
 * sauf dans les champs de saisie et sur une sélection de texte.
 */
export function useSuppressNativeMenu(): void {
  useEffect(() => {
    const onMenu = (e: MouseEvent) => { if (!keepsNativeMenu(e.target)) e.preventDefault(); };
    document.addEventListener("contextmenu", onMenu);
    return () => document.removeEventListener("contextmenu", onMenu);
  }, []);
}

interface Open { x: number; y: number; label: string; items: MenuItem[]; trigger: HTMLElement | null }

/**
 * Clic droit maison : `bind(label, items)` donne le gestionnaire `onContextMenu` d'un élément, `menu` est à rendre
 * une fois dans l'écran. La touche Menu et Maj+F10 (événement sans coordonnées) placent le menu sous l'élément.
 */
export function useContextMenu(): {
  bind: (label: string, items: MenuItem[] | (() => MenuItem[])) => (e: ReactMouseEvent<HTMLElement>) => void;
  menu: ReactNode;
} {
  const [open, setOpen] = useState<Open | null>(null);
  const bind = useCallback((label: string, items: MenuItem[] | (() => MenuItem[])) => (e: ReactMouseEvent<HTMLElement>) => {
    if (keepsNativeMenu(e.target)) return;
    const list = typeof items === "function" ? items() : items;
    if (!list.length) return;
    e.preventDefault();
    e.stopPropagation();
    const el = e.currentTarget;
    let { clientX: x, clientY: y } = e;
    if (x === 0 && y === 0) { const r = el.getBoundingClientRect(); x = r.left + 8; y = r.bottom + 4; }
    setOpen({ x, y, label, items: list, trigger: el });
  }, []);
  const menu = open && (
    <ContextMenu
      label={open.label} x={open.x} y={open.y}
      items={open.items}
      onClose={(restore) => { if (restore) open.trigger?.focus(); setOpen(null); }}
    />
  );
  return { bind, menu };
}
