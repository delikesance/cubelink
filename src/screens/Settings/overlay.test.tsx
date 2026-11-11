import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Shell } from "../../app/shell";
import { createFakeServices } from "../../services/fake";
import { validateHotkey } from "./index";

beforeEach(() => { window.location.hash = "#/settings"; });
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

describe("validation du raccourci", () => {
  it("accepte modificateur(s) + une touche, refuse le reste en français", () => {
    expect(validateHotkey("Ctrl+Shift+C")).toBeNull();
    expect(validateHotkey("Alt+F10")).toBeNull();
    expect(validateHotkey("")).toMatch(/Indique un raccourci/);
    expect(validateHotkey("C")).toMatch(/modificateur/);
    expect(validateHotkey("Ctrl+Shift")).toMatch(/une touche/);
    expect(validateHotkey("Ctrl+A+B")).toMatch(/une touche/);
    expect(validateHotkey("Ctrl++")).toMatch(/invalide/);
    expect(validateHotkey("Ctrl+Bidule")).toMatch(/non reconnue/);
  });
});

describe("Panneau en jeu (Réglages > Réseau)", () => {
  async function open(svc = createFakeServices({ tickMs: 0 })) {
    render(<Shell services={svc} />);
    fireEvent.click(screen.getByRole("tab", { name: "Réseau" }));
    const input = await screen.findByLabelText(/Raccourci du panneau/) as HTMLInputElement;
    await waitFor(() => expect(input.value).toBe("Ctrl+Shift+C"));
    return { svc, input };
  }

  it("lit le raccourci actuel et l'interrupteur, enregistre l'interrupteur via settings_set", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    const set = vi.spyOn(svc, "settingsSet");
    await open(svc);
    const sw = screen.getByLabelText(/Panneau en jeu/, { selector: "input" }) as HTMLInputElement;
    expect(sw.checked).toBe(true);
    fireEvent.click(sw);
    await waitFor(() => expect(set).toHaveBeenCalledTimes(1));
    expect(set.mock.calls[0][0]).toMatchObject({ overlay_enabled: false, overlay_hotkey: "Ctrl+Shift+C" });
  });

  it("enregistre un raccourci valide à la sortie du champ", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    const set = vi.spyOn(svc, "settingsSet");
    const { input } = await open(svc);
    fireEvent.change(input, { target: { value: "Ctrl+Alt+J" } });
    fireEvent.blur(input);
    await waitFor(() => expect(set).toHaveBeenCalledTimes(1));
    expect(set.mock.calls[0][0]).toMatchObject({ overlay_hotkey: "Ctrl+Alt+J" });
  });

  it("refuse un raccourci invalide : message en français, rien d'enregistré", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    const set = vi.spyOn(svc, "settingsSet");
    const { input } = await open(svc);
    fireEvent.change(input, { target: { value: "C" } });
    fireEvent.blur(input);
    expect(await screen.findByText(/au moins un modificateur/)).toBeTruthy();
    expect(set).not.toHaveBeenCalled();
  });
});
