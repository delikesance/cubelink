/* Field + Input : champ de formulaire etiquete.
   <Field label="Nom" hint="aide" error="message">{(p) => <Input {...p} />}</Field>
   Field relie label/aide/erreur au controle (htmlFor, aria-describedby, invalid) via la fonction enfant ;
   avec un enfant element simple, fournir id/aria-* soi-meme (prop id de Field = id du controle).
   Input : <input> stylise (44 px), prop invalid pour l'etat d'erreur. L'erreur est annoncee (role="alert"). */
import { forwardRef, useId, type InputHTMLAttributes, type ReactNode } from "react";
import "./ui.css";

export interface InputProps extends InputHTMLAttributes<HTMLInputElement> { invalid?: boolean }
export const Input = forwardRef<HTMLInputElement, InputProps>(function Input({ invalid, className, ...rest }, ref) {
  return <input ref={ref} className={["cl-input", className].filter(Boolean).join(" ")} aria-invalid={invalid || undefined} {...rest} />;
});

export interface FieldControlProps { id: string; "aria-describedby"?: string; invalid?: boolean }
export interface FieldProps {
  label: string;
  hint?: string;
  error?: string;
  id?: string;
  className?: string;
  children: ReactNode | ((p: FieldControlProps) => ReactNode);
}

export function Field({ label, hint, error, id, className, children }: FieldProps) {
  const auto = useId();
  const cid = id ?? auto;
  const hid = `${cid}-hint`, eid = `${cid}-err`;
  const describedBy = [hint ? hid : "", error ? eid : ""].filter(Boolean).join(" ") || undefined;
  return (
    <div className={["cl-field", className].filter(Boolean).join(" ")}>
      <label htmlFor={cid} className="cl-label">{label}</label>
      {typeof children === "function" ? children({ id: cid, "aria-describedby": describedBy, invalid: !!error }) : children}
      {hint && <p id={hid} className="cl-hint">{hint}</p>}
      {error && <p id={eid} className="cl-error" role="alert">{error}</p>}
    </div>
  );
}
