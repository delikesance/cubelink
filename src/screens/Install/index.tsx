import { useCallback, useEffect, useRef, useState } from "react";
import "./install.css";
import { useScreenTitle, useShell } from "../../app/context";
import type { TransferErrorDetails, TransferProgress } from "../../services/types";
import { Button, Card } from "../../ui";
import { friendlyJoinError } from "../Join/messages";
import { DefaultArt } from "../Main/DefaultArt";
import { ICONS, Icon } from "../Main/Panel";
import {
  classifyFailure, etaSeconds, formatEta, percent, progressLabel, smoothSpeed,
} from "./format";

type StepState = "todo" | "cur" | "done";
const STEPS = ["Préparation", "Réception des fichiers", "Vérification d'intégrité", "Lancement"] as const;

const PHASE_TEXT: Record<TransferProgress["phase"], string> = {
  preparing: "Préparation en cours",
  receiving: "Réception des fichiers",
  verifying: "Vérification d'intégrité",
  launching: "Lancement du jeu",
  paused: "Transfert en pause",
  failed: "Le transfert a échoué",
  done: "Installation terminée",
};

function stepIndex(phase: TransferProgress["phase"], resumeAt: number): number {
  switch (phase) {
    case "preparing": return 0;
    case "receiving": return 1;
    case "verifying": return 2;
    case "launching": return 3;
    case "done": return 4;
    default: return resumeAt;
  }
}

