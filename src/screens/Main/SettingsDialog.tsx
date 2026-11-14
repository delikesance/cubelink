import { useId, useState, type FormEvent } from "react";
import { Button, Field, Input, RamPicker, useDefaultRam } from "../../ui";
import { useServices } from "../../app/context";
import { validateName } from "../Create";
import type { Instance, InstancePatch } from "../../services";
import { ICONS, Icon } from "../../ui";
import { Panel } from "./Panel";

export interface SettingsDialogProps {
  instance: Instance;
  onSave: (patch: InstancePatch) => Promise<void>;
  /** Absent : panneau en pleine page (pas de bouton Annuler, la page reste ouverte après l'enregistrement). */
  onClose?: () => void;
  onOpenFolder: () => void;
  /** Suppression confirmée depuis la zone dangereuse. */
  onDelete: () => void;
}

/** Système Linux (interface WebKitGTK) : seul système où la commande d'enrobage s'applique. */
export function isLinuxHost(ua: string = typeof navigator === "undefined" ? "" : navigator.userAgent): boolean {
  return /\bLinux\b/.test(ua) && !/Android/i.test(ua);
}

/** Panneau « Paramètres » d'une instance (A5) : nom, mémoire, Java, arguments JVM, commande d'enrobage (Linux), dossier, zone dangereuse. */
export function SettingsDialog({ instance, onSave, onClose, onOpenFolder, onDelete }: SettingsDialogProps) {
  const uid = useId();
  const [name, setName] = useState(instance.name);
  const [ram, setRam] = useState(instance.ram_mb);
  const defaultRam = useDefaultRam(useServices());
  const [java, setJava] = useState(instance.java_path ?? "");
  const [jvm, setJvm] = useState(instance.jvm_args ?? "");
  const [linux] = useState(() => isLinuxHost());
  const [wrapper, setWrapper] = useState(instance.wrapper ?? "");
  const [nameError, setNameError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirmDel, setConfirmDel] = useState(false);
  const [saved, setSaved] = useState(false);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (busy) return;
    const problem = validateName(name);
    setNameError(problem);
    if (problem) { document.getElementById(`${uid}-name`)?.focus(); return; }
    setError(null);
    setSaved(false);
    setBusy(true);
    try {
      const patch: InstancePatch = { name: name.trim(), ram_mb: ram, java_path: java.trim() || null, jvm_args: jvm.trim() };
      if (linux) patch.wrapper = wrapper.trim();
      await onSave(patch);
      // En tiroir le parent se ferme ; en page on reste là et on le dit.
      if (!onClose) { setBusy(false); setSaved(true); }
    } catch (err) {
      setError(err instanceof Error && err.message ? err.message : String(err));
      setBusy(false);
    }
  };

  return (
    <Panel onClose={onClose}>
      <form id={`${uid}-form`} className="dr-body dr-form" noValidate onChange={() => setSaved(false)} onSubmit={(e) => { void submit(e); }}>
        <Field label="Nom de l'instance" id={`${uid}-name`} error={nameError ?? undefined}>
          {(p) => <Input {...p} type="text" value={name} autoComplete="off" onChange={(e) => { setName(e.target.value); setNameError(null); }} />}
        </Field>

        <div className="dr-mem">
          <RamPicker value={ram} onChange={(mb) => { setRam(mb); setSaved(false); }} defaultMb={defaultRam} disabled={busy} />
        </div>

        <Field label="Chemin de Java (facultatif)" id={`${uid}-java`} hint="Vide : Cubelink choisit et installe la bonne version.">
          {(p) => <Input {...p} type="text" value={java} autoComplete="off" placeholder="Automatique" onChange={(e) => setJava(e.target.value)} />}
        </Field>
        <Field label="Arguments JVM" id={`${uid}-jvm`}>
          {(p) => <Input {...p} type="text" value={jvm} autoComplete="off" placeholder="-XX:+UseG1GC" onChange={(e) => setJvm(e.target.value)} />}
        </Field>
        {linux && (
          <Field label="Commande d'enrobage (facultatif)" id={`${uid}-wrapper`} hint="Lancée devant Java, par exemple gamemoderun ou mangohud. Vide : Java est lancé directement.">
            {(p) => <Input {...p} type="text" value={wrapper} autoComplete="off" spellCheck={false} placeholder="gamemoderun" onChange={(e) => setWrapper(e.target.value)} />}
          </Field>
        )}

        <div className="dr-folder">
          <span className="cl-label">Dossier du jeu</span>
          <Button variant="secondary" icon={<Icon d={ICONS.folder} />} onClick={onOpenFolder}>Ouvrir le dossier</Button>
        </div>

        {error && <p className="home-err" role="alert">{error}</p>}
        {saved && !error && <p className="dr-notice" role="status">Réglages enregistrés.</p>}

        <div className="dr-danger">
          <div className="dr-danger-t">Zone dangereuse</div>
          <Button variant="secondary" className="dr-danger-btn" aria-expanded={confirmDel} icon={<Icon d={ICONS.trash} />} onClick={() => setConfirmDel(true)}>
            Supprimer l'instance
          </Button>
          {confirmDel && (
            <div className="dr-confirm enter" role="alertdialog" aria-label="Confirmation de suppression">
              <p><b>Supprimer {instance.name} ?</b> Les mondes et les fichiers de l'instance seront effacés. Impossible d'annuler.</p>
              <div className="dr-confirm-act">
                <Button variant="secondary" data-autofocus onClick={() => setConfirmDel(false)}>Garder</Button>
                <Button variant="danger" onClick={onDelete}>Supprimer</Button>
              </div>
            </div>
          )}
        </div>
      </form>
      <footer className="dr-foot">
        {onClose && <Button variant="secondary" onClick={onClose}>Annuler</Button>}
        <Button type="submit" form={`${uid}-form`} loading={busy}>{busy ? "Enregistrement…" : "Enregistrer"}</Button>
      </footer>
    </Panel>
  );
}
