import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { ShellContext, type ShellContextValue } from "../../app/context";
import { createFakeServices } from "../../services/fake";
import type { Services } from "../../services";
import Main from "./index";

afterEach(cleanup);

function setup(over: Partial<Services> = {}, opts: Parameters<typeof createFakeServices>[0] = { tickMs: 0, requestCount: 3 }) {
  const services = { ...createFakeServices(opts), modsCheck: async () => ({ issues: [], checked: 0 }), ...over };
  const navigate = vi.fn();
  const value = { services, route: "main", params: {}, navigate, setTitle: () => {}, goBack: () => {}, canGoBack: false } as ShellContextValue;
  render(<ShellContext.Provider value={value}><Main /></ShellContext.Provider>);
  return { services, navigate };
}
const grid = () => screen.getByRole("group", { name: "Instances" });
const side = () => screen.getByRole("complementary", { name: "Amis" });

describe("Main", () => {
  it("affiche les modpacks en tuiles ; un clic ouvre la page du modpack", async () => {
    const { navigate } = setup();
    const tile = await within(await screen.findByRole("group", { name: "Instances" })).findByRole("button", { name: "Create: Aeronautics" });
    expect(screen.queryByRole("heading", { level: 2, name: "Create: Aeronautics" })).toBeNull();
    fireEvent.click(tile);
    expect(navigate).toHaveBeenCalledWith("instance", { id: "inst-aeronautics" });
  });

  it("focus initial sur la première tuile et flèches dans la grille", async () => {
    setup();
    await screen.findByRole("group", { name: "Instances" });
    const btns = Array.from(grid().querySelectorAll<HTMLElement>(".home-tile"));
    expect(btns.length).toBeGreaterThan(2);
    await waitFor(() => expect(document.activeElement).toBe(btns[0]));
    fireEvent.keyDown(btns[0], { key: "ArrowRight" });
    expect(document.activeElement).toBe(btns[1]);
    fireEvent.keyDown(btns[1], { key: "ArrowLeft" });
    expect(document.activeElement).toBe(btns[0]);
    fireEvent.keyDown(btns[0], { key: "ArrowDown" });
    expect(document.activeElement).toBe(btns[2]);
    fireEvent.keyDown(btns[2], { key: "End" });
    expect(document.activeElement).toBe(btns[btns.length - 1]);
  });

  it("affiche un état vide avec appel à créer", async () => {
    const { navigate } = setup({ instancesList: async () => [] });
    expect(await screen.findByRole("heading", { name: "Aucune instance" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Créer une instance" }));
    expect(navigate).toHaveBeenCalledWith("create");
  });

  it("lancement rapide : le ▶ d'une tuile lance ce modpack, devient Arrêter, un seul jeu à la fois", async () => {
    let emit: (s: { id: string; state: "running" }) => void = () => {};
    const launch = vi.fn(async () => {});
    const kill = vi.fn(async () => {});
    const { navigate } = setup({ instancesLaunch: launch, instancesKill: kill, onLaunchState: (cb) => { emit = cb; return () => {}; } });
    await screen.findByRole("group", { name: "Instances" });
    const quick = within(grid()).getAllByRole("button", { name: /^Jouer à / })[1];
    const name = quick.getAttribute("aria-label")!.replace("Jouer à ", "");
    fireEvent.click(quick);
    await waitFor(() => expect(launch).toHaveBeenCalledTimes(1));
    expect(navigate).not.toHaveBeenCalled();
    const id = (launch.mock.calls[0] as unknown as [string])[0];
    act(() => emit({ id, state: "running" }));
    fireEvent.click(within(grid()).getByRole("button", { name: `Arrêter ${name}` }));
    expect(kill).toHaveBeenCalledWith(id);
    expect(within(grid()).getAllByRole("button", { name: /^Jouer à / }).every((b) => (b as HTMLButtonElement).disabled)).toBe(true);
  });

  it("demandes d'amis : total et 99+, lien vers Paramètres > Amis", async () => {
    const { navigate } = setup(undefined, { tickMs: 0, requestCount: 1000 });
    fireEvent.click(await screen.findByRole("button", { name: "99+ demandes" }));
    expect(navigate).toHaveBeenCalledWith("settings", { tab: "amis" });
    cleanup();
    setup(undefined, { tickMs: 0, requestCount: 7 });
    expect(await screen.findByRole("button", { name: "7 demandes" })).toBeTruthy();
  });

  it("navigation : Ajouter un modpack, Ajouter un ami, paramètres de Cubelink", async () => {
    const { navigate } = setup();
    await screen.findByRole("group", { name: "Instances" });
    expect(screen.queryByRole("button", { name: "Parcourir" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Nouvelle instance" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Ajouter un modpack" }));
    expect(navigate).toHaveBeenCalledWith("create");
    fireEvent.click(within(side()).getByRole("button", { name: "Ajouter un ami" }));
    expect(navigate).toHaveBeenCalledWith("settings", { tab: "amis" });
    fireEvent.click(screen.getByRole("button", { name: "Paramètres de Cubelink" }));
    expect(navigate).toHaveBeenCalledWith("settings");
  });

  it("parties en cours : une carte par ami qui héberge, Rejoindre, ami sur la tuile du pack", async () => {
    const { services, navigate } = setup();
    await screen.findByRole("group", { name: "Instances" });
    expect(screen.getByText(/Aucune partie en cours/)).toBeTruthy();
    const alex = (await services.friendList())[0].friend;
    act(() => { (services as unknown as { emitInvite: (i: object) => void }).emitInvite({ public_key: alex.public_key, world_name: "Îles flottantes", pack: "Create: Aeronautics" }); });
    const list = await screen.findByRole("list", { name: "Parties en cours" });
    expect(within(list).getByText("Îles flottantes")).toBeTruthy();
    fireEvent.click(within(list).getByRole("button", { name: `Rejoindre ${alex.name}` }));
    expect(navigate).toHaveBeenCalledWith("join", { friend: alex.public_key });
    expect(within(grid()).getByRole("button", { name: "Create: Aeronautics" }).textContent).toContain("1 ami y joue");
    // la colonne d'amis le montre aussi en partie
    expect(within(side()).getByText("Joue à Create: Aeronautics")).toBeTruthy();
  });

  it("plus de quatre parties : trois cartes et « +N autres parties » sélectionne l'onglet En partie", async () => {
    const now = Math.floor(Date.now() / 1000);
    const inv = (k: number) => ({ id: `i${k}`, public_key: `k${k}`, name: `Ami${k}`, world_name: `Monde ${k}`, minecraft: "1.21.1", received_at: now - k, expires_at: now + 600 });
    setup({ friendInvites: async () => [1, 2, 3, 4, 5, 6].map(inv) });
    const list = await screen.findByRole("list", { name: "Parties en cours" });
    await waitFor(() => expect(within(list).getAllByRole("button", { name: /^Rejoindre / }).length).toBe(3));
    expect(within(side()).getByRole("tab", { name: /^Tous/ }).getAttribute("aria-selected")).toBe("true");
    fireEvent.click(within(list).getByRole("button", { name: "+3 autres parties" }));
    expect(within(side()).getByRole("tab", { name: /^En partie/ }).getAttribute("aria-selected")).toBe("true");
  });

  it("colonne d'amis permanente : liste, onglets, recherche, Demander puis Rejoindre", async () => {
    const { navigate, services } = setup();
    const ctl = services as unknown as { setJoinAnswer: (a: string) => void; joinRequestCalls: Array<{ kind: string; arg: string }> };
    ctl.setJoinAnswer("accepted");
    await screen.findByRole("group", { name: "Instances" });
    const list = within(side()).getByRole("list", { name: "Amis" });
    await waitFor(() => expect(within(list).getAllByRole("listitem").length).toBeGreaterThan(1));
    // Un ami en ligne sans invitation : on ne le rejoint pas directement, on le demande.
    expect(within(list).queryByRole("button", { name: /^Rejoindre / })).toBeNull();
    fireEvent.click(within(list).getByRole("button", { name: /^Demander à rejoindre / }));
    // Demande acceptée : le bouton devient Rejoindre.
    fireEvent.click(await within(list).findByRole("button", { name: /^Rejoindre / }));
    expect(ctl.joinRequestCalls.map((c) => c.kind)).toEqual(["ask"]);
    expect(navigate).toHaveBeenCalledWith("join", expect.objectContaining({ friend: expect.any(String) }));
    fireEvent.click(within(side()).getByRole("tab", { name: /^En ligne/ }));
    expect(within(side()).getByRole("tab", { name: /^En ligne/ }).getAttribute("aria-selected")).toBe("true");
    fireEvent.change(within(side()).getByRole("searchbox"), { target: { value: "zzz" } });
    expect(within(side()).getByText("Aucun ami ne correspond.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Tous mes amis/ })).toBeNull();
  });

  it("amis hors ligne : dernière connexion sous le nom", async () => {
    const now = Math.floor(Date.now() / 1000);
    setup({ friendList: async () => [{ friend: { public_key: "k1", name: "Noor", trust: "always", added_at: now }, presence: "offline", last_seen: now - 3 * 3600 }] });
    await screen.findByRole("group", { name: "Instances" });
    expect(await within(side()).findByText("Vu il y a 3 h")).toBeTruthy();
  });

  it("met à jour la présence des amis en direct", async () => {
    let push: (e: { public_key: string; presence: "online" | "offline" }) => void = () => {};
    const base = createFakeServices({ tickMs: 0, requestCount: 0 });
    setup({ onFriendPresence: (cb) => { push = cb; return () => {}; } });
    await screen.findByRole("group", { name: "Instances" });
    await waitFor(() => expect(screen.getByText(/1 en ligne/)).toBeTruthy());
    const list = await base.friendList();
    const noor = list.find((f) => f.presence === "offline")!.friend;
    act(() => push({ public_key: noor.public_key, presence: "online" }));
    expect(screen.getByText(/2 en ligne/)).toBeTruthy();
  });

  it("demande à rejoindre : « Demande envoyée… » pendant l'attente, puis refusée : on peut redemander", async () => {
    const { services } = setup();
    const ctl = services as unknown as { setJoinAnswer: (a: string) => void; resolveJoinRequest: (k: string, a: string) => void };
    ctl.setJoinAnswer("hold");
    await screen.findByRole("group", { name: "Instances" });
    const alex = (await services.friendList())[0].friend;
    const list = within(side()).getByRole("list", { name: "Amis" });
    fireEvent.click(await within(list).findByRole("button", { name: `Demander à rejoindre ${alex.name}` }));
    const pending = await within(list).findByRole("button", { name: `Demander à rejoindre ${alex.name}` });
    expect(pending.textContent).toContain("Demande envoyée…");
    expect(pending.getAttribute("aria-busy")).toBe("true");
    act(() => ctl.resolveJoinRequest(alex.public_key, "refused"));
    await waitFor(() => expect(within(list).getByText("Demande refusée")).toBeTruthy());
    expect(within(list).getByRole("button", { name: `Demander à rejoindre ${alex.name}` }).textContent).toBe("Demander");
    expect(within(list).queryByRole("button", { name: `Rejoindre ${alex.name}` })).toBeNull();
  });

  it("demande à rejoindre qui échoue : le message d'erreur s'affiche", async () => {
    setup({ friendJoinRequest: async () => { throw new Error("Ton ami n'est pas en ligne. Il doit ouvrir Cubelink."); } });
    await screen.findByRole("group", { name: "Instances" });
    const list = within(side()).getByRole("list", { name: "Amis" });
    fireEvent.click(await within(list).findByRole("button", { name: /^Demander à rejoindre / }));
    expect(await screen.findByText(/n'est pas en ligne/)).toBeTruthy();
  });
});
