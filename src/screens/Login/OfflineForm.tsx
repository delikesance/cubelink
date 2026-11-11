import { useState } from "react";
import { Button, Field, Input } from "../../ui";
import { validateOfflineName } from "../../services/accounts";
import { messageOf } from "./useDeviceLogin";

/** Champ pseudo + validation pour un compte hors ligne (écran Connexion et Réglages). */
export function OfflineForm({ id, submitLabel, onSubmit, onCancel }: {
  id?: string;
  submitLabel: string;
  onSubmit: (name: string) => Promise<unknown>;
  onCancel?: () => void;
}) {
  const [pseudo, setPseudo] = useState("");
  const [error, setError] = useState<string | undefined>(undefined);
  const [busy, setBusy] = useState(false);

  const submit = async () => {
    const name = pseudo.trim();
    const invalid = validateOfflineName(name);
    if (invalid) { setError(invalid); return; }
    setBusy(true);
    setError(undefined);
    try {
      await onSubmit(name);
    } catch (e) {
      setError(messageOf(e));
    }
    setBusy(false);
  };

  return (
    <form id={id} className="login-offline enter" noValidate
      onKeyDown={(e) => { if (e.key === "Escape" && onCancel) { e.stopPropagation(); onCancel(); } }}
      onSubmit={(e) => { e.preventDefault(); void submit(); }}>
      <div className="row">
        <Field label="Pseudo hors ligne" hint="3 à 16 caractères : lettres, chiffres ou _." error={error}>
          {(p) => <Input {...p} value={pseudo} autoComplete="off" spellCheck={false} maxLength={32} autoFocus onChange={(e) => setPseudo(e.target.value)} />}
        </Field>
        <Button type="submit" variant="secondary" loading={busy}>{submitLabel}</Button>
      </div>
    </form>
  );
}
