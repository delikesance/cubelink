import { cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { stageLayout, useStageLayout } from "./stage";

describe("stageLayout : px CSS vers pixels physiques", () => {
  const rect = { left: 0, top: 36, width: 1101, height: 683 };

  it("échelle 1 : valeurs inchangées", () => {
    expect(stageLayout(rect, 380, 1)).toEqual({ x: 0, y: 36, w: 1101, h: 683, panel_w: 380 });
  });

  it("échelle 1,25 : arrondi au plus proche", () => {
    // 1101 × 1,25 = 1376,25 ; 683 × 1,25 = 853,75
    expect(stageLayout(rect, 380, 1.25)).toEqual({ x: 0, y: 45, w: 1376, h: 854, panel_w: 475 });
  });

  it("échelle 1,5", () => {
    // 1101 × 1,5 = 1651,5 ; 683 × 1,5 = 1024,5
    expect(stageLayout(rect, 380, 1.5)).toEqual({ x: 0, y: 54, w: 1652, h: 1025, panel_w: 570 });
  });

  it("échelle 2", () => {
    expect(stageLayout(rect, 380, 2)).toEqual({ x: 0, y: 72, w: 2202, h: 1366, panel_w: 760 });
  });

  it("toujours des entiers, même avec un rectangle fractionnaire", () => {
    const l = stageLayout({ left: 0.4, top: 36.3, width: 799.6, height: 563.7 }, 380.5, 1.25);
    for (const v of Object.values(l)) expect(Number.isInteger(v)).toBe(true);
  });

  it("tailles minimales : 1 × 1, panneau jamais négatif, pas de zéro négatif", () => {
    const l = stageLayout({ left: -0.2, top: 0, width: 0, height: -5 }, -10, 1.5);
    expect(l).toEqual({ x: 0, y: 0, w: 1, h: 1, panel_w: 0 });
    expect(Object.is(l.x, -0)).toBe(false);
  });

  it("échelle invalide : traitée comme 1", () => {
    expect(stageLayout(rect, 380, 0)).toEqual({ x: 0, y: 36, w: 1101, h: 683, panel_w: 380 });
    expect(stageLayout(rect, 380, Number.NaN)).toEqual({ x: 0, y: 36, w: 1101, h: 683, panel_w: 380 });
  });
});

describe("useStageLayout : publication à Rust", () => {
  type Rect = { left: number; top: number; width: number; height: number };
  type Internals = { invoke: (cmd: string, args?: unknown) => Promise<unknown> };
  const w = window as unknown as { __TAURI_INTERNALS__?: Internals };

  // Le vrai `invoke` de @tauri-apps/api/core délègue à window.__TAURI_INTERNALS__.invoke : on le remplace ici.
  const invoke = vi.fn<Internals["invoke"]>();
  let rect: Rect;
  let observers: { cb: () => void; observe: ReturnType<typeof vi.fn>; disconnect: ReturnType<typeof vi.fn> }[];
  let media: { query: string; listeners: Set<() => void> }[];

  function slot() {
    const el = document.createElement("div");
    el.getBoundingClientRect = () => ({ ...rect, x: rect.left, y: rect.top, right: rect.left + rect.width, bottom: rect.top + rect.height, toJSON: () => ({}) });
    return { current: el as HTMLElement | null };
  }
  const setDpr = (v: number) => Object.defineProperty(window, "devicePixelRatio", { value: v, configurable: true });
  const calls = () => invoke.mock.calls.filter(([cmd]) => cmd === "stage_layout").map(([, args]) => args);
  const sent = (n: number) => vi.waitFor(() => expect(calls()).toHaveLength(n));

  beforeEach(() => {
    invoke.mockReset();
    invoke.mockResolvedValue(undefined);
    rect = { left: 0, top: 36, width: 1100, height: 684 };
    observers = [];
    media = [];
    setDpr(1);
    // jsdom n'a ni ResizeObserver ni matchMedia.
    vi.stubGlobal("ResizeObserver", class {
      observe = vi.fn();
      unobserve = vi.fn();
      disconnect = vi.fn();
      constructor(cb: () => void) { observers.push({ cb, observe: this.observe, disconnect: this.disconnect }); }
    });
    vi.stubGlobal("matchMedia", (query: string) => {
      const m = { query, listeners: new Set<() => void>() };
      media.push(m);
      return {
        media: query, matches: true,
        addEventListener: (_: string, fn: () => void) => { m.listeners.add(fn); },
        removeEventListener: (_: string, fn: () => void) => { m.listeners.delete(fn); },
      };
    });
    w.__TAURI_INTERNALS__ = { invoke };
  });

  afterEach(() => {
    cleanup();
    delete w.__TAURI_INTERNALS__;
    document.documentElement.style.removeProperty("--panel-w");
    vi.unstubAllGlobals();
  });

  it("publie une fois au montage, avec les noms d'arguments attendus par Rust", async () => {
    const ref = slot();
    renderHook(() => useStageLayout(ref));
    await sent(1);
    expect(calls()[0]).toEqual({ x: 0, y: 36, w: 1100, h: 684, panel_w: 380 });
    expect(observers).toHaveLength(1);
    expect(observers[0].observe).toHaveBeenCalledWith(ref.current);
  });

  it("ne republie pas une mise en page identique, republie quand le rectangle change", async () => {
    const ref = slot();
    renderHook(() => useStageLayout(ref));
    await sent(1);
    // Mêmes mesures : ni l'observateur ni « resize » ne déclenchent d'envoi.
    observers[0].cb();
    window.dispatchEvent(new Event("resize"));
    rect = { left: 0, top: 36, width: 1280, height: 764 };
    window.dispatchEvent(new Event("resize"));
    await sent(2);
    expect(calls()[1]).toEqual({ x: 0, y: 36, w: 1280, h: 764, panel_w: 380 });
    rect = { left: 0, top: 36, width: 1280, height: 800 };
    observers[0].cb();
    await sent(3);
    expect(calls()[2]).toEqual({ x: 0, y: 36, w: 1280, h: 800, panel_w: 380 });
  });

  it("changement d'échelle : republie en pixels physiques et réarme l'écoute", async () => {
    const ref = slot();
    renderHook(() => useStageLayout(ref));
    await sent(1);
    expect(media).toHaveLength(1);
    expect(media[0].query).toBe("(resolution: 1dppx)");
    setDpr(1.5);
    for (const fn of [...media[0].listeners]) fn();
    await sent(2);
    expect(calls()[1]).toEqual({ x: 0, y: 54, w: 1650, h: 1026, panel_w: 570 });
    expect(media[0].listeners.size).toBe(0);
    expect(media[1].query).toBe("(resolution: 1.5dppx)");
    expect(media[1].listeners.size).toBe(1);
  });

  it("lit la largeur du panneau dans --panel-w", async () => {
    document.documentElement.style.setProperty("--panel-w", "420px");
    const ref = slot();
    renderHook(() => useStageLayout(ref));
    await sent(1);
    expect(calls()[0]).toMatchObject({ panel_w: 420 });
  });

  it("échec de l'envoi : avalé, puis retenté au changement suivant", async () => {
    invoke.mockRejectedValueOnce(new Error("commande absente"));
    const ref = slot();
    renderHook(() => useStageLayout(ref));
    await sent(1);
    await vi.waitFor(() => { window.dispatchEvent(new Event("resize")); expect(calls()).toHaveLength(2); });
    expect(calls()[1]).toEqual(calls()[0]);
  });

  it("démontage : plus d'écoute ni d'envoi", async () => {
    const ref = slot();
    const { unmount } = renderHook(() => useStageLayout(ref));
    await sent(1);
    unmount();
    expect(observers[0].disconnect).toHaveBeenCalled();
    expect(media[0].listeners.size).toBe(0);
    rect = { left: 0, top: 36, width: 640, height: 400 };
    window.dispatchEvent(new Event("resize"));
    observers[0].cb();
    await import("@tauri-apps/api/core");
    await Promise.resolve();
    expect(calls()).toHaveLength(1);
  });

  it("hors Tauri : ne mesure rien et n'envoie rien", async () => {
    delete w.__TAURI_INTERNALS__;
    const ref = slot();
    renderHook(() => useStageLayout(ref));
    window.dispatchEvent(new Event("resize"));
    await import("@tauri-apps/api/core");
    await Promise.resolve();
    expect(invoke).not.toHaveBeenCalled();
    expect(observers).toHaveLength(0);
    expect(media).toHaveLength(0);
  });
});
