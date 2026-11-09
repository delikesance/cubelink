import { useEffect, useRef, useState } from "react";
import "./add.css";
import { useServices } from "../../app/context";

const CODE_RE = /^(mcl1-[A-Za-z0-9-]{16,}|launcher:\/\/add\/\S+)$/i;

/** Retourne un message d'erreur, ou null si le code est valide. */
export function validateFriendCode(raw: string): string | null {
  const v = raw.trim();
  if (!v) return "Colle le code de ton ami.";
  if (!CODE_RE.test(v)) {
    return "Ce code n'est pas reconnu. Vérifie qu'il commence par « mcl1- » ou « launcher://add/ » et qu'il est complet.";
  }
  return null;
}

function messageOf(e: unknown): string {
  const raw = e instanceof Error ? e.message : typeof e === "string" ? e : "";
  if (!raw) return "L'envoi de la demande a échoué. Réessaie dans un instant.";
  return raw;
}

type Status =
  | { step: "idle" }
  | { step: "sending" }
  | { step: "sent"; verify: string }
  | { step: "error"; message: string };

export default function Add() {
  const services = useServices();
  const [value, setValue] = useState("");
  const [touched, setTouched] = useState(false);
  const [status, setStatus] = useState<Status>({ step: "idle" });
  const alive = useRef(true);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; };
  }, []);

  const empty = value.trim() === "";
  const fieldError = touched && !empty ? validateFriendCode(value) : null;
  const sending = status.step === "sending";
  const serviceError = status.step === "error" ? status.message : null;
  const describedBy = ["add-help", fieldError ? "add-error" : null, serviceError ? "add-svc-error" : null]
    .filter(Boolean).join(" ");

  const onChange = (v: string) => {
    setValue(v);
    if (status.step === "error") setStatus({ step: "idle" });
  };

  const paste = async () => {
    try {
      if (!navigator.clipboard?.readText) throw new Error("indisponible");
      const text = await navigator.clipboard.readText();
      if (!alive.current) return;
      if (text.trim()) { onChange(text.trim()); setTouched(true); }
    } catch {
      // Repli silencieux : le focus revient sur le champ pour Ctrl+V.
    }
    inputRef.current?.focus();
  };

  const submit = async () => {
    if (sending) return;
    setTouched(true);
    if (validateFriendCode(value)) return;
    setStatus({ step: "sending" });
    try {
      const verify = await services.friendSendRequest(value.trim());
      if (!alive.current) return;
      setValue("");
      setTouched(false);
      setStatus({ step: "sent", verify });
    } catch (e) {
      if (alive.current) setStatus({ step: "error", message: messageOf(e) });
    }
  };

  const live =
    status.step === "sending" ? "Envoi de la demande…"
      : status.step === "sent" ? "Demande envoyée. Compare ce code avec celui de ton ami."
        : "";

  return (
    <section className="add" aria-labelledby="add-title">
      <h2 id="add-title">Colle le code de ton ami</h2>
      <form
        className="add-row"
        noValidate
        onSubmit={(e) => { e.preventDefault(); void submit(); }}
      >
        <input
          ref={inputRef}
          className="add-input"
          type="text"
          value={value}
          onChange={(e) => onChange(e.target.value)}
          onBlur={() => { if (value.trim()) setTouched(true); }}
          placeholder="mcl1-…"
          aria-label="Code ami de ton ami"
          aria-describedby={describedBy}
          aria-invalid={fieldError ? true : undefined}
          autoComplete="off"
          spellCheck={false}
          disabled={sending}
        />
        <button type="button" className="add-ghost" onClick={() => { void paste(); }} disabled={sending}>
          Coller
        </button>
        <button type="submit" className="add-btn" disabled={empty || sending}>
          {sending ? "Envoi…" : "Envoyer la demande"}
        </button>
      </form>
      <p id="add-help" className="add-help">Il commence par «&nbsp;mcl1-&nbsp;» ou par launcher://add/…</p>
      {fieldError && <p id="add-error" className="add-error">{fieldError}</p>}
      {serviceError && <p id="add-svc-error" className="add-error" role="alert">{serviceError}</p>}

      {status.step === "sent" && (
        <div className="add-sent">
          <p className="add-ok">Demande envoyée. Compare ce code avec celui de ton ami.</p>
          <code className="add-verify" aria-label={`Code de vérification : ${status.verify}`}>{status.verify}</code>
          <p className="add-help">Les deux codes doivent être identiques. Sinon, annule et redemande un nouveau code.</p>
        </div>
      )}

      <div className="add-live" aria-live="polite" aria-atomic="true">{live}</div>
    </section>
  );
}
