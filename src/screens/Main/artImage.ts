import { useEffect, useRef, useState } from "react";
import type { Services } from "../../services";

const cache = new Map<string, string>();
const pending = new Map<string, Promise<string>>();
const keyOf = (id: string, file: string, maxPx?: number) => `${id}|${file}|${maxPx ?? ""}`;

/** Vide le cache mémoire (tests). */
export function clearArtCache() {
  cache.clear();
  pending.clear();
}

export function loadArtImage(services: Pick<Services, "instancesArtImage">, id: string, file: string, maxPx?: number): Promise<string> {
  const key = keyOf(id, file, maxPx);
  const hit = cache.get(key);
  if (hit) return Promise.resolve(hit);
  let p = pending.get(key);
  if (!p) {
    p = services.instancesArtImage(id, file, maxPx).then((u) => { cache.set(key, u); return u; }).finally(() => pending.delete(key));
    pending.set(key, p);
  }
  return p;
}

export type ArtImageState = { status: "idle" | "loading" | "ready" | "error"; src: string | null };

/** Charge une image du modpack (cache mémoire par instance + fichier). `lazy` : attend que l'élément soit visible. */
export function useArtImage(services: Pick<Services, "instancesArtImage">, id: string, file: string | null | undefined, lazy = false, maxPx?: number) {
  const ref = useRef<HTMLButtonElement & HTMLSpanElement>(null);
  const [visible, setVisible] = useState(!lazy || typeof IntersectionObserver === "undefined");
  const [state, setState] = useState<ArtImageState>(() => {
    const hit = file ? cache.get(keyOf(id, file, maxPx)) : undefined;
    return hit ? { status: "ready", src: hit } : { status: "idle", src: null };
  });

  useEffect(() => {
    if (visible || !ref.current) return;
    const io = new IntersectionObserver((es) => { if (es.some((e) => e.isIntersecting)) { setVisible(true); io.disconnect(); } });
    io.observe(ref.current);
    return () => io.disconnect();
  }, [visible]);

  useEffect(() => {
    if (!file) { setState({ status: "idle", src: null }); return; }
    const hit = cache.get(keyOf(id, file, maxPx));
    if (hit) { setState({ status: "ready", src: hit }); return; }
    if (!visible) { setState({ status: "idle", src: null }); return; }
    let alive = true;
    setState({ status: "loading", src: null });
    loadArtImage(services, id, file, maxPx).then(
      (src) => { if (alive) setState({ status: "ready", src }); },
      () => { if (alive) setState({ status: "error", src: null }); },
    );
    return () => { alive = false; };
  }, [services, id, file, visible, maxPx]);

  return { ref, ...state, markError: () => setState({ status: "error", src: null }) };
}
