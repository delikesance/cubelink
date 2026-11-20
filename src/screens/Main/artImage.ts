import { useEffect, useRef, useState } from "react";
import type { Services } from "../../services";

/** Origine d'une image : les illustrations du modpack (`art/`) ou les captures du joueur (`screenshots/`). */
export type ImageSource = "art" | "shot";
/** Services d'images : `instancesScreenshotImage` n'est requis que pour la source « shot ». */
export type ArtImageServices = Pick<Services, "instancesArtImage"> & Partial<Pick<Services, "instancesScreenshotImage">>;

const cache = new Map<string, string>();
const pending = new Map<string, Promise<string>>();
// Deux sources peuvent avoir un fichier de même nom (`icon.png`) : la source fait partie de la clé.
const keyOf = (id: string, file: string, maxPx?: number, source: ImageSource = "art") => `${source}|${id}|${file}|${maxPx ?? ""}`;

/** Vide le cache mémoire (tests). */
export function clearArtCache() {
  cache.clear();
  pending.clear();
}

function loadArtImage(services: ArtImageServices, id: string, file: string, maxPx?: number, source: ImageSource = "art"): Promise<string> {
  const key = keyOf(id, file, maxPx, source);
  const hit = cache.get(key);
  if (hit) return Promise.resolve(hit);
  let p = pending.get(key);
  if (!p) {
    const req = source === "shot" ? services.instancesScreenshotImage?.(id, file, maxPx) : services.instancesArtImage(id, file, maxPx);
    p = (req ?? Promise.reject(new Error("source d'images indisponible"))).then((u) => { cache.set(key, u); return u; }).finally(() => pending.delete(key));
    pending.set(key, p);
  }
  return p;
}

export type ArtImageState = { status: "idle" | "loading" | "ready" | "error"; src: string | null };

/** Charge une image du modpack, ou une capture du joueur avec `source = "shot"` (cache mémoire par source, instance et fichier). `lazy` : attend que l'élément soit visible. */
export function useArtImage(services: ArtImageServices, id: string, file: string | null | undefined, lazy = false, maxPx?: number, source: ImageSource = "art") {
  const ref = useRef<HTMLButtonElement & HTMLSpanElement>(null);
  const [visible, setVisible] = useState(!lazy || typeof IntersectionObserver === "undefined");
  const [state, setState] = useState<ArtImageState>(() => {
    const hit = file ? cache.get(keyOf(id, file, maxPx, source)) : undefined;
    return hit ? { status: "ready", src: hit } : { status: "idle", src: null };
  });

  useEffect(() => {
    if (visible || !ref.current) return;
    const io = new IntersectionObserver((es) => { if (es.some((e) => e.isIntersecting)) { setVisible(true); io.disconnect(); } });
    io.observe(ref.current);
    return () => io.disconnect();
  }, [visible, maxPx]);

  useEffect(() => {
    if (!file) { setState({ status: "idle", src: null }); return; }
    const hit = cache.get(keyOf(id, file, maxPx, source));
    if (hit) { setState({ status: "ready", src: hit }); return; }
    if (!visible) { setState({ status: "idle", src: null }); return; }
    let alive = true;
    setState({ status: "loading", src: null });
    loadArtImage(services, id, file, maxPx, source).then(
      (src) => { if (alive) setState({ status: "ready", src }); },
      () => { if (alive) setState({ status: "error", src: null }); },
    );
    return () => { alive = false; };
  }, [services, id, file, visible, maxPx, source]);

  return { ref, ...state, markError: () => setState({ status: "error", src: null }) };
}
