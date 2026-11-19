/* BackButton : l'unique bouton « retour » des écrans (haut de page). Bouton secondaire de taille standard (44 px) avec chevron ;
   libellé court : « Retour » par défaut, ou la destination (« Accueil », « Retour aux résultats »). */
import type { ButtonHTMLAttributes } from "react";
import { Button } from "./Button";

export interface BackButtonProps extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, "children"> {
  label?: string;
}

export function BackButton({ label = "Retour", ...rest }: BackButtonProps) {
  return (
    <Button
      variant="secondary" {...rest}
      icon={<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M15 6l-6 6 6 6" /></svg>}
    >{label}</Button>
  );
}
