import { useCallback, useEffect, useRef, useState } from "react";
import { isTauri } from "../services";

/** Zone de jeu intégrée (Windows, Linux X11) : un jeu est rattaché, c'est lui qu'on voit, panneau « en jeu » ouvert. */
export type StageState = { attached: boolean; game: boolean; panel: boolean };

/** Suit l'état de la zone de jeu (événement `stage://state`). Hors Tauri : jamais rattaché. */
export function useStage(): StageState {
  const [st, setSt] = useState<StageState>({ attached: false, game: false, panel: false });
  useEffect(() => {
    if (!isTauri()) return;
    let off: (() => void) | undefined;
    let dead = false;
    void (async () => {
      const [{ invoke }, { listen }] = await Promise.all([import("@tauri-apps/api/core"), import("@tauri-apps/api/event")]);
      try { const s = await invoke<StageState>("stage_state"); if (!dead) setSt(s); } catch { /* best effort */ }
      const un = await listen<StageState>("stage://state", (e) => setSt(e.payload));
      if (dead) un(); else off = un;
    })().catch(() => { /* best effort */ });
    return () => { dead = true; off?.(); };
  }, []);
  return st;
}

/** Événement `stage://fallback` : le jeu n'a pas pu être affiché dans la fenêtre de Cubelink, il a gardé la sienne. */
export type StageFallback = {
  /** Identifiant technique de la raison (journal `stage.log`), jamais montré au joueur. */
  reason: string;
  /** Trop d'échecs de suite : le jeu s'ouvrira dans sa propre fenêtre jusqu'au redémarrage de Cubelink (le message dit comment revenir). */
  session_off: boolean;
};

/** Phrase montrée au joueur après un repli : ce qu'il voit, sans jargon ni raison technique. */
export function stageFallbackMessage(f: StageFallback): string {
  return f.session_off
    ? "Le jeu s'est ouvert dans sa propre fenêtre, et ce sera le cas jusqu'à la fermeture de Cubelink. Pour retrouver le jeu dans Cubelink, ferme puis rouvre Cubelink."
    : "Le jeu s'est ouvert dans sa propre fenêtre.";
}

/** Dernier repli annoncé par Rust (`stage://fallback`), jusqu'à ce qu'il soit écarté. Hors Tauri : jamais. */
export function useStageFallback(): { notice: StageFallback | null; dismiss: () => void } {
  const [notice, setNotice] = useState<StageFallback | null>(null);
  useEffect(() => {
    if (!isTauri()) return;
    let off: (() => void) | undefined;
    let dead = false;
    void (async () => {
      const { listen } = await import("@tauri-apps/api/event");
      const un = await listen<StageFallback>("stage://fallback", (e) => {
        setNotice({ reason: String(e.payload?.reason ?? ""), session_off: e.payload?.session_off === true });
      });
      if (dead) un(); else off = un;
    })().catch(() => { /* best effort */ });
    return () => { dead = true; off?.(); };
  }, []);
  const dismiss = useCallback(() => setNotice(null), []);
  return { notice, dismiss };
}

/** La zone de jeu intégrée existe sur ce système (Windows, Linux en session X11). Hors Tauri, ou tant que Rust n'a pas répondu : non. */
export function useStageSupported(): boolean {
  const [ok, setOk] = useState(false);
  useEffect(() => {
    if (!isTauri()) return;
    let dead = false;
    void (async () => {
      const { invoke } = await import("@tauri-apps/api/core");
      const v = await invoke<boolean>("stage_supported");
      if (!dead) setOk(v === true);
    })().catch(() => { /* réglage masqué */ });
    return () => { dead = true; };
  }, []);
  return ok;
}

/** Rectangle de la zone de jeu et largeur du panneau, en pixels physiques (origine : coin haut-gauche de la zone cliente). */
export type StageLayout = { x: number; y: number; w: number; h: number; panel_w: number };

const PANEL_W_FALLBACK = 380;

/** Convertit un rectangle CSS (repère du viewport) en pixels physiques entiers. Taille minimale 1 × 1, panneau jamais négatif. */
export function stageLayout(
  rect: { left: number; top: number; width: number; height: number },
  panelCssPx: number,
  dpr: number,
): StageLayout {
  const scale = Number.isFinite(dpr) && dpr > 0 ? dpr : 1;
  // « + 0 » : évite le zéro négatif de Math.round(-0.2).
  const px = (v: number) => (Number.isFinite(v) ? Math.round(v * scale) + 0 : 0);
  return {
    x: px(rect.left),
    y: px(rect.top),
    w: Math.max(1, px(rect.width)),
    h: Math.max(1, px(rect.height)),
    panel_w: Math.max(0, px(panelCssPx)),
  };
}

