import { useCallback, useEffect, useId, useRef, useState, type KeyboardEvent } from "react";
import "./settings.css";
import { useScreenTitle, useServices } from "../../app/context";
import { navigate } from "../../app/router";
import type { Settings as BaseSettings } from "../../services/types";

/**
 * Le design expose deux réglages absents de `Settings` (relay_only, share_mods) :
 * ils sont transmis en champs additionnels, à intégrer au type par le team lead.
 */
export type SettingsData = BaseSettings & { relay_only?: boolean; share_mods?: boolean };

export const RAM_MIN_MB = 512;
export const RAM_MAX_MB = 65536;

/** Message d'erreur en français ou null. Réglage absent du design actuel (voir rapport). */
export function validateRam(mb: number): string | null {
  if (!Number.isInteger(mb) || mb < RAM_MIN_MB || mb > RAM_MAX_MB) {
    return `La mémoire doit être comprise entre ${RAM_MIN_MB} et ${RAM_MAX_MB} Mo.`;
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

const SECTIONS = [
  { id: "net", label: "Réseau P2P" },
  { id: "codes", label: "Codes" },
  { id: "cache", label: "Cache et partage" },
  { id: "credits", label: "Crédits et licences" },
] as const;
type SectionId = (typeof SECTIONS)[number]["id"];

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
  const uid = useId();
  const alive = useRef(true);
  const cancelClearRef = useRef<HTMLButtonElement>(null);
  const tabRefs = useRef<Record<string, HTMLButtonElement | null>>({});

  const [section, setSection] = useState<SectionId>("net");
  const [settings, setSettings] = useState<SettingsData | null>(null);
  const [saved, setSaved] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [code, setCode] = useState<string | null>(null);
  const [copied, setCopied] = useState("");
  const [netMsg, setNetMsg] = useState("");
  const [confirmClear, setConfirmClear] = useState(false);
  const [cacheMsg, setCacheMsg] = useState("");

  useEffect(() => {
    alive.current = true;
    services.settingsGet().then(
      (s) => { if (alive.current) setSettings(s); },
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
      setSettings(back as SettingsData);
      setSaved("Enregistré");
    } catch (e) {
      if (alive.current) setError(messageOf(e));
    }
  }, [services, settings]);

  const onTabKey = (e: KeyboardEvent<HTMLButtonElement>, i: number) => {
    let n = i;
    if (e.key === "ArrowDown" || e.key === "ArrowRight") n = (i + 1) % SECTIONS.length;
    else if (e.key === "ArrowUp" || e.key === "ArrowLeft") n = (i + SECTIONS.length - 1) % SECTIONS.length;
    else if (e.key === "Home") n = 0;
    else if (e.key === "End") n = SECTIONS.length - 1;
    else return;
    e.preventDefault();
    setSection(SECTIONS[n].id);
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

  // Focus initial dans la confirmation (bouton le moins destructeur), restitué à la fermeture.
  useEffect(() => {
    if (!confirmClear) return;
    const opener = document.activeElement as HTMLElement | null;
    cancelClearRef.current?.focus();
    return () => { if (opener?.isConnected) opener.focus(); };
  }, [confirmClear]);

  // Échap : ferme d'abord la confirmation, sinon le dialogue.
  useEffect(() => {
    const onEsc = (e: globalThis.KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      e.preventDefault();
      if (confirmClear) setConfirmClear(false);
      else navigate("main");
    };
    document.addEventListener("keydown", onEsc);
    return () => document.removeEventListener("keydown", onEsc);
  }, [confirmClear]);

  const dis = !settings;

  return (
    <div className="set">
      <div className="set-dialog" role="dialog" aria-modal="true" aria-labelledby={`${uid}-t`}>
        <header className="set-head">
          <h1 id={`${uid}-t`}>Paramètres</h1>
          <button type="button" className="set-close" aria-label="Fermer les paramètres" onClick={() => navigate("main")}>
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true"><path d="M6 6l12 12M18 6L6 18" /></svg>
          </button>
        </header>
        <div className="set-lay">
          <nav className="set-nav" aria-label="Sections">
            <div role="tablist" aria-orientation="vertical" aria-label="Sections des paramètres" className="set-tabs">
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
                  onClick={() => setSection(s.id)}
                  onKeyDown={(e) => onTabKey(e, i)}
                >{s.label}</button>
              ))}
            </div>
            <a className="set-navlink" href="#/legal">Mentions légales et confidentialité</a>
          </nav>
          <section className="set-pane" aria-label="Contenu des paramètres">
            <div className="set-status" role="status" aria-live="polite">{saved}</div>
            {error && <p className="set-error" role="alert">{error}</p>}

            <div role="tabpanel" id={`${uid}-p-net`} aria-labelledby={`${uid}-tab-net`} className="set-panel" hidden={section !== "net"}>
              <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
                <h2>Réseau P2P</h2>
                <p className="set-sub">Connexion directe quand c'est possible, sinon via un relais (un serveur intermédiaire public, plus lent).</p>
              </div>
              <div className="set-row">
                <div className="set-txt"><b>Ta connexion avec tes amis</b><span role="status" aria-live="polite">{netMsg || "Aucun test effectué pour l'instant."}</span></div>
                <button type="button" className="set-btn" onClick={() => void runTest()}>Tester ma connexion</button>
              </div>
              <label className="set-row" style={{ cursor: "pointer" }}>
                <div className="set-txt"><b>Passer toujours par le relais</b><span>Utiliser uniquement le relais&nbsp;: tes amis ne voient pas ton adresse IP, mais le relais la voit. Les transferts seront plus lents.</span></div>
                <input type="checkbox" className="set-check" disabled={dis} checked={!!settings?.relay_only} onChange={(e) => void update({ relay_only: e.target.checked })} />
              </label>
              <details className="set-more">
                <summary>En savoir plus</summary>
                <p>Ce test ne garantit pas la connexion directe avec chaque ami&nbsp;: certains réseaux l'empêchent, et Cubelink passe alors par le relais.</p>
              </details>
            </div>

            <div role="tabpanel" id={`${uid}-p-codes`} aria-labelledby={`${uid}-tab-codes`} className="set-panel" hidden={section !== "codes"}>
              <h2>Codes</h2>
              <div className="set-field">
                <b id={`${uid}-fc`}>Mon code ami</b>
                <div className="set-cr">
                  <code className="set-code" aria-labelledby={`${uid}-fc`}>{code ?? "Indisponible"}</code>
                  <button type="button" className="set-btn" aria-label="Copier mon code ami" disabled={!code} onClick={() => code && void copy("Code ami", code)}>Copier</button>
                </div>
                <span className="set-hint">Ce code ne contient aucun secret. Il permet à ton ami de te joindre&nbsp;: ne le publie pas.</span>
              </div>
              <div className="set-field">
                <b id={`${uid}-vc`}>Code de vérification</b>
                <div className="set-cr">
                  <code className="set-code" aria-labelledby={`${uid}-vc`}>Indisponible</code>
                  <button type="button" className="set-btn" aria-label="Copier mon code de vérification" disabled>Copier</button>
                </div>
                <span className="set-hint">À comparer avec ton ami par un autre canal (Discord, SMS).</span>
              </div>
              <div role="status" aria-live="polite" className="set-hint">{copied}</div>
            </div>

            <div role="tabpanel" id={`${uid}-p-cache`} aria-labelledby={`${uid}-tab-cache`} className="set-panel" hidden={section !== "cache"}>
              <h2>Cache et partage</h2>
              <div className="set-row">
                <div className="set-txt"><b>Cache de mods</b><span>Fichiers partagés entre tes instances.</span></div>
                <button type="button" className="set-btn set-danger" aria-expanded={confirmClear} onClick={() => setConfirmClear(true)}>Vider le cache…</button>
              </div>
              {confirmClear && (
                <div role="alertdialog" aria-labelledby={`${uid}-clr`} className="set-alert">
                  <strong id={`${uid}-clr`}>Vider le cache de mods&nbsp;?</strong>
                  <span className="set-hint">Les mods dont tu auras besoin seront retéléchargés. En attendant, certaines instances ou certains profils invités peuvent ne pas se lancer.</span>
                  <div style={{ display: "flex", flexWrap: "wrap", gap: 12 }}>
                    <button type="button" className="set-btn set-pri" onClick={() => void doClear()}>Vider le cache</button>
                    <button type="button" ref={cancelClearRef} className="set-btn set-sec" onClick={() => setConfirmClear(false)}>Annuler</button>
                  </div>
                </div>
              )}
              <div role="status" aria-live="polite" className="set-hint">{cacheMsg}</div>
              <label className="set-row" style={{ cursor: "pointer" }}>
                <div className="set-txt"><b>Envoyer des mods à mes amis</b><span>Mods et réglages de ton instance, pour les amis que tu as acceptés.</span></div>
                <input type="checkbox" className="set-check" disabled={dis} checked={settings?.share_mods ?? true} onChange={(e) => void update({ share_mods: e.target.checked })} />
              </label>
              <details className="set-more">
                <summary>En savoir plus</summary>
                <p>Les amis que tu as acceptés peuvent récupérer chez toi les mods et les réglages (fichiers de configuration) de ton instance quand ils te rejoignent. Ta sauvegarde n'est pas concernée. Vérifie tes fichiers de configuration si tu y as mis des mots de passe. Cela utilise ta bande passante.</p>
              </details>
            </div>

            <div role="tabpanel" id={`${uid}-p-credits`} aria-labelledby={`${uid}-tab-credits`} className="set-panel" hidden={section !== "credits"}>
              <h2>Crédits et licences</h2>
              <p style={{ color: "#F2F2F2" }}>Cubelink est un projet indépendant. Il n'est pas un produit Minecraft officiel, n'est pas approuvé par Mojang ou Microsoft et n'est pas associé à eux.</p>
              <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
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
              <a className="set-lnk" href="#/legal" style={{ minHeight: 44, display: "inline-flex", alignItems: "center", alignSelf: "flex-start" }}>Mentions légales et confidentialité</a>
            </div>
          </section>
        </div>
      </div>
    </div>
  );
}
