import { useRef, useState, type ReactNode } from "react";
import "./install.css";
import { Button, Card } from "../../ui";
import { ICONS, Icon } from "../Main/Panel";
import { etaSeconds, formatEta, percent, progressLabel } from "./format";
import type { useTransfer } from "./useTransfer";
import type { TransferProgress } from "../../services/types";

type StepState = "todo" | "cur" | "done";
const STEPS = ["Préparation", "Réception des fichiers", "Vérification d'intégrité", "Lancement"] as const;

export const PHASE_TEXT: Record<TransferProgress["phase"], string> = {
  preparing: "Préparation en cours",
  receiving: "Réception des fichiers",
  verifying: "Vérification d'intégrité",
  launching: "Lancement du jeu",
  paused: "Transfert en pause",
  failed: "Le transfert a échoué",
  done: "Installation terminée",
};

export interface TransferFailure { title: string; body: ReactNode; more?: string }

interface Props {
  t: ReturnType<typeof useTransfer>;
  /** Contenu de l'échec (par défaut : message réel du transfert). */
  failure?: TransferFailure;
  /** Message d'erreur supplémentaire (lancement, connexion...). */
  extraError?: string | null;
  /** Boutons propres à l'appelant (Jouer, Réessayer...), placés avant Annuler. */
  extra?: ReactNode;
  /** Ligne de statut à côté des boutons. */
  status?: ReactNode;
  onCancelled: () => void;
  onHome: () => void;
  onPause: () => Promise<void>;
  onResume: () => Promise<void>;
  style?: React.CSSProperties;
}

/** Carte de progression d'un transfert : barre, étapes, échec, actions (annuler/pause/reprise). */
export function TransferPanel({ t, failure, extraError, extra, status, onCancelled, onHome, onPause, onResume, style }: Props) {
  const { p, phase, speed, errDetails, unknownMods, busy, actionError, run, cancel, cur } = t;
  const [confirming, setConfirming] = useState(false);
  const [copied, setCopied] = useState(false);
  const cancelBtn = useRef<HTMLButtonElement>(null);

  const pct = p ? percent(p) : 0;
  const label = p ? progressLabel(p) : "Préparation…";
  const eta = p && phase === "receiving" ? formatEta(etaSeconds(p, speed)) : "";
  const stepState = (i: number): StepState => (i < cur ? "done" : i === cur ? "cur" : "todo");
  const stepText = (i: number, s: StepState) =>
    s === "done" ? "Terminée" : s === "todo" ? "À venir"
      : i === 1 && p ? `En cours · ${p.files_done} sur ${p.files_total}` : "En cours";
  const finished = phase === "done" || phase === "failed";

  return (
    <>
      <Card className="inst-card enter" style={style}>
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
          {STEPS.map((s, i) => {
            const st = stepState(i);
            return (
              <li key={s} className={st} aria-current={st === "cur" ? "step" : undefined}>
                <span className="inst-ic" aria-hidden="true">{st === "done" ? <Icon d={ICONS.check} size={14} /> : null}</span>
                <span className="inst-tx">
                  <span className="t">{i + 1}. {s}</span>
                  <span className="s">{stepText(i, st)}</span>
                </span>
              </li>
            );
          })}
        </ol>
        <span className="inst-stats">{label}{eta ? ` · ${eta}` : ""}</span>

        {phase === "failed" && (
          <div role="alert" className="inst-fail">
            {failure ? (
              <>
                <p className="inst-err">{failure.title}</p>
                <p className="inst-note">{failure.body}{failure.more ? ` ${failure.more}` : ""}</p>
              </>
            ) : (
              <p className="inst-err">{p?.error || errDetails?.message || "Le transfert a échoué."} Tu peux réessayer ou revenir à l'accueil.</p>
            )}
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
        {extraError && <p className="inst-err" role="alert">{extraError}</p>}

        {confirming ? (
          <div className="inst-confirm" role="group" aria-label="Confirmer l'annulation">
            <p>Annuler l'installation ? Les fichiers déjà reçus restent dans ton cache.</p>
            <div className="inst-actions" style={{ border: 0, padding: 0 }}>
              <Button variant="danger" disabled={busy} onClick={() => { void cancel(onCancelled); }}>Oui, annuler</Button>
              <Button variant="secondary" autoFocus onClick={() => { setConfirming(false); queueMicrotask(() => cancelBtn.current?.focus()); }}>Continuer l'installation</Button>
            </div>
          </div>
        ) : (
          <div className="inst-actions">
            {status}
            {extra}
            <Button variant="secondary" ref={cancelBtn} disabled={busy}
              onClick={() => (finished ? onHome() : setConfirming(true))}>
              {finished ? "Retour à l'accueil" : "Annuler"}
            </Button>
            {!finished && phase !== "launching" && (
              phase === "paused"
                ? <Button variant="secondary" disabled={busy} onClick={() => { void run(onResume, "Impossible de reprendre."); }}>Reprendre</Button>
                : <Button variant="secondary" disabled={busy || phase === "preparing"} onClick={() => { void run(onPause, "Impossible de mettre en pause."); }}>Pause</Button>
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
    </>
  );
}
