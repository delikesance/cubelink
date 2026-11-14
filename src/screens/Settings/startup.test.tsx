import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Shell } from "../../app/shell";
import { createFakeServices } from "../../services/fake";

beforeEach(() => { window.location.hash = "#/settings"; });
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

async function openTab() {
  fireEvent.click(screen.getByRole("tab", { name: "Réseau" }));
  const input = await screen.findByLabelText(/Raccourci du panneau/) as HTMLInputElement;
  await waitFor(() => expect(input.value).toBe("Ctrl+Shift+C"));
}

const label = /Lancer Cubelink au démarrage de l'ordinateur/;

describe("Réglage « Lancer Cubelink au démarrage »", () => {
  it("coché par défaut, explique le fond, enregistré sans écraser les autres réglages", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    const set = vi.spyOn(svc, "settingsSet");
    render(<Shell services={svc} />);
    await openTab();
    const box = screen.getByLabelText(label, { selector: "input" }) as HTMLInputElement;
    expect(box.checked).toBe(true);
    expect(screen.getByText(/Fermer la fenêtre le garde ouvert dans la zone de notification/)).toBeTruthy();
    fireEvent.click(box);
    await waitFor(() => expect(set).toHaveBeenCalledTimes(1));
    expect(set.mock.calls[0][0]).toMatchObject({ start_with_os: false, lan_auto: true, embedded_game: true });
    await waitFor(() => expect(box.checked).toBe(false));
  });

  it("ancien réglage sans le champ : affiché coché", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    const full = await svc.settingsGet();
    const { start_with_os: _dropped, ...old } = full;
    vi.spyOn(svc, "settingsGet").mockResolvedValue(old as typeof full);
    render(<Shell services={svc} />);
    await openTab();
    expect((screen.getByLabelText(label, { selector: "input" }) as HTMLInputElement).checked).toBe(true);
  });
});
