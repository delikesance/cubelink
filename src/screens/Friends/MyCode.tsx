import { useCallback, useEffect, useRef, useState } from "react";
import "./mycode.css";
import { useServices } from "../../app/context";
import { Button } from "../../ui";

type State =
  | { step: "loading" }
  | { step: "ready"; code: string }
  | { step: "error"; message: string };

function messageOf(e: unknown): string {
  if (e instanceof Error && e.message) return e.message;
  if (typeof e === "string" && e) return e;
  return "Impossible de récupérer ton code ami. Réessaie dans un instant.";
}

export default function MyCode() {
  const services = useServices();
  const [state, setState] = useState<State>({ step: "loading" });
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [regenError, setRegenError] = useState<string | null>(null);
  const [notice, setNotice] = useState("");
  const codeRef = useRef<HTMLElement>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      if (timer.current) clearTimeout(timer.current);
    };
  }, []);

  const load = useCallback(async () => {
    setState({ step: "loading" });
    try {
      const code = await services.friendMyCode();
      if (alive.current) setState({ step: "ready", code });
    } catch (e) {
      if (alive.current) setState({ step: "error", message: messageOf(e) });
    }
  }, [services]);

  useEffect(() => { void load(); }, [load]);

  const say = (msg: string) => {
    setNotice(msg);
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => setNotice(""), 2000);
  };

  const selectCode = () => {
    const el = codeRef.current;
    const sel = window.getSelection();
    if (!el || !sel) return;
    const r = document.createRange();
    r.selectNodeContents(el);
    sel.removeAllRanges();
    sel.addRange(r);
  };

  const copy = async () => {
    if (state.step !== "ready") return;
    try {
      if (!navigator.clipboard?.writeText) throw new Error("indisponible");
      await navigator.clipboard.writeText(state.code);
      say("Copié");
    } catch {
      selectCode();
      if (timer.current) clearTimeout(timer.current);
      setNotice("Copie impossible : le code est sélectionné, utilise Ctrl+C.");
    }
  };

  const regenerate = async () => {
    setBusy(true);
    setRegenError(null);
    try {
      const code = await services.friendRegenerateCode();
      if (!alive.current) return;
      setState({ step: "ready", code });
      setConfirming(false);
      say("Nouveau code généré");
    } catch (e) {
      if (alive.current) setRegenError(messageOf(e));
    } finally {
      if (alive.current) setBusy(false);
    }
  };

  return (
    <section className="mycode" aria-labelledby="mycode-title">
      <div className="mycode-head">
        <div>
          <h2 id="mycode-title">Mon code</h2>
          <p className="mycode-sub">Partage-le pour qu'un ami t'ajoute.</p>
        </div>
      </div>
      <div className="mycode-tools">
        {!confirming && (
          <button
            type="button"
            className="mycode-link"
            disabled={state.step !== "ready"}
            onClick={() => { setRegenError(null); setConfirming(true); }}
          >
            Générer un nouveau code
          </button>
        )}
      </div>

      {confirming && (
        <div className="mycode-confirm" role="group" aria-label="Confirmer le nouveau code">
          <p>Ton ancien code ne fonctionnera plus. Tes amis actuels ne sont pas affectés. Continuer&nbsp;?</p>
          <div className="mycode-actions">
            <Button variant="danger" onClick={() => { void regenerate(); }} loading={busy}>Générer un nouveau code</Button>
            <Button variant="secondary" onClick={() => setConfirming(false)} disabled={busy}>Annuler</Button>
          </div>
          {regenError && <p className="mycode-error" role="alert">{regenError}</p>}
        </div>
      )}

      <div className="mycode-row">
        {state.step === "loading" && <p className="mycode-info" role="status">Chargement de ton code…</p>}
        {state.step === "error" && (
          <>
            <p className="mycode-error" role="alert">{state.message}</p>
            <Button onClick={() => { void load(); }}>Réessayer</Button>
          </>
        )}
        {state.step === "ready" && (
          <>
            <code className="mycode-code" ref={codeRef} aria-label={`Mon code ami : ${state.code}`}>{state.code}</code>
            <Button variant="secondary" className={notice === "Copié" ? "is-copied" : undefined} onClick={() => { void copy(); }}>{notice === "Copié" ? "Copié" : "Copier"}</Button>
          </>
        )}
      </div>

      <div className="mycode-live" aria-live="polite" aria-atomic="true">{notice === "Copié" ? "" : notice}</div>
      <span className="mycode-help">Ce code ne contient aucun secret. Il permet à ton ami de te joindre&nbsp;: ne le publie pas.</span>
    </section>
  );
}
