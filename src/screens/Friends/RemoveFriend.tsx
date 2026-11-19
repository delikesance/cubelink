import { useState } from "react";
import { Button } from "../../ui";
import { Modal } from "../Main/Modal";
import { errMsg } from "../../app/useLaunch";
import { useEscapeCancel } from "../../ui/useEscapeCancel";
import type { Services } from "../../services/types";

/** Confirmation « Retirer des amis » : à ouvrir pour un ami, ferme et prévient le parent une fois retiré. */
export function RemoveFriendDialog({ services, friend, onClose, onRemoved }: {
  services: Services;
  friend: { public_key: string; name: string };
  onClose: () => void;
  onRemoved: (publicKey: string) => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const confirm = async () => {
    setBusy(true);
    setError(null);
    try {
      await services.friendRemove(friend.public_key);
      onRemoved(friend.public_key);
    } catch (e) {
      setError(errMsg(e));
      setBusy(false);
    }
  };
  // Échap annule la boîte au lieu de quitter Paramètres ; avalé pendant le retrait.
  useEscapeCancel(true, () => { if (!busy) onClose(); });
  return (
    <Modal title="Retirer cet ami ?" role="alertdialog" onClose={busy ? () => {} : onClose}>
      <p className="home-modal-text">
        {friend.name} ne verra plus ta présence ni tes invitations, et si tu héberges une partie, sa connexion est coupée.
        Il pourra te redemander en ami avec ton code : tu peux le refuser, ou régénérer ton code.
      </p>
      <p className="home-modal-text">S'il est membre d'un groupe Hearth, il y reste : retire-le aussi du groupe.</p>
      {error && <p className="home-modal-text" role="alert" style={{ color: "var(--danger)" }}>{error}</p>}
      <div className="home-modal-foot">
        <Button variant="secondary" data-autofocus disabled={busy} onClick={onClose}>Annuler</Button>
        <Button variant="danger" loading={busy} onClick={() => { void confirm(); }}>Retirer</Button>
      </div>
    </Modal>
  );
}
