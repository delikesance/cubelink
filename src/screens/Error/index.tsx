import { useEffect, useRef, useState } from "react";
import "./error.css";
import { useShell } from "../../app/context";
import { buildStates, resolveState, type Action, type ErrorContext } from "./states";

export default function ErrorScreen() {
  const { services, params, navigate } = useShell();
  const id = resolveState(params.state);
  const friendKey = params.friend;
  const [friendName, setFriendName] = useState<string | undefined>(undefined);
  const others = Number(params.others);
  const ctx: ErrorContext = {
    friend: params.name || friendName,
    file: params.file || undefined,
    others: params.others && Number.isFinite(others) ? others : undefined,
    need: params.need || undefined,
  };
  const def = buildStates(ctx)[id];
  const [retryFailed, setRetryFailed] = useState(false);
  const title = useRef<HTMLHeadingElement>(null);
  const [busy, setBusy] = useState(false);
  const [reopenFailed, setReopenFailed] = useState(false);

  useEffect(() => { title.current?.focus(); setReopenFailed(false); }, [id]);

  useEffect(() => {
    if (!friendKey || params.name) return;
    let alive = true;
    services.friendList().then(
      (l) => { if (alive) setFriendName(l.find((f) => f.friend.public_key === friendKey)?.friend.name); },
      () => {},
    );
    return () => { alive = false; };
  }, [services, friendKey, params.name]);

  const retry = async (a: Action & { kind: "retry" }) => {
    if (!friendKey) { navigate(a.fallback); return; }
    setBusy(true);
    setRetryFailed(false);
    try {
      await services.joinStart(friendKey);
      navigate("install", { friend: friendKey });
    } catch { setRetryFailed(true); } finally { setBusy(false); }
  };

  const reopen = async () => {
    setBusy(true);
    setReopenFailed(false);
    try { await services.authBeginLogin(); } catch { setReopenFailed(true); } finally { setBusy(false); }
  };

  const renderAction = (a: Action) => {
    if (a.kind === "ext") {
      return <a key={a.label} className="err-btn" href={a.href} target="_blank" rel="noopener noreferrer">{a.label}</a>;
    }
    if (a.kind === "reopen") {
      return <button key={a.label} type="button" className="err-btn" disabled={busy} onClick={() => { void reopen(); }}>{a.label}</button>;
    }
    if (a.kind === "retry") {
      return <button key={a.label} type="button" className={a.primary ? "err-btn err-pri" : "err-btn"} disabled={busy} onClick={() => { void retry(a); }}>{a.label}</button>;
    }
    return (
      <button key={a.label} type="button" className={a.primary ? "err-btn err-pri" : "err-btn"} onClick={() => navigate(a.route)}>
        {a.label}
      </button>
    );
  };

  return (
    <div className={def.wait ? "err err-wait" : "err"} data-state={id}>
      <section className="err-panel" aria-labelledby="err-title">
        <div className="err-head" role={def.wait ? "status" : "alert"}>
          <svg className={def.wait ? "err-spin" : undefined} width="40" height="40" viewBox="0 0 24 24" fill="none" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"
            stroke={def.wait ? "#FF5C00" : "#FF8A80"} strokeWidth={def.wait ? 2.5 : 2}>
            {def.wait ? <path d="M12 3a9 9 0 109 9" /> : <><path d="M12 3l10 18H2L12 3z" /><path d="M12 10v5M12 18v.5" /></>}
          </svg>
          <span className="err-kicker">{def.kicker}</span>
          <h2 id="err-title" ref={title} tabIndex={-1}>{def.title}</h2>
        </div>
        <p>{def.body}</p>
        {def.more && <details className="err-more"><summary>En savoir plus</summary><p>{def.more}</p></details>}
        {reopenFailed && <p role="alert">Impossible de rouvrir la fenêtre. Réessaie dans un instant.</p>}
        {retryFailed && <p role="alert">Impossible de relancer le transfert. Vérifie que ton ami est en ligne, puis réessaie.</p>}
        <div className="err-actions">{def.actions.map(renderAction)}</div>
      </section>
    </div>
  );
}
