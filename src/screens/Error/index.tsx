import { useEffect, useRef, useState } from "react";
import "./error.css";
import { useShell } from "../../app/context";
import { Button, Card } from "../../ui";
import { BlockPattern, Logo } from "../Login/Logo";
import { buildStates, resolveState, type Action, type ErrorContext } from "./states";

const stagger = (i: number) => ({ "--i": i }) as React.CSSProperties;

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
  const title = useRef<HTMLHeadingElement>(null);
  const [open, setOpen] = useState(true);

  useEffect(() => { title.current?.focus(); }, [id]);

  useEffect(() => {
    if (!friendKey || params.name) return;
    let alive = true;
    services.friendList().then(
      (l) => { if (alive) setFriendName(l.find((f) => f.friend.public_key === friendKey)?.friend.name); },
      () => {},
    );
    return () => { alive = false; };
  }, [services, friendKey, params.name]);

  // Le parcours ami se déroule maintenant dans l'écran Rejoindre : on y retourne (le plan est recalculé).
  const retry = (a: Action & { kind: "retry" }) => {
    navigate(friendKey ? "join" : a.fallback, friendKey ? { friend: friendKey } : undefined);
  };

  const variant = (a: { primary?: boolean }) => (a.primary ? "primary" : "secondary") as "primary" | "secondary";

  const renderAction = (a: Action) => {
    if (a.kind === "ext") {
      return <a key={a.label} className="cl-btn cl-btn-secondary" href={a.href} target="_blank" rel="noopener noreferrer">{a.label}</a>;
    }
    if (a.kind === "retry") {
      return <Button key={a.label} variant={variant(a)} onClick={() => retry(a)}>{a.label}</Button>;
    }
    return <Button key={a.label} variant={variant(a)} onClick={() => navigate(a.route)}>{a.label}</Button>;
  };

  return (
    <div className="err" data-state={id}>
      <BlockPattern />
      <section className="err-panel" aria-labelledby="err-title">
        <div className="enter">
          <Logo width={160} far shadow />
        </div>
        <div className="err-head enter" style={stagger(1)} role="alert">
          <span className="err-kicker">{def.kicker}</span>
          <h1 id="err-title" ref={title} tabIndex={-1}>{def.title}</h1>
        </div>
        <p className="err-body enter" style={stagger(1)}>{def.body}</p>
        <div className="err-actions enter" style={stagger(2)}>{def.actions.map(renderAction)}</div>
        {def.more && (
          <Card padded={false} className="err-more enter" style={stagger(2)}>
            <button type="button" aria-expanded={open} aria-controls="err-more-in" onClick={() => setOpen((o) => !o)}>
              En savoir plus<span>{open ? "Replier" : "Déplier"}</span>
            </button>
            {open && (
              <div className="err-more-in" id="err-more-in">
                <p>{def.more}</p>
              </div>
            )}
          </Card>
        )}
      </section>
    </div>
  );
}
