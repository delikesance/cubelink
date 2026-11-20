import { useEffect, useRef, useState } from "react";
import "./add.css";
import { useServices } from "../../app/context";
import { takeFriendLink, usePendingFriendLink } from "../../app/friendLink";
import { clearOutgoing, trackOutgoing, type Outgoing, useOutgoing, useOutgoingIntake } from "../../app/outgoingRequest";
import { Button } from "../../ui";

const ErrIcon = () => (
  <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true"><circle cx="12" cy="12" r="9" /><path d="M12 7.5v5.5M12 16.5v.01" /></svg>
);

const CODE_RE = /^(mcl1-[A-Za-z0-9-]{16,}|(cubelink|launcher):\/\/add\/\S+)$/i;

/** Retourne un message d'erreur, ou null si le code est valide. */
export function validateFriendCode(raw: string): string | null {
  const v = raw.trim();
  if (!v) return "Colle le code de ton ami.";
  if (!CODE_RE.test(v)) {
    return "Ce code n'est pas reconnu. Vérifie qu'il commence par « mcl1- » ou « cubelink://add/ » et qu'il est complet.";
  }
  return null;
}

function messageOf(e: unknown): string {
  const raw = e instanceof Error ? e.message : typeof e === "string" ? e : "";
  if (!raw) return "L'envoi de la demande a échoué. Réessaie dans un instant.";
  return raw;
}

/** Envoi en cours ou en erreur ; la demande envoyée (en attente ou répondue) vit dans `outgoingRequest`. */
type Status =
  | { step: "idle" }
  | { step: "sending" }
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

  // Demande envoyée et sa réponse : suivies hors de l'écran (le Shell écoute aussi), pour ne rien perdre si on le quitte.
  useOutgoingIntake(services);
  const outgoing = useOutgoing();
  const waiting = outgoing?.step === "sent";
  const busy = waiting || status.step === "sending";

  // Lien ami ouvert depuis le système (clic sur cubelink://add/…) : il remplit le champ, sans rien envoyer. Pendant
  // qu'une demande est en cours, il attend (sinon on perdrait le code de vérification à comparer).
  const incoming = usePendingFriendLink();
  const [fromLink, setFromLink] = useState(false);
  useEffect(() => {
    if (!incoming || busy) return;
    const link = takeFriendLink();
    if (!link) return;
    setValue(link);
    setTouched(true);
    setFromLink(true);
    setStatus((s) => (s.step === "error" ? { step: "idle" } : s));
    if (outgoing?.step === "answered") clearOutgoing();
    inputRef.current?.focus();
  }, [incoming, busy, outgoing]);

  const empty = value.trim() === "";
  const fieldError = touched && !empty ? validateFriendCode(value) : null;
  const sending = status.step === "sending";
  const serviceError = status.step === "error" ? status.message : null;
  const describedBy = ["add-help", fieldError ? "add-error" : null, serviceError ? "add-svc-error" : null]
    .filter(Boolean).join(" ");

  const onChange = (v: string) => {
    setValue(v);
    setFromLink(false);
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
    if (busy) return;
    setTouched(true);
    if (validateFriendCode(value)) return;
    setFromLink(false);
    clearOutgoing();
    setStatus({ step: "sending" });
    try {
      const sentCode = value.trim();
      const verify = await services.friendSendRequest(sentCode);
      // Suivie même si l'écran a été quitté pendant l'envoi : la demande est partie, sa réponse doit être retrouvée.
      trackOutgoing(verify, sentCode);
      if (!alive.current) return;
      setValue("");
      setTouched(false);
      setStatus({ step: "idle" });
    } catch (e) {
      if (alive.current) setStatus({ step: "error", message: messageOf(e) });
    }
  };

  const cancel = async (code: string) => {
    try { await services.friendCancelRequest(code); } catch { /* idempotent : rien de plus à faire */ }
    clearOutgoing();
    if (!alive.current) return;
    setStatus({ step: "idle" });
    inputRef.current?.focus();
  };

  const answeredText = (s: Extract<Outgoing, { step: "answered" }>) =>
    s.kind === "accepted" ? `${s.name || "Ton ami"} a accepté ta demande. Vous êtes amis.`
      : s.kind === "refused" ? "Ta demande n'a pas été acceptée."
        : "Ta demande est restée sans réponse. Vérifie que ton ami est connecté, puis redemande-lui son code.";

  const live =
    fromLink && status.step === "idle" && !outgoing ? "Lien d'ami reçu. Rien n'est envoyé tant que tu ne cliques pas sur « Envoyer la demande »."
    : status.step === "sending" ? "Envoi de la demande…"
      : outgoing?.step === "sent" ? "Demande envoyée. Compare ce code avec celui de ton ami."
        : outgoing?.step === "answered" ? answeredText(outgoing)
          : "";

  return (
    <section className="add" aria-labelledby="add-title">
      <h2 id="add-title">Ajouter un ami</h2>
      <p className="add-sub">Colle le code de ton ami. Il reçoit ta demande tout de suite.</p>
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
        <Button variant="secondary" onClick={() => { void paste(); }} disabled={sending}>Coller</Button>
        <Button type="submit" disabled={empty || waiting} loading={sending}>Envoyer la demande</Button>
      </form>
      <p id="add-help" className="add-help">Il commence par «&nbsp;mcl1-&nbsp;» ou par cubelink://add/…</p>
      {fromLink && status.step === "idle" && !outgoing && (
        <p className="add-help">Tu as ouvert un lien d'ami : rien n'est envoyé pour l'instant. Si tu connais cette personne, clique sur «&nbsp;Envoyer la demande&nbsp;».</p>
      )}
      {incoming && busy && (
        <p className="add-help">Un lien d'ami est arrivé : il sera repris quand cette demande sera terminée.</p>
      )}
      {fieldError && <p id="add-error" className="add-error"><ErrIcon />{fieldError}</p>}
      {serviceError && <p id="add-svc-error" className="add-error" role="alert"><ErrIcon />{serviceError}</p>}

      {outgoing?.step === "sent" && (
        <div className="add-sent">
          <p className="add-ok">Demande envoyée. Compare ce code avec celui de ton ami.</p>
          <code className="add-verify" aria-label={`Code de vérification : ${outgoing.verify}`}>{outgoing.verify}</code>
          <p className="add-help">Les deux codes doivent être identiques. Sinon, annule la demande puis redemande-la : une seule demande peut attendre à la fois.</p>
          <div className="add-sent-actions">
            <Button variant="secondary" onClick={() => { void cancel(outgoing.code); }}>Annuler la demande</Button>
          </div>
        </div>
      )}
      {outgoing?.step === "answered" && (
        <div className={`add-sent${outgoing.kind === "accepted" ? "" : " add-sent-neutral"}`}>
          <p className={outgoing.kind === "accepted" ? "add-ok" : "add-help"}>{answeredText(outgoing)}</p>
        </div>
      )}

      <div className="add-live" aria-live="polite" aria-atomic="true">{live}</div>
    </section>
  );
}
