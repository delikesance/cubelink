import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { ShellContext, type ShellContextValue } from "../../app/context";
import { createFakeServices } from "../../services/fake";
import type { Services } from "../../services";
import Main from "./index";

afterEach(cleanup);

function setup(over: Partial<Services> = {}, opts: Parameters<typeof createFakeServices>[0] = { tickMs: 0, requestCount: 3 }) {
  const services = { ...createFakeServices(opts), ...over };
  const navigate = vi.fn();
  const value = { services, route: "main", params: {}, navigate, setTitle: () => {} } as ShellContextValue;
  render(<ShellContext.Provider value={value}><Main /></ShellContext.Provider>);
  return { services, navigate };
}

describe("Main", () => {
  it("affiche la liste des instances et sélectionne la première", async () => {
    setup();
    expect(await screen.findByRole("heading", { level: 1, name: "Create: Aeronautics" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Jouer" })).toBeTruthy();
    expect(screen.getAllByRole("button", { pressed: true }).length).toBeGreaterThan(0);
  });

  it("affiche un état vide avec appel à créer", async () => {
    const { navigate } = setup({ instancesList: async () => [] });
    expect(await screen.findByRole("heading", { name: "Aucune instance" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Jouer" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Créer une instance" }));
    expect(navigate).toHaveBeenCalledWith("create");
  });

  it("Jouer lance l'instance et suit launch://state", async () => {
    let emit: (s: { id: string; state: "running" | "failed"; message?: string }) => void = () => {};
    const launch = vi.fn(async () => {});
    setup({
      instancesLaunch: launch,
      onLaunchState: (cb) => { emit = cb; return () => {}; },
    });
    fireEvent.click(await screen.findByRole("button", { name: "Jouer" }));
    expect(launch).toHaveBeenCalledWith("inst-aeronautics");
    expect(screen.getByText("Préparation du jeu…")).toBeTruthy();
    act(() => emit({ id: "inst-aeronautics", state: "running" }));
    expect(screen.getByText("Minecraft est en cours d'exécution.")).toBeTruthy();
    act(() => emit({ id: "inst-aeronautics", state: "failed", message: "Java introuvable" }));
    expect(screen.getByText("Échec du lancement : Java introuvable")).toBeTruthy();
    expect((screen.getByRole("button", { name: "Jouer" }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("affiche l'échec si instancesLaunch est rejeté", async () => {
    setup({ instancesLaunch: async () => { throw new Error("Instance introuvable."); } });
    fireEvent.click(await screen.findByRole("button", { name: "Jouer" }));
    expect(await screen.findByText("Échec du lancement : Instance introuvable.")).toBeTruthy();
  });

  it("badge de demandes d'amis : total et 99+", async () => {
    setup(undefined, { tickMs: 0, requestCount: 1000 });
    const btn = await screen.findByRole("button", { name: /^Amis \(99\+ demandes\)/ });
    expect(btn.textContent).toContain("99+");
    cleanup();
    setup(undefined, { tickMs: 0, requestCount: 7 });
    await waitFor(() => expect(screen.getByRole("button", { name: /7 demandes/ }).textContent).toContain("7"));
  });

  it("navigation : nouvelle instance, amis, paramètres", async () => {
    const { navigate } = setup();
    await screen.findByRole("button", { name: "Jouer" });
    fireEvent.click(screen.getByRole("button", { name: "Nouvelle instance" }));
    expect(navigate).toHaveBeenCalledWith("create");
    fireEvent.click(screen.getByRole("button", { name: /^Amis/ }));
    expect(navigate).toHaveBeenCalledWith("friends");
    fireEvent.click(screen.getByRole("button", { name: "Paramètres de Cubelink" }));
    expect(navigate).toHaveBeenCalledWith("settings");
  });

  it("met à jour la présence des amis en direct", async () => {
    let push: (e: { public_key: string; presence: "online" | "offline" }) => void = () => {};
    const base = createFakeServices({ tickMs: 0, requestCount: 0 });
    setup({ onFriendPresence: (cb) => { push = cb; return () => {}; } });
    await screen.findByRole("heading", { level: 1, name: "Create: Aeronautics" });
    const list = await base.friendList();
    const noor = list.find((f) => f.presence === "offline")!.friend;
    const before = document.querySelector(".home-friends")!.textContent;
    act(() => push({ public_key: noor.public_key, presence: "online" }));
    expect(document.querySelector(".home-friends")!.textContent).not.toBe(before);
  });
});
