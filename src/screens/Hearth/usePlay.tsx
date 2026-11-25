import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import type { Services } from "../../services/types";
import { Button } from "../../ui";
import { Modal } from "../Main/Modal";
import { errText, isEulaRequired, parseHeadUnavailable, type HeadUnavailable } from "./format";

/**
 * Lance une action Hearth qui peut démarrer un serveur (jouer, reprise après partie mal terminée).
 * Si le contrat d'utilisation de Minecraft n'est pas encore accepté, une boîte l'explique une seule fois ;
 * « J'accepte » enregistre le choix puis relance l'action.
 * Si la dernière version n'a de contenu chez personne de joignable (`head_unavailable`), une boîte propose d'attendre son
 * auteur ou de jouer depuis la dernière version complète.
 */
export function useHearthRun(services: Services): {
  /** `groupId` : le groupe concerné, pour proposer le repli quand la dernière version n'a de contenu chez personne. */
  run: (action: () => Promise<void>, groupId?: string) => Promise<void>;
  error: string | null;
  busy: boolean;
  clearError: () => void;
  eula: ReactNode;
} {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [pending, setPending] = useState<{ action: () => Promise<void>; groupId?: string } | null>(null);
  const [missing, setMissing] = useState<{ info: HeadUnavailable; id: string } | null>(null);
  const [url, setUrl] = useState("https://aka.ms/MinecraftEULA");
  const alive = useRef(true);
  useEffect(() => () => { alive.current = false; }, []);

  const run = useCallback(async (action: () => Promise<void>, groupId?: string) => {
    setError(null);
    setBusy(true);
    try {
      await action();
    } catch (e) {
      if (!alive.current) return;
      if (isEulaRequired(e)) {
        services.hearthEula().then((x) => { if (alive.current) setUrl(x.url); }, () => {});
        setPending({ action, groupId });
      } else if (groupId && parseHeadUnavailable(e)) {
        setMissing({ info: parseHeadUnavailable(e)!, id: groupId });
      } else {
        setError(errText(e));
      }
    } finally {
      if (alive.current) setBusy(false);
    }
  }, [services]);

  const accept = async () => {
    const again = pending;
    if (!again) return;
    try {
      await services.hearthEulaAccept();
    } catch (e) {
      setPending(null);
      setError(errText(e));
      return;
    }
    setPending(null);
    await run(again.action, again.groupId);
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

  const playFromPrevious = () => {
    const m = missing;
    setMissing(null);
    if (m) void run(() => services.hearthPlayFromPrevious(m.id));
  };

  const headDialog = missing ? (
    <Modal title={`La partie de ${missing.info.author} n'a pas fini d'être envoyée`} onClose={() => setMissing(null)}>
      <div className="home-modal-text">
        <p>Personne de connecté n'a encore le monde de la n° {missing.info.head}.</p>
        {missing.info.previous !== null ? (
          <p>
            Tu peux attendre {missing.info.author}, ou continuer depuis la n° {missing.info.previous}. Dans ce cas, quand{" "}
            {missing.info.author} reviendra, vous aurez deux versions du monde et vous choisirez laquelle garder.
          </p>
        ) : (
          <p>Tu n'as aucune version complète à laquelle revenir : il faut attendre {missing.info.author}.</p>
        )}
      </div>
      <div className="home-modal-foot">
        <Button variant="secondary" onClick={() => setMissing(null)}>Attendre {missing.info.author}</Button>
        {missing.info.previous !== null && <Button data-autofocus onClick={playFromPrevious}>Jouer depuis la n° {missing.info.previous}</Button>}
      </div>
    </Modal>
  ) : null;

  return { run, error, busy, clearError: () => setError(null), eula: <>{eula}{headDialog}</> };
}
