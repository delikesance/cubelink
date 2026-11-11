/* Button : bouton « clay » avec tranche.
   Props : variant "primary" (defaut) | "secondary" | "danger" ; size "md" (44 px, defaut) | "lg" (56 px, reserve a « Jouer ») ;
   loading (affiche une pastille, aria-busy, clic ignore) ; disabled ; icon (ReactNode optionnel avant le libelle) ;
   + tous les attributs <button>. Bouton icone seul : utiliser IconButton (aria-label obligatoire).
   Un bouton `loading` reste focalisable (aria-disabled) pour ne pas perdre le focus. */
import { forwardRef, type ButtonHTMLAttributes, type ReactNode } from "react";
import "./ui.css";

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: "primary" | "secondary" | "danger";
  size?: "md" | "lg";
  loading?: boolean;
  icon?: ReactNode;
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { variant = "primary", size = "md", loading = false, icon, className, children, onClick, type = "button", ...rest }, ref,
) {
  return (
    <button
      ref={ref} type={type}
      className={["cl-btn", `cl-btn-${variant}`, size === "lg" ? "cl-btn-lg" : "", className].filter(Boolean).join(" ")}
      aria-busy={loading || undefined}
      aria-disabled={loading || undefined}
      onClick={loading ? (e) => e.preventDefault() : onClick}
      {...rest}
    >
      {loading ? <span className="cl-spin" aria-hidden="true" /> : icon}
      {children}
    </button>
  );
});
