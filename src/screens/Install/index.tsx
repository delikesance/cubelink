import { useCallback, useEffect, useRef, useState } from "react";
import "./install.css";
import { useScreenTitle, useShell } from "../../app/context";
import type { TransferProgress } from "../../services/types";
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

  const retry = () => run(async () => {
    cancelled.current = false;
    resumeAt.current = 0;
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
      <section className="inst-main" aria-labelledby="inst-title">
        <span className="inst-badge">Monde d'un ami</span>
        <h1 id="inst-title" ref={title} tabIndex={-1}>Installation en cours</h1>

        <div className="inst-card">
          <div className="inst-vh" role="status" aria-live="polite">{PHASE_TEXT[phase]}</div>
          <div className="inst-row">
            <b id="inst-label">{PHASE_TEXT[phase]}</b>
            <span className="inst-pct" aria-hidden="true">{pct} %</span>
          </div>
          <div className="inst-bar" role="progressbar" aria-labelledby="inst-label"
            aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct} aria-valuetext={`${pct} % · ${label}`}>
            <div className="inst-fill" style={{ width: `${pct}%` }} />
          </div>
          <ol className="inst-steps">
            {STEPS.map((t, i) => {
              const s = stepState(i);
              return (
                <li key={t} className={s === "cur" ? "cur" : s === "done" ? "done" : ""} aria-current={s === "cur" ? "step" : undefined}>
                  <span className="t">{i + 1}. {t}</span>
                  <span className="s">{stepText(i, s)}</span>
                </li>
              );
            })}
          </ol>
          <span className="inst-stats">{label}{eta ? ` · ${eta}` : ""}</span>

          {phase === "failed" && (
            <p className="inst-err" role="alert">Le transfert a échoué. Tu peux réessayer ou revenir à l'accueil.</p>
          )}
          {actionError && <p className="inst-err" role="alert">{actionError}</p>}

          {confirming ? (
            <div className="inst-confirm" role="group" aria-label="Confirmer l'annulation">
              <p>Annuler l'installation ? Les fichiers déjà reçus restent dans ton cache.</p>
              <div className="inst-actions" style={{ border: 0, padding: 0 }}>
                <button type="button" className="inst-btn inst-cancel" disabled={busy} onClick={() => { void confirmCancel(); }}>Oui, annuler</button>
                <button type="button" className="inst-btn" autoFocus onClick={() => { setConfirming(false); queueMicrotask(() => cancelBtn.current?.focus()); }}>Continuer l'installation</button>
              </div>
            </div>
          ) : (
            <div className="inst-actions">
              {phase === "done" && (
                <button type="button" className="inst-btn inst-pri" disabled={busy} onClick={() => { void play(); }}>Jouer</button>
              )}
              {phase === "failed" && (
                <button type="button" className="inst-btn inst-pri" disabled={busy} onClick={() => { void retry(); }}>Réessayer</button>
              )}
              <button type="button" ref={cancelBtn} className="inst-btn inst-cancel" disabled={busy}
                onClick={() => (finished ? navigate("main") : setConfirming(true))}>
                {finished ? "Retour à l'accueil" : "Annuler"}
              </button>
              {!finished && phase !== "launching" && (
                phase === "paused"
                  ? <button type="button" className="inst-btn" disabled={busy} onClick={() => { void run(() => services.joinResume(), "Impossible de reprendre."); }}>Reprendre</button>
                  : <button type="button" className="inst-btn" disabled={busy || phase === "preparing"} onClick={() => { void run(() => services.joinPause(), "Impossible de mettre en pause."); }}>Pause</button>
              )}
            </div>
          )}
        </div>
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
