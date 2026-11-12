import { act, cleanup, render, screen, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { StageNotice, STAGE_NOTICE_MS } from "./StageNotice";
import { stageFallbackMessage } from "./stage";

// Écouteurs enregistrés par le composant, déclenchés à la main (aucun vrai Tauri).
const bus = vi.hoisted(() => ({ handlers: new Map<string, Set<(e: { payload: unknown }) => void>>() }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: async (name: string, cb: (e: { payload: unknown }) => void) => {
    const set = bus.handlers.get(name) ?? new Set();
    set.add(cb);
    bus.handlers.set(name, set);
    return () => { set.delete(cb); };
  },
}));

const w = window as unknown as { __TAURI_INTERNALS__?: unknown };
const listeners = () => bus.handlers.get("stage://fallback")?.size ?? 0;
const fallback = (payload: unknown) => act(() => { bus.handlers.get("stage://fallback")?.forEach((cb) => cb({ payload })); });

beforeEach(() => { bus.handlers.clear(); w.__TAURI_INTERNALS__ = {}; });
afterEach(() => { cleanup(); vi.useRealTimers(); delete w.__TAURI_INTERNALS__; });

describe("stageFallbackMessage", () => {
  it("phrase simple, sans la raison technique", () => {
    const m = stageFallbackMessage({ reason: "hook_failed", session_off: false });
    expect(m).toBe("Le jeu s'est ouvert dans sa propre fenêtre.");
    // coupure de session : le joueur doit savoir que ça dure ET comment revenir
    const off = stageFallbackMessage({ reason: "no_window", session_off: true });
    expect(off).toMatch(/jusqu'à la fermeture de Cubelink/);
    expect(off).toMatch(/ferme puis rouvre Cubelink/);
    for (const r of ["hook_failed", "no_window", "adoption_failed"]) {
      expect(stageFallbackMessage({ reason: r, session_off: true })).not.toContain(r);
    }
  });
});

describe("StageNotice : repli de la zone de jeu", () => {
  it("rien tant qu'aucun repli n'est annoncé", async () => {
    render(<StageNotice />);
    await waitFor(() => expect(listeners()).toBe(1));
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("affiche le message au repli, le bouton Fermer l'écarte", async () => {
    render(<StageNotice />);
    await waitFor(() => expect(listeners()).toBe(1));
    fallback({ reason: "no_window", session_off: false });
    expect(screen.getByRole("status").textContent).toContain("Le jeu s'est ouvert dans sa propre fenêtre.");
    expect(screen.getByRole("status").textContent).not.toContain("no_window");
    fireEvent.click(screen.getByRole("button", { name: "Fermer" }));
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("le message simple disparaît tout seul, l'annonce « jusqu'au redémarrage » reste", async () => {
    render(<StageNotice />);
    await waitFor(() => expect(listeners()).toBe(1));
    vi.useFakeTimers();
    fallback({ reason: "adoption_failed", session_off: false });
    expect(screen.getByRole("status")).toBeTruthy();
    act(() => { vi.advanceTimersByTime(STAGE_NOTICE_MS + 1); });
    expect(screen.queryByRole("status")).toBeNull();

    fallback({ reason: "adoption_failed", session_off: true });
    act(() => { vi.advanceTimersByTime(STAGE_NOTICE_MS * 3); });
    expect(screen.getByRole("status").textContent).toMatch(/ferme puis rouvre Cubelink/);
  });

  it("charge utile incomplète : message simple, pas d'erreur", async () => {
    render(<StageNotice />);
    await waitFor(() => expect(listeners()).toBe(1));
    fallback(undefined);
    expect(screen.getByRole("status").textContent).toContain("Le jeu s'est ouvert dans sa propre fenêtre.");
  });

  it("se désabonne au démontage ; hors Tauri n'écoute rien", async () => {
    const { unmount } = render(<StageNotice />);
    await waitFor(() => expect(listeners()).toBe(1));
    unmount();
    expect(listeners()).toBe(0);
    delete w.__TAURI_INTERNALS__;
    render(<StageNotice />);
    await new Promise((r) => setTimeout(r, 20));
    expect(listeners()).toBe(0);
  });
});
