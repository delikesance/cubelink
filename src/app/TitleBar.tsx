import { useEffect, useState } from "react";
import { isTauri } from "../services";
import { Logo } from "../screens/Login/Logo";
import { goBack, useCanGoBack, useLocation } from "./router";
import { useServices } from "./context";
import type { StageState } from "./stage";

type WindowAction = "minimize" | "close" | "toggleMaximize";

async function windowAction(action: WindowAction): Promise<void> {
  if (!isTauri()) return;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const w = getCurrentWindow();
  if (action === "minimize") await w.minimize();
  else if (action === "toggleMaximize") await w.toggleMaximize();
  else await w.close();
}

/** Suit l'état agrandi/restauré de la fenêtre (Tauri uniquement). */
function useMaximized(): boolean {
  const [max, setMax] = useState(false);
  useEffect(() => {
    if (!isTauri()) return;
    let off: (() => void) | undefined;
    let dead = false;
    void (async () => {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      const w = getCurrentWindow();
      const refresh = async () => { const m = await w.isMaximized(); if (!dead) setMax(m); };
      await refresh();
      const un = await w.onResized(() => { void refresh(); });
      if (dead) un(); else off = un;
    })().catch(() => { /* best effort */ });
    return () => { dead = true; off?.(); };
  }, []);
  return max;
}

export function TitleBar({ context, stage }: { context: string; stage: StageState }) {
  const services = useServices();
  const canBack = useCanGoBack();
  const { route } = useLocation();
  const maximized = useMaximized();
  const showBack = route !== "login" && canBack;
  return (
    <header className="titlebar" data-tauri-drag-region>
      <span className="titlebar-spacer">
        <button
          type="button" className="tb-btn tb-back" aria-label="Retour"
          disabled={!showBack} onClick={() => goBack()}
        >
          <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true">
            <path d="M11 6H1M5 2L1 6l4 4" fill="none" stroke="currentColor" />
          </svg>
        </button>
        <span className="tb-brand"><Logo width={18} small />Cubelink</span>
        {stage.attached && (
          <button type="button" className="tb-btn tb-stage" onClick={() => void services.stageView(!stage.game).catch(() => {})}>
            {stage.game ? "Launcher" : "Retour au jeu"}
          </button>
        )}
      </span>
      <span className="titlebar-drag" data-tauri-drag-region />
      <span className="titlebar-title">{context}</span>
      <div className="titlebar-buttons">
        <button type="button" className="tb-btn" aria-label="Réduire" onClick={() => void windowAction("minimize")}>
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><path d="M0 5h10" stroke="currentColor" /></svg>
        </button>
        <button
          type="button" className="tb-btn" aria-label={maximized ? "Restaurer" : "Agrandir"}
          onClick={() => void windowAction("toggleMaximize")}
        >
          {maximized ? (
            <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
              <path d="M2.5 2.5v-2h7v7h-2M0.5 2.5h7v7h-7z" fill="none" stroke="currentColor" />
            </svg>
          ) : (
            <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
              <rect x="0.5" y="0.5" width="9" height="9" fill="none" stroke="currentColor" />
            </svg>
          )}
        </button>
        <button type="button" className="tb-btn tb-close" aria-label="Fermer" onClick={() => void windowAction("close")}>
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><path d="M0 0l10 10M10 0L0 10" stroke="currentColor" /></svg>
        </button>
      </div>
    </header>
  );
}
