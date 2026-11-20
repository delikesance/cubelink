import { useId, useState } from "react";
import { useServices } from "../../app/context";
import { Button, Card, Field, Input } from "../../ui";
import { messageOf } from "../Login/useDeviceLogin";

const EXTENSION = "cubelink-id";
export const MIN_PASSPHRASE = 8;

type Mode = "none" | "export" | "import";
type Done = { kind: "exported" | "staged"; key: string };

const short = (key: string) => `${key.slice(0, 8)}…`;

/** Réglages > Compte : sauvegarde chiffrée de l'identité (clé + amis) et restauration depuis un fichier `.cubelink-id`. */
export function IdentityBackup() {
  const services = useServices();
  const uid = useId();
  const [mode, setMode] = useState<Mode>("none");
  const [pass, setPass] = useState("");
  const [confirm, setConfirm] = useState("");
  const [error, setError] = useState<string | undefined>(undefined);
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState<Done | null>(null);
  const [restarting, setRestarting] = useState(false);

  const open = (m: Mode) => {
    setMode((cur) => (cur === m && !done ? "none" : m));
    setPass("");
    setConfirm("");
    setError(undefined);
    setDone(null);
  };

  const exportIdentity = async () => {
    if ([...pass].length < MIN_PASSPHRASE) { setError(`La phrase de passe doit faire ${MIN_PASSPHRASE} caractères au minimum.`); return; }
    if (pass !== confirm) { setError("Les deux phrases de passe ne sont pas identiques."); return; }
    setBusy(true);
    setError(undefined);
    try {
      let path = await services.pickSavePath({ defaultName: `Cubelink.${EXTENSION}`, extension: EXTENSION });
      if (!path) return;
      if (!path.toLowerCase().endsWith(`.${EXTENSION}`)) path += `.${EXTENSION}`;
      const key = await services.identityExport(path, pass);
      setPass("");
      setConfirm("");
      setDone({ kind: "exported", key });
    } catch (e) {
      setError(messageOf(e));
    } finally {
      setBusy(false);
    }
  };

  const importIdentity = async () => {
    if (!pass) { setError("Saisis la phrase de passe de la sauvegarde."); return; }
    setBusy(true);
    setError(undefined);
    try {
      const [path] = await services.pickFiles({ extensions: [EXTENSION] });
      if (!path) return;
      const key = await services.identityImport(path, pass);
      setPass("");
      setDone({ kind: "staged", key });
    } catch (e) {
      setError(messageOf(e));
    } finally {
      setBusy(false);
    }
  };

  const restart = async () => {
    setRestarting(true);
    setError(undefined);
    try {
      await services.identityRestart();
    } catch (e) {
      setError(messageOf(e));
      setRestarting(false);
    }
  };

  const form = mode !== "none" && !done;
  return (
    <Card as="section" aria-label="Sauvegarde de l'identité" className="acc-card idb-card">
      <div className="set-txt">
        <b>Ton identité Cubelink</b>
        <span>
          Ta clé d'identité et ta liste d'amis restent sur ce PC. Si tu changes de PC ou réinstalles Windows sans sauvegarde,
          tes amis devront refaire l'échange de codes avec toi. Garde le fichier de sauvegarde hors de ce PC.
        </span>
      </div>
      <div className="set-acts">
        <Button variant="secondary" aria-expanded={mode === "export"} aria-controls={`${uid}-panel`} onClick={() => open("export")}>Sauvegarder mon identité</Button>
        <Button variant="secondary" aria-expanded={mode === "import"} aria-controls={`${uid}-panel`} onClick={() => open("import")}>Restaurer une identité</Button>
      </div>

      <div id={`${uid}-panel`} role="status" aria-live="polite" className="idb-live">
        {done?.kind === "exported" && (
          <p className="idb-ok">Sauvegarde enregistrée pour l'identité {short(done.key)}. Garde bien ta phrase de passe : sans elle, le fichier est inutilisable.</p>
        )}
      </div>

      {form && mode === "export" && (
        <form className="acc-panel enter" aria-label="Sauvegarder mon identité" noValidate onSubmit={(e) => { e.preventDefault(); void exportIdentity(); }}>
          <p className="set-warn idb-warn">
            Si tu perds cette phrase de passe, la sauvegarde est perdue : personne ne peut la récupérer, pas même nous.
            Les groupes Hearth ne sont pas encore inclus.
          </p>
          <Field label="Phrase de passe" hint={`${MIN_PASSPHRASE} caractères au minimum. Une phrase de plusieurs mots est plus sûre.`}>
            {(p) => <Input {...p} type="password" value={pass} autoComplete="new-password" spellCheck={false} autoFocus onChange={(e) => setPass(e.target.value)} />}
          </Field>
          <Field label="Confirme la phrase de passe" error={error}>
            {(p) => <Input {...p} type="password" value={confirm} autoComplete="new-password" spellCheck={false} onChange={(e) => setConfirm(e.target.value)} />}
          </Field>
          <div className="set-acts">
            <Button type="submit" loading={busy}>Choisir où enregistrer…</Button>
            <Button type="button" variant="secondary" onClick={() => open("none")}>Annuler</Button>
          </div>
        </form>
      )}

      {form && mode === "import" && (
        <form className="acc-panel enter" aria-label="Restaurer une identité" noValidate onSubmit={(e) => { e.preventDefault(); void importIdentity(); }}>
          <p className="set-warn idb-warn">
            La restauration remplace ton identité et ta liste d'amis actuelles. Une copie de l'existant est gardée dans le dossier
            de données (fichiers .bak). Cubelink redémarre pour l'appliquer. N'utilise pas la même identité sur deux PC en même temps.
          </p>
          <Field label="Phrase de passe de la sauvegarde" error={error}>
            {(p) => <Input {...p} type="password" value={pass} autoComplete="current-password" spellCheck={false} autoFocus onChange={(e) => setPass(e.target.value)} />}
          </Field>
          <div className="set-acts">
            <Button type="submit" loading={busy}>Choisir la sauvegarde…</Button>
            <Button type="button" variant="secondary" onClick={() => open("none")}>Annuler</Button>
          </div>
        </form>
      )}

      {done?.kind === "staged" && (
        <div className="acc-panel enter" role="group" aria-label="Restauration prête">
          <p className="idb-ok">Sauvegarde vérifiée : identité {short(done.key)}. Cubelink doit redémarrer pour la restaurer.</p>
          {error && <p className="set-error" role="alert">{error}</p>}
          <div className="set-acts">
            <Button loading={restarting} onClick={() => { void restart(); }}>Redémarrer maintenant</Button>
            <Button variant="secondary" disabled={restarting} onClick={() => open("none")}>Plus tard</Button>
          </div>
          <p className="set-hint">Si tu choisis « Plus tard », la restauration s'applique au prochain démarrage de Cubelink.</p>
        </div>
      )}
    </Card>
  );
}
