// Contrôle de compatibilité avant lancement : une erreur détectée ouvre une confirmation, sinon le jeu démarre directement.
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import type { Instance, ModIssue, Services } from "../services";
import { Button } from "../ui";
import { Modal } from "../screens/Main/Modal";

/** Au-delà, on lance sans bloquer : la vérification ne doit jamais empêcher de jouer. */
export const CHECK_TIMEOUT_MS = 4000;
const SHOWN = 3;

export interface ModCheckGate {
  /** Vérifie `instance` puis appelle `onPlay`, ou ouvre la confirmation s'il y a des erreurs. */
  guard: (instance: Pick<Instance, "id" | "name">, onPlay: () => void, onSeeMods?: () => void, onFix?: () => void) => void;
  checking: boolean;
  /** À rendre dans la page. */
  modal: ReactNode;
}

interface Pending { errors: ModIssue[]; onPlay: () => void; onSeeMods?: () => void; onFix?: () => void }

export function useModCheckGate(services: Pick<Services, "modsCheck">): ModCheckGate {
  const [checking, setChecking] = useState(false);
  const [pending, setPending] = useState<Pending | null>(null);
  const busy = useRef(false);
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);

  const guard = useCallback<ModCheckGate["guard"]>((instance, onPlay, onSeeMods, onFix) => {
    if (busy.current) return;
    busy.current = true;
    setChecking(true);
    let timer: number | undefined;
    const timeout = new Promise<null>((r) => { timer = window.setTimeout(() => r(null), CHECK_TIMEOUT_MS); });
    let call: Promise<ModIssue[] | null>;
    try { call = services.modsCheck(instance.id).then((c) => c.issues, () => null); }
    catch { call = Promise.resolve(null); }
    void Promise.race([call, timeout]).then((issues) => {
      window.clearTimeout(timer);
      busy.current = false;
      if (alive.current) setChecking(false);
      const errors = (issues ?? []).filter((i) => i.severity === "error");
      if (errors.length === 0) onPlay();
      else if (alive.current) setPending({ errors, onPlay, onSeeMods, onFix });
    });
  }, [services]);

  const close = () => setPending(null);
  const modal = pending && (
    <Modal title="Ce pack risque de planter" role="alertdialog" onClose={close}>
      <div className="home-modal-text">
        <p>{pending.errors.length} problème{pending.errors.length > 1 ? "s" : ""} de compatibilité détecté{pending.errors.length > 1 ? "s" : ""} :</p>
        <ul className="dr-issue-short">
          {pending.errors.slice(0, SHOWN).map((i, k) => <li key={`${i.file}-${k}`}><strong>{i.mod_name}</strong> : {i.message}</li>)}
        </ul>
        {pending.errors.length > SHOWN && <p>… et {pending.errors.length - SHOWN} autre{pending.errors.length - SHOWN > 1 ? "s" : ""}</p>}
      </div>
      <div className="home-modal-foot">
        <Button variant="secondary" data-autofocus onClick={() => { const p = pending; close(); p.onSeeMods?.(); }}>Voir les mods</Button>
        <Button variant="secondary" onClick={() => { const p = pending; close(); p.onPlay(); }}>Jouer quand même</Button>
        {pending.onFix && <Button onClick={() => { const p = pending; close(); p.onFix?.(); }}>Corriger</Button>}
      </div>
    </Modal>
  );

  return { guard, checking, modal };
}
