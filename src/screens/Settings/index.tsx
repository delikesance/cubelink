import { useCallback, useEffect, useId, useRef, useState, type CSSProperties, type KeyboardEvent, type ReactNode } from "react";
import "./settings.css";
import { useScreenTitle, useServices, useShell } from "../../app/context";
import { useBack } from "../../app/context";
import { Button, Card, Field, Input } from "../../ui";
import { UpdateSection } from "./UpdateSection";
import { Logo } from "../Login/Logo";
import { Accounts } from "./Accounts";
import { useStageSupported } from "../../app/stage";
import type { NetTest, Settings as BaseSettings } from "../../services/types";
import { Icon } from "../../ui";
import "../Friends/friends.css";
import MyCode from "../Friends/MyCode";
import Requests from "../Friends/Requests";
import Add from "../Friends/Add";

/** Icônes Iconoir du menu des réglages. */
const SECTION_ICONS: Record<string, ReactNode> = {
  compte: <><path d="M5 20v-1a7 7 0 0 1 14 0v1" /><path d="M12 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8z" /></>,
  amis: <><path d="M1 20v-1a7 7 0 0 1 14 0v1M13 14a5 5 0 0 1 10 0v.5" /><path d="M8 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM18 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6z" /></>,
  reseau: <path d="M12 19.51l.01-.011M2 8c6-4.5 14-4.5 20 0M5 12c4-3 10-3 14 0M8.5 15.5c2.25-1.5 4.75-1.5 7 0" />,
  java: <path d="M17 11.6V15a6 6 0 0 1-6 6H9a6 6 0 0 1-6-6v-3.4a.6.6 0 0 1 .6-.6h12.8a.6.6 0 0 1 .6.6zM12 9c0-1 .714-3 3.571-3C18.43 6 18.43 3 18.43 3M8 9v-.5a2 2 0 0 1 2-2 2 2 0 0 0 2-2V4M16 14h2.5a2.5 2.5 0 0 0 0-5H17" />,
  stockage: <path d="M5 12v6s0 3 7 3 7-3 7-3v-6M5 6v6s0 3 7 3 7-3 7-3V6M12 3c7 0 7 3 7 3s0 3-7 3-7-3-7-3 0-3 7-3z" />,
  apropos: <path d="M12 11.5v5M12 7.51l.01-.011M12 22c5.523 0 10-4.477 10-10S17.523 2 12 2 2 6.477 2 12s4.477 10 10 10z" />,
};

export type SettingsData = BaseSettings;

export const RAM_MIN_MB = 512;
export const RAM_MAX_MB = 65536;

/** Message d'erreur en français ou null. */
export function validateRam(mb: number): string | null {
  if (!Number.isInteger(mb) || mb < RAM_MIN_MB || mb > RAM_MAX_MB) {
    return `La mémoire doit être comprise entre ${RAM_MIN_MB} et ${RAM_MAX_MB} Mo.`;
  }
  return null;
}

export const DEFAULT_HOTKEY = "Ctrl+Shift+C";
const HOTKEY_MODS = ["ctrl", "control", "alt", "shift", "super", "cmd", "command", "meta", "win", "cmdorctrl", "commandorcontrol"];

/** Raccourci global du type « Ctrl+Shift+C » : au moins un modificateur et une seule touche. Message en français ou null. */
export function validateHotkey(value: string): string | null {
  const parts = value.split("+").map((p) => p.trim());
  if (value.trim() === "") return "Indique un raccourci, par exemple Ctrl+Shift+C.";
  if (parts.some((p) => p === "")) return "Raccourci invalide : écris les touches séparées par « + », par exemple Ctrl+Shift+C.";
  const mods = parts.filter((p) => HOTKEY_MODS.includes(p.toLowerCase()));
  const keys = parts.filter((p) => !HOTKEY_MODS.includes(p.toLowerCase()));
  if (keys.length !== 1) return "Le raccourci doit contenir exactement une touche (lettre, chiffre, F1 à F24…).";
  if (mods.length === 0) return "Ajoute au moins un modificateur (Ctrl, Alt ou Maj) pour ne pas gêner la frappe.";
  if (!/^([a-z0-9]|f([1-9]|1\d|2[0-4])|space|tab|enter|escape|esc|backspace|delete|insert|home|end|pageup|pagedown|up|down|left|right|arrow(up|down|left|right))$/i.test(keys[0])) {
    return `Touche « ${keys[0]} » non reconnue.`;
  }
  return null;
}

