import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { ShellContext, type ShellContextValue } from "../../app/context";
import { setImportWarning } from "../../app/importWarning";
import { createFakeServices } from "../../services/fake";
import type { Services } from "../../services";
import Main from "./index";

afterEach(() => { cleanup(); setImportWarning(null); });

function setup(over: Partial<Services> = {}) {
  const services = { ...createFakeServices({ tickMs: 0, requestCount: 0 }), modsCheck: async () => ({ issues: [], checked: 0 }), ...over };
  const navigate = vi.fn();
  const value = { services, route: "main", params: {}, navigate, setTitle: () => {}, goBack: () => {}, canGoBack: false } as ShellContextValue;
  render(<ShellContext.Provider value={value}><Main /></ShellContext.Provider>);
  return { services, navigate };
}

describe("Main : échec du lancement rapide", () => {
  it("événement failed : bandeau nommant le modpack, lien vers ses logs, Ignorer", async () => {
    let emit: (s: { id: string; state: "failed"; message?: string }) => void = () => {};
    const { navigate } = setup({ onLaunchState: (cb) => { emit = cb; return () => {}; } });
    await screen.findByRole("button", { name: "Jouer à Create: Aeronautics" });
    act(() => emit({ id: "inst-aeronautics", state: "failed", message: "Java introuvable" }));
    const alert = screen.getByRole("alert");
    expect(alert.textContent).toContain("Échec du lancement de Create: Aeronautics");
    expect(alert.textContent).toContain("Java introuvable");
    fireEvent.click(within(alert).getByRole("button", { name: "Voir les logs" }));
    expect(navigate).toHaveBeenCalledWith("instance", { id: "inst-aeronautics", tab: "logs" });
    fireEvent.click(within(alert).getByRole("button", { name: "Ignorer" }));
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("appel de lancement rejeté : l'échec est visible", async () => {
    const { navigate } = setup({ instancesLaunch: async () => { throw new Error("Instance introuvable."); } });
    fireEvent.click(await screen.findByRole("button", { name: "Jouer à Create: Aeronautics" }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Instance introuvable.");
    fireEvent.click(within(alert).getByRole("button", { name: "Voir les logs" }));
    expect(navigate).toHaveBeenCalledWith("instance", { id: "inst-aeronautics", tab: "logs" });
  });
});

describe("Main : avertissement d'import", () => {
  it("affiche un bandeau non bloquant pour les mods manquants", async () => {
    let push: (e: { missing: string[] }) => void = () => {};
    setup({ onImportWarning: (cb) => { push = cb; return () => {}; } });
    await screen.findByRole("group", { name: "Instances" });
    act(() => push({ missing: ["sodium", "iris"] }));
    expect(screen.getByText(/2 mods introuvables : sodium, iris/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Ignorer" }));
    expect(screen.queryByText(/introuvables/)).toBeNull();
  });

  it("reprend l'avertissement reçu pendant l'import (écran Create)", async () => {
    setImportWarning(["jei"]);
    setup();
    expect(await screen.findByText(/1 mod introuvable : jei/)).toBeTruthy();
  });
});
