import type { ReactNode } from "react";
import type { RouteName } from "../../app/router";

export const ERROR_STATES = [
  "waiting", "no-license", "offline", "cancelled", "bad-hash", "friend-left", "disk-full", "java-missing", "launch-failed",
] as const;
export type ErrorStateId = (typeof ERROR_STATES)[number];
export const DEFAULT_ERROR_STATE: ErrorStateId = "cancelled";

export function resolveState(raw: string | undefined): ErrorStateId {
  return (ERROR_STATES as readonly string[]).includes(raw ?? "") ? (raw as ErrorStateId) : DEFAULT_ERROR_STATE;
}

export type Action =
  | { kind: "go"; label: string; route: RouteName; primary?: boolean }
  /** Relance join_start pour l'ami du contexte, puis ouvre l'installation ; sans ami : `fallback`. */
  | { kind: "retry"; label: string; fallback: RouteName; primary?: boolean }
  | { kind: "reopen"; label: string }
  /** Relance le jeu pour l'instance du contexte (`instance`) ; sans instance : `fallback`. */
  | { kind: "launch"; label: string; fallback: RouteName; primary?: boolean }
  | { kind: "ext"; label: string; href: string };

export interface StateDef {
  /** true = attente (role="status"), false = échec (role="alert"). */
  wait: boolean;
  kicker: string;
  title: string;
  body: ReactNode;
  more?: string;
  /** Texte brut du « Détail technique » (copiable), fourni par l'appelant. */
  detail?: string;
  actions: Action[];
}

/** Données réelles de l'échec ; chaque champ absent donne un libellé neutre (aucune valeur inventée). */
export interface ErrorContext {
  /** Nom de l'ami. */
  friend?: string;
  /** Fichier fautif. */
  file?: string;
  /** Nombre d'autres mods déjà reçus. */
  others?: number;
  /** Espace à libérer, déjà formaté (ex. « 1,2 Go »). */
  need?: string;
  /** Détail technique d'un échec de lancement (message du jeu, code de sortie...). */
  detail?: string;
}

const retryInstall: Action[] = [
  { kind: "retry", label: "Réessayer", fallback: "main", primary: true },
  { kind: "go", label: "Annuler", route: "main" },
];

export function buildStates(c: ErrorContext = {}): Record<ErrorStateId, StateDef> {
  const who = c.friend ?? "ton ami";
  const whoPoss = c.friend ? `de ${c.friend}` : "de ton ami";
  return {
  waiting: {
    wait: true,
    kicker: "Connexion Microsoft",
    title: "Ouverture de Microsoft dans ton navigateur…",
    body: "Termine la connexion dans la fenêtre qui vient de s'ouvrir, puis reviens ici. La demande expire au bout de 5 minutes.",
    actions: [{ kind: "go", label: "Annuler", route: "login" }, { kind: "reopen", label: "Rouvrir la fenêtre" }],
  },
  "no-license": {
    wait: false,
    kicker: "Connexion Microsoft · échec",
    title: "Ce compte Microsoft ne possède pas Minecraft : Java Edition",
    body: "Aucune licence Minecraft : Java Edition n'a été trouvée sur ce compte. Utilise un autre compte ou obtiens le jeu ; si tu viens de l'acheter, patiente quelques minutes.",
    actions: [
      { kind: "go", label: "Réessayer", route: "login", primary: true },
      { kind: "ext", label: "Obtenir Minecraft", href: "https://www.minecraft.net/" },
    ],
  },
  offline: {
    wait: false,
    kicker: "Connexion Microsoft · échec",
    title: "Cubelink n'arrive pas à joindre Microsoft",
    body: "La connexion a échoué, sans doute parce que ton ordinateur n'est pas connecté à Internet. Vérifie ta connexion, puis réessaie.",
    actions: [
      { kind: "go", label: "Réessayer", route: "login", primary: true },
      { kind: "go", label: "Annuler", route: "login" },
    ],
  },
  cancelled: {
    wait: false,
    kicker: "Connexion Microsoft · annulée",
    title: "La connexion a été annulée",
    body: "Tu as fermé la fenêtre de Microsoft avant la fin de la connexion. Rien n'a été enregistré.",
    actions: [{ kind: "go", label: "Relancer la connexion", route: "login", primary: true }],
  },
  "bad-hash": {
    wait: false,
    kicker: "Installation · échec",
    title: c.friend ? `Un fichier reçu de ${c.friend} ne correspond pas à ce qu'il avait annoncé` : "Un fichier reçu ne correspond pas à ce que ton ami avait annoncé",
    body: (
      <>
        {c.file ? <strong>{c.file}</strong> : "Un fichier"} ne correspond pas à l'empreinte (la signature numérique) annoncée par {who}. Cubelink l'a supprimé et ne l'a pas exécuté.
        {c.others !== undefined && c.others > 0 && ` Les ${c.others} autre${c.others > 1 ? "s" : ""} mod${c.others > 1 ? "s" : ""} reçu${c.others > 1 ? "s" : ""} restent dans ton cache.`}
      </>
    ),
    more: `Cela peut venir d'une erreur de transfert ou d'un changement chez ${who}. Réessaie ; si cela recommence, préviens ${who}.`,
    actions: retryInstall,
  },
  "friend-left": {
    wait: false,
    kicker: "Installation · en pause",
    title: c.friend ? `${c.friend} s'est déconnecté` : "Ton ami s'est déconnecté",
    body: `La réception reprend dès que ${who} revient en ligne. Les fichiers déjà reçus sont conservés.`,
    actions: retryInstall,
  },
  "disk-full": {
    wait: false,
    kicker: "Installation · échec",
    title: "Espace disque insuffisant",
    body: `Il n'y a pas assez de place pour installer l'instance ${whoPoss}. ${c.need ? `Libère au moins ${c.need} sur ton disque` : "Libère de la place sur ton disque"}, puis réessaie.`,
    actions: retryInstall,
  },
  "java-missing": {
    wait: false,
    kicker: "Installation · échec",
    title: "Java 21 est introuvable sur cet ordinateur",
    body: "Le monde d'Alex a besoin de Java 21. Cubelink peut l'installer pour toi en environ 2 minutes.",
    actions: [
      { kind: "go", label: "Installer Java 21", route: "install", primary: true },
      { kind: "go", label: "Annuler", route: "main" },
    ],
  },
  "launch-failed": {
    wait: false,
    kicker: "Lancement · échec",
    title: "Minecraft s'est arrêté avant de démarrer",
    body: "Ce n'est pas ta faute. Réessaie, et si ça recommence, copie le détail pour le montrer à un ami.",
    detail: c.detail,
    actions: [
      { kind: "launch", label: "Réessayer", fallback: "main", primary: true },
      { kind: "go", label: "Retour à l'accueil", route: "main" },
    ],
  },
  };
}

export const STATES: Record<ErrorStateId, StateDef> = buildStates();
