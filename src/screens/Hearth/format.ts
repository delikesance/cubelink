import { HEARTH_ALONE_REQUIRED, HEARTH_EULA_REQUIRED, type HearthCard } from "../../services/types";

export const errText = (e: unknown): string => (e instanceof Error ? e.message : typeof e === "string" ? e : "Une erreur est survenue.");

/** Vrai quand l'erreur de `hearthPlay` demande d'accepter le contrat d'utilisation de Minecraft. */
export const isAloneRequired = (e: unknown): boolean => errText(e).includes(HEARTH_ALONE_REQUIRED);
export const isEulaRequired = (e: unknown): boolean => errText(e).includes(HEARTH_EULA_REQUIRED);

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
      return `Aucun autre membre du groupe n'est joignable${who}. Pour lancer ce serveur, au moins un autre membre doit avoir Cubelink ouvert : sinon deux versions du monde pourraient apparaître. Demande à un ami d'ouvrir Cubelink, ou joue seul (le monde sera envoyé aux autres à leur retour).`;
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
    // Aucun autre membre joignable : on peut jouer seul, après confirmation (l'écran la demande).
    case "blocked": return { kind: "play", label: "Jouer seul" };
    default: return { kind: "none" };
  }
}

/** Libellé de l'action de reprise après une partie mal terminée. */
export function resumeLabel(c: HearthCard): { choice: "checkpoint" | "last"; label: string } {
  return c.unclean?.checkpoint_at != null
    ? { choice: "checkpoint", label: "Jouer depuis le point de reprise" }
    : { choice: "last", label: "Jouer depuis la dernière version" };
}
