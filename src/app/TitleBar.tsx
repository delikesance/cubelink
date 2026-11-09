import { isTauri } from "../services";

async function windowAction(action: "minimize" | "close"): Promise<void> {
  if (!isTauri()) return;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const w = getCurrentWindow();
  if (action === "minimize") await w.minimize();
  else await w.close();
}

export function TitleBar({ context }: { context: string }) {
  return (
    <header className="titlebar" data-tauri-drag-region>
      <span className="titlebar-spacer" />
      <h1 className="titlebar-title" data-tauri-drag-region>Cubelink · {context}</h1>
      <div className="titlebar-buttons">
        <button type="button" className="tb-btn" aria-label="Réduire" onClick={() => void windowAction("minimize")}>
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><path d="M0 5h10" stroke="currentColor" /></svg>
        </button>
        <button type="button" className="tb-btn tb-close" aria-label="Fermer" onClick={() => void windowAction("close")}>
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><path d="M0 0l10 10M10 0L0 10" stroke="currentColor" /></svg>
        </button>
      </div>
    </header>
  );
}
