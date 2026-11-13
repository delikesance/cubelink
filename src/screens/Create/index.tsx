import { useEffect, useId, useRef, useState, type FormEvent, type ReactNode } from "react";
import "./create.css";
import { useBack, useScreenTitle, useServices } from "../../app/context";
import { navigate } from "../../app/router";
import { setImportWarning } from "../../app/importWarning";
import type { Loader } from "../../services/types";
import { Button, Field, Input, RAM_DEFAULT, RamPicker, useDefaultRam } from "../../ui";
import { ICONS, Icon } from "../Main/Panel";

export const RECOMMENDED_VERSION = "1.21.1";
const FORBIDDEN = /[<>:"/\\|?*]/;
const LOADERS: { value: Loader; label: string }[] = [
  { value: "vanilla", label: "Aucun" },
  { value: "neoforge", label: "NeoForge" },
  { value: "fabric", label: "Fabric" },
  { value: "quilt", label: "Quilt" },
  { value: "forge", label: "Forge" },
];
export const RAM_OPTIONS = [
  { mb: 2048, label: "2\u00a0Go" },
  { mb: 4096, label: "4\u00a0Go (recommandée)" },
  { mb: 6144, label: "6\u00a0Go" },
  { mb: 8192, label: "8\u00a0Go" },
];
const isPack = (p: string) => /\.(mrpack|zip)$/i.test(p);

/** Renvoie un message d'erreur en français, ou null si le nom est valide. */
export function validateName(raw: string): string | null {
  const name = raw.trim();
  if (!name) return "Le nom de l'instance est obligatoire.";
  if (name.length > 64) return "Le nom ne peut pas dépasser 64 caractères.";
  if (FORBIDDEN.test(name)) return "Le nom ne peut pas contenir les caractères < > : \" / \\ | ? *";
  return null;
}

function messageOf(e: unknown): string {
  if (e instanceof Error && e.message) return e.message;
  if (typeof e === "string" && e) return e;
  return "L'opération a échoué. Réessaie dans un instant.";
}

export interface CreateProps {
  /** Sélecteur de fichier natif ; renvoie null si annulé. Par défaut : services.pickFiles. */
  pickMrpack?: () => Promise<string | null>;
}

function Choice({ icon, title, text, disabled, onClick }: { icon: ReactNode; title: string; text: string; disabled?: boolean; onClick: () => void }) {
  return (
    <button type="button" className="cl-card cl-card-i create-choice enter" disabled={disabled} onClick={onClick}>
      <span className="create-choice-ic" aria-hidden="true">{icon}</span>
      <strong>{title}</strong>
      <span className="create-choice-tx">{text}</span>
    </button>
  );
}

export default function Create({ pickMrpack }: CreateProps = {}) {
  useScreenTitle(null);
  const services = useServices();
  const uid = useId();
  const alive = useRef(true);

  const [view, setView] = useState<"choose" | "form">("choose");
  const [name, setName] = useState("");
  const [nameError, setNameError] = useState<string | null>(null);
  const [versions, setVersions] = useState<string[] | null>(null);
  const [minecraft, setMinecraft] = useState("");
  const [loader, setLoader] = useState<Loader>("neoforge");
  const [ram, setRam] = useState(RAM_DEFAULT);
  const defaultRam = useDefaultRam(services);
  const [advanced, setAdvanced] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pathOpen, setPathOpen] = useState(false);
  const [path, setPath] = useState("");
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; };
  }, []);

  // Focus : premier choix à l'arrivée, champ Nom en entrant dans le formulaire.
  useEffect(() => {
    const el = view === "form"
      ? rootRef.current?.querySelector<HTMLElement>("input[type=text]")
      : rootRef.current?.querySelector<HTMLElement>(".create-choice");
    el?.focus();
  }, [view]);

  useEffect(() => {
    let cancelled = false;
    services.minecraftVersions().then(
      (list) => {
        if (cancelled) return;
        setVersions(list);
        setMinecraft(list.includes(RECOMMENDED_VERSION) ? RECOMMENDED_VERSION : (list[0] ?? ""));
      },
      (e) => { if (!cancelled) { setVersions([]); setError(messageOf(e)); } },
    );
    return () => { cancelled = true; };
  }, [services]);

  useEffect(() => services.onImportWarning((e) => setImportWarning(e.missing)), [services]);

  const back = useBack("main");
  const backRef = useRef(back);
  backRef.current = back;
  const viewRef = useRef(view);
  viewRef.current = view;
  const cancel = () => back();

  // Échap : retour à l'accueil depuis n'importe où. Écouteur document (le focus peut être sur <body>).
  useEffect(() => {
    const onEsc = (e: globalThis.KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      e.preventDefault();
      backRef.current();
    };
    document.addEventListener("keydown", onEsc);
    return () => document.removeEventListener("keydown", onEsc);
  }, []);

  /** Page de la nouvelle instance si son id est connu, sinon l'accueil. */
  const openCreated = (id?: string) => {
    if (id) navigate("instance", { id });
    else navigate("main");
  };

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (busy) return;
    const problem = validateName(name);
    setNameError(problem);
    if (problem) {
      document.getElementById(`${uid}-name`)?.focus();
      return;
    }
    setError(null);
    setBusy(true);
    try {
      const created = await services.instancesCreate({ name: name.trim(), minecraft, loader: { loader, version: "" }, ram_mb: ram });
      if (alive.current) openCreated(created?.id);
    } catch (err) {
      if (alive.current) { setError(messageOf(err)); setBusy(false); }
    }
  };

  const runImport = async (given?: string) => {
    if (busy) return;
    const file = (given ?? path).trim();
    if (!file) { setError("Indique le chemin du modpack (.mrpack ou .zip)."); return; }
    setError(null);
    setImportWarning(null);
    setBusy(true);
    try {
      // Nom, version et loader servent seulement aux zip sans métadonnées.
      const imported = await services.instancesImportMrpack(file, {
        name: name.trim() || undefined,
        minecraft: minecraft || undefined,
        loader: { loader, version: "" },
        ram_mb: ram,
      });
      if (alive.current) openCreated(imported?.id);
    } catch (err) {
      if (alive.current) { setError(messageOf(err)); setBusy(false); }
    }
  };

  // Glisser-déposer d'un modpack sur l'écran de choix.
  const runImportRef = useRef(runImport);
  runImportRef.current = runImport;
  useEffect(() => services.onFilesDropped((paths) => {
    if (viewRef.current !== "choose") return;
    const f = paths.find(isPack);
    if (f) void runImportRef.current(f);
    else setError("Dépose un fichier .mrpack ou .zip.");
  }), [services]);

  const onImportClick = async () => {
    setError(null);
    try {
      if (pickMrpack) {
        const file = await pickMrpack();
        if (file) await runImport(file);
        return;
      }
      const files = await services.pickFiles({ extensions: ["mrpack", "zip"] });
      if (files[0]) await runImport(files[0]);
    } catch (err) {
      if (alive.current) { setError(messageOf(err)); setPathOpen(true); }
    }
  };

  const nameId = `${uid}-name`;
  const mcId = `${uid}-mc`;
  const idx = Math.max(0, LOADERS.findIndex((l) => l.value === loader));

  return (
    <div className="create screen" ref={rootRef}>
      {view === "choose" ? (
        <div className="create-wrap" aria-busy={busy}>
          <header className="create-head enter">
            <h1>Créer une instance</h1>
            <p>Choisis comment démarrer.</p>
          </header>
          <div className="create-choices">
            <Choice icon={<Icon d={ICONS.search} size={28} />} title="Parcourir les modpacks"
              text="Installer un modpack depuis CurseForge ou Modrinth, avec son illustration." disabled={busy}
              onClick={() => navigate("browse", { kind: "modpack" })} />
            <Choice icon={<Icon d={ICONS.plus} size={28} />} title="Créer une instance vide"
              text="Choisir un nom, une version de Minecraft et un loader." disabled={busy}
              onClick={() => setView("form")} />
            <Choice icon={<Icon d={ICONS.folder} size={28} />} title="Importer un fichier"
              text="Un modpack .mrpack ou .zip déjà téléchargé. Tu peux aussi le déposer ici." disabled={busy}
              onClick={() => { void onImportClick(); }} />
          </div>
          {pathOpen && (
            <div className="create-path">
              <Field label="Chemin du modpack (.mrpack ou .zip)">
                {(p) => (
                  <div className="create-path-row">
                    <Input {...p} type="text" value={path} placeholder="C:\Modpacks\monpack.zip" autoComplete="off"
                      onChange={(e) => setPath(e.target.value)}
                      onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); void runImport(); } }} />
                    <Button disabled={busy} onClick={() => { void runImport(); }}>Importer</Button>
                  </div>
                )}
              </Field>
            </div>
          )}
          <div className="create-live" aria-live="polite">
            {busy && <p className="create-info">Opération en cours…</p>}
            {error && <p className="create-error" role="alert">{error}</p>}
          </div>
          <div className="create-actions">
            <Button variant="secondary" size="lg" className="create-cancel" icon={<Icon d={ICONS.close} size={20} />} onClick={cancel}>Annuler</Button>
          </div>
        </div>
      ) : (
        <form className="create-form enter" aria-busy={busy} noValidate onSubmit={(e) => { void submit(e); }}>
          <h1>Créer une instance vide</h1>

          <Field label="Nom" id={nameId} error={nameError ?? undefined}>
            {(p) => (
              <Input {...p} type="text" value={name} placeholder="Ex. : Create: Aeronautics" autoComplete="off"
                onChange={(e) => { setName(e.target.value); if (nameError) setNameError(null); }} />
            )}
          </Field>

          <div className="cl-field">
            <label htmlFor={mcId} className="cl-label">Version de Minecraft</label>
            <select id={mcId} className="cl-input create-select" value={minecraft}
              disabled={versions === null || versions.length === 0} onChange={(e) => setMinecraft(e.target.value)}>
              {versions === null && <option value="">Chargement des versions…</option>}
              {versions?.map((v) => <option key={v} value={v}>{v === RECOMMENDED_VERSION ? `${v} (recommandée)` : v}</option>)}
            </select>
          </div>

          <fieldset className="create-fieldset">
            <legend className="cl-label">Mods</legend>
            <div className="create-seg" style={{ "--n": LOADERS.length, "--k": idx } as React.CSSProperties}>
              <span className="create-seg-pill" aria-hidden="true" />
              {LOADERS.map((l) => (
                <label key={l.value} className="create-seg-opt">
                  <input type="radio" name={`${uid}-loader`} value={l.value} checked={loader === l.value} onChange={() => setLoader(l.value)} />
                  <span>{l.label}</span>
                </label>
              ))}
            </div>
            <p className="cl-hint">Le système qui permet de charger des mods. Cubelink installe la version recommandée et Java&nbsp;21 si besoin.</p>
          </fieldset>

          <div className="create-adv">
            <button type="button" className="create-adv-btn" aria-expanded={advanced} aria-controls={`${uid}-adv`} onClick={() => setAdvanced((v) => !v)}>
              Options avancées
            </button>
            {advanced && (
              <div id={`${uid}-adv`} className="create-adv-body enter">
                <RamPicker value={ram} onChange={setRam} defaultMb={defaultRam} disabled={busy} />
              </div>
            )}
          </div>

          <div className="create-live" aria-live="polite">
            {busy && <p className="create-info">Opération en cours…</p>}
            {error && <p className="create-error" role="alert">{error}</p>}
          </div>

          <div className="create-actions">
            <Button variant="secondary" onClick={() => { setView("choose"); setError(null); }}>Autre méthode</Button>
            <Button variant="secondary" onClick={cancel}>Annuler</Button>
            <Button type="submit" loading={busy} disabled={versions === null}>{busy ? "Création…" : "Créer l'instance"}</Button>
          </div>
        </form>
      )}
    </div>
  );
}
