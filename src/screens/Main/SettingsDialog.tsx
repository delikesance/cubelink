import { useId, useState, type FormEvent } from "react";
import { Button, Field, Input } from "../../ui";
import { validateName } from "../Create";
import type { Instance, InstancePatch } from "../../services";
import { ICONS, Icon, Panel } from "./Panel";

export interface SettingsDialogProps {
  instance: Instance;
  onSave: (patch: InstancePatch) => Promise<void>;
  onClose: () => void;
  onOpenFolder: () => void;
  /** Suppression confirmée depuis la zone dangereuse. */
  onDelete: () => void;
}

const GB = 1024;
const MIN_GB = 2;
const memLabel = (mb: number) => `${Math.round((mb / GB) * 10) / 10} Go`.replace(".", ",");

/** Panneau « Paramètres » d'une instance (A5) : nom, mémoire, Java, arguments JVM, dossier, zone dangereuse. */
export function SettingsDialog({ instance, onSave, onClose, onOpenFolder, onDelete }: SettingsDialogProps) {
  const uid = useId();
  const [name, setName] = useState(instance.name);
  const [ram, setRam] = useState(instance.ram_mb);
  const [java, setJava] = useState(instance.java_path ?? "");
  const [jvm, setJvm] = useState(instance.jvm_args ?? "");
  const [nameError, setNameError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirmDel, setConfirmDel] = useState(false);

  const maxGb = Math.max(16, Math.ceil(instance.ram_mb / GB));

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (busy) return;
    const problem = validateName(name);
    setNameError(problem);
    if (problem) { document.getElementById(`${uid}-name`)?.focus(); return; }
    setError(null);
    setBusy(true);
    try {
      await onSave({ name: name.trim(), ram_mb: ram, java_path: java.trim() || null, jvm_args: jvm.trim() });
    } catch (err) {
      setError(err instanceof Error && err.message ? err.message : String(err));
      setBusy(false);
    }
  };

  return (
    <Panel onClose={onClose}>
      <form id={`${uid}-form`} className="dr-body dr-form" noValidate onSubmit={(e) => { void submit(e); }}>
        <Field label="Nom de l'instance" id={`${uid}-name`} error={nameError ?? undefined}>
          {(p) => <Input {...p} type="text" value={name} autoComplete="off" onChange={(e) => { setName(e.target.value); setNameError(null); }} />}
        </Field>

        <div className="dr-mem">
          <div className="dr-mem-head">
            <label htmlFor={`${uid}-ram`} className="cl-label">Mémoire allouée</label>
            <output htmlFor={`${uid}-ram`} className="dr-mem-val">{memLabel(ram)}</output>
          </div>
          <input
            id={`${uid}-ram`} className="dr-range" type="range" min={MIN_GB} max={maxGb} step={1}
            value={Math.min(maxGb, Math.max(MIN_GB, Math.round(ram / GB)))}
            aria-valuetext={memLabel(ram)}
            onChange={(e) => setRam(Number(e.target.value) * GB)}
          />
          <div className="dr-mem-scale"><span>{MIN_GB}&nbsp;Go</span><span>{maxGb}&nbsp;Go</span></div>
        </div>

        <Field label="Chemin de Java (facultatif)" id={`${uid}-java`} hint="Vide : Cubelink choisit et installe la bonne version.">
          {(p) => <Input {...p} type="text" value={java} autoComplete="off" placeholder="Automatique" onChange={(e) => setJava(e.target.value)} />}
        </Field>
        <Field label="Arguments JVM" id={`${uid}-jvm`}>
          {(p) => <Input {...p} type="text" value={jvm} autoComplete="off" placeholder="-XX:+UseG1GC" onChange={(e) => setJvm(e.target.value)} />}
        </Field>

        <div className="dr-folder">
          <span className="cl-label">Dossier du jeu</span>
          <Button variant="secondary" icon={<Icon d={ICONS.folder} />} onClick={onOpenFolder}>Ouvrir le dossier</Button>
        </div>

        {error && <p className="home-err" role="alert">{error}</p>}

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
        <Button variant="secondary" onClick={onClose}>Annuler</Button>
        <Button type="submit" form={`${uid}-form`} loading={busy}>{busy ? "Enregistrement…" : "Enregistrer"}</Button>
      </footer>
    </Panel>
  );
}
