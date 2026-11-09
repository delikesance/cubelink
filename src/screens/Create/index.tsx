import { useEffect, useId, useRef, useState, type FormEvent, type KeyboardEvent } from "react";
import "./create.css";
import { useScreenTitle, useServices } from "../../app/context";
import { navigate } from "../../app/router";
import type { Loader } from "../../services/types";

export const RECOMMENDED_VERSION = "1.21.1";
const FORBIDDEN = /[<>:"/\\|?*]/;
const LOADERS: { value: Loader; label: string }[] = [
  { value: "vanilla", label: "Aucun" },
  { value: "neoforge", label: "NeoForge" },
  { value: "fabric", label: "Fabric" },
  { value: "quilt", label: "Quilt" },
  { value: "forge", label: "Forge" },
];
const RAM_OPTIONS = [
  { mb: 2048, label: "2\u00a0Go" },
  { mb: 4096, label: "4\u00a0Go (recommandée)" },
  { mb: 6144, label: "6\u00a0Go" },
  { mb: 8192, label: "8\u00a0Go" },
];

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
  /** Sélecteur de fichier natif (à brancher plus tard) ; renvoie null si annulé. Par défaut : champ texte. */
  pickMrpack?: () => Promise<string | null>;
}

const FOCUSABLE = 'a[href],button:not(:disabled),input:not(:disabled),select:not(:disabled),summary,[tabindex]:not([tabindex="-1"])';

