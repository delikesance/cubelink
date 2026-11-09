import { useCallback, useEffect, useRef, useState } from "react";
import "./join.css";
import { useShell } from "../../app/context";
import type { JoinPlan } from "../../services/types";

type PlanState =
  | { kind: "loading" }
  | { kind: "ready"; plan: JoinPlan }
  | { kind: "error"; message: string };

const mo = (bytes: number) => `${(bytes / 1_000_000).toLocaleString("fr-FR", { maximumFractionDigits: 1 })} Mo`;

export default function Join() {
  const { services, params, navigate } = useShell();
  const friend = params.friend;
  const title = useRef<HTMLHeadingElement>(null);
  const [state, setState] = useState<PlanState>({ kind: "loading" });
  const [trusted, setTrusted] = useState(false);
  const [starting, setStarting] = useState(false);
  const [startError, setStartError] = useState<string | null>(null);

  const load = useCallback(() => {
    if (!friend) return () => {};
    let alive = true;
    setState({ kind: "loading" });
    setTrusted(false);
    services.joinPlan(friend).then(
      (plan) => { if (alive) setState({ kind: "ready", plan }); },
      (e: unknown) => {
        if (alive) setState({ kind: "error", message: e instanceof Error && e.message ? e.message : "Impossible de joindre ton ami. Il est peut-être hors ligne." });
      },
    );
    return () => { alive = false; };
  }, [friend, services]);

  useEffect(() => load(), [load]);
  useEffect(() => { title.current?.focus(); }, [state.kind]);

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
      setStartError(e instanceof Error && e.message ? e.message : "Le téléchargement n'a pas pu démarrer.");
      setStarting(false);
    }
  };

  const cancel = () => navigate("main");

  let content;
  if (!friend) {
    content = (
      <div className="join-body" role="alert">
        <h1 ref={title} tabIndex={-1}>Ami introuvable</h1>
        <p className="join-sub">Aucun ami n'est indiqué pour cette invitation. Reviens à l'accueil et choisis un ami.</p>
      </div>
    );
  } else if (state.kind === "loading") {
    content = (
      <div className="join-body" role="status" aria-live="polite">
        <h1 ref={title} tabIndex={-1}>Préparation…</h1>
        <p className="join-sub">Récupération du plan de ton ami. Rien n'est téléchargé pour l'instant.</p>
      </div>
    );
  } else if (state.kind === "error") {
    content = (
      <div className="join-body" role="alert">
        <h1 ref={title} tabIndex={-1}>Impossible de rejoindre pour l'instant</h1>
        <p className="join-error">{state.message}</p>
        <p className="join-sub">Vérifie que ton ami est en ligne avec Cubelink ouvert, puis réessaie.</p>
      </div>
    );
  } else {
    const { plan } = state;
    const m = plan.manifest;
    content = (
      <div className="join-body">
        <div>
          <h1 ref={title} tabIndex={-1}>Rejoindre « {m.name} »</h1>
          <span className="join-sub">
            {plan.cached}&nbsp;fichiers déjà dans ton cache · {plan.to_receive}&nbsp;à recevoir ({mo(plan.to_receive_bytes)})
          </span>
        </div>
        <ul className="join-facts">
          <li><span>Minecraft</span><strong>{m.minecraft}</strong></li>
          <li><span>Loader</span><strong>{m.loader.loader} {m.loader.version}</strong></li>
          <li><span>Java</span><strong>Java {m.java_major}</strong></li>
          <li><span>À recevoir</span><strong>{plan.to_receive} fichiers · {mo(plan.to_receive_bytes)}</strong></li>
        </ul>
        {unknown.length > 0 && (
          <div className="join-warn" role="note">
            <h2>{unknown.length > 1 ? `${unknown.length} mods inconnus vont s'exécuter sur ton ordinateur` : "Un mod inconnu va s'exécuter sur ton ordinateur"}</h2>
            <ul>{unknown.map((u) => <li key={u}>{u}</li>)}</ul>
            <p>Ces mods ne sont pas référencés dans les catalogues : Cubelink ne peut pas les comparer à une version connue. Ils s'exécuteront avec les mêmes droits que le jeu. L'intégrité est vérifiée, pas la sécurité : vérifie-les avant d'accepter.</p>
            <label className="join-trust">
              <input type="checkbox" checked={trusted} onChange={(e) => setTrusted(e.target.checked)} />
              <span>Je fais confiance à mon ami pour {unknown.length > 1 ? "ces mods" : "ce mod"}</span>
            </label>
          </div>
        )}
        {unchecked.length > 0 && (
          <div className="join-warn" role="note" data-testid="unchecked">
            <h2>{unchecked.length > 1 ? `${unchecked.length} fichiers n'ont pas pu être vérifiés` : "Un fichier n'a pas pu être vérifié"}</h2>
            <ul>{unchecked.map((u) => <li key={u}>{u}</li>)}</ul>
            <p>Cubelink n'a pas pu comparer ces jars aux catalogues (vérification impossible, par exemple sans réseau) : ils ne sont ni reconnus ni signalés comme inconnus. Ne les considère pas comme sûrs.</p>
          </div>
        )}
        <details className="join-files">
          <summary>Liste des fichiers ({m.files.length})</summary>
          <ul>{m.files.map((f) => <li key={f.path}><span>{f.path}</span><span>{mo(f.size)}</span></li>)}</ul>
        </details>
        <p className="join-note">Chaque fichier est comparé à l'empreinte SHA-256 annoncée ; un fichier qui ne correspond pas est supprimé. Rien n'est téléchargé avant ton clic sur « Rejoindre ».</p>
        {startError && <p className="join-error" role="alert">{startError}</p>}
      </div>
    );
  }

  return (
    <div className="join">
      <section className="join-dialog" aria-label="Rejoindre un ami">
        {content}
        <div className="join-foot">
          {friend && state.kind === "ready" && (
            <button type="button" className="join-btn join-pri" disabled={!canJoin} onClick={() => { void join(); }}>Rejoindre</button>
          )}
          {friend && state.kind === "error" && (
            <button type="button" className="join-btn join-pri" onClick={load}>Réessayer</button>
          )}
          <button type="button" className="join-btn" onClick={cancel}>{friend ? "Annuler" : "Retour à l'accueil"}</button>
          {state.kind === "ready" && unknown.length > 0 && !trusted && (
            <span className="join-sub" aria-live="polite">Coche la case de confiance pour activer « Rejoindre ».</span>
          )}
        </div>
      </section>
    </div>
  );
}