export default function Install() {
  const { services, params, navigate } = useShell();
  const friend = params.friend ?? "";
  const [p, setP] = useState<TransferProgress | null>(null);
  const [speed, setSpeed] = useState<number | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [playError, setPlayError] = useState<string | null>(null);
  const [errDetails, setErrDetails] = useState<TransferErrorDetails | null>(null);
  const [copied, setCopied] = useState(false);
  const [playing, setPlaying] = useState(false);
  const autoPlayed = useRef(false);
  const [unknownMods, setUnknownMods] = useState<string[]>([]);
  const resumeAt = useRef(0);
  const cancelled = useRef(false);
  const title = useRef<HTMLHeadingElement>(null);
  const cancelBtn = useRef<HTMLButtonElement>(null);

  useScreenTitle("Installation");
  useEffect(() => { title.current?.focus(); }, []);

  useEffect(() => {
    return services.onTransferProgress((next) => {
      if (cancelled.current) return;
      if (next.phase !== "paused" && next.phase !== "failed") {
        resumeAt.current = Math.min(stepIndex(next.phase, resumeAt.current), 3);
      }
      if (next.phase === "failed") {
        const kind = classifyFailure((next as { error?: string }).error);
        if (kind) {
          const file = /[\w.+-]+\.jar/i.exec(next.error ?? "")?.[0];
          navigate("error", { state: kind, ...(friend ? { friend } : {}), ...(file ? { file } : {}) });
          return;
        }
      }
      if (next.phase === "receiving") setSpeed((s) => smoothSpeed(s, next.speed_bps));
      setP(next);
    });
  }, [services, navigate, friend]);

  useEffect(() => services.onTransferErrorDetails((e) => setErrDetails(e)), [services]);

  useEffect(() => services.onUnknownMods((e) => setUnknownMods(e.unknown_mods)), [services]);

  const run = useCallback(async (fn: () => Promise<void>, msg: string) => {
    setBusy(true);
    setActionError(null);
    try { await fn(); } catch { setActionError(msg); } finally { setBusy(false); }
  }, []);

  const confirmCancel = () => run(async () => {
    cancelled.current = true;
    try { await services.joinCancel(); } catch (e) { cancelled.current = false; throw e; }
    navigate("main");
  }, "Impossible d'annuler pour l'instant. Réessaie.");

  const play = () => run(async () => {
    const id = params.instance ?? (await services.instancesList())[0]?.id;
    if (!id) throw new Error("no instance");
    await services.instancesLaunch(id);
    navigate("main");
  }, "Le lancement a échoué. Réessaie.");

  const joinAndPlay = useCallback(async () => {
    setPlaying(true);
    setPlayError(null);
    try {
      const id = params.instance ?? (await services.instancesList())[0]?.id;
      if (!id) throw new Error("Instance introuvable.");
      let name = "";
      try { name = (await services.friendList()).find((f) => f.friend.public_key === friend)?.friend.name ?? ""; } catch { /* nom facultatif */ }
      try { await services.joinPlay(friend, id); } catch (e) { setPlayError(friendlyJoinError(e, name)); return; }
      navigate("main");
    } finally { setPlaying(false); }
  }, [services, friend, params.instance, navigate]);

  useEffect(() => {
    if (p?.phase === "done" && friend && !autoPlayed.current) {
      autoPlayed.current = true;
      void joinAndPlay();
    }
  }, [p?.phase, friend, joinAndPlay]);

  const retry = () => run(async () => {
    cancelled.current = false;
    resumeAt.current = 0;
    setErrDetails(null);
    setCopied(false);
    setP(null);
    setSpeed(null);
    await services.joinStart(friend);
  }, "Impossible de relancer le transfert.");

  const phase = p?.phase ?? "preparing";
  const pct = p ? percent(p) : 0;
  const cur = stepIndex(phase, resumeAt.current);
  const label = p ? progressLabel(p) : "Préparation…";
  const eta = p && phase === "receiving" ? formatEta(etaSeconds(p, speed)) : "";
  const stepState = (i: number): StepState => (i < cur ? "done" : i === cur ? "cur" : "todo");
  const stepText = (i: number, s: StepState) =>
    s === "done" ? "Terminée" : s === "todo" ? "À venir"
      : i === 1 && p ? `En cours · ${p.files_done} sur ${p.files_total}` : "En cours";

  const finished = phase === "done" || phase === "failed";

  return (
    <div className="inst">
      <div className="inst-bg" aria-hidden="true"><DefaultArt className="inst-art" /><div className="inst-veil" /></div>
      <section className="inst-main" aria-labelledby="inst-title">
        <span className="inst-badge enter">Monde d'un ami</span>
        <h1 id="inst-title" ref={title} tabIndex={-1} className="enter" style={{ "--i": 1 } as React.CSSProperties}>Installation en cours</h1>

        <Card className="inst-card enter" style={{ "--i": 2 } as React.CSSProperties}>
          <div className="inst-vh" role="status" aria-live="polite">{PHASE_TEXT[phase]}</div>
          <div className="inst-row">
            <b id="inst-label">{PHASE_TEXT[phase]}</b>
            <span className="inst-pct" aria-hidden="true">{pct} %</span>
          </div>
          <div className={`cl-bar cl-tone-${phase === "failed" ? "danger" : phase === "done" ? "ok" : "accent"}${phase === "preparing" && !p ? " is-indeterminate" : ""}`}
            role="progressbar" aria-labelledby="inst-label"
            aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct} aria-valuetext={`${pct} % · ${label}`}>
            <div className="cl-bar-fill" style={{ width: `${pct}%` }} />
          </div>
          <ol className="inst-steps">
            {STEPS.map((t, i) => {
              const s = stepState(i);
              return (
                <li key={t} className={s} aria-current={s === "cur" ? "step" : undefined}>
                  <span className="inst-ic" aria-hidden="true">{s === "done" ? <Icon d={ICONS.check} size={14} /> : null}</span>
                  <span className="inst-tx">
                    <span className="t">{i + 1}. {t}</span>
                    <span className="s">{stepText(i, s)}</span>
                  </span>
                </li>
              );
            })}
          </ol>
          <span className="inst-stats">{label}{eta ? ` · ${eta}` : ""}</span>

          {phase === "failed" && (
            <div role="alert" className="inst-fail">
              <p className="inst-err">{p?.error || errDetails?.message || "Le transfert a échoué."} Tu peux réessayer ou revenir à l'accueil.</p>
              {errDetails?.details && (
                <details className="inst-details">
                  <summary>Détails</summary>
                  <pre tabIndex={0} aria-label="Détails de l'erreur">{errDetails.details}</pre>
                  <Button variant="secondary" onClick={() => {
                    void navigator.clipboard?.writeText(errDetails.details).then(() => setCopied(true), () => setCopied(false));
                  }}>{copied ? "Copié" : "Copier"}</Button>
                  {errDetails.log_file && <p className="inst-note">Journal complet : {errDetails.log_file}</p>}
                </details>
              )}
            </div>
          )}
          {actionError && <p className="inst-err" role="alert">{actionError}</p>}
          {playError && <p className="inst-err" role="alert">{playError}</p>}

          {confirming ? (
            <div className="inst-confirm" role="group" aria-label="Confirmer l'annulation">
              <p>Annuler l'installation ? Les fichiers déjà reçus restent dans ton cache.</p>
              <div className="inst-actions" style={{ border: 0, padding: 0 }}>
                <Button variant="danger" disabled={busy} onClick={() => { void confirmCancel(); }}>Oui, annuler</Button>
                <Button variant="secondary" autoFocus onClick={() => { setConfirming(false); queueMicrotask(() => cancelBtn.current?.focus()); }}>Continuer l'installation</Button>
              </div>
            </div>
          ) : (
            <div className="inst-actions">
              {phase === "done" && friend && playing && <span role="status">Lancement et connexion…</span>}
              {phase === "done" && friend && playError && <Button size="lg" disabled={busy || playing} onClick={() => { void joinAndPlay(); }}>Réessayer</Button>}
              {phase === "done" && !friend && <Button size="lg" disabled={busy} onClick={() => { void play(); }}>Jouer</Button>}
              {phase === "failed" && <Button disabled={busy} onClick={() => { void retry(); }}>Réessayer</Button>}
              <Button variant="secondary" ref={cancelBtn} disabled={busy}
                onClick={() => (finished ? navigate("main") : setConfirming(true))}>
                {finished ? "Retour à l'accueil" : "Annuler"}
              </Button>
              {!finished && phase !== "launching" && (
                phase === "paused"
                  ? <Button variant="secondary" disabled={busy} onClick={() => { void run(() => services.joinResume(), "Impossible de reprendre."); }}>Reprendre</Button>
                  : <Button variant="secondary" disabled={busy || phase === "preparing"} onClick={() => { void run(() => services.joinPause(), "Impossible de mettre en pause."); }}>Pause</Button>
              )}
            </div>
          )}
        </Card>
        {unknownMods.length > 0 && (
          <span className="inst-note" role="note">
            {unknownMods.length > 1 ? `${unknownMods.length} mods reçus ne sont pas référencés` : "Un mod reçu n'est pas référencé"} dans les catalogues : {unknownMods.join(", ")}.
          </span>
        )}
        {p?.via_relay && (
          <span className="inst-note">Transfert via un relais (serveur intermédiaire), plus lent qu'une connexion directe. Les fichiers restent vérifiés par leur empreinte.</span>
        )}
      </section>
    </div>
  );
}
