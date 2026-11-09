import { useCallback, useEffect, useRef, useState } from "react";
import "./login.css";
import { useScreenTitle, useServices } from "../../app/context";
import { navigate } from "../../app/router";
import type { DeviceCode } from "../../services/types";

type State =
  | { step: "idle" }
  | { step: "starting" }
  | { step: "code"; code: DeviceCode }
  | { step: "error"; message: string };

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

function messageOf(e: unknown): string {
  if (e instanceof Error && e.message) return e.message;
  if (typeof e === "string" && e) return e;
  return "La connexion a échoué. Réessaie dans un instant.";
}

export default function Login() {
  useScreenTitle(null);
  const services = useServices();
  const [state, setState] = useState<State>({ step: "idle" });
  const [left, setLeft] = useState(0);
  const run = useRef(0);

  useEffect(() => () => { run.current += 1; }, []);

  const start = useCallback(async () => {
    const id = ++run.current;
    setState({ step: "starting" });
    try {
      const code = await services.authBeginLogin();
      if (id !== run.current) return;
      setLeft(code.expires_in);
      setState({ step: "code", code });
      await services.authPollLogin();
      if (id !== run.current) return;
      navigate("main");
    } catch (e) {
      if (id !== run.current) return;
      const message = messageOf(e);
      const kind = classifyLoginFailure(message);
      if (kind) { navigate("error", { state: kind }); return; }
      setState({ step: "error", message });
    }
  }, [services]);

  const waiting = state.step === "code";
  useEffect(() => {
    if (!waiting) return;
    const t = setInterval(() => setLeft((v) => Math.max(0, v - 1)), 1000);
    return () => clearInterval(t);
  }, [waiting]);

  useEffect(() => {
    if (waiting && left === 0) {
      run.current += 1;
      setState({ step: "error", message: "Le code a expiré. Demande un nouveau code pour te connecter." });
    }
  }, [waiting, left]);

  return (
    <div className="login">
      <div className="login-bg" aria-hidden="true" />
      <div className="login-main">
        <section className="login-card" aria-labelledby="login-title">
          <div className="login-brand">
            <svg width="56" height="56" viewBox="0 0 24 24" fill="none" stroke="#FF5C00" strokeWidth="1.6" strokeLinejoin="round" aria-hidden="true">
              <path d="M12 3l8 4.5v9L12 21l-8-4.5v-9L12 3z" /><path d="M12 12l8-4.5M12 12v9M12 12L4 7.5" />
            </svg>
            <h2 id="login-title">CUBELINK</h2>
          </div>
          <p className="login-lead">Connecte-toi avec ton compte Microsoft pour jouer.</p>

          <div className="login-live" aria-live="polite" aria-atomic="true">
            {state.step === "starting" && <p className="login-info">Demande du code en cours…</p>}
            {state.step === "code" && (
              <div className="login-device">
                <p>
                  Ouvre{" "}
                  <a className="ulink" href={state.code.verification_uri} target="_blank" rel="noopener noreferrer">
                    {state.code.verification_uri}
                  </a>{" "}
                  et saisis ce code :
                </p>
                <p className="login-code" aria-label={`Code : ${state.code.user_code.split("").join(" ")}`}>
                  {state.code.user_code}
                </p>
                <p className="login-info">
                  En attente de ta validation… Le code expire dans{" "}
                  <span data-testid="countdown">{formatCountdown(left)}</span>.
                </p>
              </div>
            )}
            {state.step === "error" && <p className="login-error" role="alert">{state.message}</p>}
          </div>

          {(state.step === "idle" || state.step === "starting") && (
            <button type="button" className="login-ms" onClick={() => { void start(); }} disabled={state.step === "starting"}>
              <svg width="21" height="21" viewBox="0 0 21 21" aria-hidden="true">
                <rect x="0" y="0" width="10" height="10" fill="#F25022" /><rect x="11" y="0" width="10" height="10" fill="#7FBA00" />
                <rect x="0" y="11" width="10" height="10" fill="#00A4EF" /><rect x="11" y="11" width="10" height="10" fill="#FFB900" />
              </svg>
              Se connecter avec Microsoft
            </button>
          )}
          {state.step === "error" && (
            <button type="button" className="login-ms" onClick={() => { void start(); }}>Réessayer</button>
          )}

          {(state.step === "code" || state.step === "error") && (
            <button type="button" className="ulink login-tall" onClick={() => { void start(); }}>Problème lors de la connexion ?</button>
          )}
          <span className="login-buy">
            Pas de compte Minecraft&nbsp;?{" "}
            <a className="ulink login-tall" href="https://www.minecraft.net/fr-fr" target="_blank" rel="noopener noreferrer">En acheter un</a>
          </span>
          <div className="login-privacy">
            <p>Cubelink ne stocke ni ton compte ni ta liste d'amis sur un serveur.</p>
            <details>
              <summary className="login-sum">En savoir plus</summary>
              <p>Un relais peut transmettre ton trafic quand la connexion directe échoue. Cubelink crée une clé privée sur cet ordinateur pour te faire reconnaître de tes amis.</p>
            </details>
          </div>
        </section>
      </div>
      <footer className="login-foot">
        <a className="ulink login-tall" href="#/legal">Mentions légales</a>
        <p>Cubelink est un projet indépendant. Il n'est pas un produit Minecraft officiel, n'est pas approuvé par Mojang ou Microsoft et n'est pas associé à eux.</p>
      </footer>
    </div>
  );
}
