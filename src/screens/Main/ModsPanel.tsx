import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { FixResult, ModEntry, ModIssue, Services } from "../../services";
import { Button, IconButton } from "../../ui";
import { avatarColors } from "./accent";
import { FixDialog } from "./FixDialog";
import { ICONS, Icon } from "../../ui";
import { Panel } from "./Panel";
import { useContextMenu } from "../../app/useContextMenu";

export interface ModsPanelProps {
  services: Services;
  instanceId: string;
  /** Absent : panneau en pleine page, sans bouton Fermer. */
  onClose?: () => void;
  /** Appelé quand le nombre de mods change (liste chargée, ajout, suppression). */
  onCount: (count: number) => void;
  onBrowse: () => void;
  /** Informe le parent du total (titre du tiroir). */
  onTotal?: (n: number | null) => void;
  /** Passe à true pour ouvrir directement la correction automatique (une fois). */
  autoFix?: boolean;
  /** Appelé quand `autoFix` a été pris en compte (le parent le remet à false). */
  onAutoFixed?: () => void;
}

// Retire le préfixe technique du backend (« introuvable : », « entrée invalide : »…).
const errMsg = (e: unknown) => (e instanceof Error ? e.message : String(e)).replace(/^[a-zéèêàçùîô' -]{3,24}\s:\s+/, "");
const isJar = (p: string) => /\.jar$/i.test(p);
const baseName = (p: string) => p.split(/[\\/]/).pop() ?? p;
/** Délai pendant lequel une suppression reste annulable. */
export const UNDO_MS = 5000;
/** Durée d'affichage du résultat d'une correction (avec « Annuler la correction »). */
export const FIX_RESULT_MS = 30000;

export function formatSize(n: number): string {
  if (n >= 1_048_576) return `${(n / 1_048_576).toFixed(1).replace(".", ",")} Mo`;
  if (n >= 1024) return `${Math.round(n / 1024)} Ko`;
  return `${n} o`;
}

const initial = (f: string) => (f.replace(/^[^A-Za-z0-9]+/, "")[0] ?? "?").toUpperCase();

/** Icônes déjà lues (instance|fichier activé → data URL ou null), partagées entre les ouvertures du panneau. */
const iconCache = new Map<string, string | null>();
const enabledName = (f: string) => f.replace(/\.disabled$/i, "");

/** Pastille d'un mod : son icône (lue dans le .jar quand la ligne devient visible), sinon son initiale. */
function ModAvatar({ services, instanceId, file }: { services: Services; instanceId: string; file: string }) {
  const key = `${instanceId}|${enabledName(file)}`;
  const [src, setSrc] = useState<string | null | undefined>(() => iconCache.get(key));
  const ref = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    if (iconCache.has(key)) { setSrc(iconCache.get(key)); return; }
    const el = ref.current;
    let alive = true;
    const load = () => services.modsIcon(instanceId, file).then(
      (u) => { iconCache.set(key, u); if (alive) setSrc(u); },
      () => { iconCache.set(key, null); if (alive) setSrc(null); },
    );
    if (!el || typeof IntersectionObserver === "undefined") { void load(); return () => { alive = false; }; }
    const io = new IntersectionObserver((es) => { if (es.some((e) => e.isIntersecting)) { io.disconnect(); void load(); } }, { rootMargin: "200px" });
    io.observe(el);
    return () => { alive = false; io.disconnect(); };
  }, [services, instanceId, file, key]);
  const c = avatarColors(null, file);
  return src ? (
    <span ref={ref} className="dr-av has-img" aria-hidden="true"><img src={src} alt="" onError={() => { iconCache.set(key, null); setSrc(null); }} /></span>
  ) : (
    <span ref={ref} className="dr-av" aria-hidden="true" style={{ background: c.bg, color: c.ink }}>{initial(file)}</span>
  );
}

