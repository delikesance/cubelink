/* IconButton : bouton compact 44x44 pour une icone seule.
   Props : aria-label OBLIGATOIRE (libelle accessible, aussi utilise comme title) ; icon ;
   variant "ghost" (defaut) | "secondary" | "primary" | "danger" ; + attributs <button>. */
import { forwardRef, type ButtonHTMLAttributes, type ReactNode } from "react";
import "./ui.css";

export interface IconButtonProps extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, "aria-label" | "children"> {
  "aria-label": string;
  icon: ReactNode;
  variant?: "ghost" | "secondary" | "primary" | "danger";
}

export const IconButton = forwardRef<HTMLButtonElement, IconButtonProps>(function IconButton(
  { icon, variant = "ghost", className, type = "button", title, ...rest }, ref,
) {
  const cls = variant === "ghost" ? "cl-iconbtn" : `cl-btn cl-btn-${variant} cl-iconbtn`;
  return (
    <button ref={ref} type={type} title={title ?? rest["aria-label"]} className={[cls, className].filter(Boolean).join(" ")} {...rest}>
      <span aria-hidden="true" className="cl-icon">{icon}</span>
    </button>
  );
});
