import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { ShellContext, type ShellContextValue } from "../../app/context";
import { createFakeServices } from "../../services/fake";
import type { Services } from "../../services";
import type { TransferProgress } from "../../services/types";
import Install from "./index";

afterEach(cleanup);

const P = (o: Partial<TransferProgress> & { error?: string } = {}): TransferProgress => ({
  phase: "receiving", files_done: 12, files_total: 40, bytes_done: 340_000_000, bytes_total: 1_200_000_000,
  speed_bps: 8_000_000, via_relay: false, ...o,
});

function setup(over: Partial<Services> = {}, params: Record<string, string> = { instance: "i1" }) {
  const services = { ...createFakeServices({ tickMs: 0 }), ...over };
  let cb: (p: TransferProgress) => void = () => {};
  services.onTransferProgress = (c) => { cb = c; return () => {}; };
  const navigate = vi.fn();
  const value = { services, route: "install", params, navigate, goBack: vi.fn(), canGoBack: false, setTitle: () => {} } as ShellContextValue;
  render(<ShellContext.Provider value={value}><Install /></ShellContext.Provider>);
  const emit = (p: TransferProgress) => act(() => cb(p));
  return { services, navigate, emit };
}

const flat = (s: string | null) => (s ?? "").replace(/[  ]/g, " ");

