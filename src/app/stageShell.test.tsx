import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { Shell } from "./shell";
import { createFakeServices } from "../services/fake";

// La coque pilote la zone de jeu par `Services` (commandes stage_*, événements stage://*), jamais par `invoke` direct.
beforeEach(() => { window.location.hash = "#/login"; });

/** Monte la coque et attend la première réponse de `stageState` (sinon elle écraserait l'état émis par le test). */
async function mount() {
  const svc = createFakeServices({ tickMs: 0, stageSupported: true });
  render(<Shell services={svc} />);
  // La mise en page n'est publiée qu'après `stageSupported`, demandé après `stageState`.
  await waitFor(() => expect(svc.stageCalls.some((c) => c.cmd === "layout")).toBe(true));
  return svc;
}
afterEach(() => { cleanup(); });

describe("Zone de jeu : la coque passe par Services", () => {
  it("bouton de la barre de titre : stageView, libellé suivant stage://state", async () => {
    const svc = await mount();
    expect(screen.queryByRole("button", { name: "Launcher" })).toBeNull();
    act(() => svc.emitStageState({ attached: true, game: true }));
    fireEvent.click(await screen.findByRole("button", { name: "Launcher" }));
    await waitFor(() => expect(svc.stageCalls).toContainEqual({ cmd: "view", arg: false }));
    expect(await screen.findByRole("button", { name: "Retour au jeu" })).toBeTruthy();
  });

  it("Échap ferme le panneau « en jeu » par stagePanel", async () => {
    const svc = await mount();
    act(() => svc.emitStageState({ attached: true, game: true, panel: true }));
    await screen.findByRole("button", { name: "Launcher" });
    fireEvent.keyDown(document, { key: "Escape" });
    await waitFor(() => expect(svc.stageCalls).toContainEqual({ cmd: "panel", arg: false }));
  });

  it("plein écran du jeu : setFullscreen(true) puis (false) à la fin du jeu", async () => {
    const svc = await mount();
    act(() => svc.emitStageState({ attached: true, game: true }));
    await screen.findByRole("button", { name: "Launcher" });
    act(() => svc.emitStageFullscreen(true));
    await waitFor(() => expect(svc.stageCalls).toContainEqual({ cmd: "fullscreen", arg: true }));
    act(() => svc.emitStageState({ attached: false, game: false }));
    await waitFor(() => expect(svc.stageCalls).toContainEqual({ cmd: "fullscreen", arg: false }));
  });

  it("zone de jeu disponible : la mise en page est publiée par stageLayout", async () => {
    const svc = await mount();
    await waitFor(() => expect(svc.stageCalls.some((c) => c.cmd === "layout")).toBe(true));
  });
});
