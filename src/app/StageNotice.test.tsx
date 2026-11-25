import { act, cleanup, render, screen, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { StageNotice, STAGE_NOTICE_MS } from "./StageNotice";
import { stageFallbackMessage } from "./stage";
import { createFakeServices } from "../services/fake";

// Faux service : les événements stage://fallback sont émis à la main ; on compte les abonnements actifs.
let svc: ReturnType<typeof createFakeServices>;
let active = 0;
const listeners = () => active;
const fallback = (payload: unknown) => act(() => { svc.emitStageFallback(payload); });

beforeEach(() => {
  svc = createFakeServices({ tickMs: 0 });
  active = 0;
  const subscribe = svc.onStageFallback;
  svc.onStageFallback = (cb) => {
    active += 1;
    const off = subscribe(cb);
    return () => { active -= 1; off(); };
  };
});
afterEach(() => { cleanup(); vi.useRealTimers(); });

describe("stageFallbackMessage", () => {
  it("phrase simple, sans la raison technique", () => {
    const m = stageFallbackMessage({ reason: "hook_failed", session_off: false });
    expect(m).toBe("Le jeu s'est ouvert dans sa propre fenêtre.");
    // coupure de session : le joueur doit savoir que ça dure ET comment revenir (fermer la fenêtre ne réactive rien)
    const off = stageFallbackMessage({ reason: "no_window", session_off: true });
    expect(off).toMatch(/prochains lancements/);
    expect(off).toMatch(/Réessayer dans Cubelink/);
    expect(off).not.toMatch(/ferme puis rouvre|fermeture de Cubelink/);
    for (const r of ["hook_failed", "no_window", "adoption_failed"]) {
      expect(stageFallbackMessage({ reason: r, session_off: true })).not.toContain(r);
    }
  });
});

describe("StageNotice : repli de la zone de jeu", () => {
  it("rien tant qu'aucun repli n'est annoncé", async () => {
    render(<StageNotice services={svc} />);
    await waitFor(() => expect(listeners()).toBe(1));
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("affiche le message au repli, le bouton Fermer l'écarte", async () => {
    render(<StageNotice services={svc} />);
    await waitFor(() => expect(listeners()).toBe(1));
    fallback({ reason: "no_window", session_off: false });
    expect(screen.getByRole("status").textContent).toContain("Le jeu s'est ouvert dans sa propre fenêtre.");
    expect(screen.getByRole("status").textContent).not.toContain("no_window");
    fireEvent.click(screen.getByRole("button", { name: "Fermer" }));
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("le message simple disparaît tout seul, l'annonce de coupure de session reste", async () => {
    render(<StageNotice services={svc} />);
    await waitFor(() => expect(listeners()).toBe(1));
    vi.useFakeTimers();
    fallback({ reason: "adoption_failed", session_off: false });
    expect(screen.getByRole("status")).toBeTruthy();
    act(() => { vi.advanceTimersByTime(STAGE_NOTICE_MS + 1); });
    expect(screen.queryByRole("status")).toBeNull();

    fallback({ reason: "adoption_failed", session_off: true });
    act(() => { vi.advanceTimersByTime(STAGE_NOTICE_MS * 3); });
    expect(screen.getByRole("status").textContent).toMatch(/Réessayer dans Cubelink/);
  });

  it("repli simple : pas de bouton « Réessayer »", async () => {
    render(<StageNotice services={svc} />);
    await waitFor(() => expect(listeners()).toBe(1));
    fallback({ reason: "no_window", session_off: false });
    expect(screen.queryByRole("button", { name: "Réessayer dans Cubelink" })).toBeNull();
  });

  it("coupure de session : « Réessayer » appelle stageRetry une fois puis écarte la notice", async () => {
    const ok = createFakeServices({ tickMs: 0, stageSupported: true });
    const subscribe = ok.onStageFallback;
    let n = 0;
    ok.onStageFallback = (cb) => { n += 1; const off = subscribe(cb); return () => { n -= 1; off(); }; };
    render(<StageNotice services={ok} />);
    await waitFor(() => expect(n).toBe(1));
    act(() => { ok.emitStageFallback({ reason: "no_window", session_off: true }); });
    const btn = screen.getByRole("button", { name: "Réessayer dans Cubelink" });
    fireEvent.click(btn);
    fireEvent.click(btn); // double clic : un seul appel
    await waitFor(() => expect(screen.queryByRole("status")).toBeNull());
    expect(ok.stageCalls.filter((c) => c.cmd === "retry")).toHaveLength(1);
  });

  it("« Réessayer » sans effet (réglage désactivé) : la notice reste, avec une explication", async () => {
    render(<StageNotice services={svc} />); // faux service par défaut : stageRetry rend faux
    await waitFor(() => expect(listeners()).toBe(1));
    fallback({ reason: "no_window", session_off: true });
    fireEvent.click(screen.getByRole("button", { name: "Réessayer dans Cubelink" }));
    await waitFor(() => expect(screen.getByRole("status").textContent).toMatch(/Jeu intégré/));
    expect(screen.getByRole("button", { name: "Réessayer dans Cubelink" })).toBeTruthy();
  });

  it("« Réessayer » qui échoue : pas de plantage, la notice reste", async () => {
    svc.stageRetry = () => Promise.reject(new Error("boom"));
    render(<StageNotice services={svc} />);
    await waitFor(() => expect(listeners()).toBe(1));
    fallback({ reason: "no_window", session_off: true });
    fireEvent.click(screen.getByRole("button", { name: "Réessayer dans Cubelink" }));
    await waitFor(() => expect(screen.getByRole("status").textContent).toMatch(/Jeu intégré/));
  });

  it("charge utile incomplète : message simple, pas d'erreur", async () => {
    render(<StageNotice services={svc} />);
    await waitFor(() => expect(listeners()).toBe(1));
    fallback(undefined);
    expect(screen.getByRole("status").textContent).toContain("Le jeu s'est ouvert dans sa propre fenêtre.");
  });

  it("se désabonne au démontage", async () => {
    const { unmount } = render(<StageNotice services={svc} />);
    await waitFor(() => expect(listeners()).toBe(1));
    unmount();
    expect(listeners()).toBe(0);
  });
});
