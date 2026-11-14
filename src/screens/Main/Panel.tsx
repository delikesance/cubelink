import type { ReactNode } from "react";
import { ICONS, Icon, IconButton } from "../../ui";
import "./shared.css";

/** Contenu d'un panneau : zone centrale + pied ; bouton « Fermer » en haut à droite seulement si `onClose` (tiroir), sans lui en pleine page. */
export function Panel({ onClose, children }: { onClose?: () => void; children: ReactNode }) {
  return (
    <div className="dr-panel">
      {children}
      {onClose && <IconButton variant="secondary" className="dr-close" aria-label="Fermer" icon={<Icon d={ICONS.close} />} onClick={onClose} />}
    </div>
  );
}
