import { useEffect, useRef, useState } from "react";
import "./login.css";
import { useScreenTitle, useServices, useShell } from "../../app/context";
import { navigate } from "../../app/router";
import { Button, Card } from "../../ui";
import { BlockPattern, Logo } from "./Logo";
import { OfflineForm } from "./OfflineForm";
import { useDeviceLogin } from "./useDeviceLogin";

/** Délai d'affichage de l'état « Connecté » avant d'ouvrir l'accueil (le bouton Continuer passe tout de suite). */
export const WELCOME_MS = 700;
const RING = 553;

export function formatCountdown(s: number): string {
  const m = Math.floor(s / 60);
  return `${m}:${String(s % 60).padStart(2, "0")}`;
}

/** Choisit l'écran Erreur adapté à un échec de connexion ; null = message en ligne avec Réessayer. */
export function classifyLoginFailure(message: string): "no-license" | "cancelled" | "offline" | null {
  const m = message.toLowerCase();
  if (/expir|jeton/.test(m)) return null;
  if (/ne possède pas|licen|aucun profil/.test(m)) return "no-license";
  if (/refusée par l'utilisateur|annul|cancel|declin/.test(m)) return "cancelled";
  if (/réseau|reseau|network|offline|hors ligne|internet|http|dns|timeout|injoignable/.test(m)) return "offline";
  return null;
}

const Tick = () => (
  <svg width="72" height="72" viewBox="0 0 24 24" fill="none" stroke="var(--ok)" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <path className="lc-tick" d="M5 12.5l4.5 4.5L19 7.5" />
  </svg>
);
const CopyTick = () => (
  <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="var(--ok)" strokeWidth="2.25" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M5 12.5l4.5 4.5L19 7.5" /></svg>
);

/** Anneau de temps restant ; `progress` entre 0 et 1, null = pas d'arc (expiré, connecté). */
function Ring({ label, progress, color, children }: { label: string; progress: number | null; color?: string; children: React.ReactNode }) {
  return (
    <div className="lc-ring">
      <svg width="180" height="180" viewBox="0 0 200 200" role="img" aria-label={label}>
        <circle cx="100" cy="100" r="88" fill="none" stroke={color ?? "var(--line)"} strokeWidth="8" />
        {progress !== null && (
          <circle cx="100" cy="100" r="88" fill="none" stroke="var(--accent)" strokeWidth="8" strokeLinecap="round"
            strokeDasharray={RING} strokeDashoffset={RING * (1 - Math.max(0, Math.min(1, progress)))} />
        )}
      </svg>
      {children}
    </div>
  );
}

function spokenTime(s: number): string {
  return `${Math.floor(s / 60)} minute${s >= 120 ? "s" : ""} ${s % 60} seconde${s % 60 > 1 ? "s" : ""}`;
}

export default function Login() {
  useScreenTitle(null);
  const services = useServices();
  const { params } = useShell();
  const [offlineOpen, setOfflineOpen] = useState(false);
  const h1 = useRef<HTMLHeadingElement>(null);
  useEffect(() => { h1.current?.focus(); }, []);
  const { state, left, copied, start, copy } = useDeviceLogin(services, (message) => {
    const kind = classifyLoginFailure(message);
    if (!kind) return false;
    navigate("error", { state: kind });
    return true;
  });

  // Connecté : on laisse voir la confirmation, puis accueil.
  const connected = state.step === "connected";
  useEffect(() => {
    if (!connected) return;
    const t = setTimeout(() => navigate("main"), WELCOME_MS);
    return () => clearTimeout(t);
  }, [connected]);

  const onCode = state.step === "code" || state.step === "expired" || state.step === "connected" || state.step === "error";

  return (
    <div className="login">
      <BlockPattern />
      <div className="login-main">
        {!onCode ? (
          <div className="login-col">
            <Card as="section" className="login-card" aria-labelledby="login-title">
              <div className="login-brand enter">
                <Logo width={44} />
                <b>Cubelink</b>
              </div>
              <h1 id="login-title" ref={h1} tabIndex={-1} className="enter" style={{ "--i": 1 } as React.CSSProperties}>Joue avec tes amis, sans serveur.</h1>
              <p className="login-lead enter" style={{ "--i": 1 } as React.CSSProperties}>Connecte ton compte Minecraft pour lancer tes modpacks.</p>
              <Button className="login-wide enter" style={{ "--i": 2 } as React.CSSProperties} loading={state.step === "starting"} onClick={() => { void start(); }}>
                Se connecter avec Microsoft
              </Button>
              <button
                type="button" className="login-ghost enter" style={{ "--i": 2 } as React.CSSProperties}
                aria-expanded={offlineOpen} aria-controls="login-offline"
                onClick={() => setOfflineOpen((o) => !o)}
              >Jouer hors ligne</button>
              {offlineOpen && (
                <OfflineForm id="login-offline" submitLabel="Jouer" onSubmit={async (name) => { await services.authOfflineLogin(name); navigate("main"); }} />
              )}
              {params.notice === "expired" && (
                <p className="login-expired" role="status">Session expirée, reconnecte-toi.</p>
              )}
            </Card>
            <p className="login-note">
              Pas de compte Minecraft&nbsp;?{" "}
              <a className="login-tall" href="https://www.minecraft.net/fr-fr" target="_blank" rel="noopener noreferrer">En acheter un</a>
            </p>
          </div>
        ) : (
          <div className="lc">
            <div className="enter">
              <h1>{state.step === "connected" ? "Connexion réussie" : "Entre ce code sur Microsoft"}</h1>
              <p className="lc-sub">
                {state.step === "code" && "Une fois validé sur Microsoft, Cubelink se connecte tout seul."}
                {state.step === "expired" && "Le code a expiré."}
                {state.step === "connected" && "Ton compte Microsoft est relié à Cubelink."}
                {state.step === "error" && "La connexion n'a pas abouti."}
              </p>
            </div>
            <Card as="section" className="lc-card enter" style={{ "--i": 1 } as React.CSSProperties} aria-label="Connexion Microsoft">
              {state.step === "code" && (
                <>
                  <span className="lc-state">En attente</span>
                  <Ring label={`Temps restant : ${spokenTime(left)}`} progress={state.code.expires_in > 0 ? left / state.code.expires_in : 0}>
                    <span className="lc-time" data-testid="countdown">{formatCountdown(left)}</span>
                  </Ring>
                  <p className="lc-code" aria-label={`Code : ${state.code.user_code.split("").join(" ")}`}>
                    {state.code.user_code.split("").map((c, i) => (
                      <span key={i} className={c === "-" ? "sep enter" : "enter"} style={{ display: "inline-block", "--i": 4 + i * 2 } as React.CSSProperties} aria-hidden="true">{c}</span>
                    ))}
                  </p>
                  <div className="lc-actions" aria-live="polite">
                    <Button variant="secondary" icon={copied ? <CopyTick /> : undefined} onClick={() => { void copy(state.code.user_code); }}>{copied ? "Copié" : "Copier"}</Button>
                    <a className="cl-btn cl-btn-primary" href={state.code.verification_uri} target="_blank" rel="noopener noreferrer">
                      Ouvrir {state.code.verification_uri.replace(/^https?:\/\//, "")}
                    </a>
                    <button type="button" className="login-link" style={{ alignSelf: "center" }} onClick={() => { void start(); }}>Problème lors de la connexion ?</button>
                  </div>
                </>
              )}
              {state.step === "expired" && (
                <>
                  <span className="lc-state">Expiré</span>
                  <Ring label="Code expiré" progress={null}><span className="lc-time is-bad">00:00</span></Ring>
                  <p className="lc-code is-dead">{state.code.user_code}</p>
                  <p className="lc-msg" role="alert">Le code a expiré. Ce code n'est plus valable : demande un nouveau code pour te connecter.</p>
                  <div className="lc-actions"><Button onClick={() => { void start(); }}>Réessayer</Button></div>
                </>
              )}
              {state.step === "error" && (
                <>
                  <span className="lc-state">Échec</span>
                  <Ring label="Connexion échouée" progress={null} color="var(--danger)"><span className="lc-time is-bad" aria-hidden="true">!</span></Ring>
                  <p className="lc-msg" role="alert">{state.message}</p>
                  <div className="lc-actions"><Button onClick={() => { void start(); }}>Réessayer</Button></div>
                </>
              )}
              {state.step === "connected" && (
                <>
                  <span className="lc-state">Connecté</span>
                  <Ring label="Connecté" progress={null} color="var(--ok)"><span className="lc-mid"><Tick /></span></Ring>
                  <p className="lc-welcome" role="status">Bienvenue {state.profile.name}</p>
                  <p className="lc-msg">Ton compte Microsoft est relié à Cubelink.</p>
                  <div className="lc-actions"><Button onClick={() => navigate("main")}>Continuer</Button></div>
                </>
              )}
            </Card>
          </div>
        )}
      </div>
      <footer className="login-foot login-note">
        <a className="login-tall" href="#/legal">Mentions légales</a>
        <p>Cubelink est un projet indépendant. Il n'est pas un produit Minecraft officiel, n'est pas approuvé par Mojang ou Microsoft et n'est pas associé à eux.</p>
      </footer>
    </div>
  );
}
