import { friendlyJoinError, classifyJoinError, joinErrorAdvice } from "./messages";
import { useCallback, useEffect, useRef, useState } from "react";
import "./join.css";
import { useBack, useShell } from "../../app/context";
import { Button, Card } from "../../ui";
import { Avatar } from "../Friends/Avatar";
import type { JoinPlan, Presence } from "../../services/types";

type PlanState =
  | { kind: "loading" }
  | { kind: "ready"; plan: JoinPlan }
  | { kind: "error"; message: string };

const mo = (bytes: number) => `${(bytes / 1_000_000).toLocaleString("fr-FR", { maximumFractionDigits: 1 })} Mo`;
const stagger = (i: number) => ({ "--i": i }) as React.CSSProperties;

export default function Join() {
  const { services, params, navigate } = useShell();
  const friend = params.friend;
  const title = useRef<HTMLHeadingElement>(null);
  const [state, setState] = useState<PlanState>({ kind: "loading" });
  const [peer, setPeer] = useState<{ name: string; presence: Presence } | null>(null);
  const peerName = useRef("");
  const [trusted, setTrusted] = useState(false);
  const [starting, setStarting] = useState(false);
  const [startError, setStartError] = useState<string | null>(null);

  // Nom et présence réels du pair (sans eux, libellé neutre).
  useEffect(() => {
    if (!friend) return;
    let alive = true;
    services.friendList().then(
      (l) => {
        const f = l.find((x) => x.friend.public_key === friend);
        if (alive && f) setPeer({ name: f.friend.name, presence: f.presence });
      },
      () => {},
    );
    const off = services.onFriendPresence((e) => {
      if (e.public_key === friend) setPeer((p) => (p ? { ...p, presence: e.presence } : p));
    });
    return () => { alive = false; off(); };
  }, [friend, services]);

  const load = useCallback(() => {
    if (!friend) return () => {};
    let alive = true;
    setState({ kind: "loading" });
    setTrusted(false);
    services.joinPlan(friend).then(
      (plan) => { if (alive) setState({ kind: "ready", plan }); },
      (e: unknown) => {
        if (alive) setState({ kind: "error", message: friendlyJoinError(e, peerName.current) });
      },
    );
    return () => { alive = false; };
  }, [friend, services]);

  useEffect(() => load(), [load]);
  useEffect(() => { title.current?.focus(); }, []);

  const unknown = state.kind === "ready" ? state.plan.unknown_mods : [];
  const unchecked = state.kind === "ready" ? (state.plan.unknown_unchecked ?? []) : [];
  const canJoin = state.kind === "ready" && !starting && (unknown.length === 0 || trusted);

  const join = async () => {
    if (!friend) return;
    setStarting(true);
    setStartError(null);
    try {
      await services.joinStart(friend);
      navigate("install", { friend });
    } catch (e) {
      setStartError(friendlyJoinError(e, peerName.current));
      setStarting(false);
    }
  };

  const cancel = useBack("main");
  const cancelRef = useRef(cancel);
  cancelRef.current = cancel;
  useEffect(() => {
    const onEsc = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !e.defaultPrevented) { e.preventDefault(); cancelRef.current(); }
    };
    document.addEventListener("keydown", onEsc);
    return () => document.removeEventListener("keydown", onEsc);
  }, []);

  const name = peer?.name ?? "ton ami";
  peerName.current = peer?.name ?? "";
  const online = peer?.presence === "online";
  const cancelBtn = <Button variant="secondary" onClick={cancel}>{friend ? "Annuler" : "Retour à l'accueil"}</Button>;

  let card;
  let sub = "";
  if (!friend) {
    sub = "Aucun ami n'est indiqué pour cette invitation.";
    card = (
      <Card as="div" className="join-card enter" style={stagger(1)} role="alert">
        <div className="join-who">
          <Avatar name="?" size={48} tone="var(--line)" />
          <div><h2>Ami introuvable</h2></div>
        </div>
        <p className="join-body">Reviens à l'accueil et choisis un ami dans la liste.</p>
        <div className="join-acts">{cancelBtn}</div>
      </Card>
    );
  } else if (state.kind === "loading") {
    sub = "Récupération du plan de la partie. Rien n'est téléchargé pour l'instant.";
    card = (
      <Card as="div" className="join-card enter" style={stagger(1)} role="status" aria-live="polite">
        <div className="join-who">
          <Avatar name={name} size={48} presence={peer ? (online ? "online" : "offline") : "none"} />
          <div><h2>Préparation…</h2><p>Connexion à {name}</p></div>
        </div>
        <div className="join-body"><div className="join-meter" aria-hidden="true"><span style={{ width: "30%" }} /></div></div>
        <div className="join-acts">{cancelBtn}</div>
      </Card>
    );
  } else if (state.kind === "error") {
    const offline = peer?.presence === "offline";
    sub = offline ? `${name} est hors ligne.` : "La partie n'est pas joignable pour l'instant.";
    card = (
      <Card as="div" className="join-card enter" style={stagger(1)} role="alert">
        <div className="join-who">
          <Avatar name={name} size={48} tone={offline ? "#4A4452" : "var(--danger)"} presence={peer ? (offline ? "offline" : "online") : "none"} />
          <div>
            <h2>Impossible de rejoindre pour l'instant</h2>
            <p>{offline ? `${name} est hors ligne` : name}</p>
          </div>
        </div>
        <div className="join-body">
          <p className="join-error">{state.message}</p>
          <p>{joinErrorAdvice(classifyJoinError(state.message), name) ?? `Vérifie que ${name} est en ligne avec Cubelink ouvert, puis réessaie.`}</p>
        </div>
        <div className="join-acts">
          <Button onClick={load}>Réessayer</Button>
          {cancelBtn}
        </div>
      </Card>
    );
  } else {
    const { plan } = state;
    const m = plan.manifest;
    const total = m.files.length;
    const pct = total > 0 ? Math.max(0, Math.min(100, (plan.to_receive / total) * 100)) : 0;
    sub = "Rien n'est téléchargé avant ton clic.";
    card = (
      <Card as="div" className="join-card enter" style={stagger(1)}>
        <div className="join-who">
          <Avatar name={name} size={48} presence={online ? "online" : "none"} />
          <div>
            <h2>{name}{online ? " · En ligne" : ""}</h2>
            <p>{m.name} · {m.loader.loader} {m.loader.version} · Minecraft {m.minecraft}</p>
          </div>
        </div>
        <div className="join-body">
          <strong>{plan.to_receive}&nbsp;fichiers à recevoir ({mo(plan.to_receive_bytes)}) sur {total}</strong>
          {plan.cached}&nbsp;fichiers sont déjà dans ton cache · Java {m.java_major}
          <div className="join-meter" aria-hidden="true"><span style={{ width: `${pct}%` }} /></div>
        </div>
        <div className="join-acts">
          <Button disabled={!canJoin} loading={starting} onClick={() => { void join(); }}>Rejoindre et jouer</Button>
          {cancelBtn}
          {unknown.length > 0 && !trusted && (
            <span className="join-hint" aria-live="polite">Coche la case de confiance pour activer « Rejoindre et jouer ».</span>
          )}
        </div>
      </Card>
    );
  }

  const ready = state.kind === "ready" ? state.plan : null;
  const files = ready?.manifest.files ?? [];

  return (
    <div className="join">
      <header className="join-head enter">
        <h1 ref={title} tabIndex={-1}>{friend ? `Rejoindre ${name}` : "Rejoindre un ami"}</h1>
        <p>{sub}</p>
      </header>
      <section aria-label="Rejoindre un ami" className="join-extra">
        {card}
        {startError && <p className="join-error" role="alert" style={{ margin: 0, color: "var(--danger)" }}>{startError}</p>}
        {ready && unknown.length > 0 && (
          <div className="join-warn enter" role="note" style={stagger(2)}>
            <h2>{unknown.length > 1 ? `${unknown.length} mods inconnus vont s'exécuter sur ton ordinateur` : "Un mod inconnu va s'exécuter sur ton ordinateur"}</h2>
            <ul>{unknown.map((u) => <li key={u}>{u}</li>)}</ul>
            <p>Ces mods ne sont pas référencés dans les catalogues : Cubelink ne peut pas les comparer à une version connue. Ils s'exécuteront avec les mêmes droits que le jeu. L'intégrité est vérifiée, pas la sécurité : vérifie-les avant d'accepter.</p>
            <label className="join-trust">
              <input type="checkbox" checked={trusted} onChange={(e) => setTrusted(e.target.checked)} />
              <span>Je fais confiance à {name} pour {unknown.length > 1 ? "ces mods" : "ce mod"}</span>
            </label>
          </div>
        )}
        {ready && unchecked.length > 0 && (
          <div className="join-warn enter" role="note" data-testid="unchecked" style={stagger(2)}>
            <h2>{unchecked.length > 1 ? `${unchecked.length} fichiers n'ont pas pu être vérifiés` : "Un fichier n'a pas pu être vérifié"}</h2>
            <ul>{unchecked.map((u) => <li key={u}>{u}</li>)}</ul>
            <p>Cubelink n'a pas pu comparer ces jars aux catalogues (vérification impossible, par exemple sans réseau) : ils ne sont ni reconnus ni signalés comme inconnus. Ne les considère pas comme sûrs.</p>
          </div>
        )}
        {ready && (
          <>
            <details className="join-files enter" style={stagger(3)}>
              <summary>Liste des fichiers ({files.length})</summary>
              <ul>{files.map((f) => <li key={f.path}><span>{f.path}</span><span>{mo(f.size)}</span></li>)}</ul>
            </details>
            <p className="join-note">Chaque fichier est comparé à l'empreinte SHA-256 annoncée ; un fichier qui ne correspond pas est supprimé. Rien n'est téléchargé avant ton clic sur « Rejoindre et jouer ».</p>
          </>
        )}
      </section>
    </div>
  );
}
