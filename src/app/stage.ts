import { useCallback, useEffect, useRef, useState } from "react";
import type { Services, StageFallback, StageLayout, StageState } from "../services";

export type { StageFallback, StageLayout, StageState };

// Zone de jeu intégrée (Windows, Linux X11). Tout passe par `Services` (commandes `stage_*`, événements `stage://*`) :
// le faux service n'a pas de zone de jeu, donc rien n'est rattaché hors de l'application.

const DETACHED: StageState = { attached: false, game: false, panel: false };

/** Suit l'état de la zone de jeu (commande `stage_state` puis événement `stage://state`). */
export function useStage(services: Services): StageState {
  const [st, setSt] = useState<StageState>(DETACHED);
  useEffect(() => {
    let dead = false;
    // Un événement arrivé avant la réponse de `stage_state` est plus récent qu'elle : la réponse est alors ignorée.
    let evented = false;
    const off = services.onStageState((s) => { evented = true; setSt(s); });
    services.stageState().then((s) => { if (!dead && !evented) setSt(s); }, () => { /* best effort */ });
    return () => { dead = true; off(); };
  }, [services]);
  return st;
}

/** Phrase montrée au joueur après un repli : ce qu'il voit, sans jargon ni raison technique. */
export function stageFallbackMessage(f: StageFallback): string {
  return f.session_off
    ? "Le jeu s'est ouvert dans sa propre fenêtre, et ce sera le cas jusqu'à la fermeture de Cubelink. Pour retrouver le jeu dans Cubelink, ferme puis rouvre Cubelink."
    : "Le jeu s'est ouvert dans sa propre fenêtre.";
}

/** Dernier repli annoncé par Rust (`stage://fallback`), jusqu'à ce qu'il soit écarté. */
export function useStageFallback(services: Services): { notice: StageFallback | null; dismiss: () => void } {
  const [notice, setNotice] = useState<StageFallback | null>(null);
  useEffect(() => services.onStageFallback((p) => {
    // Charge utile lue avec prudence : une forme inattendue donne le message simple.
    const f = p as Partial<StageFallback> | null | undefined;
    setNotice({ reason: String(f?.reason ?? ""), session_off: f?.session_off === true });
  }), [services]);
  const dismiss = useCallback(() => setNotice(null), []);
  return { notice, dismiss };
}

/** La zone de jeu intégrée existe sur ce système (Windows, Linux en session X11). Tant que Rust n'a pas répondu : non. */
export function useStageSupported(services: Services): boolean {
  const [ok, setOk] = useState(false);
  useEffect(() => {
    let dead = false;
    services.stageSupported().then((v) => { if (!dead) setOk(v === true); }, () => { /* réglage masqué */ });
    return () => { dead = true; };
  }, [services]);
  return ok;
}

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
 * d'affichage changent ; rien n'est envoyé si la mise en page est identique à la dernière. Sans zone de jeu
 * (`stage_supported` faux, faux service) : rien n'est mesuré.
 */
export function useStageLayout(services: Services, ref: React.RefObject<HTMLElement | null>): void {
  const supported = useStageSupported(services);
  useEffect(() => {
    if (!supported) return;
    const el = ref.current;
    if (!el) return;
    let dead = false;
    let last = "";
    const publish = () => {
      if (dead) return;
      const layout = stageLayout(el.getBoundingClientRect(), panelCssPx(), window.devicePixelRatio || 1);
      const key = `${layout.x},${layout.y},${layout.w},${layout.h},${layout.panel_w}`;
      if (key === last) return;
      last = key;
      // Échec : on oublie l'envoi pour que le prochain changement le retente.
      services.stageLayout(layout).catch(() => { if (last === key) last = ""; });
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
  }, [services, ref, supported]);
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
export function useStageFullscreen(services: Services, st: StageState): boolean {
  const [requested, setRequested] = useState(false);
  useEffect(() => services.onStageFullscreen((on) => setRequested(on === true)), [services]);
  useEffect(() => { if (!st.attached) setRequested(false); }, [st.attached]);

  const on = stageFullscreen(requested, st);
  // Ne rend la fenêtre (false) que si c'est nous qui l'avions mise en plein écran.
  const applied = useRef(false);
  useEffect(() => {
    if (applied.current === on) return;
    applied.current = on;
    services.setFullscreen(on).catch(() => { /* best effort */ });
  }, [services, on]);
  return on;
}