/** Chemin Java facultatif : vide accepté, sinon sans saut de ligne ni caractères interdits. */
export function validateJavaPath(path: string): string | null {
  const p = path.trim();
  if (!p) return null;
  if (/[\r\n"<>|]/.test(p)) return "Le chemin de Java contient des caractères invalides.";
  return null;
}

export const SECTIONS = [
  { id: "compte", label: "Compte" },
  { id: "amis", label: "Amis" },
  { id: "reseau", label: "Réseau" },
  { id: "java", label: "Java" },
  { id: "stockage", label: "Stockage" },
  { id: "apropos", label: "À propos" },
] as const;
export type SectionId = (typeof SECTIONS)[number]["id"];

/** Région lisible d'un relais n0 (« euc1-1.relay.n0.iroh.link » → « Europe »). */
function relayPlace(host: string | null): string {
  const h = (host ?? "").toLowerCase();
  if (h.startsWith("eu")) return "Europe";
  if (h.startsWith("us")) return "Amérique du Nord";
  if (h.startsWith("ap")) return "Asie-Pacifique";
  return host ?? "";
}

export type NetTone = "ok" | "warn" | "danger" | "neutral";
/** Un point vérifié : `ok` null = non mesuré ou sans objet. */
export interface NetCheck { label: string; ok: boolean | null; detail: string }
/** Verdict lisible d'un coup d'œil : niveau (couleur + icône + mot), ce que ça change, puis le détail. */
export interface NetVerdict { tone: NetTone; label: string; summary: string; checks: NetCheck[] }

export function netVerdict(t: NetTest): NetVerdict {
  const where = relayPlace(t.relay);
  const relayDetail = t.relay_connected
    ? ["joignable", where, t.relay_ms != null ? `${t.relay_ms} ms` : ""].filter(Boolean).join(" · ")
    : "injoignable";
  const direct: NetCheck = t.relay_only
    ? { label: "Connexion directe", ok: null, detail: "désactivée par ton réglage" }
    : t.partial
      ? { label: "Connexion directe", ok: null, detail: "pas encore mesurée" }
      : !t.udp
        ? { label: "Connexion directe", ok: false, detail: "bloquée par un pare-feu (UDP filtré)" }
        : t.direct === false
          ? { label: "Connexion directe", ok: false, detail: "gênée par ton routeur" }
          : { label: "Connexion directe", ok: true, detail: "possible" };
  const relay: NetCheck = { label: "Relais de secours", ok: t.relay_connected, detail: relayDetail };
  const checks = [direct, relay];
  const v = (tone: NetTone, label: string, summary: string): NetVerdict => ({ tone, label, summary, checks });

  if (!t.relay_connected && (!t.udp || t.relay_only)) {
    return v("danger", "Hors ligne", t.relay_only
      ? "Personne ne peut te rejoindre : le relais est injoignable et tu as choisi de toujours passer par lui."
      : "Personne ne peut te rejoindre. Vérifie ta connexion Internet ou ton pare-feu.");
  }
  if (t.relay_only) return v("ok", "Relais uniquement", "Comme demandé, tout passe par le relais : tes amis te rejoignent, un peu plus lentement.");
  if (t.partial) return v("neutral", "Test incomplet", "La mesure n'est pas terminée. Réessaie dans quelques secondes.");
  if (!t.udp) return v("warn", "Limitée", "Tes amis peuvent te rejoindre, mais tout passe par le relais : plus lent.");
  if (t.direct === false) {
    return t.relay_connected
      ? v("warn", "Correcte", "Tes amis peuvent te rejoindre ; certaines parties passeront par le relais, un peu plus lent.")
      : v("danger", "Fragile", "Certains amis risquent de ne pas pouvoir te rejoindre.");
  }
  return t.relay_connected
    ? v("ok", "Excellente", "Tout est bon : tes amis peuvent te rejoindre directement, sans ralentissement.")
    : v("warn", "Bonne", "Connexion directe possible, mais sans secours si le routeur d'un ami la bloque.");
}

/** Icônes Iconoir (trait) : réussite, avertissement, échec, non mesuré. */
const NET_ICONS: Record<NetTone, ReactNode> = {
  ok: <path d="M7 12.5l3 3 7-7M12 22c5.523 0 10-4.477 10-10S17.523 2 12 2 2 6.477 2 12s4.477 10 10 10z" />,
  warn: <path d="M12 8v4M12 16.01l.01-.011M12 22c5.523 0 10-4.477 10-10S17.523 2 12 2 2 6.477 2 12s4.477 10 10 10z" />,
  danger: <path d="M9.172 14.828L12.001 12m2.828-2.828L12.001 12m0 0L9.172 9.172M12.001 12l2.828 2.828M12 22c5.523 0 10-4.477 10-10S17.523 2 12 2 2 6.477 2 12s4.477 10 10 10z" />,
  neutral: <path d="M8 12h8M12 22c5.523 0 10-4.477 10-10S17.523 2 12 2 2 6.477 2 12s4.477 10 10 10z" />,
};

function NetIcon({ tone, size = 18 }: { tone: NetTone; size?: number }) {
  return (
    <svg className={`set-net-ic set-net-${tone}`} width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      {NET_ICONS[tone]}
    </svg>
  );
}

function NetResult({ v }: { v: NetVerdict }) {
  return (
    <div className="set-net">
      <div className="set-net-head">
        <span className={`set-net-pill set-net-${v.tone}`}><NetIcon tone={v.tone} size={16} />{v.label}</span>
        <span className="set-net-sum">{v.summary}</span>
      </div>
      <ul className="set-net-checks">
        {v.checks.map((c) => (
          <li key={c.label}>
            <NetIcon tone={c.ok === null ? "neutral" : c.ok ? "ok" : "warn"} size={16} />
            <span className="set-net-lbl">{c.label}</span>
            <span className="set-net-det">{c.detail}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}

function messageOf(e: unknown): string {
  if (e instanceof Error && e.message) return e.message;
  if (typeof e === "string" && e) return e;
  return "L'opération a échoué. Réessaie dans un instant.";
}

export interface SettingsProps {
  /** Test de connexion réel (à brancher) ; renvoie une phrase de résultat. */
  testConnection?: () => Promise<string>;
  /** Vidage du cache (à brancher) ; sans lui, l'écran indique que ce n'est pas disponible. */
  clearCache?: () => Promise<void>;
}

export default function Settings({ testConnection, clearCache }: SettingsProps = {}) {
  useScreenTitle(null);
  const services = useServices();
  const { params } = useShell();
  const uid = useId();
  const alive = useRef(true);
  const cancelClearRef = useRef<HTMLButtonElement>(null);
  const back = useBack("main");
  const backRef = useRef(back);
  backRef.current = back;
  const h1 = useRef<HTMLHeadingElement>(null);
  useEffect(() => { h1.current?.focus(); }, []);
  const tabRefs = useRef<Record<string, HTMLButtonElement | null>>({});

  const initial = SECTIONS.find((s) => s.id === params.tab)?.id ?? "compte";
  const [section, setSection] = useState<SectionId>(initial);
  const [settings, setSettings] = useState<SettingsData | null>(null);
  const [saved, setSaved] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [netMsg, setNetMsg] = useState("");
  const [confirmClear, setConfirmClear] = useState(false);
  const [cacheMsg, setCacheMsg] = useState("");
  const [javaPath, setJavaPath] = useState("");
  const [ram, setRam] = useState("");
  const [hotkey, setHotkey] = useState(DEFAULT_HOTKEY);
  // Le jeu intégré n'existe que sous Windows et Linux (X11) : ailleurs le réglage n'aurait aucun effet, il n'est pas montré.
  const stageSupported = useStageSupported(services);
  // Sans jeu intégré il n'y a pas de panneau « en jeu » : son raccourci ne sert à rien, on le dit plutôt que de le laisser modifiable.
  const embeddedOff = stageSupported && settings?.embedded_game === false;

  useEffect(() => {
    alive.current = true;
    services.settingsGet().then(
      (s) => {
        if (!alive.current) return;
        setSettings(s);
        setJavaPath(s.java_path ?? "");
        setRam(String(s.default_ram_mb));
        setHotkey(s.overlay_hotkey || DEFAULT_HOTKEY);
      },
      (e) => { if (alive.current) setError(messageOf(e)); },
    );
    return () => { alive.current = false; };
  }, [services]);

  const update = useCallback(async (patch: Partial<SettingsData>) => {
    if (!settings) return;
    const next = { ...settings, ...patch };
    setSettings(next);
    setSaved("");
    setError(null);
    try {
      const back = await services.settingsSet(next);
      if (!alive.current) return;
      setSettings(back);
      setSaved("Enregistré");
    } catch (e) {
      if (alive.current) setError(messageOf(e));
    }
  }, [services, settings]);

  const select = (id: SectionId) => {
    setSection(id);
    setSaved("");
  };

  const onTabKey = (e: KeyboardEvent<HTMLButtonElement>, i: number) => {
    let n = i;
    if (e.key === "ArrowDown" || e.key === "ArrowRight") n = (i + 1) % SECTIONS.length;
    else if (e.key === "ArrowUp" || e.key === "ArrowLeft") n = (i + SECTIONS.length - 1) % SECTIONS.length;
    else if (e.key === "Home") n = 0;
    else if (e.key === "End") n = SECTIONS.length - 1;
    else return;
    e.preventDefault();
    select(SECTIONS[n].id);
    tabRefs.current[SECTIONS[n].id]?.focus();
  };

  const [netBusy, setNetBusy] = useState(false);
  const [netRes, setNetRes] = useState<NetVerdict | null>(null);
  const runTest = async () => {
    if (netBusy) return;
    setNetBusy(true);
    setNetRes(null);
    setNetMsg("Test en cours… (quelques secondes)");
    try {
      if (testConnection) setNetMsg(await testConnection());
      else { setNetRes(netVerdict(await services.netTest())); setNetMsg(""); }
    } catch (e) {
      setNetMsg(messageOf(e));
    } finally {
      setNetBusy(false);
    }
  };

  const doClear = async () => {
    try {
      if (!clearCache) setCacheMsg("Le vidage du cache n'est pas encore disponible.");
      else { await clearCache(); setCacheMsg("Cache vidé."); }
    } catch (e) {
      setCacheMsg(messageOf(e));
    }
    setConfirmClear(false);
  };

  // Réglages Java : enregistrés à la sortie du champ, seulement s'ils sont valides et modifiés.
  const javaError = validateJavaPath(javaPath) ?? undefined;
  const ramNum = Number(ram);
  const ramError = ram.trim() === "" ? "Indique une mémoire en Mo." : validateRam(ramNum) ?? undefined;
  const commitJava = () => {
    if (!settings || javaError) return;
    const next = javaPath.trim() === "" ? null : javaPath.trim();
    if (next !== (settings.java_path ?? null)) void update({ java_path: next });
  };
  const hotkeyError = validateHotkey(hotkey) ?? undefined;
  const commitHotkey = () => {
    if (!settings || hotkeyError) return;
    const next = hotkey.trim();
    if (next !== (settings.overlay_hotkey || DEFAULT_HOTKEY)) void update({ overlay_hotkey: next });
  };
  const commitRam = () => {
    if (!settings || ramError) return;
    if (ramNum !== settings.default_ram_mb) void update({ default_ram_mb: ramNum });
  };

  // Focus initial dans la confirmation (bouton le moins destructeur), restitué à la fermeture.
  useEffect(() => {
    if (!confirmClear) return;
    const opener = document.activeElement as HTMLElement | null;
    cancelClearRef.current?.focus();
    return () => { if (opener?.isConnected) opener.focus(); };
  }, [confirmClear]);

  // Échap : ferme d'abord la confirmation, sinon revient en arrière.
  useEffect(() => {
    const onEsc = (e: globalThis.KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      const t = e.target as HTMLElement | null;
      if (t && /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName) && (t as HTMLInputElement).type !== "checkbox") return;
      e.preventDefault();
      if (confirmClear) setConfirmClear(false);
      else backRef.current();
    };
    document.addEventListener("keydown", onEsc);
    return () => document.removeEventListener("keydown", onEsc);
  }, [confirmClear]);

  const dis = !settings;
  const idx = SECTIONS.findIndex((s) => s.id === section);
  const panel = (id: SectionId) => ({
    role: "tabpanel" as const, id: `${uid}-p-${id}`, "aria-labelledby": `${uid}-tab-${id}`, className: "set-panel enter", hidden: section !== id,
  });

  return (
    <div className="set screen">
      <nav className="set-nav">
      <h1 ref={h1} tabIndex={-1} className="enter">Paramètres</h1>
      <div role="tablist" aria-label="Sections des réglages" aria-orientation="vertical" className="set-tabs enter" style={{ "--i": 1 } as CSSProperties}>
        {SECTIONS.map((s, i) => (
          <button
            key={s.id}
            type="button"
            role="tab"
            id={`${uid}-tab-${s.id}`}
            aria-selected={section === s.id}
            aria-controls={`${uid}-p-${s.id}`}
            tabIndex={section === s.id ? 0 : -1}
            className="set-tab"
            ref={(el) => { tabRefs.current[s.id] = el; }}
            onClick={() => select(s.id)}
            onKeyDown={(e) => onTabKey(e, i)}
          ><Icon d={SECTION_ICONS[s.id]} size={20} />{s.label}</button>
        ))}
      </div>
      </nav>
      <div className="set-main">
      <div className="set-head">
        <h2 className="set-title">{SECTIONS[idx].label}</h2>
        <div className="set-status" role="status" aria-live="polite">{saved}</div>
      </div>
      {error && <p className="set-error" role="alert">{error}</p>}

      <div {...panel("compte")}>
        <Accounts onError={setError} />
      </div>

      <div {...panel("amis")}>
        {section === "amis" && <>
          <Requests />
          <div className="fr-cards">
            <Card as="div" className="fr-card"><Add /></Card>
            <Card as="div" className="fr-card"><MyCode /></Card>
          </div>
        </>}
      </div>

      <div {...panel("reseau")}>
        <p className="set-sub">Connexion directe quand c'est possible, sinon via un relais (un serveur intermédiaire public, plus lent).</p>
        <Card className="set-list">
          <div className="set-row">
            <div className="set-txt">
              <b>Ta connexion avec tes amis</b>
              <div role="status" aria-live="polite">
                {netRes ? <NetResult v={netRes} /> : <span className="set-net-msg">{netMsg || "Aucun test effectué pour l'instant."}</span>}
              </div>
            </div>
            <Button variant="secondary" disabled={netBusy} onClick={() => void runTest()}>{netBusy ? "Test en cours…" : "Tester ma connexion"}</Button>
          </div>
          <label className="set-row" style={{ cursor: "pointer" }}>
            <div className="set-txt"><b>Passer toujours par le relais</b><span>Utiliser uniquement le relais&nbsp;: tes amis ne voient pas ton adresse IP, mais le relais la voit. Les transferts seront plus lents.</span></div>
            <input type="checkbox" className="set-check" disabled={dis} checked={!!settings?.relay_only} onChange={(e) => void update({ relay_only: e.target.checked })} />
          </label>
          <label className="set-row" style={{ cursor: "pointer" }}>
            <div className="set-txt"><b>Retrouver mes amis même si leur adresse change</b><span>Publie ta présence via les serveurs n0 pour que tes amis te retrouvent. Désactive pour rester totalement privé. Appliqué au prochain démarrage.</span></div>
            <input type="checkbox" className="set-check" disabled={dis} checked={settings?.discovery !== false} onChange={(e) => void update({ discovery: e.target.checked })} />
          </label>
          <label className="set-row" style={{ cursor: "pointer" }}>
            <div className="set-txt"><b>Ouvrir mon monde au réseau local automatiquement</b><span>Quand tu entres dans un monde solo, Cubelink l'ouvre pour que tes amis puissent te rejoindre. Appliqué au prochain lancement du jeu.</span></div>
            <input type="checkbox" className="set-check" disabled={dis} checked={settings?.lan_auto !== false} onChange={(e) => void update({ lan_auto: e.target.checked })} />
          </label>
        </Card>
        <Card as="section" aria-label="Raccourci jeu et launcher" className="set-field">
          {stageSupported && (
            <label className="set-row" style={{ cursor: "pointer" }}>
              <div className="set-txt"><b>Jeu intégré</b><span>Le jeu s'affiche dans la fenêtre de Cubelink. Désactive si le jeu ne s'affiche pas correctement. Appliqué au prochain lancement du jeu.</span></div>
              <input type="checkbox" className="set-check" disabled={dis} checked={settings?.embedded_game !== false} onChange={(e) => void update({ embedded_game: e.target.checked })} />
            </label>
          )}
          <label className="set-row" style={{ cursor: "pointer" }}>
            <div className="set-txt"><b>Raccourci jeu et launcher</b><span>Pendant une partie, un raccourci bascule entre le jeu et le launcher (écran Amis : invitations, rejoindre un ami), même quand le jeu a le clavier.</span></div>
            <input type="checkbox" className="set-check" disabled={dis} checked={settings?.overlay_enabled !== false} onChange={(e) => void update({ overlay_enabled: e.target.checked })} />
          </label>
          <Field
            label="Raccourci du panneau"
            hint={embeddedOff
              ? "Indisponible : le panneau s'ouvre par-dessus le jeu intégré. Active « Jeu intégré » pour t'en servir."
              : "Exemple : Ctrl+Shift+C. Windows et Linux (X11) : le jeu est affiché dans la fenêtre du launcher."}
            error={embeddedOff ? undefined : hotkeyError}
          >
            {(p) => (
              <Input {...p} type="text" autoComplete="off" spellCheck={false} disabled={dis || embeddedOff || settings?.overlay_enabled === false}
                value={hotkey} onChange={(e) => setHotkey(e.target.value)} onBlur={commitHotkey}
                onKeyDown={(e) => { if (e.key === "Enter") commitHotkey(); }} />
            )}
          </Field>
        </Card>
        <details className="set-more">
          <summary>En savoir plus</summary>
          <p>Ce test ne garantit pas la connexion directe avec chaque ami&nbsp;: certains réseaux l'empêchent, et Cubelink passe alors par le relais.</p>
        </details>
      </div>

      <div {...panel("java")}>
        <p className="set-sub">Réglages appliqués par défaut aux nouvelles instances. Chaque instance garde ses propres valeurs.</p>
        <Card className="set-two">
          <Field label="Mémoire par défaut (Mo)" error={ramError} hint="Utilisée par toutes les instances réglées sur « Valeur des paramètres », dont les mondes de tes amis.">
            {(p) => (
              <Input {...p} type="number" inputMode="numeric" min={RAM_MIN_MB} max={RAM_MAX_MB} step={256} disabled={dis}
                value={ram} onChange={(e) => setRam(e.target.value)} onBlur={commitRam}
                onKeyDown={(e) => { if (e.key === "Enter") commitRam(); }} />
            )}
          </Field>
          <Field label="Chemin de Java" hint="Facultatif. Laisse vide si tu ne sais pas." error={javaError}>
            {(p) => (
              <Input {...p} type="text" spellCheck={false} autoComplete="off" disabled={dis}
                value={javaPath} onChange={(e) => setJavaPath(e.target.value)} onBlur={commitJava}
                onKeyDown={(e) => { if (e.key === "Enter") commitJava(); }} />
            )}
          </Field>
        </Card>
      </div>

      <div {...panel("stockage")}>
        <Card className="set-list">
          <div className="set-row">
            <div className="set-txt"><b>Cache de mods</b><span>Fichiers partagés entre tes instances.</span></div>
            <Button variant="secondary" aria-expanded={confirmClear} onClick={() => setConfirmClear(true)}>Vider le cache…</Button>
          </div>
          <label className="set-row" style={{ cursor: "pointer" }}>
            <div className="set-txt"><b>Envoyer des mods à mes amis</b><span>Mods et réglages de ton instance, pour les amis que tu as acceptés.</span></div>
            <input type="checkbox" className="set-check" disabled={dis} checked={settings?.share_mods ?? true} onChange={(e) => void update({ share_mods: e.target.checked })} />
          </label>
        </Card>
        {confirmClear && (
          <div role="alertdialog" aria-labelledby={`${uid}-clr`} className="set-alert enter">
            <strong id={`${uid}-clr`}>Vider le cache de mods&nbsp;?</strong>
            <span className="set-hint">Les mods dont tu auras besoin seront retéléchargés. En attendant, certaines instances ou certains profils invités peuvent ne pas se lancer.</span>
            <div className="set-acts">
              <Button variant="danger" onClick={() => void doClear()}>Vider le cache</Button>
              <Button variant="secondary" ref={cancelClearRef} onClick={() => setConfirmClear(false)}>Annuler</Button>
            </div>
          </div>
        )}
        <div role="status" aria-live="polite" className="set-hint">{cacheMsg}</div>
        <details className="set-more">
          <summary>En savoir plus</summary>
          <p>Les amis que tu as acceptés peuvent récupérer chez toi les mods et les réglages (fichiers de configuration) de ton instance quand ils te rejoignent. Ta sauvegarde n'est pas concernée. Vérifie tes fichiers de configuration si tu y as mis des mots de passe. Cela utilise ta bande passante.</p>
        </details>
      </div>

      <div {...panel("apropos")}>
        <Card className="set-about">
          <Logo width={40} />
          <div><b>Cubelink</b><span>Launcher Minecraft pour jouer avec tes amis</span></div>
          <a className="cl-btn cl-btn-secondary set-linkbtn" href="#/legal">Mentions légales</a>
        </Card>
        <UpdateSection />
        <p style={{ margin: 0, color: "var(--muted)", lineHeight: 1.6 }}>Cubelink est un projet indépendant. Il n'est pas un produit Minecraft officiel, n'est pas approuvé par Mojang ou Microsoft et n'est pas associé à eux.</p>
        <div>
          <details className="set-more">
            <summary>Images (licence OGA-BY&nbsp;3.0 et CC0)</summary>
            <p style={{ marginBottom: 12 }}>«&nbsp;Moon and Sea Pixel Art Background&nbsp;» par CraftPix.net 2D Game Assets, <a className="set-lnk" href="https://opengameart.org/content/moon-and-sea-pixel-art-background" target="_blank" rel="noopener noreferrer" style={{ wordBreak: "break-all" }}>https://opengameart.org/content/moon-and-sea-pixel-art-background</a>, licence <a className="set-lnk" href="https://opengameart.org/content/oga-by-30-faq" target="_blank" rel="noopener noreferrer">OGA-BY&nbsp;3.0</a>. Modifié&nbsp;: converti en JPEG ; recadré pour la vignette «&nbsp;Créer un monde&nbsp;».</p>
            <p>Autres illustrations (CC0), via OpenGameArt.org&nbsp;: «&nbsp;City Parallax pixel art&nbsp;» par Gustavo Saraiva, «&nbsp;City Scrollable Background Pixel Art&nbsp;» par Quantiset, «&nbsp;Desert Forest&nbsp;» par LLGD, «&nbsp;RPG portraits&nbsp;» par Buch. Aucune ressource Minecraft ou Mojang n'est utilisée.</p>
          </details>
          <details className="set-more">
            <summary>Polices (SIL Open Font License&nbsp;1.1)</summary>
            <p>Polices Inter (The Inter Project Authors) et Montserrat (The Montserrat Project Authors), sous licence <a className="set-lnk" href="https://openfontlicense.org/" target="_blank" rel="noopener noreferrer">SIL Open Font License&nbsp;1.1</a>.</p>
          </details>
        </div>
      </div>
      </div>
    </div>
  );
}
