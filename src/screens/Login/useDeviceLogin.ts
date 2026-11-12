import { useCallback, useEffect, useRef, useState } from "react";
import type { DeviceCode, Profile, Services } from "../../services/types";

export type DeviceLoginState =
  | { step: "idle" }
  | { step: "starting" }
  | { step: "code"; code: DeviceCode }
  | { step: "expired"; code: DeviceCode }
  | { step: "connected"; profile: Profile }
  | { step: "error"; message: string };

export function messageOf(e: unknown): string {
  if (e instanceof Error && e.message) return e.message;
  if (typeof e === "string" && e) return e;
  return "La connexion a échoué. Réessaie dans un instant.";
}

/**
 * Flux Microsoft par code d'appareil, partagé par l'écran Connexion et Réglages > Comptes.
 * `onFailure` reçoit le message d'échec ; il renvoie true s'il l'a pris en charge (ex. écran Erreur),
 * sinon l'état « error » est exposé.
 */
export function useDeviceLogin(services: Services, onFailure?: (message: string) => boolean) {
  const [state, setState] = useState<DeviceLoginState>({ step: "idle" });
  const [left, setLeft] = useState(0);
  const [copied, setCopied] = useState(false);
  const run = useRef(0);
  const copyTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const failure = useRef(onFailure);
  failure.current = onFailure;

  useEffect(() => () => {
    run.current += 1;
    if (copyTimer.current) clearTimeout(copyTimer.current);
  }, []);

  const start = useCallback(async () => {
    const id = ++run.current;
    setState({ step: "starting" });
    try {
      const code = await services.authBeginLogin();
      if (id !== run.current) return;
      setLeft(code.expires_in);
      setCopied(false);
      setState({ step: "code", code });
      const profile = await services.authPollLogin();
      if (id !== run.current) return;
      setState({ step: "connected", profile });
    } catch (e) {
      if (id !== run.current) return;
      const message = messageOf(e);
      if (failure.current?.(message)) return;
      setState({ step: "error", message });
    }
  }, [services]);

  /** Abandonne le flux en cours (la réponse tardive est ignorée). */
  const cancel = useCallback(() => {
    run.current += 1;
    setState({ step: "idle" });
  }, []);

  const waiting = state.step === "code";
  useEffect(() => {
    if (!waiting) return;
    const t = setInterval(() => setLeft((v) => Math.max(0, v - 1)), 1000);
    return () => clearInterval(t);
  }, [waiting]);

  useEffect(() => {
    if (state.step === "code" && left === 0) {
      run.current += 1;
      setState({ step: "expired", code: state.code });
    }
  }, [state, left]);

  const copy = useCallback(async (text: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      if (copyTimer.current) clearTimeout(copyTimer.current);
      copyTimer.current = setTimeout(() => setCopied(false), 2400);
    } catch { /* presse-papiers indisponible : le code reste lisible à l'écran */ }
  }, []);

  return { state, left, copied, start, cancel, copy };
}
