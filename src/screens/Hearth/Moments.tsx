import { useEffect, useState } from "react";
import "./hearth.css";
import type { HearthHandover, HearthHostPrompt, HearthRamWarning, Services } from "../../services/types";
import { Button } from "../../ui";
import { Modal } from "../Main/Modal";
import { useHearthInvites } from "../../app/useHearth";
import { errText, plural } from "./format";
import { useHearthRun } from "./usePlay";
import { useHearthEnabled } from "./useEnabled";

/** Invitation à un groupe : « Rejoindre le groupe » ou « Plus tard » (l'invitation reste proposée à la prochaine ouverture). */
function InviteMoment({ services }: { services: Services }) {
  const invites = useHearthInvites(services);
  const [later, setLater] = useState<Set<string>>(() => new Set());
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const inv = invites.find((i) => !later.has(i.group_id));
  if (!inv) return null;
  const accept = async () => {
    setError(null);
    setBusy(true);
    try { await services.hearthInviteAccept(inv.group_id); }
    catch (e) { setError(errText(e)); }
    finally { setBusy(false); }
  };
  return (
    <Modal title="Invitation à un serveur de groupe" onClose={() => setLater((s) => new Set(s).add(inv.group_id))}>
      <div className="home-modal-text hearth-moment">
        <p><strong>{inv.from.name}</strong> t'invite dans le groupe <strong>« {inv.name} »</strong>.</p>
        <p>{plural(inv.members, "membre", "membres")} · Minecraft {inv.minecraft} · {inv.loader}. Le monde est copié sur ton PC et reste à jour entre vous.</p>
        {error && <p className="hearth-err" role="alert">{error}</p>}
      </div>
      <div className="home-modal-foot">
        <Button variant="secondary" onClick={() => setLater((s) => new Set(s).add(inv.group_id))}>Plus tard</Button>
        <Button loading={busy} data-autofocus onClick={() => { void accept(); }}>Rejoindre le groupe</Button>
      </div>
    </Modal>
  );
}

/** L'hôte a arrêté le serveur pendant que tu jouais : tu peux prendre le relais. */
function HandoverMoment({ services, e, onClose }: { services: Services; e: HearthHandover; onClose: () => void }) {
  const { run, error, busy, eula } = useHearthRun(services);
  return (
    <Modal title="L'hôte a arrêté le serveur" onClose={onClose}>
      <div className="home-modal-text hearth-moment">
        <p><strong>{e.from_name}</strong> a arrêté le serveur « {e.name} » pendant que tu jouais.</p>
        <p>Tu peux reprendre le monde chez toi pour que la partie continue.</p>
        {error && <p className="hearth-err" role="alert">{error}</p>}
      </div>
      <div className="home-modal-foot">
        <Button variant="secondary" onClick={onClose}>Fermer</Button>
        <Button loading={busy} data-autofocus onClick={() => { void run(async () => { await services.hearthPlay(e.group_id); onClose(); }, e.group_id); }}>Reprendre le serveur</Button>
      </div>
      {eula}
    </Modal>
  );
}

/** Ton jeu s'est fermé alors que d'autres joueurs sont encore sur ton serveur. */
function HostPromptMoment({ services, e, onClose }: { services: Services; e: HearthHostPrompt; onClose: () => void }) {
  const [always, setAlways] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const keep = async () => {
    setBusy(true);
    try {
      if (always) await services.hearthSetBackground(e.group_id, true);
      onClose();
    } catch (x) { setError(errText(x)); }
    finally { setBusy(false); }
  };
  const stop = async () => {
    setBusy(true);
    try { await services.hearthStop(e.group_id); onClose(); }
    catch (x) { setError(errText(x)); }
    finally { setBusy(false); }
  };
  return (
    <Modal title="Ton serveur est encore ouvert" role="alertdialog" onClose={onClose}>
      <div className="home-modal-text hearth-moment">
        <p>Ton jeu est fermé, mais {plural(e.players, "joueur est", "joueurs sont")} encore sur « {e.name} ».</p>
        <label className="hearth-check">
          <input type="checkbox" checked={always} onChange={(ev) => setAlways(ev.target.checked)} />
          <span>Toujours garder le serveur ouvert quand mon jeu se ferme</span>
        </label>
        <p className="hearth-hint">Même ouvert, le serveur s'arrête tout seul après 30 minutes sans joueur.</p>
        {error && <p className="hearth-err" role="alert">{error}</p>}
      </div>
      <div className="home-modal-foot">
        <Button variant="danger" loading={busy} onClick={() => { void stop(); }}>Arrêter le serveur</Button>
        <Button loading={busy} data-autofocus onClick={() => { void keep(); }}>Garder le serveur ouvert</Button>
      </div>
    </Modal>
  );
}

/** Le serveur démarre, mais le PC risque de manquer de mémoire : simple information, le lancement continue. */
function RamWarningMoment({ e, onClose }: { e: HearthRamWarning; onClose: () => void }) {
  return (
    <Modal title="Ton PC risque de manquer de mémoire" onClose={onClose}>
      <div className="home-modal-text hearth-moment">
        <p>{e.message}</p>
        <p className="hearth-hint">Le serveur « {e.name} » démarre quand même.</p>
      </div>
      <div className="home-modal-foot">
        <Button data-autofocus onClick={onClose}>Compris</Button>
      </div>
    </Modal>
  );
}

/** Les moments du groupe qui interrompent : invitation reçue, passage de relais, serveur resté ouvert, mémoire juste. */
export function HearthMoments({ services }: { services: Services }) {
  const [handover, setHandover] = useState<HearthHandover | null>(null);
  const [prompt, setPrompt] = useState<HearthHostPrompt | null>(null);
  const [ram, setRam] = useState<HearthRamWarning | null>(null);
  const enabled = useHearthEnabled(services);
  useEffect(() => services.onHearthHandover((e) => setHandover(e)), [services]);
  useEffect(() => services.onHearthHostPrompt((e) => setPrompt(e)), [services]);
  useEffect(() => services.onHearthRamWarning((e) => setRam(e)), [services]);
  return (
    <>
      {enabled && <InviteMoment services={services} />}
      {handover && <HandoverMoment services={services} e={handover} onClose={() => setHandover(null)} />}
      {prompt && <HostPromptMoment services={services} e={prompt} onClose={() => setPrompt(null)} />}
      {ram && <RamWarningMoment e={ram} onClose={() => setRam(null)} />}
    </>
  );
}