/** Largeur du panneau « en jeu » en px CSS, lue dans `--panel-w` (380 si illisible). */
function panelCssPx(): number {
  try {
    const m = /^\s*(\d+(?:\.\d+)?)px\s*$/.exec(getComputedStyle(document.documentElement).getPropertyValue("--panel-w"));
    if (m) return Number(m[1]);
  } catch { /* valeur de repli */ }
  return PANEL_W_FALLBACK;
}

/**
 * Publie à Rust (`stage_layout`) le rectangle de l'élément réservé à la zone de jeu : le DOM est la seule
 * source de vérité pour la géométrie. Envoi au montage, puis quand l'élément, la fenêtre ou l'échelle
 * d'affichage changent ; rien n'est envoyé si la mise en page est identique à la dernière. Hors Tauri : rien.
 */
export function useStageLayout(ref: React.RefObject<HTMLElement | null>): void {
  useEffect(() => {
    if (!isTauri()) return;
    const el = ref.current;
    if (!el) return;
    let dead = false;
    let last = "";
    // Un seul chargement : les envois s'enchaînent sur cette promesse, donc dans l'ordre.
    const core = import("@tauri-apps/api/core");
    const publish = () => {
      if (dead) return;
      const layout = stageLayout(el.getBoundingClientRect(), panelCssPx(), window.devicePixelRatio || 1);
      const key = `${layout.x},${layout.y},${layout.w},${layout.h},${layout.panel_w}`;
      if (key === last) return;
      last = key;
      // Échec : on oublie l'envoi pour que le prochain changement le retente.
      void core.then(({ invoke }) => invoke("stage_layout", layout)).catch(() => { if (last === key) last = ""; });
    };

    const ro = typeof ResizeObserver === "function" ? new ResizeObserver(publish) : undefined;
    ro?.observe(el);
    window.addEventListener("resize", publish);

    // Changement d'échelle (autre écran, réglage Windows) : la requête ne vaut que pour l'échelle courante, on la réarme à chaque fois.
    let mq: MediaQueryList | undefined;
    const onDpr = () => { arm(); publish(); };
    const arm = () => {
      mq?.removeEventListener("change", onDpr);
      mq = undefined;
      if (typeof window.matchMedia !== "function") return;
      mq = window.matchMedia(`(resolution: ${window.devicePixelRatio || 1}dppx)`);
      mq.addEventListener("change", onDpr);
    };
    arm();
    publish();

    return () => {
      dead = true;
      ro?.disconnect();
      window.removeEventListener("resize", publish);
      mq?.removeEventListener("change", onDpr);
    };
  }, [ref]);
}

/** Le launcher est en plein écran pour le jeu : le jeu l'a demandé (F11), il est rattaché et c'est lui qu'on voit. */
export function stageFullscreen(requested: boolean, st: StageState): boolean {
  return requested && st.attached && st.game;
}

/**
 * Plein écran du jeu (événement `stage://fullscreen`, F11 dans le jeu) : le jeu est calé sur la zone de jeu, c'est donc la
 * fenêtre du launcher qui passe en plein écran, et la zone (sans barre de titre) couvre tout l'écran. Retour au launcher :
 * plein écran suspendu ; fin du jeu : oublié. Rend vrai tant que le launcher est en plein écran pour le jeu.
 */
export function useStageFullscreen(st: StageState): boolean {
  const [requested, setRequested] = useState(false);
  useEffect(() => {
    if (!isTauri()) return;
    let off: (() => void) | undefined;
    let dead = false;
    void (async () => {
      const { listen } = await import("@tauri-apps/api/event");
      const un = await listen<boolean>("stage://fullscreen", (e) => setRequested(e.payload === true));
      if (dead) un(); else off = un;
    })().catch(() => { /* best effort */ });
    return () => { dead = true; off?.(); };
  }, []);
  useEffect(() => { if (!st.attached) setRequested(false); }, [st.attached]);

  const on = stageFullscreen(requested, st);
  // Ne rend la fenêtre (false) que si c'est nous qui l'avions mise en plein écran.
  const applied = useRef(false);
  useEffect(() => {
    if (!isTauri() || applied.current === on) return;
    applied.current = on;
    void import("@tauri-apps/api/window").then(({ getCurrentWindow }) => getCurrentWindow().setFullscreen(on)).catch(() => { /* best effort */ });
  }, [on]);
  return on;
}

export async function switchStageView(game: boolean): Promise<void> {
  const { invoke } = await import("@tauri-apps/api/core");
  await invoke("stage_view", { game });
}

/** Ouvre ou ferme le panneau « en jeu » (Échap, bouton Fermer). */
export async function setStagePanel(open: boolean): Promise<void> {
  const { invoke } = await import("@tauri-apps/api/core");
  await invoke("stage_panel", { open });
}
