import { useEffect, useId, useRef, useState, type FormEvent, type ReactNode } from "react";
import "./create.css";
import { useBack, useScreenTitle, useServices } from "../../app/context";
import { navigate } from "../../app/router";
import { setImportWarning } from "../../app/importWarning";
import type { DetectedInstance, Launcher, Loader } from "../../services/types";
import { Badge, Button, Field, Input, ProgressBar, RAM_DEFAULT, Select, RamPicker, useDefaultRam } from "../../ui";
import { ICONS, Icon } from "../../ui";

export const RECOMMENDED_VERSION = "1.21.1";
const FORBIDDEN = /[<>:"/\\|?*]/;
const LOADERS: { value: Loader; label: string }[] = [
  { value: "vanilla", label: "Aucun" },
  { value: "neoforge", label: "NeoForge" },
  { value: "fabric", label: "Fabric" },
  { value: "quilt", label: "Quilt" },
  { value: "forge", label: "Forge" },
];
const isPack = (p: string) => /\.(mrpack|zip)$/i.test(p);
const LAUNCHER_LABEL: Record<Launcher, string> = { prism: "Prism", curseforge: "CurseForge", modrinth: "Modrinth" };
const LOADER_LABEL: Record<Loader, string> = { vanilla: "Sans mods", neoforge: "NeoForge", fabric: "Fabric", quilt: "Quilt", forge: "Forge" };

/** « Minecraft 1.21.1 · Fabric 0.16.5 · 42 mods · mondes » : ce que le joueur reconnaît de son instance. */
export function describeDetected(d: DetectedInstance): string {
  const loader = d.loader.loader === "vanilla" ? LOADER_LABEL.vanilla : `${LOADER_LABEL[d.loader.loader]}${d.loader.version ? ` ${d.loader.version}` : ""}`;
  const parts = [`Minecraft ${d.minecraft}`, loader];
  if (d.mod_count > 0) parts.push(`${d.mod_count} mod${d.mod_count > 1 ? "s" : ""}`);
  if (d.has_saves) parts.push("avec ses mondes");
  return parts.join(" · ");
}

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

  const [view, setView] = useState<"choose" | "form" | "launcher">("choose");
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
  // Import depuis un autre lanceur.
  const [found, setFound] = useState<DetectedInstance[] | null>(null);
  const [manual, setManual] = useState(false);
  const [folder, setFolder] = useState("");
  const [importing, setImporting] = useState<string | null>(null);
  const [copied, setCopied] = useState<{ done: number; total: number } | null>(null);

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
  useEffect(() => services.onBrowseProgress((e) => { if (e.total > 0) setCopied({ done: e.done, total: e.total }); }), [services]);

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

  /** Cherche les instances des autres lanceurs : dossiers habituels, ou le dossier `at` choisi à la main. */
  const scanLaunchers = async (at?: string) => {
    setError(null);
    setFound(null);
    try {
      const list = await services.launcherImportScan(at ?? null);
      if (!alive.current) return;
      setFound(list);
      if (list.length === 0) setManual(true);
    } catch (err) {
      if (alive.current) { setFound([]); setManual(true); setError(messageOf(err)); }
    }
  };

  const openLaunchers = () => {
    setView("launcher");
    setManual(false);
    setFolder("");
    void scanLaunchers();
  };

  const pickLauncherFolder = async () => {
    setError(null);
    try {
      const chosen = await services.pickFolder();
      if (chosen) { setFolder(chosen); await scanLaunchers(chosen); }
    } catch (err) {
      if (alive.current) { setError(messageOf(err)); setManual(true); }
    }
  };

  const importFrom = async (d: DetectedInstance) => {
    if (busy) return;
    setError(null);
    setImportWarning(null);
    setCopied(null);
    setBusy(true);
    setImporting(d.path);
    try {
      const imported = await services.launcherImportRun(d.path, d.launcher, { ram_mb: ram });
      if (alive.current) openCreated(imported?.id);
    } catch (err) {
      if (alive.current) { setError(messageOf(err)); setBusy(false); setImporting(null); }
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
            <Choice icon={<Icon d={ICONS.download} size={28} />} title="Importer depuis un autre lanceur"
              text="Tes instances et tes mondes de Prism, CurseForge ou Modrinth, sans rien retélécharger." disabled={busy}
              onClick={openLaunchers} />
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
            <Button variant="secondary" className="create-cancel" icon={<Icon d={ICONS.close} size={20} />} onClick={cancel}>Annuler</Button>
          </div>
        </div>
      ) : view === "launcher" ? (
        <div className="create-wrap create-launchers" aria-busy={busy}>
          <header className="create-head enter">
            <h1>Importer depuis un autre lanceur</h1>
            <p>Tes instances restent intactes chez l'autre lanceur : Cubelink en fait une copie.</p>
          </header>
          {found === null && !error && <p className="create-info" role="status">Recherche de tes instances…</p>}
          {found !== null && found.length > 0 && (
            <ul className="create-found" aria-label="Instances trouvées">
              {found.map((d) => (
                <li key={`${d.launcher}:${d.path}`} className="cl-card create-found-item">
                  <div className="create-found-main">
                    <strong className="create-found-name">{d.name}</strong>
                    <Badge>{LAUNCHER_LABEL[d.launcher]}</Badge>
                    <span className="create-found-tx">{describeDetected(d)}</span>
                    {d.unsupported && <span className="create-found-why">{d.unsupported}</span>}
                    {importing === d.path && (
                      <ProgressBar className="create-found-bar" label={`Import de ${d.name}`} value={copied ? (copied.done / copied.total) * 100 : null} />
                    )}
                  </div>
                  <Button disabled={busy || !!d.unsupported} loading={importing === d.path} aria-label={`Importer ${d.name}`} onClick={() => { void importFrom(d); }}>
                    {importing === d.path ? "Copie…" : "Importer"}
                  </Button>
                </li>
              ))}
            </ul>
          )}
          {found !== null && found.length === 0 && !error && (
            <p className="create-info" role="status">Aucune instance trouvée aux emplacements habituels. Choisis le dossier d'une instance, ou celui de ton lanceur.</p>
          )}
          {found !== null && (
            <div className="create-path">
              <div className="create-path-row">
                <Button variant="secondary" disabled={busy} icon={<Icon d={ICONS.folder} size={20} />} onClick={() => { void pickLauncherFolder(); }}>Choisir un dossier…</Button>
                {!manual && <Button variant="secondary" disabled={busy} onClick={() => setManual(true)}>Saisir un chemin</Button>}
              </div>
              {manual && (
                <Field label="Dossier d'une instance ou d'un lanceur">
                  {(p) => (
                    <div className="create-path-row">
                      <Input {...p} type="text" value={folder} placeholder="C:\Users\moi\AppData\Roaming\PrismLauncher" autoComplete="off"
                        onChange={(e) => setFolder(e.target.value)}
                        onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); void scanLaunchers(folder.trim()); } }} />
                      <Button disabled={busy || !folder.trim()} onClick={() => { void scanLaunchers(folder.trim()); }}>Chercher</Button>
                    </div>
                  )}
                </Field>
              )}
            </div>
          )}
          <div className="create-live" aria-live="polite">
            {busy && <p className="create-info">Copie en cours : cela peut prendre un moment si tu as de gros mondes.</p>}
            {error && <p className="create-error" role="alert">{error}</p>}
          </div>
          <div className="create-actions">
            <Button variant="secondary" disabled={busy} onClick={() => { setView("choose"); setError(null); }}>Autre méthode</Button>
            <Button variant="secondary" disabled={busy} className="create-cancel" icon={<Icon d={ICONS.close} size={20} />} onClick={cancel}>Annuler</Button>
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
            <Select id={mcId} className="create-select" value={minecraft} placeholder={versions === null ? "Chargement des versions…" : undefined}
              disabled={versions === null || versions.length === 0} onChange={setMinecraft}
              options={(versions ?? []).map((v) => ({ value: v, label: v === RECOMMENDED_VERSION ? `${v} (recommandée)` : v }))} />
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
