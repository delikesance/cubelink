// Correction automatique des mods : recherche des correctifs, prévisualisation cochable, application avec progression.
import { useEffect, useRef, useState } from "react";
import type { FixAction, FixPlan, FixProgressEvent, FixResult, Services } from "../../services";
import { Button } from "../../ui";
import { Modal } from "./Modal";

export interface FixDialogProps {
  services: Pick<Services, "modsFixPlan" | "modsFixApply" | "onModsFixProgress">;
  instanceId: string;
  /** Un seul problème : seules les actions qui le règlent (issue_files) sont cochées, les autres restent proposées décochées. */
  only?: string;
  onClose: () => void;
  onDone: (result: FixResult) => void;
}

const errMsg = (e: unknown) => (e instanceof Error ? e.message : String(e)).replace(/^[a-zéèêàçùîô' -]{3,24}\s:\s+/, "");
const sourceLabel = (a: FixAction) => (a.source === "modrinth" ? "Modrinth" : a.source === "curseforge" ? "CurseForge" : "Local");

function Progress({ label, p }: { label: string; p: FixProgressEvent | null }) {
  const pct = p && p.total > 0 ? Math.min(100, Math.round((p.done / p.total) * 100)) : null;
  return (
    <div className="fx-prog">
      <div className="fx-bar" role="progressbar" aria-label={label} aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct ?? undefined} aria-busy="true">
        <span className={pct === null ? "is-indet" : undefined} style={pct === null ? undefined : { width: `${pct}%` }} />
      </div>
      <p className="fx-status" role="status">{p && p.total > 0 ? `${p.done}/${p.total} · ${p.label}` : label}</p>
    </div>
  );
}

export function FixDialog({ services, instanceId, only, onClose, onDone }: FixDialogProps) {
  const [phase, setPhase] = useState<"planning" | "ready" | "applying" | "error">("planning");
  const [plan, setPlan] = useState<FixPlan | null>(null);
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const [err, setErr] = useState<string | null>(null);
  const [applyErr, setApplyErr] = useState<string | null>(null);
  const [progress, setProgress] = useState<FixProgressEvent | null>(null);
  const [attempt, setAttempt] = useState(0);
  const applyBtn = useRef<HTMLButtonElement>(null);
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);

  useEffect(() => services.onModsFixProgress((p) => { if (alive.current) setProgress(p); }), [services]);

  useEffect(() => {
    let live = true;
    setPhase("planning");
    setProgress(null);
    setErr(null);
    let call: Promise<FixPlan>;
    try { call = services.modsFixPlan(instanceId); } catch (e) { call = Promise.reject(e); }
    call.then(
      (p) => {
        if (!live) return;
        setPlan(p);
        setChecked(new Set(p.actions.filter((a) => !only || a.issue_files.includes(only)).map((a) => a.id)));
        setProgress(null);
        setPhase("ready");
      },
      (e: unknown) => { if (live) { setErr(errMsg(e)); setPhase("error"); } },
    );
    return () => { live = false; };
  }, [services, instanceId, only, attempt]);

  // Focus initial sur « Appliquer » dès que le plan est affiché.
  useEffect(() => { if (phase === "ready") applyBtn.current?.focus(); }, [phase]);

  const toggle = (id: string) => setChecked((s) => { const n = new Set(s); if (n.has(id)) n.delete(id); else n.add(id); return n; });

  const apply = async () => {
    if (!plan || checked.size === 0) return;
    setApplyErr(null);
    setProgress(null);
    setPhase("applying");
    try {
      const ids = plan.actions.filter((a) => checked.has(a.id)).map((a) => a.id);
      const r = await services.modsFixApply(instanceId, plan.plan_id, ids);
      if (alive.current) onDone(r);
    } catch (e) {
      if (alive.current) { setApplyErr(errMsg(e)); setPhase("ready"); }
    }
  };

  const unresolved = (plan?.unresolved ?? []).filter((u) => !only || u.file === only);
  const busy = phase === "applying";
  const close = () => { if (!busy) onClose(); };

  return (
    <Modal title="Corrections proposées" onClose={close}>
      <div className="fx">
        {phase === "planning" && (
          <>
            <Progress label="Recherche des correctifs…" p={progress} />
            <div className="home-modal-foot"><Button variant="secondary" onClick={onClose}>Annuler</Button></div>
          </>
        )}
        {phase === "error" && (
          <>
            <p className="home-err" role="alert">{err}</p>
            <div className="home-modal-foot">
              <Button variant="secondary" onClick={onClose}>Fermer</Button>
              <Button onClick={() => setAttempt((n) => n + 1)}>Réessayer</Button>
            </div>
          </>
        )}
        {(phase === "ready" || phase === "applying") && plan && (
          <>
            {plan.actions.length === 0 ? (
              <p className="home-modal-text">Aucune correction automatique trouvée.</p>
            ) : (
              <ul className="fx-list" aria-label="Corrections">
                {plan.actions.map((a) => (
                  <li key={a.id} className="fx-item">
                    <label>
                      <input type="checkbox" checked={checked.has(a.id)} disabled={busy} onChange={() => toggle(a.id)} />
                      <span className="fx-txt">
                        <span className="fx-t">{a.title}</span>
                        {a.detail && <span className="fx-d">{a.detail}</span>}
                      </span>
                      <span className="dr-chip fx-src">{sourceLabel(a)}</span>
                    </label>
                  </li>
                ))}
              </ul>
            )}
            {unresolved.length > 0 && (
              <section className="fx-un" aria-label="À régler toi-même">
                <h3>À régler toi-même</h3>
                <ul>
                  {unresolved.map((u) => (
                    <li key={`${u.file}-${u.message}`}>
                      <span><strong>{u.mod_name}</strong> : {u.message}</span>
                      <span className="fx-d">{u.reason}</span>
                      {u.page_url && <a className="home-link" href={u.page_url} target="_blank" rel="noopener noreferrer">Ouvrir la page</a>}
                    </li>
                  ))}
                </ul>
              </section>
            )}
            {busy && <Progress label="Application des corrections…" p={progress} />}
            {applyErr && (
              <div className="fx-applyerr">
                <p className="home-err" role="alert">{applyErr}</p>
                {/* Plan périmé (liste des mods changée, délai dépassé) : seul un nouveau calcul peut aboutir. */}
                <Button variant="secondary" onClick={() => { setApplyErr(null); setAttempt((n) => n + 1); }}>Relancer la recherche</Button>
              </div>
            )}
            <div className="home-modal-foot">
              <Button variant="secondary" disabled={busy} onClick={onClose}>Annuler</Button>
              <Button ref={applyBtn} disabled={busy || checked.size === 0} onClick={() => { void apply(); }}>Appliquer ({checked.size})</Button>
            </div>
          </>
        )}
      </div>
    </Modal>
  );
}
