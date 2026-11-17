import { useDeferredValue, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { Services } from "../../services";
import { IconButton } from "../../ui";
import { ICONS, Icon } from "../../ui";
import { Panel } from "./Panel";

const MAX_LINES = 5000;
const trim = (l: string[]) => (l.length > MAX_LINES ? l.slice(l.length - MAX_LINES) : l);

export type Level = "INFO" | "WARN" | "ERROR";
export interface ParsedLine { time: string; level: Level; thread: string; text: string }

// Format Minecraft : « [21:14:02] [main/INFO]: message » (millisecondes et nom de logger facultatifs).
const LOG4J = /^\[(\d{1,2}:\d{2}:\d{2}(?:[.,]\d+)?)\]\s*\[([^\]]*?)(?:\/(\w+))?\]\s*(?:\[[^\]]*\])?:?\s*(.*)$/;

/** Découpe une ligne de journal en heure, niveau, fil et message (niveau déduit du texte à défaut de format reconnu). */
export function parseLine(raw: string): ParsedLine {
  const m = LOG4J.exec(raw);
  const guess = (t: string): Level => (/\b(ERROR|FATAL|SEVERE)\b/.test(t) ? "ERROR" : /\bWARN(ING)?\b/.test(t) ? "WARN" : "INFO");
  if (!m) return { time: "", level: guess(raw), thread: "", text: raw };
  const lv = (m[3] ?? "").toUpperCase();
  const level: Level = lv === "ERROR" || lv === "FATAL" || lv === "SEVERE" ? "ERROR" : lv.startsWith("WARN") ? "WARN" : lv === "INFO" || lv === "DEBUG" || lv === "TRACE" ? "INFO" : guess(m[4]);
  return { time: m[1], level, thread: m[2], text: m[4] };
}

type Filter = "all" | "warn" | "error";

/** Préfixe de mod en tête de message (« [SDLNativesLoader] », « [ETF]: ») : affiché en retrait pour que le message ressorte. */
const TAG = /^(\[[^\]\s][^\]]{0,40}\]:?\s+)/;
export function splitTag(text: string): [string, string] {
  const m = TAG.exec(text);
  return m ? [m[1], text.slice(m[1].length)] : ["", text];
}

/** Surligne chaque occurrence de `q` (sans tenir compte de la casse). */
export function highlight(text: string, q: string): ReactNode {
  if (!q) return text;
  const low = text.toLowerCase();
  const out: ReactNode[] = [];
  let from = 0;
  for (let at = low.indexOf(q, 0); at !== -1; at = low.indexOf(q, from)) {
    if (at > from) out.push(text.slice(from, at));
    out.push(<mark key={at} className="dr-hit">{text.slice(at, at + q.length)}</mark>);
    from = at + q.length;
  }
  if (!out.length) return text;
  if (from < text.length) out.push(text.slice(from));
  return out;
}

export interface LogsPanelProps {
  services: Services;
  instanceId: string;
  /** Absent : panneau en pleine page, sans bouton Fermer. */
  onClose?: () => void;
}

/** Icônes Iconoir propres aux logs. */
const LOG_ICONS = {
  warn: <><path d="M20.043 21H3.957c-1.538 0-2.5-1.664-1.734-2.997l8.043-13.988c.77-1.337 2.699-1.337 3.468 0l8.043 13.988C22.543 19.336 21.58 21 20.043 21z" /><path d="M12 9v4M12 17.01l.01-.011" /></>,
  error: <path d="M9.172 14.828L12.001 12m2.828-2.828L12.001 12m0 0L9.172 9.172M12.001 12l2.828 2.828M12 22c5.523 0 10-4.477 10-10S17.523 2 12 2 2 6.477 2 12s4.477 10 10 10z" />,
  down: <path d="M12 3v18m0 0l8.5-8.5M12 21l-8.5-8.5" />,
};

/** Proche du bas : on suit les nouvelles lignes ; plus haut, l'utilisateur lit, on ne bouge pas. */
const atBottom = (el: HTMLElement) => el.scrollHeight - el.scrollTop - el.clientHeight < 24;