export default function Create({ pickMrpack }: CreateProps = {}) {
  useScreenTitle(null);
  const services = useServices();
  const uid = useId();
  const dialogRef = useRef<HTMLFormElement>(null);
  const alive = useRef(true);

  const [name, setName] = useState("");
  const [nameError, setNameError] = useState<string | null>(null);
  const [versions, setVersions] = useState<string[] | null>(null);
  const [minecraft, setMinecraft] = useState("");
  const [loader, setLoader] = useState<Loader>("neoforge");
  const [ram, setRam] = useState(4096);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [importOpen, setImportOpen] = useState(false);
  const [path, setPath] = useState("");

  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; };
  }, []);

  // Focus : entrée dans le dialogue, restauration à la fermeture.
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    dialogRef.current?.querySelector<HTMLElement>("input[type=text]")?.focus();
    return () => { previous?.focus?.(); };
  }, []);

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

  const cancel = () => navigate("main");

  // Échap ferme depuis n'importe où : écouteur au niveau du document (le focus peut être sur <body>).
  useEffect(() => {
    const onEsc = (e: globalThis.KeyboardEvent) => {
      if (e.key === "Escape" && !e.defaultPrevented) { e.preventDefault(); navigate("main"); }
    };
    document.addEventListener("keydown", onEsc);
    return () => document.removeEventListener("keydown", onEsc);
  }, []);

  const onKeyDown = (e: KeyboardEvent<HTMLFormElement>) => {
    if (e.key !== "Tab") return;
    const items = Array.from(dialogRef.current?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? [])
      .filter((el) => el.offsetParent !== null || el.tagName === "INPUT");
    if (!items.length) return;
    const first = items[0];
    const last = items[items.length - 1];
    const active = document.activeElement;
    if (e.shiftKey && (active === first || !dialogRef.current?.contains(active))) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && (active === last || !dialogRef.current?.contains(active))) {
      e.preventDefault();
      first.focus();
    }
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
      await services.instancesCreate({
        name: name.trim(),
        minecraft,
        loader: { loader, version: "" },
        ram_mb: ram,
      });
      if (alive.current) navigate("main");
    } catch (err) {
      if (alive.current) { setError(messageOf(err)); setBusy(false); }
    }
  };

  const runImport = async (given?: string) => {
    if (busy) return;
    const file = (given ?? path).trim();
    if (!file) { setError("Indique le chemin du fichier .mrpack."); return; }
    setError(null);
    setBusy(true);
    try {
      await services.instancesImportMrpack(file);
      if (alive.current) navigate("main");
    } catch (err) {
      if (alive.current) { setError(messageOf(err)); setBusy(false); }
    }
  };

  const onImportClick = async () => {
    if (pickMrpack) {
      try {
        const file = await pickMrpack();
        if (file) await runImport(file);
      } catch (err) {
        if (alive.current) setError(messageOf(err));
      }
      return;
    }
    setImportOpen((v) => !v);
  };

  const nameId = `${uid}-name`;
  const errId = `${uid}-name-err`;
  const titleId = `${uid}-title`;

  return (
    <div className="create">
      <form
        ref={dialogRef}
        className="create-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-busy={busy}
        noValidate
        onSubmit={(e) => { void submit(e); }}
        onKeyDown={onKeyDown}
      >
        <header className="create-head">
          <h1 id={titleId}>Créer une instance</h1>
          <button type="button" className="create-close" aria-label="Fermer" onClick={cancel}>
            <svg width="14" height="14" viewBox="0 0 12 12" aria-hidden="true"><path d="M2.5 2.5l7 7M9.5 2.5l-7 7" stroke="currentColor" strokeWidth="1.4" /></svg>
          </button>
        </header>

        <div className="create-body">
          <div className="create-field">
            <label htmlFor={nameId}>Nom</label>
            <input
              id={nameId}
              className="create-input"
              type="text"
              value={name}
              placeholder="Ex. : Create: Aeronautics"
              autoComplete="off"
              aria-invalid={nameError ? true : undefined}
              aria-describedby={nameError ? errId : undefined}
              onChange={(e) => { setName(e.target.value); if (nameError) setNameError(null); }}
            />
            {nameError && <p id={errId} className="create-error" role="alert">{nameError}</p>}
          </div>

          <div className="create-field">
            <label htmlFor={`${uid}-mc`}>Version de Minecraft</label>
            <select
              id={`${uid}-mc`}
              className="create-input"
              value={minecraft}
              disabled={versions === null || versions.length === 0}
              onChange={(e) => setMinecraft(e.target.value)}
            >
              {versions === null && <option value="">Chargement des versions…</option>}
              {versions?.map((v) => (
                <option key={v} value={v}>{v === RECOMMENDED_VERSION ? `${v} (recommandée)` : v}</option>
              ))}
            </select>
          </div>

          <fieldset className="create-fieldset">
            <legend>Mods</legend>
            <div className="create-loaders">
              {LOADERS.map((l) => (
                <label key={l.value} className="create-loader-wrap">
                  <input type="radio" name={`${uid}-loader`} value={l.value} checked={loader === l.value} onChange={() => setLoader(l.value)} />
                  <span className="create-loader">{l.label}</span>
                </label>
              ))}
            </div>
            <span className="create-hint">Le système qui permet de charger des mods. Cubelink installe la version recommandée et Java&nbsp;21 si besoin.</span>
          </fieldset>

          <details className="create-adv">
            <summary>Options avancées</summary>
            <div className="create-field">
              <label htmlFor={`${uid}-ram`}>Mémoire allouée</label>
              <select id={`${uid}-ram`} className="create-input" value={ram} onChange={(e) => setRam(Number(e.target.value))}>
                {RAM_OPTIONS.map((o) => <option key={o.mb} value={o.mb}>{o.label}</option>)}
              </select>
            </div>
          </details>

          {importOpen && !pickMrpack && (
            <div className="create-field create-import">
              <label htmlFor={`${uid}-path`}>Chemin du fichier .mrpack</label>
              <div className="create-import-row">
                <input
                  id={`${uid}-path`}
                  className="create-input"
                  type="text"
                  value={path}
                  placeholder="C:\Modpacks\monpack.mrpack"
                  autoComplete="off"
                  onChange={(e) => setPath(e.target.value)}
                  onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); void runImport(); } }}
                />
                <button type="button" className="create-btn" disabled={busy} onClick={() => { void runImport(); }}>Importer</button>
              </div>
            </div>
          )}

          <div className="create-live" aria-live="polite">
            {busy && <p className="create-info">Opération en cours…</p>}
            {error && <p className="create-error" role="alert">{error}</p>}
          </div>
        </div>

        <footer className="create-foot">
          <button type="button" className="create-link" aria-expanded={pickMrpack ? undefined : importOpen} disabled={busy} onClick={() => { void onImportClick(); }}>
            Importer un modpack (.mrpack)
          </button>
          <button type="button" className="create-btn" onClick={cancel}>Annuler</button>
          <button type="submit" className="create-btn create-primary" disabled={busy || versions === null}>
            {busy ? "Création…" : "Créer l'instance"}
          </button>
        </footer>
      </form>
    </div>
  );
}
