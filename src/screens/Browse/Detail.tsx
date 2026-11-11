import { useEffect, useId, useRef, useState, type KeyboardEvent } from "react";
import { navigate } from "../../app/router";
import type { BrowseHit, BrowseInstallResult, BrowseVersion, Instance, Services } from "../../services/types";

export const SOURCE_LABEL = { curseforge: "CurseForge", modrinth: "Modrinth" } as const;

/** CurseForge n'est pas utilisable sans la clé API de Cubelink. */
export const CF_UNAVAILABLE = "CurseForge n'est pas configuré dans cette version de Cubelink. Utilise Modrinth.";
export function isCurseForgeDown(e: unknown): boolean {
  const m = e instanceof Error ? e.message : typeof e === "string" ? e : "";
  return /curseforge/i.test(m) && /(disponible|clé api|cle api)/i.test(m);
}

/** Message affichable : jamais de texte backend brut pour CurseForge, préfixes techniques retirés. */
export function messageOf(e: unknown): string {
  if (isCurseForgeDown(e)) return CF_UNAVAILABLE;
  const raw = e instanceof Error && e.message ? e.message : typeof e === "string" ? e : "";
  const m = raw.replace(/^entrée invalide\s*:\s*/i, "").trim();
  return m || "L'opération a échoué. Réessaie dans un instant.";
}

const FOCUSABLE = 'a[href],button:not(:disabled),input:not(:disabled),select:not(:disabled),[tabindex]:not([tabindex="-1"])';

export interface DetailProps {
  hit: BrowseHit;
  services: Services;
  minecraft: string;
  loader: string;
  instance: Instance | null;
  onClose: () => void;
}

