// Dans la fenêtre Tauri, un lien target="_blank" n'ouvre rien : on l'envoie au navigateur par défaut de l'OS.
export function installExternalLinks(): void {
  if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) return;
  document.addEventListener("click", (e) => {
    if (e.defaultPrevented || e.button !== 0) return;
    const a = (e.target as Element | null)?.closest?.("a[href]") as HTMLAnchorElement | null;
    if (!a || !/^https?:\/\//i.test(a.href)) return;
    e.preventDefault();
    void import("@tauri-apps/plugin-opener").then((m) => m.openUrl(a.href)).catch(() => undefined);
  });
}
