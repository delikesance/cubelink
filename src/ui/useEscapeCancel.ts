import { useEffect, useRef } from "react";

/**
 * Confirmation en ligne : Échap l'annule au lieu de laisser l'écran parent (Paramètres) revenir en arrière.
 *
 * Tant que `active` est vrai, un écouteur en phase de capture sur `document` avale la touche Échap
 * (preventDefault + stopPropagation) puis appelle `onCancel`. La capture passe avant le gestionnaire
 * de l'écran parent, quel que soit l'élément qui a le focus. `onCancel` décide s'il annule vraiment
 * (par exemple pas pendant un appel en cours), mais Échap reste avalé dans tous les cas.
 */
export function useEscapeCancel(active: boolean, onCancel: () => void): void {
  const cancel = useRef(onCancel);
  cancel.current = onCancel;
  useEffect(() => {
    if (!active) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      e.preventDefault();
      e.stopPropagation();
      cancel.current();
    };
    document.addEventListener("keydown", onKey, true);
    return () => document.removeEventListener("keydown", onKey, true);
  }, [active]);
}
