import { useEffect, useId, useRef, useState } from "react";
import { navigate } from "../../app/router";
import type { BrowseHit, BrowseInstallResult, BrowseVersion, ContentCheck, Instance, Services } from "../../services/types";
import { RAM_DEFAULT, RamPicker, useDefaultRam } from "../../ui";

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

export interface DetailProps {
  hit: BrowseHit;
  services: Services;
  minecraft: string;
  loader: string;
  /** Instance cible pour un mod / pack de ressources / shader. */
  instance: Instance | null;
  /** Instance déjà installée à partir de ce modpack (même source et même projet), sinon null. */
  installed: Instance | null;
  /** Retour à la liste. */
  onBack: () => void;
}

/** Fiche d'un résultat, rendue comme une page (état dans l'URL, voir Browse). */
export function Detail({ hit, services, minecraft, loader, instance, installed, onBack }: DetailProps) {
  const uid = useId();
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
  // Compatibilité avec les mods de l'instance cible (mods seulement).
  const [check, setCheck] = useState<ContentCheck | null>(null);
  const [checking, setChecking] = useState(false);
  const [checkErr, setCheckErr] = useState<string | null>(null);
  const [checkNonce, setCheckNonce] = useState(0);
  const [removing, setRemoving] = useState(false);

  useEffect(() => {
    alive.current = true;
    titleRef.current?.focus();
    return () => { alive.current = false; };
  }, []);

  // Pour un modpack, la version de Minecraft/loader des filtres n'est pas pertinente.
  const modpack = hit.kind === "modpack";
  const [ram, setRam] = useState(RAM_DEFAULT);
  const defaultRam = useDefaultRam(services);
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

  // Une fois les versions connues : déjà installé ? quelle version va avec les autres mods ? (simulation côté Rust)
  const checkable = hit.kind === "mod" && !!instance;
  const instanceId = instance?.id;
  const haveVersions = !!versions && versions.length > 0;
  useEffect(() => {
    if (!checkable || !instanceId || !haveVersions) { setCheck(null); return; }
    let cancelled = false;
    setChecking(true);
    setCheckErr(null);
    services.browseCheck(hit.source, hit.id, hit.kind, instanceId).then(
      (c) => {
        if (cancelled) return;
        setCheck(c);
        setChecking(false);
        if (c.recommended) setVersionId(c.recommended);
      },
      (e) => { if (!cancelled) { setCheck(null); setChecking(false); setCheckErr(messageOf(e)); } },
    );
    return () => { cancelled = true; };
  }, [services, checkable, instanceId, haveVersions, hit.source, hit.id, hit.kind, checkNonce]);

  // Version choisie à la main et pas encore jugée : on la vérifie à son tour.
  const judged = check?.verdicts.some((v) => v.version_id === versionId) ?? false;
  useEffect(() => {
    if (!check || !instanceId || !versionId || judged) return;
    let cancelled = false;
    setChecking(true);
    services.browseCheck(hit.source, hit.id, hit.kind, instanceId, versionId).then(
      (c) => {
        if (cancelled) return;
        setChecking(false);
        // Sans verdict (version non jugeable) : bloquée seulement si un mod présent bloque déjà tout ; l'installation
        // revérifie de toute façon côté Rust.
        setCheck((prev) => {
          if (!prev) return prev;
          const got = c.verdicts.length > 0 ? c.verdicts : [{ version_id: versionId, ok: prev.blockers.length === 0, problems: prev.blockers }];
          return { ...prev, verdicts: [...prev.verdicts.filter((v) => v.version_id !== versionId), ...got] };
        });
      },
      () => { if (!cancelled) setChecking(false); },
    );
    return () => { cancelled = true; };
  }, [services, check, judged, instanceId, versionId, hit.source, hit.id, hit.kind]);

  const chosen = versions?.find((v) => v.id === versionId);
  const downloadable = hit.downloadable && (chosen ? chosen.downloadable : true);
  const source = SOURCE_LABEL[hit.source];
  const verdict = check?.verdicts.find((v) => v.version_id === versionId) ?? null;
  const problems = verdict && !verdict.ok ? verdict.problems : !verdict && check && !check.recommended ? check.blockers : [];
  const blocked = problems.length > 0;
  const installedFiles = check?.installed ?? [];
  const isBad = (id: string) => check?.verdicts.some((v) => v.version_id === id && !v.ok) ?? false;

  const install = async () => {
    if (busy) return;
    if (!modpack && !instance) {
      setError("Choisis d'abord une instance cible (liste « Instance » de la liste des résultats) pour y installer ce contenu.");
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
        ram_mb: modpack ? ram : undefined,
      });
      if (!alive.current) return;
      setResult(r);
      setBusy(false);
      setCheckNonce((n) => n + 1);
    } catch (e) {
      if (alive.current) { setError(messageOf(e)); setBusy(false); }
    }
  };

  const uninstall = async () => {
    if (!instance || removing || installedFiles.length === 0) return;
    setError(null);
    setRemoving(true);
    try {
      for (const f of installedFiles) await services.modsRemove(instance.id, f);
      if (!alive.current) return;
      setResult(null);
      setCheckNonce((n) => n + 1);
    } catch (e) {
      if (alive.current) setError(messageOf(e));
    } finally {
      if (alive.current) setRemoving(false);
    }
  };

  const pct = progress && progress.total > 0 ? Math.min(100, Math.round((progress.done / progress.total) * 100)) : 0;
  const openInstance = (id: string) => navigate("instance", { id });
  const openTarget = modpack ? (result?.instance ?? installed) : null;

  return (
    <div className="br-page" aria-busy={busy}>
      <div>
        <button type="button" className="cl-btn cl-btn-secondary" onClick={onBack}>Retour aux résultats</button>
      </div>
      <header className="br-detail-head">
        {hit.icon_url ? <img className="br-icon br-icon-lg" src={hit.icon_url} alt="" /> : <span className="br-icon br-icon-lg br-icon-ph" aria-hidden="true" />}
        <div className="br-detail-titles">
          <h1 id={`${uid}-title`} ref={titleRef} tabIndex={-1}>{hit.name}</h1>
          <p className="br-muted">par {hit.author} · {source}</p>
        </div>
      </header>

      <div className="br-detail-body">
        <p className="br-desc">{hit.summary || "Aucune description fournie."}</p>
        <p className="br-source">
          Source : {source}.{" "}
          <a href={hit.page_url} target="_blank" rel="noopener noreferrer">Ouvrir la page sur {source}<span className="br-sr"> (nouvel onglet)</span></a>
        </p>

        {installed && !result && (
          <p className="br-muted" role="status">Ce modpack est déjà installé : « {installed.name} ».</p>
        )}

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
                  {check?.recommended === v.id ? " (recommandée)" : isBad(v.id) ? " (incompatible)" : ""}
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
        {modpack && !openTarget && (
          <div className="br-field">
            <RamPicker value={ram} onChange={setRam} defaultMb={defaultRam} disabled={busy}
              hint="Les gros modpacks (plus de 200 mods) demandent souvent 8 Go ou plus." />
          </div>
        )}

        {checkable && checking && <p className="br-muted" role="status">Vérification de la compatibilité avec les mods de « {instance?.name} »…</p>}
        {checkable && checkErr && <p className="br-muted" role="status">Compatibilité non vérifiée : {checkErr}</p>}
        {installedFiles.length > 0 && !removing && (
          <p className="br-muted" role="status">Déjà installé dans « {instance?.name} » : {installedFiles.join(", ")}.</p>
        )}
        {blocked && installedFiles.length === 0 && (
          <div className="br-error" role="alert">
            <p>{check && !check.recommended ? "Aucune version n'est compatible avec les mods déjà présents :" : "Cette version n'est pas compatible avec les mods déjà présents :"}</p>
            <ul>{problems.map((m) => <li key={m}>{m}</li>)}</ul>
          </div>
        )}

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
        {openTarget ? (
          <>
            {!result && (
              <button type="button" className="cl-btn cl-btn-secondary" disabled={busy || !downloadable} onClick={() => { void install(); }}>
                {busy ? "Installation…" : "Réinstaller"}
              </button>
            )}
            <button type="button" className="cl-btn cl-btn-primary" onClick={() => openInstance(openTarget.id)}>
              {result ? "Ouvrir le modpack" : "Ouvrir"}
            </button>
          </>
        ) : installedFiles.length > 0 ? (
          <button type="button" className="cl-btn cl-btn-secondary" disabled={removing || busy} onClick={() => { void uninstall(); }}>
            {removing ? "Désinstallation…" : "Désinstaller"}
          </button>
        ) : (
          <button type="button" className="cl-btn cl-btn-primary" disabled={busy || !downloadable || blocked || (checkable && checking)} onClick={() => { void install(); }}>
            {busy ? "Installation…" : modpack ? "Installer le modpack" : "Installer"}
          </button>
        )}
      </footer>
    </div>
  );
}