describe("Install", () => {
  it("receiving : barre accessible, libellé, vitesse, durée", () => {
    const { emit } = setup();
    emit(P());
    const bar = screen.getByRole("progressbar");
    expect(bar.getAttribute("aria-valuenow")).toBe("28");
    expect(bar.getAttribute("aria-valuemin")).toBe("0");
    expect(bar.getAttribute("aria-valuemax")).toBe("100");
    expect(flat(bar.getAttribute("aria-valuetext"))).toContain("12 / 40 fichiers · 340 Mo / 1,2 Go · 8 Mo/s");
    expect(flat(document.querySelector(".inst-stats")?.textContent ?? "")).toMatch(/12 \/ 40 fichiers · 340 Mo \/ 1,2 Go · 8 Mo\/s · environ/);
    expect(screen.getByText("2. Réception des fichiers").closest("li")?.getAttribute("aria-current")).toBe("step");
  });

  it("l'annonce live ne change qu'avec la phase", () => {
    const { emit } = setup();
    emit(P());
    const live = screen.getByRole("status");
    expect(live.textContent).toBe("Réception des fichiers");
    emit(P({ files_done: 20, bytes_done: 500_000_000 }));
    expect(live.textContent).toBe("Réception des fichiers");
    emit(P({ phase: "verifying" }));
    expect(live.textContent).toBe("Vérification d'intégrité");
  });

  it("preparing et launching", () => {
    const { emit } = setup();
    expect(screen.getByRole("status").textContent).toBe("Préparation en cours");
    emit(P({ phase: "launching" }));
    expect(screen.getByRole("status").textContent).toBe("Lancement du jeu");
    expect(screen.queryByRole("button", { name: "Pause" })).toBeNull();
  });

  it("via un relais", () => {
    const { emit } = setup();
    emit(P());
    expect(screen.queryByText(/via un relais/)).toBeNull();
    emit(P({ via_relay: true }));
    expect(screen.getByText(/via un relais/)).toBeTruthy();
  });

  it("pause puis reprise", async () => {
    const pause = vi.fn(async () => {});
    const resume = vi.fn(async () => {});
    const { emit } = setup({ joinPause: pause, joinResume: resume });
    emit(P());
    fireEvent.click(screen.getByRole("button", { name: "Pause" }));
    await waitFor(() => expect(pause).toHaveBeenCalled());
    emit(P({ phase: "paused", speed_bps: 0 }));
    expect(screen.getByRole("status").textContent).toBe("Transfert en pause");
    expect(screen.getByText("2. Réception des fichiers").closest("li")?.getAttribute("aria-current")).toBe("step");
    fireEvent.click(screen.getByRole("button", { name: "Reprendre" }));
    await waitFor(() => expect(resume).toHaveBeenCalled());
  });

  it("annulation abandonnée", () => {
    const cancel = vi.fn(async () => {});
    const { emit, navigate } = setup({ joinCancel: cancel });
    emit(P());
    fireEvent.click(screen.getByRole("button", { name: "Annuler" }));
    fireEvent.click(screen.getByRole("button", { name: "Continuer l'installation" }));
    expect(cancel).not.toHaveBeenCalled();
    expect(navigate).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Annuler" })).toBeTruthy();
  });

  it("annulation confirmée", async () => {
    const cancel = vi.fn(async () => {});
    const { emit, navigate } = setup({ joinCancel: cancel });
    emit(P());
    fireEvent.click(screen.getByRole("button", { name: "Annuler" }));
    fireEvent.click(screen.getByRole("button", { name: "Oui, annuler" }));
    await waitFor(() => expect(navigate).toHaveBeenCalledWith("main"));
    expect(cancel).toHaveBeenCalled();
    emit(P({ phase: "failed" }));
    expect(navigate).not.toHaveBeenCalledWith("error", expect.anything());
  });

  it("échec : message réel, bloc Détails et journal", async () => {
    let det: (e: { step: string; message: string; details: string; log_file: string }) => void = () => {};
    const { emit } = setup({ onTransferErrorDetails: (c: typeof det) => { det = c; return () => {}; } } as never);
    act(() => det({ step: "Réception", message: "Réception : réponse inattendue de l'hôte", details: "étape: Réception | détail: reset", log_file: "C:\logs\join-errors.log" }));
    emit(P({ phase: "failed", error: "Réception : réponse inattendue de l'hôte" }));
    expect((await screen.findByRole("alert")).textContent).toMatch(/^Réception : réponse inattendue de l'hôte/);
    expect(screen.queryByText(/Le transfert a échoué\./)).toBeNull();
    expect(screen.getByText("Détails")).toBeTruthy();
    expect(screen.getByLabelText("Détails de l'erreur").textContent).toContain("détail: reset");
    expect(screen.getByRole("button", { name: "Copier" })).toBeTruthy();
    expect(screen.getByText(/join-errors\.log/)).toBeTruthy();
  });

  it("échec identifiable : écran d'erreur", () => {
    const { emit, navigate } = setup();
    emit(P({ phase: "failed", error: "no space left on device" }));
    expect(navigate).toHaveBeenCalledWith("error", { state: "disk-full" });
  });

  it("échec inconnu : message sur place et retour à l'accueil, sans relance de transfert", () => {
    const start = vi.fn(async () => {});
    const { emit, navigate } = setup({ joinStart: start });
    emit(P({ phase: "failed" }));
    expect(navigate).not.toHaveBeenCalled();
    expect(screen.getByRole("alert").textContent).toMatch(/échoué/);
    expect(screen.queryByRole("button", { name: "Réessayer" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Retour à l'accueil" }));
    expect(navigate).toHaveBeenCalledWith("main");
    expect(start).not.toHaveBeenCalled();
  });

  it("done sans ami : Jouer lance l'instance, aucun joinPlay automatique", async () => {
    const play = vi.fn(async () => {});
    const launch = vi.fn(async () => {});
    const { emit, navigate } = setup({ joinPlay: play, instancesLaunch: launch });
    emit(P({ phase: "done", files_done: 40, bytes_done: 1_200_000_000, speed_bps: 0 }));
    expect(screen.getByRole("progressbar").getAttribute("aria-valuenow")).toBe("100");
    fireEvent.click(screen.getByRole("button", { name: "Jouer" }));
    await waitFor(() => expect(navigate).toHaveBeenCalledWith("main"));
    expect(launch).toHaveBeenCalledWith("i1");
    expect(play).not.toHaveBeenCalled();
  });

  it("pas de <main> imbriqué et titre focalisé", () => {
    setup();
    expect(document.querySelectorAll("main").length).toBe(0);
    expect(document.activeElement).toBe(screen.getByRole("heading", { level: 1, name: "Installation en cours" }));
  });

  it("affiche les mods inconnus reçus (transfer://unknown_mods)", () => {
    let push: (e: { instance_id: string; unknown_mods: string[] }) => void = () => {};
    setup({ onUnknownMods: (cb) => { push = cb; return () => {}; } });
    act(() => push({ instance_id: "i1", unknown_mods: ["mods/maison.jar"] }));
    expect(screen.getByRole("note").textContent).toContain("mods/maison.jar");
  });

  it("échec avec jar : l'écran Erreur reçoit le fichier", () => {
    const { emit, navigate } = setup();
    emit(P({ phase: "failed", error: "intégrité : aero.jar corrompu" }));
    expect(navigate).toHaveBeenCalledWith("error", { state: "bad-hash", file: "aero.jar" });
  });
});
