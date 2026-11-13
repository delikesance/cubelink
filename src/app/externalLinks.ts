/** Ouvre une adresse http(s) dans le navigateur par défaut (fenêtre Tauri), sinon dans un nouvel onglet. */
export function openExternal(url: string): void {
  if (!/^https?:\/\//i.test(url)) return;
  if (typeof window !== "undefined" && "__TAURI_INTERNALS__" in window) {
    void import("@tauri-apps/plugin-opener").then((m) => m.openUrl(url)).catch(() => undefined);
  } else {
    window.open(url, "_blank", "noopener,noreferrer");
  }
}

/** Lien qui sort de l'appli : http(s) vers une autre origine. En production la page est servie depuis
 *  `http://tauri.localhost`, donc un lien interne `#/legal` devient `http://tauri.localhost/#/legal` : il reste dans l'appli. */
export function isExternalHref(href: string, origin: string): boolean {
  if (!/^https?:\/\//i.test(href)) return false;
  try { return new URL(href).origin !== origin; } catch { return false; }
}

// Dans la fenêtre Tauri, un lien target="_blank" n'ouvre rien : on l'envoie au navigateur par défaut de l'OS.
export function installExternalLinks(): void {
  if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) return;
  document.addEventListener("click", (e) => {
    if (e.defaultPrevented || e.button !== 0) return;
    const a = (e.target as Element | null)?.closest?.("a[href]") as HTMLAnchorElement | null;
    if (!a || !isExternalHref(a.href, window.location.origin)) return;
    e.preventDefault();
    void import("@tauri-apps/plugin-opener").then((m) => m.openUrl(a.href)).catch(() => undefined);
  });
}