export function Detail({ hit, services, minecraft, loader, instance, onClose }: DetailProps) {
  const uid = useId();
  const ref = useRef<HTMLDivElement>(null);
  const titleRef = useRef<HTMLHeadingElement>(null);
  const alive = useRef(true);
  const [versions, setVersions] = useState<BrowseVersion[] | null>(null);
  const [versionsError, setVersionsError] = useState<string | null>(null);
  const [versionId, setVersionId] = useState("");
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState<{ done: number; total: number; label: string } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<BrowseInstallResult | null>(null);
  const [retry, setRetry] = useState(0);

  useEffect(() => {
    alive.current = true;
    const previous = document.activeElement as HTMLElement | null;
    titleRef.current?.focus();
    return () => { alive.current = false; previous?.focus?.(); };
  }, []);

  // Échap ferme aussi quand le focus est retombé sur <body> (ex. bouton « Installer » devenu désactivé).
  const closeRef = useRef(onClose);
  closeRef.current = onClose;
  useEffect(() => {
    const onEsc = (e: globalThis.KeyboardEvent) => {
      if (e.key === "Escape" && !e.defaultPrevented) { e.preventDefault(); closeRef.current(); }
    };
    document.addEventListener("keydown", onEsc);
    return () => document.removeEventListener("keydown", onEsc);
  }, []);

  // Pour un modpack, la version de Minecraft/loader des filtres n'est pas pertinente.
  const modpack = hit.kind === "modpack";
  useEffect(() => {
    let cancelled = false;
    setVersions(null);
    setVersionsError(null);
    services.browseVersions(hit.source, hit.id, hit.kind, modpack ? undefined : minecraft || undefined, modpack ? undefined : loader || undefined).then(
      (list) => {
        if (cancelled) return;
        setVersions(list);
        setVersionId((list.find((v) => v.downloadable) ?? list[0])?.id ?? "");
      },
      (e) => { if (!cancelled) { setVersions([]); setVersionsError(messageOf(e)); } },
    );
    return () => { cancelled = true; };
  }, [services, hit.source, hit.id, hit.kind, modpack, minecraft, loader, retry]);

  useEffect(() => services.onBrowseProgress((e) => { if (alive.current) setProgress(e); }), [services]);

  const chosen = versions?.find((v) => v.id === versionId);
  const downloadable = hit.downloadable && (chosen ? chosen.downloadable : true);
  const source = SOURCE_LABEL[hit.source];

  const install = async () => {
    if (busy) return;
    if (!modpack && !instance) {
      setError("Choisis d'abord une instance cible (liste « Instance » en haut de l'écran) pour y installer ce contenu.");
      return;
    }
    setError(null);
    setResult(null);
    setProgress(null);
    setBusy(true);
    try {
      const r = await services.browseInstall({
        source: hit.source, id: hit.id, kind: hit.kind,
        version_id: versionId || undefined,
        instance_id: modpack ? undefined : instance?.id,
      });
      if (!alive.current) return;
      if (modpack) { navigate("main"); return; }
      setResult(r);
      setBusy(false);
    } catch (e) {
      if (alive.current) { setError(messageOf(e)); setBusy(false); }
    }
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); onClose(); return; }
    if (e.key !== "Tab") return;
    const items = Array.from(ref.current?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? []);
    if (!items.length) return;
    const first = items[0];
    const last = items[items.length - 1];
    const active = document.activeElement;
    const inside = ref.current?.contains(active) && active !== titleRef.current;
    if (e.shiftKey && (active === first || !inside)) { e.preventDefault(); last.focus(); }
    else if (!e.shiftKey && (active === last || !inside)) { e.preventDefault(); first.focus(); }
  };

  const pct = progress && progress.total > 0 ? Math.min(100, Math.round((progress.done / progress.total) * 100)) : 0;

  return (
    <div className="br-overlay" onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }}>
      <div
        ref={ref}
        className="br-detail"
        role="dialog"
        aria-modal="true"
        aria-labelledby={`${uid}-title`}
        aria-busy={busy}
        onKeyDown={onKeyDown}
      >
        <header className="br-detail-head">
          {hit.icon_url ? <img className="br-icon br-icon-lg" src={hit.icon_url} alt="" /> : <span className="br-icon br-icon-lg br-icon-ph" aria-hidden="true" />}
          <div className="br-detail-titles">
            <h2 id={`${uid}-title`} ref={titleRef} tabIndex={-1}>{hit.name}</h2>
            <p className="br-muted">par {hit.author} · {source}</p>
          </div>
          <button type="button" className="cl-btn cl-btn-secondary br-close" aria-label="Fermer la fiche" onClick={onClose}>
            <svg width="14" height="14" viewBox="0 0 12 12" aria-hidden="true"><path d="M2.5 2.5l7 7M9.5 2.5l-7 7" stroke="currentColor" strokeWidth="1.4" /></svg>
          </button>
        </header>

        <div className="br-detail-body">
          <p className="br-desc">{hit.summary || "Aucune description fournie."}</p>
          <p className="br-source">
            Source : {source}.{" "}
            <a href={hit.page_url} target="_blank" rel="noopener noreferrer">Ouvrir la page sur {source}<span className="br-sr"> (nouvel onglet)</span></a>
          </p>

          <div className="br-field">
            <label htmlFor={`${uid}-ver`}>Version</label>
            {versions === null && <p className="br-muted" role="status">Chargement des versions…</p>}
            {versionsError && (
              <p className="br-error" role="alert">
                {versionsError}{" "}
                <button type="button" className="br-link" onClick={() => setRetry((n) => n + 1)}>Réessayer</button>
              </p>
            )}
            {versions && !versionsError && versions.length === 0 && (
              <p className="br-muted">Aucune version compatible{minecraft && !modpack ? ` avec Minecraft ${minecraft}` : ""}{loader && !modpack ? ` (${loader})` : ""}.</p>
            )}
            {versions && versions.length > 0 && (
              <select id={`${uid}-ver`} className="br-input" value={versionId} onChange={(e) => setVersionId(e.target.value)}>
                {versions.map((v) => (
                  <option key={v.id} value={v.id}>
                    {v.name} — MC {v.minecraft.join(", ") || "?"}{v.loaders.length ? ` · ${v.loaders.join(", ")}` : ""}{v.downloadable ? "" : " (non téléchargeable)"}
                  </option>
                ))}
              </select>
            )}
          </div>

          {!modpack && (
            <p className="br-muted">
              Instance cible : {instance ? <strong>{instance.name}</strong> : "aucune sélectionnée"}
            </p>
          )}
          {modpack && <p className="br-muted">Une nouvelle instance sera créée à partir de ce modpack.</p>}

          {!downloadable && (
            <p className="br-warn" role="status">
              Ce contenu n'est pas téléchargeable depuis Cubelink (l'auteur a désactivé le téléchargement par des applications tierces).{" "}
              <a className="cl-btn cl-btn-secondary" href={hit.page_url} target="_blank" rel="noopener noreferrer">Ouvrir sur {source}</a>
            </p>
          )}

          <div className="br-live" aria-live="polite">
            {busy && (
              <div className="br-progress">
                <div className="br-bar" role="progressbar" aria-label="Progression de l'installation" aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct}>
                  <span style={{ width: `${pct}%` }} />
                </div>
                <p className="br-muted">{progress?.label || "Installation en cours…"}</p>
              </div>
            )}
            {result && (
              <div className="br-summary">
                <p>{result.installed.length} élément{result.installed.length > 1 ? "s" : ""} installé{result.installed.length > 1 ? "s" : ""} dans « {result.instance.name} ».</p>
                {result.missing.length > 0 && (
                  <>
                    <p className="br-warn">{result.missing.length} fichier{result.missing.length > 1 ? "s" : ""} impossible{result.missing.length > 1 ? "s" : ""} à télécharger automatiquement :</p>
                    <ul>{result.missing.map((m) => <li key={m}>{m}</li>)}</ul>
                  </>
                )}
              </div>
            )}
          </div>
          {error && <p className="br-error" role="alert">{error}</p>}
        </div>

        <footer className="br-detail-foot">
          <button type="button" className="cl-btn cl-btn-secondary" onClick={onClose}>Fermer</button>
          <button type="button" className="cl-btn cl-btn-primary" disabled={busy || !downloadable} onClick={() => { void install(); }}>
            {busy ? "Installation…" : modpack ? "Installer le modpack" : "Installer"}
          </button>
        </footer>
      </div>
    </div>
  );
}
