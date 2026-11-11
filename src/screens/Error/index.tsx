import { useEffect, useRef, useState } from "react";
import "./error.css";
import { useShell } from "../../app/context";
import { Button, Card } from "../../ui";
import { BlockPattern, Cube } from "../Login/Cube";
import { buildStates, resolveState, type Action, type ErrorContext } from "./states";

const stagger = (i: number) => ({ "--i": i }) as React.CSSProperties;

export default function ErrorScreen() {
  const { services, params, navigate } = useShell();
  const id = resolveState(params.state);
  const friendKey = params.friend;
  const instanceId = params.instance;
  const [friendName, setFriendName] = useState<string | undefined>(undefined);
  const others = Number(params.others);
  const ctx: ErrorContext = {
    friend: params.name || friendName,
    file: params.file || undefined,
    others: params.others && Number.isFinite(others) ? others : undefined,
    need: params.need || undefined,
    detail: params.detail || undefined,
  };
  const def = buildStates(ctx)[id];
  const [retryFailed, setRetryFailed] = useState(false);
  const title = useRef<HTMLHeadingElement>(null);
  const [busy, setBusy] = useState(false);
  const [reopenFailed, setReopenFailed] = useState(false);
  const [launchFailed, setLaunchFailed] = useState<string | null>(null);
  const [open, setOpen] = useState(true);
  const [copied, setCopied] = useState(false);
  const copyTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => { title.current?.focus(); setReopenFailed(false); setLaunchFailed(null); }, [id]);
  useEffect(() => () => { if (copyTimer.current) clearTimeout(copyTimer.current); }, []);

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

  const relaunch = async (a: Action & { kind: "launch" }) => {
    if (!instanceId) { navigate(a.fallback); return; }
    setBusy(true);
    setLaunchFailed(null);
    try {
      await services.instancesLaunch(instanceId);
      navigate("main");
    } catch (e) {
      setLaunchFailed(e instanceof Error && e.message ? e.message : "Le lancement a échoué de nouveau.");
    } finally { setBusy(false); }
  };

  const copyDetail = async () => {
    try {
      await navigator.clipboard.writeText(def.detail ?? "");
      setCopied(true);
      if (copyTimer.current) clearTimeout(copyTimer.current);
      copyTimer.current = setTimeout(() => setCopied(false), 2400);
    } catch { /* presse-papiers indisponible : le détail reste sélectionnable */ }
  };

  const variant = (a: { primary?: boolean }) => (a.primary ? "primary" : "secondary") as "primary" | "secondary";

  const renderAction = (a: Action) => {
    if (a.kind === "ext") {
      return <a key={a.label} className="cl-btn cl-btn-secondary" href={a.href} target="_blank" rel="noopener noreferrer">{a.label}</a>;
    }
    if (a.kind === "reopen") {
      return <Button key={a.label} loading={busy} onClick={() => { void reopen(); }}>{a.label}</Button>;
    }
    if (a.kind === "retry") {
      return <Button key={a.label} variant={variant(a)} loading={busy} onClick={() => { void retry(a); }}>{a.label}</Button>;
    }
    if (a.kind === "launch") {
      return <Button key={a.label} variant={variant(a)} loading={busy} onClick={() => { void relaunch(a); }}>{a.label}</Button>;
    }
    return <Button key={a.label} variant={variant(a)} onClick={() => navigate(a.route)}>{a.label}</Button>;
  };

  const hasMore = Boolean(def.more || def.detail);

  return (
    <div className={def.wait ? "err err-wait" : "err"} data-state={id}>
      <BlockPattern />
      <section className="err-panel" aria-labelledby="err-title">
        <div className="enter">
          {def.wait ? (
            <svg className="err-spin" width="72" height="72" viewBox="0 0 24 24" fill="none" stroke="var(--accent)" strokeWidth="2.5" strokeLinecap="round" aria-hidden="true"><path d="M12 3a9 9 0 109 9" /></svg>
          ) : (
            <Cube width={160} crack shadow />
          )}
        </div>
        <div className="err-head enter" style={stagger(1)} role={def.wait ? "status" : "alert"}>
          <span className="err-kicker">{def.kicker}</span>
          <h1 id="err-title" ref={title} tabIndex={-1}>{def.title}</h1>
        </div>
        <p className="err-body enter" style={stagger(1)}>{def.body}</p>
        {reopenFailed && <p className="err-note" role="alert">Impossible de rouvrir la fenêtre. Réessaie dans un instant.</p>}
        {retryFailed && <p className="err-note" role="alert">Impossible de relancer le transfert. Vérifie que ton ami est en ligne, puis réessaie.</p>}
        {launchFailed && <p className="err-note" role="alert">{launchFailed}</p>}
        <div className="err-actions enter" style={stagger(2)}>{def.actions.map(renderAction)}</div>
        {hasMore && (
          <Card padded={false} className="err-more enter" style={stagger(2)}>
            <button type="button" aria-expanded={open} aria-controls="err-more-in" onClick={() => setOpen((o) => !o)}>
              {def.detail ? "Détail technique" : "En savoir plus"}<span>{open ? "Replier" : "Déplier"}</span>
            </button>
            {open && (
              <div className="err-more-in" id="err-more-in">
                {def.detail && <pre className="err-pre">{def.detail}</pre>}
                {def.more && <p>{def.more}</p>}
                {def.detail && (
                  <Button variant="secondary" onClick={() => { void copyDetail(); }}>
                    {copied ? "Copié" : "Copier le détail"}
                  </Button>
                )}
              </div>
            )}
          </Card>
        )}
      </section>
    </div>
  );
}