/** Ligne « retiré, annuler (5 s) » affichée pendant le délai d'annulation. */
function UndoRow({ file, onUndo }: { file: string; onUndo: () => void }) {
  const [left, setLeft] = useState(Math.round(UNDO_MS / 1000));
  useEffect(() => {
    const t = setInterval(() => setLeft((s) => Math.max(0, s - 1)), 1000);
    return () => clearInterval(t);
  }, []);
  return (
    <li className="dr-undo enter" role="status">
      <span className="dr-undo-txt">« {file} » retiré.</span>
      <Button variant="secondary" onClick={onUndo}>Annuler ({left} s)</Button>
    </li>
  );
}

const issueWord = (n: number) => `${n} problème${n > 1 ? "s" : ""} de compatibilité`;

/** Pastille d'alerte d'une ligne : message en title et en texte masqué. */
function IssueBadge({ issues }: { issues: ModIssue[] }) {
  const danger = issues.some((i) => i.severity === "error");
  const text = issues.map((i) => i.message).join(" ");
  return (
    <span className={`dr-flag ${danger ? "is-error" : "is-warn"}`} title={text}>
      <Icon d={ICONS.alert} size={14} />
      <span className="home-sr">{text}</span>
    </span>
  );
}

/** Panneau « Mods » (A4) : recherche, liste, activation, suppression annulable 5 s, ajout (bouton ou glisser-déposer). */
export function ModsPanel({ services, instanceId, onClose, onCount, onBrowse, onTotal, autoFix, onAutoFixed }: ModsPanelProps) {
  const [mods, setMods] = useState<ModEntry[] | null>(null);
  const [filter, setFilter] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [removing, setRemoving] = useState<string[]>([]);
  const [over, setOver] = useState(false);
  const countRef = useRef(onCount);
  countRef.current = onCount;
  const totalRef = useRef(onTotal);
  totalRef.current = onTotal;
  const timers = useRef(new Map<string, number>());
  const ctx = useContextMenu();
  const [issues, setIssues] = useState<ModIssue[]>([]);
  const [issuesOpen, setIssuesOpen] = useState(false);
  const checkSeq = useRef(0);
  const checkTimer = useRef<number | undefined>(undefined);
  const [fix, setFix] = useState<{ only?: string } | null>(null);
  const [fixRes, setFixRes] = useState<{ applied: number; failed: FixResult["failed"]; backup: string | null } | null>(null);
  const [undoing, setUndoing] = useState(false);
  const fixTimer = useRef<number | undefined>(undefined);
  const onAutoFixedRef = useRef(onAutoFixed);
  onAutoFixedRef.current = onAutoFixed;
  useEffect(() => () => window.clearTimeout(fixTimer.current), []);
  useEffect(() => {
    if (!autoFix) return;
    setFix({});
    onAutoFixedRef.current?.();
  }, [autoFix]);

  /** Relance la vérification (anti-rebond) ; une réponse périmée est ignorée, un échec n'affiche rien. */
  const recheck = useCallback((delay = 300) => {
    window.clearTimeout(checkTimer.current);
    const seq = ++checkSeq.current;
    checkTimer.current = window.setTimeout(() => {
      let p: Promise<{ issues: ModIssue[] }>;
      try { p = services.modsCheck(instanceId); } catch { return; }
      p.then((c) => { if (seq === checkSeq.current) setIssues(c.issues); }, () => { if (seq === checkSeq.current) setIssues([]); });
    }, delay);
  }, [services, instanceId]);
  useEffect(() => {
    recheck(0);
    return () => { checkSeq.current++; window.clearTimeout(checkTimer.current); };
  }, [recheck]);

  const apply = useCallback((next: ModEntry[]) => setMods(next), []);

  // Compteur et total remontés au parent après le rendu (jamais depuis un updater : le parent se met à jour aussi).
  useEffect(() => {
    if (mods === null) return;
    countRef.current(mods.length);
    totalRef.current?.(mods.length);
  }, [mods?.length]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    let alive = true;
    services.modsList(instanceId).then(
      (l) => { if (alive) apply(l); },
      (e: unknown) => { if (alive) { setMods([]); setError(errMsg(e)); } },
    );
    return () => { alive = false; totalRef.current?.(null); };
  }, [services, instanceId, apply]);

  // À la fermeture du tiroir, les suppressions encore annulables sont appliquées sans attendre.
  useEffect(() => {
    const map = timers.current;
    return () => {
      map.forEach((t, file) => { window.clearTimeout(t); void services.modsRemove(instanceId, file).catch(() => {}); });
      map.clear();
    };
  }, [services, instanceId]);

  const addPaths = useCallback(async (paths: string[]) => {
    setError(null);
    setNotice(null);
    const jars = paths.filter(isJar);
    const refused = paths.filter((p) => !isJar(p)).map(baseName);
    const added: ModEntry[] = [];
    const failed: string[] = [];
    for (const p of jars) {
      try { added.push(await services.modsAdd(instanceId, p)); }
      catch (e) { failed.push(`${baseName(p)} (${errMsg(e)})`); }
    }
    if (added.length) {
      setMods((cur) => [...(cur ?? []), ...added]);
      recheck();
    }
    const parts: string[] = [];
    if (added.length) parts.push(`${added.length} mod${added.length > 1 ? "s ajoutés" : " ajouté"}.`);
    if (refused.length) parts.push(`Fichier${refused.length > 1 ? "s" : ""} refusé${refused.length > 1 ? "s" : ""} (seuls les .jar sont acceptés) : ${refused.join(", ")}.`);
    if (failed.length) setError(`Échec de l'ajout : ${failed.join(", ")}.`);
    if (parts.length) setNotice(parts.join(" "));
  }, [services, instanceId, recheck]);

  useEffect(() => services.onFilesDropped((paths) => { setOver(false); void addPaths(paths); }), [services, addPaths]);

  const pick = async () => {
    setError(null);
    try {
      const paths = await services.pickFiles({ extensions: ["jar"], multiple: true });
      if (paths.length) await addPaths(paths);
    } catch (e) { setError(errMsg(e)); }
  };

  const toggle = async (m: ModEntry) => {
    setError(null);
    try {
      await services.modsToggle(instanceId, m.file, !m.enabled);
      setMods((cur) => (cur ?? []).map((x) => (x.file === m.file ? { ...x, enabled: !m.enabled } : x)));
      recheck();
    } catch (e) { setError(errMsg(e)); }
  };

  const commit = async (file: string) => {
    timers.current.delete(file);
    setRemoving((r) => r.filter((f) => f !== file));
    try {
      await services.modsRemove(instanceId, file);
      setMods((cur) => (cur ?? []).filter((x) => x.file !== file));
      recheck();
    } catch (e) { setError(errMsg(e)); }
  };
  const startRemove = (m: ModEntry) => {
    setError(null);
    setNotice(null);
    setRemoving((r) => [...r, m.file]);
    timers.current.set(m.file, window.setTimeout(() => { void commit(m.file); }, UNDO_MS));
  };
  const undo = (file: string) => {
    const t = timers.current.get(file);
    if (t !== undefined) window.clearTimeout(t);
    timers.current.delete(file);
    setRemoving((r) => r.filter((f) => f !== file));
  };

  /** Recharge la liste des mods (après une correction ou son annulation). */
  const reload = useCallback(async () => {
    try { setMods(await services.modsList(instanceId)); } catch (e) { setError(errMsg(e)); }
  }, [services, instanceId]);

  const onFixed = (r: FixResult) => {
    setFix(null);
    setError(null);
    setNotice(null);
    checkSeq.current++;
    window.clearTimeout(checkTimer.current);
    setIssues(r.check.issues);
    setFixRes({ applied: r.applied.length, failed: r.failed, backup: r.backup_id });
    window.clearTimeout(fixTimer.current);
    fixTimer.current = window.setTimeout(() => setFixRes(null), FIX_RESULT_MS);
    void reload();
  };
  const undoFix = async () => {
    if (!fixRes?.backup) return;
    setUndoing(true);
    setError(null);
    try {
      const c = await services.modsFixUndo(instanceId, fixRes.backup);
      checkSeq.current++;
      window.clearTimeout(checkTimer.current);
      setIssues(c.issues);
      window.clearTimeout(fixTimer.current);
      setFixRes(null);
      await reload();
    } catch (e) { setError(errMsg(e)); }
    finally { setUndoing(false); }
  };
  const modOf = (file: string) => (mods ?? []).find((m) => m.enabled && enabledName(m.file) === file);

  const shown = useMemo(() => {
    const t = filter.trim().toLowerCase();
    return (mods ?? []).filter((m) => !t || m.file.toLowerCase().includes(t));
  }, [mods, filter]);
  const total = mods?.length ?? 0;
  const enabled = (mods ?? []).filter((m) => m.enabled).length;

  const hasFiles = (e: React.DragEvent) => Array.from(e.dataTransfer?.types ?? []).includes("Files");

  return (
    <Panel onClose={onClose}>
      <div
        className={`dr-body dr-mods${over ? " is-over" : ""}`}
        onDragEnter={(e) => { if (hasFiles(e)) setOver(true); }}
        onDragOver={(e) => { if (hasFiles(e)) e.preventDefault(); }}
        onDragLeave={(e) => { if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setOver(false); }}
        onDrop={() => setOver(false)}
      >
        <div className="dr-search">
          <label className="home-sr" htmlFor="dr-mods-q">Rechercher un mod</label>
          <div className="dr-searchbox">
            <Icon d={ICONS.search} />
            <input
              id="dr-mods-q" type="search" placeholder={`Rechercher parmi ${total} mod${total > 1 ? "s" : ""}`}
              value={filter} onChange={(e) => setFilter(e.target.value)}
            />
          </div>
          <div className="dr-search-actions">
            <Button icon={<Icon d={ICONS.plus} />} onClick={onBrowse}>Parcourir des mods</Button>
            <Button variant="secondary" onClick={() => { void pick(); }}>Ajouter un .jar</Button>
          </div>
        </div>
        {issues.length > 0 && (
          <div className={`dr-issues${issues.some((i) => i.severity === "error") ? " is-error" : " is-warn"}`}>
            <div className="dr-issues-bar">
              <button type="button" className="dr-issues-h" aria-expanded={issuesOpen} onClick={() => setIssuesOpen((o) => !o)}>
                <Icon d={ICONS.alert} size={16} />
                <span>{issueWord(issues.length)}</span>
                <span className="dr-issues-c" aria-hidden="true">{issuesOpen ? "▴" : "▾"}</span>
              </button>
              {issues.some((i) => i.severity === "error") && (
                <Button onClick={() => setFix({})}>Corriger automatiquement</Button>
              )}
            </div>
            {issuesOpen && (
              <ul className="dr-issues-list" aria-label="Problèmes de compatibilité">
                {issues.map((i, k) => {
                  const mine = i.kind === "incompatible" ? modOf(i.file) : undefined;
                  const other = i.kind === "incompatible" && i.related ? modOf(i.related) : undefined;
                  return (
                    <li key={`${i.file}-${i.kind}-${k}`}>
                      <span><strong>{i.mod_name}</strong> : {i.message}</span>
                      <span className="dr-issues-act">
                        {i.related && (mods ?? []).some((m) => m.file === i.related) && (
                          <button type="button" className="home-link" aria-label={`Voir ${i.related}`} onClick={() => setFilter(i.related!)}>Voir</button>
                        )}
                        {i.severity === "error" && i.kind !== "incompatible" && (
                          <Button variant="secondary" aria-label={`Corriger ${i.mod_name}`} onClick={() => setFix({ only: i.file })}>Corriger</Button>
                        )}
                        {mine && <Button variant="secondary" onClick={() => { void toggle(mine); }}>Désactiver {i.mod_name}</Button>}
                        {other && <Button variant="secondary" onClick={() => { void toggle(other); }}>Désactiver {i.related}</Button>}
                      </span>
                    </li>
                  );
                })}
              </ul>
            )}
          </div>
        )}
        {fixRes && (
          <div className={`dr-fixres${fixRes.failed.length ? " has-fail" : ""}`} role="status">
            <span className="dr-fixres-txt">
              <span>{fixRes.applied === 0 ? "Aucune correction appliquée." : `${fixRes.applied} correction${fixRes.applied > 1 ? "s" : ""} appliquée${fixRes.applied > 1 ? "s" : ""}.`}</span>
              {fixRes.failed.map((f) => <span key={f.title}>1 a échoué : {f.title} — {f.error}</span>)}
            </span>
            {fixRes.backup && <Button variant="secondary" loading={undoing} onClick={() => { void undoFix(); }}>Annuler la correction</Button>}
            <IconButton aria-label="Fermer le résultat" icon={<Icon d={ICONS.close} />} onClick={() => setFixRes(null)} />
          </div>
        )}
        <div className="dr-live" aria-live="polite">
          <p className="dr-count" role="status">
            {mods === null ? "Chargement des mods…"
              : `${total} mod${total > 1 ? "s" : ""} · ${enabled} activé${enabled > 1 ? "s" : ""}${filter.trim() ? ` · ${shown.length} affiché${shown.length > 1 ? "s" : ""}` : ""}`}
          </p>
          {error && <p className="home-err" role="alert">{error}</p>}
          {notice && <p className="dr-notice" role="status">{notice}</p>}
        </div>
        {mods !== null && (
          shown.length === 0 ? (
            <p className="dr-empty">
              {total === 0 ? "Aucun mod. Ajoute un fichier .jar ou glisse-le ici." : "Aucun mod ne correspond à la recherche."}
            </p>
          ) : (
            <ul className="dr-list" aria-label="Mods installés">
              {shown.map((m) => removing.includes(m.file) ? (
                <UndoRow key={m.file} file={m.file} onUndo={() => undo(m.file)} />
              ) : (
                <li
                  key={m.file} className={`dr-row${m.enabled ? "" : " is-off"}`}
                  onContextMenu={ctx.bind(`Actions pour ${m.file}`, [
                    { key: "toggle", label: m.enabled ? "Désactiver" : "Activer", icon: <Icon d={m.enabled ? ICONS.close : ICONS.check} />, onSelect: () => { void toggle(m); } },
                    { key: "copy", label: "Copier le nom du fichier", icon: <Icon d={ICONS.copy} />, onSelect: () => { void navigator.clipboard?.writeText(m.file).catch(() => {}); } },
                    { key: "delete", label: "Supprimer", icon: <Icon d={ICONS.trash} />, danger: true, separator: true, onSelect: () => startRemove(m) },
                  ])}
                >
                  <ModAvatar services={services} instanceId={instanceId} file={m.file} />
                  <span className="dr-nm">
                    <span className="n">
                      {m.file}
                      {issues.some((i) => i.file === enabledName(m.file)) && <IssueBadge issues={issues.filter((i) => i.file === enabledName(m.file))} />}
                    </span>
                    <span className="m">
                      <span>{formatSize(m.size)}</span>
                      {m.custom && <span className="dr-chip">Personnalisé</span>}
                    </span>
                  </span>
                  <button
                    type="button" role="switch" aria-checked={m.enabled} className="dr-switch"
                    aria-label={`${m.enabled ? "Désactiver" : "Activer"} ${m.file}`}
                    onClick={() => { void toggle(m); }}
                  ><i aria-hidden="true" /></button>
                  <IconButton aria-label={`Supprimer ${m.file}`} icon={<Icon d={ICONS.trash} />} onClick={() => startRemove(m)} />
                </li>
              ))}
            </ul>
          )
        )}
        {ctx.menu}
        {fix && <FixDialog services={services} instanceId={instanceId} only={fix.only} onClose={() => setFix(null)} onDone={onFixed} />}
        <div className="dr-drop" aria-hidden={!over}>Dépose des fichiers .jar ici pour les ajouter</div>
      </div>
    </Panel>
  );
}
