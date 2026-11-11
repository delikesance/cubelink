/* Card : panneau opaque (surface #1B181F, rayon 24, contour fin, ombre douce). Jamais translucide.
   Props : as ("div" defaut | "section" | "article" | "li"...) ; interactive (survol/curseur, a combiner avec role/onClick de l'appelant) ;
   padded (defaut true) ; + attributs HTML. */
import { createElement, type HTMLAttributes } from "react";
import "./ui.css";

export interface CardProps extends HTMLAttributes<HTMLElement> {
  as?: string;
  interactive?: boolean;
  padded?: boolean;
}

export function Card({ as = "div", interactive, padded = true, className, ...rest }: CardProps) {
  const cls = ["cl-card", interactive ? "cl-card-i" : "", padded ? "" : "cl-card-flush", className].filter(Boolean).join(" ");
  return createElement(as, { className: cls, ...rest });
}
