import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ModEntry, Services } from "../../services";
import { Button, IconButton } from "../../ui";
import { avatarColors } from "./accent";
import { ICONS, Icon, Panel } from "./Panel";

export interface ModsPanelProps {
  services: Services;
  instanceId: string;
  onClose: () => void;
  /** Appelé quand le nombre de mods change (liste chargée, ajout, suppression). */
  onCount: (count: number) => void;
  onBrowse: () => void;
  /** Informe le parent du total (titre du tiroir). */
  onTotal?: (n: number | null) => void;
}

// Retire le préfixe technique du backend (« introuvable : », « entrée invalide : »…).
const errMsg = (e: unknown) => (e instanceof Error ? e.message : String(e)).replace(/^[a-zéèêàçùîô' -]{3,24}\s:\s+/, "");
const isJar = (p: string) => /\.jar$/i.test(p);
const baseName = (p: string) => p.split(/[\\/]/).pop() ?? p;
/** Délai pendant lequel une suppression reste annulable. */
export const UNDO_MS = 5000;

export function formatSize(n: number): string {
  if (n >= 1_048_576) return `${(n / 1_048_576).toFixed(1).replace(".", ",")} Mo`;
  if (n >= 1024) return `${Math.round(n / 1024)} Ko`;
  return `${n} o`;
}

const initial = (f: string) => (f.replace(/^[^A-Za-z0-9]+/, "")[0] ?? "?").toUpperCase();

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

/** Panneau « Mods » (A4) : recherche, liste, activation, suppression annulable 5 s, ajout (bouton ou glisser-déposer). */
export function ModsPanel({ services, instanceId, onClose, onCount, onBrowse, onTotal }: ModsPanelProps) {
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

  const apply = useCallback((next: ModEntry[]) => {
    setMods(next);
    countRef.current(next.length);
    totalRef.current?.(next.length);
  }, []);

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
      setMods((cur) => {
        const next = [...(cur ?? []), ...added];
        countRef.current(next.length);
        totalRef.current?.(next.length);
        return next;
      });
    }
    const parts: string[] = [];
    if (added.length) parts.push(`${added.length} mod${added.length > 1 ? "s ajoutés" : " ajouté"}.`);
    if (refused.length) parts.push(`Fichier${refused.length > 1 ? "s" : ""} refusé${refused.length > 1 ? "s" : ""} (seuls les .jar sont acceptés) : ${refused.join(", ")}.`);
    if (failed.length) setError(`Échec de l'ajout : ${failed.join(", ")}.`);
    if (parts.length) setNotice(parts.join(" "));
  }, [services, instanceId]);

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
    } catch (e) { setError(errMsg(e)); }
  };

  const commit = async (file: string) => {
    timers.current.delete(file);
    setRemoving((r) => r.filter((f) => f !== file));
    try {
      await services.modsRemove(instanceId, file);
      setMods((cur) => {
        const next = (cur ?? []).filter((x) => x.file !== file);
        countRef.current(next.length);
        totalRef.current?.(next.length);
        return next;
      });
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
        </div>
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
                <li key={m.file} className={`dr-row${m.enabled ? "" : " is-off"}`}>
                  <span className="dr-av" aria-hidden="true" style={{ background: avatarColors(null, m.file).bg, color: avatarColors(null, m.file).ink }}>{initial(m.file)}</span>
                  <span className="dr-nm">
                    <span className="n">{m.file}</span>
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
        <div className="dr-drop" aria-hidden={!over}>Dépose des fichiers .jar ici pour les ajouter</div>
      </div>
      <footer className="dr-foot">
        <Button icon={<Icon d={ICONS.plus} />} onClick={onBrowse}>Parcourir des mods</Button>
        <Button variant="secondary" onClick={() => { void pick(); }}>Ajouter un .jar</Button>
      </footer>
    </Panel>
  );
}
