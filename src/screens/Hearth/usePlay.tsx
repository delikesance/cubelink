import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import type { Services } from "../../services/types";
import { Button } from "../../ui";
import { Modal } from "../Main/Modal";
import { errText, isEulaRequired } from "./format";

/**
 * Lance une action Hearth qui peut démarrer un serveur (jouer, reprise après partie mal terminée).
 * Si le contrat d'utilisation de Minecraft n'est pas encore accepté, une boîte l'explique une seule fois ;
 * « J'accepte » enregistre le choix puis relance l'action.
 */
export function useHearthRun(services: Services): {
  run: (action: () => Promise<void>) => Promise<void>;
  error: string | null;
  busy: boolean;
  clearError: () => void;
  eula: ReactNode;
} {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [pending, setPending] = useState<(() => Promise<void>) | null>(null);
  const [url, setUrl] = useState("https://aka.ms/MinecraftEULA");
  const alive = useRef(true);
  useEffect(() => () => { alive.current = false; }, []);

  const run = useCallback(async (action: () => Promise<void>) => {
    setError(null);
    setBusy(true);
    try {
      await action();
    } catch (e) {
      if (!alive.current) return;
      if (isEulaRequired(e)) {
        services.hearthEula().then((x) => { if (alive.current) setUrl(x.url); }, () => {});
        setPending(() => action);
      } else {
        setError(errText(e));
      }
    } finally {
      if (alive.current) setBusy(false);
    }
  }, [services]);

  const accept = async () => {
    const action = pending;
    if (!action) return;
    try {
      await services.hearthEulaAccept();
    } catch (e) {
      setPending(null);
      setError(errText(e));
      return;
    }
    setPending(null);
    await run(action);
  };

  const eula = pending ? (
    <Modal title="Contrat d'utilisation de Minecraft" onClose={() => setPending(null)}>
      <div className="home-modal-text hearth-eula">
        <p>
          Pour héberger le serveur du groupe, Cubelink lance le serveur officiel de Minecraft sur ton PC.
          Mojang demande pour cela d'accepter son contrat d'utilisation.
        </p>
        <p><a href={url} target="_blank" rel="noreferrer">Lire le contrat d'utilisation de Minecraft</a></p>
        <p>Cette question n'est posée qu'une fois.</p>
      </div>
      <div className="home-modal-foot">
        <Button variant="secondary" onClick={() => setPending(null)}>Annuler</Button>
        <Button data-autofocus onClick={() => { void accept(); }}>J'accepte</Button>
      </div>
    </Modal>
  ) : null;

  return { run, error, busy, clearError: () => setError(null), eula };
}
