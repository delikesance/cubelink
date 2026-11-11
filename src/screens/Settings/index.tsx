import { useCallback, useEffect, useId, useRef, useState, type CSSProperties, type KeyboardEvent } from "react";
import "./settings.css";
import { useScreenTitle, useServices, useShell } from "../../app/context";
import { useBack } from "../../app/context";
import { Button, Card, Field, Input } from "../../ui";
import { UpdateSection } from "./UpdateSection";
import { Logo } from "../Login/Logo";
import { Accounts } from "./Accounts";
import type { Settings as BaseSettings } from "../../services/types";

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
  { id: "reseau", label: "Réseau" },
  { id: "java", label: "Java" },
  { id: "stockage", label: "Stockage" },
  { id: "apropos", label: "À propos" },
] as const;
export type SectionId = (typeof SECTIONS)[number]["id"];

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
  const [code, setCode] = useState<string | null>(null);
  const [copied, setCopied] = useState("");
  const [netMsg, setNetMsg] = useState("");
  const [confirmClear, setConfirmClear] = useState(false);
  const [cacheMsg, setCacheMsg] = useState("");
  const [javaPath, setJavaPath] = useState("");
  const [ram, setRam] = useState("");
  const [hotkey, setHotkey] = useState(DEFAULT_HOTKEY);

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
    services.friendMyCode().then(
      (c) => { if (alive.current) setCode(c); },
      () => { /* code indisponible : affiché comme tel */ },
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

  const copy = async (what: string, text: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(`${what} copié.`);
    } catch {
      setCopied("Copie impossible : sélectionne le texte à la main.");
    }
  };

  const runTest = async () => {
    setNetMsg("Test en cours…");
    try {
      setNetMsg(testConnection ? await testConnection() : "Le test de connexion n'est pas encore disponible.");
    } catch (e) {
      setNetMsg(messageOf(e));
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
    <div className="set">
      <h1 ref={h1} tabIndex={-1} className="enter">Réglages</h1>
      <div role="tablist" aria-label="Sections des réglages" className="set-tabs enter" style={{ "--i": 1 } as CSSProperties}>
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
          >{s.label}</button>
        ))}
        <div className="set-bar" aria-hidden="true" style={{ transform: `translateX(${idx * 140}px)` }} />
      </div>
      <div className="set-status" role="status" aria-live="polite">{saved}</div>
      {error && <p className="set-error" role="alert">{error}</p>}

      <div {...panel("compte")}>
        <Accounts onError={setError} />
        <Card as="section" aria-label="Code ami" className="set-field">
          <b id={`${uid}-fc`}>Mon code ami</b>
          <div className="set-cr">
            <code className="set-code" aria-labelledby={`${uid}-fc`}>{code ?? "Indisponible"}</code>
            <Button variant="secondary" aria-label="Copier mon code ami" disabled={!code} onClick={() => code && void copy("Code ami", code)}>Copier</Button>
          </div>
          <span className="set-hint">Ce code ne contient aucun secret. Il permet à ton ami de te joindre&nbsp;: ne le publie pas.</span>
          <div role="status" aria-live="polite" className="set-hint">{copied}</div>
        </Card>
      </div>

      <div {...panel("reseau")}>
        <p className="set-sub">Connexion directe quand c'est possible, sinon via un relais (un serveur intermédiaire public, plus lent).</p>
        <Card className="set-list">
          <div className="set-row">
            <div className="set-txt"><b>Ta connexion avec tes amis</b><span role="status" aria-live="polite">{netMsg || "Aucun test effectué pour l'instant."}</span></div>
            <Button variant="secondary" onClick={() => void runTest()}>Tester ma connexion</Button>
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
        <Card as="section" aria-label="Panneau en jeu" className="set-field">
          <label className="set-row" style={{ cursor: "pointer" }}>
            <div className="set-txt"><b>Panneau en jeu</b><span>Affiche par-dessus Minecraft tes amis, les invitations et tes modpacks, avec un raccourci. Appliqué au prochain lancement du jeu.</span></div>
            <input type="checkbox" className="set-check" disabled={dis} checked={settings?.overlay_enabled !== false} onChange={(e) => void update({ overlay_enabled: e.target.checked })} />
          </label>
          <Field label="Raccourci du panneau" hint="Exemple : Ctrl+Shift+C. En plein écran exclusif, le panneau peut être masqué : passe en fenêtré ou sans bordure." error={hotkeyError}>
            {(p) => (
              <Input {...p} type="text" autoComplete="off" spellCheck={false} disabled={dis || settings?.overlay_enabled === false}
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
          <Field label="Mémoire par défaut (Mo)" error={ramError}>
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
  );
}
