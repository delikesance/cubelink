import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { ShellContext, type ShellContextValue } from "../../app/context";
import { createFakeServices } from "../../services/fake";
import type { Services } from "../../services";
import Main from "./index";

afterEach(cleanup);

function setup(over: Partial<Services> = {}, opts: Parameters<typeof createFakeServices>[0] = { tickMs: 0, requestCount: 3 }) {
  const services = { ...createFakeServices(opts), ...over };
  const navigate = vi.fn();
  const value = { services, route: "main", params: {}, navigate, setTitle: () => {}, goBack: () => {}, canGoBack: false } as ShellContextValue;
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

  it("focus initial sur Jouer et flèches haut/bas dans la barre d'instances", async () => {
    setup();
    const play = await screen.findByRole("button", { name: "Jouer" });
    await waitFor(() => expect(document.activeElement).toBe(play));
    const rail = screen.getByRole("navigation", { name: "Instances" });
    const btns = Array.from(rail.querySelectorAll<HTMLElement>(".home-rail-btn"));
    btns[0].focus();
    fireEvent.keyDown(btns[0], { key: "ArrowDown" });
    expect(document.activeElement).toBe(btns[1]);
    fireEvent.keyDown(btns[1], { key: "ArrowUp" });
    expect(document.activeElement).toBe(btns[0]);
    fireEvent.keyDown(btns[0], { key: "End" });
    expect(document.activeElement).toBe(btns[btns.length - 1]);
  });

  it("affiche un état vide avec appel à créer", async () => {
    const { navigate } = setup({ instancesList: async () => [] });
    expect(await screen.findByRole("heading", { name: "Aucune instance" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Jouer" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Créer une instance" }));
    expect(navigate).toHaveBeenCalledWith("create");
  });

  it("Jouer lance l'instance et suit launch://state", async () => {
    let emit: (s: { id: string; state: "running" | "failed" | "exited"; message?: string }) => void = () => {};
    const launch = vi.fn(async () => {});
    setup({
      instancesLaunch: launch,
      onLaunchState: (cb) => { emit = cb; return () => {}; },
    });
    fireEvent.click(await screen.findByRole("button", { name: "Jouer" }));
    expect(launch).toHaveBeenCalledWith("inst-aeronautics");
    expect(screen.getByText("Préparation du jeu…")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Annuler le lancement" })).toBeTruthy();
    act(() => emit({ id: "inst-aeronautics", state: "running" }));
    expect(screen.getByRole("button", { name: "Arrêter" })).toBeTruthy();
    expect(screen.getByText("En jeu")).toBeTruthy();
    expect(screen.getByText(/session/)).toBeTruthy();
    act(() => emit({ id: "inst-aeronautics", state: "failed", message: "Java introuvable" }));
    const alert = screen.getByRole("alert");
    expect(alert.textContent).toContain("Échec du lancement");
    expect(alert.textContent).toContain("Java introuvable");
    expect((screen.getByRole("button", { name: "Jouer" }) as HTMLButtonElement).disabled).toBe(false);
    act(() => emit({ id: "inst-aeronautics", state: "exited" }));
    expect(screen.queryByText("Minecraft est fermé.")).toBeNull();
    expect((screen.getByRole("button", { name: "Jouer" }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("affiche l'échec si instancesLaunch est rejeté", async () => {
    setup({ instancesLaunch: async () => { throw new Error("Instance introuvable."); } });
    fireEvent.click(await screen.findByRole("button", { name: "Jouer" }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Instance introuvable.");
    expect(within(alert).getByRole("button", { name: "Voir les logs" })).toBeTruthy();
  });

  it("badge de demandes d'amis : total et 99+", async () => {
    setup(undefined, { tickMs: 0, requestCount: 1000 });
    const btn = await screen.findByRole("button", { name: /^Amis \(99\+ demandes\)/ });
    expect(btn.textContent).toContain("99+");
    cleanup();
    setup(undefined, { tickMs: 0, requestCount: 7 });
    await waitFor(() => expect(screen.getByRole("button", { name: /7 demandes/ }).textContent).toContain("7"));
  });

  it("navigation : nouvelle instance, paramètres de Cubelink", async () => {
    const { navigate } = setup();
    await screen.findByRole("button", { name: "Jouer" });
    fireEvent.click(screen.getByRole("button", { name: "Nouvelle instance" }));
    expect(navigate).toHaveBeenCalledWith("create");
    fireEvent.click(screen.getByRole("button", { name: "Paramètres de Cubelink" }));
    expect(navigate).toHaveBeenCalledWith("settings");
  });

  it("la pastille Amis ouvre le tiroir : Rejoindre un ami en ligne, accès à l'écran Amis", async () => {
    const { navigate } = setup();
    await screen.findByRole("button", { name: "Jouer" });
    fireEvent.click(await screen.findByRole("button", { name: /^Amis/ }));
    const dlg = await screen.findByRole("dialog", { name: /^Amis · \d+/ });
    const join = within(dlg).getAllByRole("button", { name: /^Rejoindre / })[0];
    fireEvent.click(join);
    expect(navigate).toHaveBeenCalledWith("join", expect.objectContaining({ friend: expect.any(String) }));
    fireEvent.click(within(dlg).getByRole("button", { name: /^Amis/ }));
    expect(navigate).toHaveBeenCalledWith("friends");
  });

  it("met à jour la présence des amis en direct", async () => {
    let push: (e: { public_key: string; presence: "online" | "offline" }) => void = () => {};
    const base = createFakeServices({ tickMs: 0, requestCount: 0 });
    setup({ onFriendPresence: (cb) => { push = cb; return () => {}; } });
    const pill = await screen.findByRole("button", { name: /^Amis/ });
    await waitFor(() => expect(pill.textContent).toMatch(/en ligne/));
    const list = await base.friendList();
    const before = pill.textContent;
    const noor = list.find((f) => f.presence === "offline")!.friend;
    act(() => push({ public_key: noor.public_key, presence: "online" }));
    expect(pill.textContent).not.toBe(before);
  });
});