/** Panneau « Logs » : recherche, filtre par niveau (avec compteurs), copie ; suit le bas tant qu'on y est. */
export function LogsPanel({ services, instanceId, onClose }: LogsPanelProps) {
  const [lines, setLines] = useState<string[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [filter, setFilter] = useState<Filter>("all");
  const [query, setQuery] = useState("");
  // Recherche : différée pour que la frappe reste fluide sur des milliers de lignes.
  const q = useDeferredValue(query.trim().toLowerCase());
  const [stuck, setStuck] = useState(true);
  const [unseen, setUnseen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    let alive = true;
    let loaded = false;
    const queue: string[] = [];
    const off = services.onLaunchLog((e) => {
      if (e.id !== instanceId) return;
      // Le backend regroupe plusieurs lignes (séparées par \n) dans un même événement.
      const incoming = e.line.split(/\r?\n/);
      if (!loaded) queue.push(...incoming);
      else setLines((l) => trim([...l, ...incoming]));
    });
    services.instancesLogs(instanceId).then(
      (text) => {
        if (!alive) return;
        loaded = true;
        setLines(trim([...(text ? text.split(/\r?\n/) : []), ...queue]));
        setLoading(false);
      },
      (e: unknown) => {
        if (!alive) return;
        loaded = true;
        setLines(trim(queue));
        setError(e instanceof Error ? e.message : String(e));
        setLoading(false);
      },
    );
    return () => { alive = false; off(); };
  }, [services, instanceId]);

  // Ctrl+F : va au champ de recherche des logs plutôt qu'à la recherche du navigateur.
  useEffect(() => {
    const onKey = (e: globalThis.KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "f") { e.preventDefault(); searchRef.current?.focus(); searchRef.current?.select(); }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, []);

  const parsed = useMemo(() => lines.map(parseLine), [lines]);
  const counts = useMemo(() => {
    let warn = 0, err = 0;
    for (const p of parsed) { if (p.level === "WARN") warn++; else if (p.level === "ERROR") err++; }
    return { warn, error: err };
  }, [parsed]);
  const shown = useMemo(
    () => parsed
      .map((p, i) => ({ p, i }))
      .filter(({ p }) => filter === "all" || (filter === "warn" ? p.level === "WARN" : p.level === "ERROR"))
      .filter(({ i }) => !q || lines[i].toLowerCase().includes(q)),
    [parsed, lines, filter, q],
  );

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    if (stuck) { el.scrollTop = el.scrollHeight; setUnseen(false); }
    else setUnseen(true);
  }, [shown]); // eslint-disable-line react-hooks/exhaustive-deps

  const toBottom = () => {
    const el = ref.current;
    if (el) el.scrollTop = el.scrollHeight;
    setStuck(true);
    setUnseen(false);
  };

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(lines.join("\n"));
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      setError("Copie impossible : le presse-papiers n'est pas accessible.");
    }
  };

  const seg = (f: Filter, label: string, icon?: ReactNode, n?: number) => (
    <button
      type="button" className={`dr-seg-btn${filter === f ? " is-on" : ""}${f !== "all" ? ` is-${f}` : ""}`} aria-pressed={filter === f}
      aria-label={n === undefined ? label : `${label} (${n})`} title={label} onClick={() => setFilter(f)}
    >
      {icon}{n === undefined ? label : <span aria-hidden="true">{n}</span>}
    </button>
  );

  return (
    <Panel onClose={onClose}>
      <div className="dr-toolbar dr-logbar">
        <label className="dr-searchbox dr-logsearch">
          <Icon d={ICONS.search} />
          <input
            ref={searchRef} type="search" value={query} placeholder="Rechercher" aria-label="Rechercher dans les logs" autoComplete="off" spellCheck={false}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => { if (e.key === "Escape" && query) { e.preventDefault(); e.stopPropagation(); setQuery(""); } }}
          />
          {q && <span className="dr-logcount" role="status">{shown.length} résultat{shown.length > 1 ? "s" : ""}</span>}
        </label>
        <div className="dr-seg" role="group" aria-label="Niveau">
          {seg("all", "Tout")}
          {seg("warn", "Avertissements", <Icon d={LOG_ICONS.warn} size={16} />, counts.warn)}
          {seg("error", "Erreurs", <Icon d={LOG_ICONS.error} size={16} />, counts.error)}
        </div>
        <IconButton
          aria-label={copied ? "Logs copiés" : "Copier les logs"} disabled={!lines.length}
          icon={<Icon d={copied ? ICONS.check : ICONS.copy} />} onClick={() => { void copy(); }}
        />
      </div>
      <div className="dr-logwrap">
        <div ref={ref} className="dr-logs" role="log" tabIndex={0} aria-label="Journal du jeu" onScroll={(e) => { const b = atBottom(e.currentTarget); setStuck(b); if (b) setUnseen(false); }}>
          {loading ? <p className="dr-logmsg">Chargement des logs…</p>
            : shown.length === 0 ? <p className="dr-logmsg">{!lines.length ? "Aucune ligne à afficher." : q ? `Aucune ligne ne contient « ${query.trim()} ».` : "Aucune ligne pour ce filtre."}</p>
            : shown.map(({ p, i }) => (
              <div key={i} className={`dr-line is-${p.level.toLowerCase()}`} title={p.thread || undefined}>
                {p.time && <span className="t">{p.time}</span>}
                <span className="lv">
                  {p.level === "WARN" ? <Icon d={LOG_ICONS.warn} size={14} /> : p.level === "ERROR" ? <Icon d={LOG_ICONS.error} size={14} /> : null}
                  {p.level !== "INFO" && <span className="cl-sr">{p.level === "WARN" ? "Avertissement" : "Erreur"}</span>}
                </span>
                <span className="tx">{(() => { const [tag, rest] = splitTag(p.text); return <>{tag && <span className="tag">{highlight(tag, q)}</span>}{highlight(rest, q)}</>; })()}</span>
              </div>
            ))}
        </div>
        {unseen && !stuck && (
          <button type="button" className="dr-newlines" onClick={toBottom}>
            <Icon d={LOG_ICONS.down} size={16} />Nouvelles lignes
          </button>
        )}
      </div>
      {error && <p className="home-err dr-logerr" role="alert">{error}</p>}
    </Panel>
  );
}
