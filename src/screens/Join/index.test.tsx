import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { ShellContext, type ShellContextValue } from "../../app/context";
import { createFakeServices } from "../../services/fake";
import type { Services } from "../../services";
import Join from "./index";

afterEach(cleanup);

function setup(over: Partial<Services> = {}, params: Record<string, string> = { friend: "pk-alex" }) {
  const services = { ...createFakeServices({ tickMs: 0 }), ...over };
  const navigate = vi.fn();
  const value = { services, route: "join", params, navigate, setTitle: () => {} } as ShellContextValue;
  render(<ShellContext.Provider value={value}><Join /></ShellContext.Provider>);
  return { services, navigate };
}

describe("Join", () => {
  it("affiche le plan sans rien télécharger, titre focalisé", async () => {
    const start = vi.fn(async () => {});
    setup({ joinStart: start });
    const h = await screen.findByRole("heading", { level: 1, name: /Rejoindre «/ });
    await waitFor(() => expect(document.activeElement).toBe(h));
    expect(screen.getByText("Minecraft")).toBeTruthy();
    expect(screen.getByText(/Java \d+/)).toBeTruthy();
    expect(document.querySelector(".join-sub")?.textContent).toContain("à recevoir");
    expect(screen.getByText(/Liste des fichiers \(/)).toBeTruthy();
    expect(start).not.toHaveBeenCalled();
  });

  it("signale les mods inconnus et exige la confiance", async () => {
    setup();
    expect(await screen.findByText("mods/mod-maison.jar")).toBeTruthy();
    expect(screen.getByText(/L'intégrité est vérifiée, pas la sécurité/)).toBeTruthy();
    const btn = screen.getByRole("button", { name: "Rejoindre" }) as HTMLButtonElement;
    expect(btn.disabled).toBe(true);
    fireEvent.click(screen.getByRole("checkbox"));
    expect(btn.disabled).toBe(false);
  });

  it("Rejoindre appelle joinStart puis va à l'installation", async () => {
    const { services, navigate } = setup();
    const spy = vi.spyOn(services, "joinStart");
    await screen.findByText("mods/mod-maison.jar");
    fireEvent.click(screen.getByRole("checkbox"));
    fireEvent.click(screen.getByRole("button", { name: "Rejoindre" }));
    await waitFor(() => expect(navigate).toHaveBeenCalledWith("install", { friend: "pk-alex" }));
    expect(spy).toHaveBeenCalledWith("pk-alex");
  });

  it("sans mod inconnu, Rejoindre est actif directement", async () => {
    const base = createFakeServices({ tickMs: 0 });
    setup({ joinPlan: async () => ({ ...(await base.joinPlan("x")), unknown_mods: [] }) });
    await screen.findByRole("heading", { level: 1 });
    expect((screen.getByRole("button", { name: "Rejoindre" }) as HTMLButtonElement).disabled).toBe(false);
    expect(screen.queryByRole("checkbox")).toBeNull();
  });

  it("Annuler retourne à l'accueil", async () => {
    const { navigate } = setup();
    await screen.findByRole("heading", { level: 1, name: /Rejoindre «/ });
    fireEvent.click(screen.getByRole("button", { name: "Annuler" }));
    expect(navigate).toHaveBeenCalledWith("main");
  });

  it("clé absente : message et retour", () => {
    const { navigate } = setup({}, {});
    expect(screen.getByRole("alert").textContent).toContain("Aucun ami");
    fireEvent.click(screen.getByRole("button", { name: "Retour à l'accueil" }));
    expect(navigate).toHaveBeenCalledWith("main");
  });

  it("erreur puis Réessayer", async () => {
    const base = createFakeServices({ tickMs: 0 });
    const plan = vi.fn()
      .mockRejectedValueOnce(new Error("Ton ami est hors ligne."))
      .mockImplementation(() => base.joinPlan("x"));
    setup({ joinPlan: plan });
    expect(await screen.findByText("Ton ami est hors ligne.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Réessayer" }));
    expect(await screen.findByRole("heading", { level: 1, name: /Rejoindre «/ })).toBeTruthy();
    expect(plan).toHaveBeenCalledTimes(2);
  });

  it("affiche les jars non vérifiés sans les présenter comme sûrs ni exiger la confiance", async () => {
    const base = createFakeServices({ tickMs: 0 });
    setup({ joinPlan: async () => ({ ...(await base.joinPlan("x")), unknown_mods: [], unknown_unchecked: ["mods/x.jar"] }) });
    expect(await screen.findByText("mods/x.jar")).toBeTruthy();
    expect(screen.getByTestId("unchecked").textContent).toMatch(/Ne les considère pas comme sûrs/);
    expect((screen.getByRole("button", { name: "Rejoindre" }) as HTMLButtonElement).disabled).toBe(false);
  });
});
