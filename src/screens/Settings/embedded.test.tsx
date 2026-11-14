import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Shell } from "../../app/shell";
import { createFakeServices } from "../../services/fake";

// La réponse de Rust (`stage_supported`) est simulée : vrai sous Windows, faux ailleurs.
const platform = vi.hoisted(() => ({ supported: true }));
vi.mock("../../app/stage", async (original) => ({
  ...(await original<typeof import("../../app/stage")>()),
  useStageSupported: () => platform.supported,
}));

beforeEach(() => { window.location.hash = "#/settings"; platform.supported = true; });
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

async function openTab() {
  fireEvent.click(screen.getByRole("tab", { name: "Réseau" }));
  // les réglages sont chargés quand le champ du raccourci est rempli
  const input = await screen.findByLabelText(/Raccourci du panneau/) as HTMLInputElement;
  await waitFor(() => expect(input.value).toBe("Ctrl+Shift+C"));
}

describe("Réglage « Jeu intégré »", () => {
  it("sous Windows : coché par défaut, phrase d'aide, enregistré sans écraser les autres réglages", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    const set = vi.spyOn(svc, "settingsSet");
    render(<Shell services={svc} />);
    await openTab();
    const box = screen.getByLabelText(/Jeu intégré/, { selector: "input" }) as HTMLInputElement;
    expect(box.checked).toBe(true);
    expect(screen.getByText(/^Le jeu s'affiche dans la fenêtre de Cubelink\. Désactive si le jeu ne s'affiche pas correctement\. Appliqué au prochain lancement du jeu, et sous Linux au prochain démarrage de Cubelink\.$/)).toBeTruthy();
    fireEvent.click(box);
    await waitFor(() => expect(set).toHaveBeenCalledTimes(1));
    expect(set.mock.calls[0][0]).toMatchObject({ embedded_game: false, overlay_enabled: true, lan_auto: true });
    await waitFor(() => expect(box.checked).toBe(false));
    fireEvent.click(box);
    await waitFor(() => expect(set).toHaveBeenCalledTimes(2));
    expect(set.mock.calls[1][0]).toMatchObject({ embedded_game: true });
  });

  it("jeu intégré décoché : le raccourci du panneau est grisé, avec la raison ; recoché, il redevient modifiable", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    render(<Shell services={svc} />);
    await openTab();
    const input = screen.getByLabelText(/Raccourci du panneau/) as HTMLInputElement;
    const box = screen.getByLabelText(/Jeu intégré/, { selector: "input" }) as HTMLInputElement;
    expect(input.disabled).toBe(false);
    expect(screen.queryByText(/Active « Jeu intégré » pour t'en servir/)).toBeNull();
    fireEvent.click(box);
    await waitFor(() => expect(input.disabled).toBe(true));
    expect(screen.getByText(/^Indisponible : le panneau s'ouvre par-dessus le jeu intégré\. Active « Jeu intégré » pour t'en servir\.$/)).toBeTruthy();
    expect(input.getAttribute("aria-describedby")).toBeTruthy();
    expect(input.value).toBe("Ctrl+Shift+C");
    fireEvent.click(box);
    await waitFor(() => expect(input.disabled).toBe(false));
    expect(screen.getByText(/Exemple : Ctrl\+Shift\+C/)).toBeTruthy();
  });

  it("ancien réglage sans le champ : affiché coché (activé par défaut)", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    const full = await svc.settingsGet();
    const { embedded_game: _dropped, ...old } = full;
    vi.spyOn(svc, "settingsGet").mockResolvedValue(old as typeof full);
    render(<Shell services={svc} />);
    await openTab();
    expect((screen.getByLabelText(/Jeu intégré/, { selector: "input" }) as HTMLInputElement).checked).toBe(true);
  });

  it("ailleurs que sous Windows : le réglage n'est pas montré", async () => {
    platform.supported = false;
    render(<Shell services={createFakeServices({ tickMs: 0 })} />);
    await openTab();
    expect(screen.queryByLabelText(/Jeu intégré/)).toBeNull();
    expect(screen.queryByText(/Le jeu s'affiche dans la fenêtre de Cubelink/)).toBeNull();
  });
});
