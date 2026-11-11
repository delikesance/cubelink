import { useEffect, useMemo, useRef, useState } from "react";
import type { Services } from "../../services";
import { Button } from "../../ui";
import { ICONS, Icon, Panel } from "./Panel";

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

export interface LogsPanelProps {
  services: Services;
  instanceId: string;
  onClose: () => void;
}

/** Panneau « Logs » (A6) : lignes colorées par niveau, filtres, copie, suivi en direct. */
export function LogsPanel({ services, instanceId, onClose }: LogsPanelProps) {
  const [lines, setLines] = useState<string[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [filter, setFilter] = useState<Filter>("all");
  const [follow, setFollow] = useState(true);
  const ref = useRef<HTMLDivElement>(null);

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

  const parsed = useMemo(() => lines.map(parseLine), [lines]);
  const shown = useMemo(
    () => parsed.map((p, i) => ({ p, i })).filter(({ p }) => filter === "all" || (filter === "warn" ? p.level === "WARN" : p.level === "ERROR")),
    [parsed, filter],
  );

  useEffect(() => {
    const el = ref.current;
    if (el && follow) el.scrollTop = el.scrollHeight;
  }, [shown, follow]);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(lines.join("\n"));
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      setError("Copie impossible : le presse-papiers n'est pas accessible.");
    }
  };

  const chip = (f: Filter, label: string) => (
    <button type="button" className={`dr-chipbtn${filter === f ? " is-on" : ""}`} aria-pressed={filter === f} onClick={() => setFilter(f)}>{label}</button>
  );

  return (
    <Panel onClose={onClose}>
      <div className="dr-toolbar">
        {chip("all", "Tout")}
        {chip("warn", "Avertissements")}
        {chip("error", "Erreurs")}
        <span className="dr-spacer" />
        <Button variant="secondary" disabled={!lines.length} icon={<Icon d={ICONS.copy} />} onClick={() => { void copy(); }}>Copier</Button>
        <Button variant="secondary" disabled={!lines.length} onClick={() => setLines([])}>Vider l'affichage</Button>
      </div>
      <div ref={ref} className="dr-logs" role="log" tabIndex={0} aria-label="Journal du jeu">
        {loading ? <p className="dr-logmsg">Chargement des logs…</p>
          : shown.length === 0 ? <p className="dr-logmsg">{lines.length ? "Aucune ligne pour ce filtre." : "Aucune ligne à afficher."}</p>
          : shown.map(({ p, i }) => (
            <div key={i} className={`dr-line is-${p.level.toLowerCase()}`}>
              {p.time && <span className="t">{p.time}</span>}
              {p.time && <span className="lv">{p.level}</span>}
              {p.thread && <span className="th" title={p.thread}>[{p.thread}]</span>}
              <span className="tx">{p.text}</span>
            </div>
          ))}
      </div>
      {error && <p className="home-err dr-logerr" role="alert">{error}</p>}
      <footer className="dr-foot dr-logfoot">
        <Button variant="secondary" aria-pressed={follow} icon={<span className={`dr-live-dot${follow ? " is-on" : ""}`} aria-hidden="true" />} onClick={() => setFollow((v) => !v)}>
          Suivre en direct
        </Button>
        <span className="dr-count" role="status">{copied ? "Logs copiés." : `${lines.length} ligne${lines.length > 1 ? "s" : ""}`}</span>
      </footer>
    </Panel>
  );
}
