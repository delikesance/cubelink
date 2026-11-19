import { HEARTH_EULA_REQUIRED, HEARTH_HEAD_UNAVAILABLE, type HearthCard } from "../../services/types";

export const errText = (e: unknown): string => (e instanceof Error ? e.message : typeof e === "string" ? e : "Une erreur est survenue.");

/** Vrai quand l'erreur de `hearthPlay` demande d'accepter le contrat d'utilisation de Minecraft. */
export const isEulaRequired = (e: unknown): boolean => errText(e).includes(HEARTH_EULA_REQUIRED);

/** La dernière version du monde est connue mais personne de joignable n'en a le contenu. */
export interface HeadUnavailable {
  author: string;
  head: number;
  /** Numéro de la dernière version dont le monde est complet ici ; `null` : aucune. */
  previous: number | null;
}

/** Lit l'erreur `head_unavailable|<n° tête>|<n° précédent ou vide>|<auteur>` de `hearthPlay` ; `null` pour toute autre erreur. */
export function parseHeadUnavailable(e: unknown): HeadUnavailable | null {
  const text = errText(e);
  const at = text.indexOf(`${HEARTH_HEAD_UNAVAILABLE}|`);
  if (at < 0) return null;
  const [head, previous, ...name] = text.slice(at + HEARTH_HEAD_UNAVAILABLE.length + 1).split("|");
  const h = Number(head);
  if (!Number.isInteger(h)) return null;
  const p = previous ? Number(previous) : null;
  return { author: name.join("|").trim() || "un ami", head: h, previous: p !== null && Number.isInteger(p) ? p : null };
}

export function fmtSize(bytes: number): string {
  if (bytes >= 1_000_000_000) return `${(bytes / 1_000_000_000).toLocaleString("fr-FR", { maximumFractionDigits: 1 })} Go`;
  return `${Math.max(1, Math.round(bytes / 1_000_000)).toLocaleString("fr-FR")} Mo`;
}

export function fmtPlayed(secs: number): string {
  const m = Math.round(secs / 60);
  if (m < 60) return `${Math.max(1, m)} min`;
  const h = Math.floor(m / 60);
  const r = m % 60;
  return r ? `${h} h ${String(r).padStart(2, "0")}` : `${h} h`;
}

export function fmtDate(sec: number): string {
  return new Date(sec * 1000).toLocaleString("fr-FR", { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" });
}

export const plural = (n: number, one: string, many: string): string => `${n} ${n > 1 ? many : one}`;

/** Phrase d'état affichée sur la carte (une seule, jamais portée par la couleur seule). */
export function stateText(c: HearthCard): string {
  switch (c.state) {
    case "idle": return "Personne ne joue";
    case "hosted": return `${c.host?.name ?? "Un ami"} héberge · ${plural(c.players, "joueur", "joueurs")}`;
    case "syncing": return `Mise à jour du monde ${c.progress ?? 0} %`;
    case "blocked": return "Aucun membre en ligne";
    case "unclean": return "Partie mal terminée";
    case "fork": return "Deux versions existent";
    case "launching": return c.step ?? "Lancement";
    case "hosting": return `Tu héberges · ${plural(c.players, "joueur", "joueurs")}`;
    case "pack_missing": return "Le modpack du groupe n'est pas installé";
  }
}

/** Détail sous l'état : la raison, en une phrase. */
export function stateHint(c: HearthCard, offline: string[] = []): string | null {
  switch (c.state) {
    case "blocked": {
      const who = offline.length > 0 ? ` (${offline.join(", ")} ${offline.length > 1 ? "sont hors ligne" : "est hors ligne"})` : "";
      return `Aucun autre membre n'est en ligne${who}. Tu peux lancer le serveur : ils le rejoindront à leur arrivée. Si l'un d'eux joue seul de son côté en même temps, le groupe choisira la version du monde à garder.`;
    }
    case "unclean": return c.unclean ? `La partie de ${c.unclean.holder.name} ne s'est pas terminée proprement.` : null;
    case "fork": return "Deux membres ont joué en même temps : le groupe choisit la version à garder.";
    case "pack_missing": return "Installe le modpack du groupe pour pouvoir jouer.";
    default: return null;
  }
}

export type MainAction =
  | { kind: "play"; label: string }
  | { kind: "stop"; label: string }
  | { kind: "none" };

/** L'unique bouton principal d'une carte. Les cas « mal terminée », « fourche » et « modpack absent » ont leurs propres actions. */
export function mainAction(c: HearthCard): MainAction {
  switch (c.state) {
    case "idle": return { kind: "play", label: "Jouer" };
    case "hosted": return { kind: "play", label: "Rejoindre" };
    case "hosting": return { kind: "stop", label: "Arrêter le serveur" };
    // Personne d'autre en ligne : on peut quand même lancer le serveur, les amis le rejoindront à leur arrivée.
    case "blocked": return { kind: "play", label: "Jouer" };
    default: return { kind: "none" };
  }
}

/** Libellé de l'action de reprise après une partie mal terminée. */
export function resumeLabel(c: HearthCard): { choice: "checkpoint" | "last"; label: string } {
  return c.unclean?.checkpoint_at != null
    ? { choice: "checkpoint", label: "Jouer depuis le point de reprise" }
    : { choice: "last", label: "Jouer depuis la dernière version" };
}
