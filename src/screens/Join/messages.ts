/** Message d'erreur du backend (français) affiché tel quel, avec « Ton ami » remplacé par le prénom. */
export function friendlyJoinError(raw: unknown, name: string): string {
  const msg = (raw instanceof Error ? raw.message : typeof raw === "string" ? raw : "").trim();
  if (!msg) return "Impossible de rejoindre pour l'instant. Réessaie.";
  return name ? msg.replace(/^Ton ami\b/i, name) : msg;
}

export type JoinErrorKind = "offline" | "world-closed" | "other";

export function classifyJoinError(msg: string): JoinErrorKind {
  const m = msg.toLowerCase();
  if (/hors ligne|injoignable|pas en ligne/.test(m)) return "offline";
  if (/monde|partie|r[eé]seau local|pas encore ouvert/.test(m)) return "world-closed";
  return "other";
}

export function joinErrorAdvice(kind: JoinErrorKind, name: string): string | null {
  if (kind === "offline") return `Demande à ${name} d'ouvrir Cubelink.`;
  if (kind === "world-closed") return `Demande à ${name} de lancer le jeu et de l'ouvrir au réseau local.`;
  return null;
}
