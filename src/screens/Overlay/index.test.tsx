import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { ShellContext, type ShellContextValue } from "../../app/context";
import { navigate, goBack } from "../../app/router";
import { createFakeServices } from "../../services/fake";
import InGamePanel from ".";

afterEach(cleanup);

const mount = (svc: ReturnType<typeof createFakeServices>) => {
  const ctx: ShellContextValue = { services: svc, route: "main", params: {}, navigate, goBack, canGoBack: false, setTitle: () => {} };
  render(<ShellContext.Provider value={ctx}><InGamePanel /></ShellContext.Provider>);
};

describe("Panneau en jeu : invités", () => {
  const hosted = async () => createFakeServices({ tickMs: 0 });
  /** Le monde est ouvert : l'événement host://state suit le montage. */
  const openWorld = async (svc: ReturnType<typeof createFakeServices>) => {
    await screen.findByRole("list", { name: "Amis" }); // la lecture initiale de host_state est passée
    act(() => svc.emitHostState({ sharing: true, world_port: 25565 } as never));
  };

  it("après une invitation : Inviter grisé « Invité · … », ligne « Peuvent rejoindre », Retirer la fait disparaître", async () => {
    const svc = await hosted();
    mount(svc);
    await openWorld(svc);
    const alex = (await svc.friendList())[0].friend;
    await act(() => svc.friendInvite(alex.public_key));
    const btn = await screen.findByRole("button", { name: new RegExp(`^Invitation envoyée à ${alex.name}, expire dans [45] min$`) });
    expect(btn.textContent).toMatch(/^Invité · [45] min$/);
    expect(btn.getAttribute("aria-disabled")).toBe("true");
    expect((btn as HTMLButtonElement).disabled).toBe(false); // reste focalisable
    const list = await screen.findByRole("list", { name: "Peuvent rejoindre" });
    expect(within(list).getByText(/^Invité · [45] min$/)).toBeTruthy();
    fireEvent.click(within(list).getByRole("button", { name: `Retirer l'accès de ${alex.name}` }));
    await waitFor(() => expect(svc.revokeCalls).toEqual([alex.public_key]));
    await waitFor(() => expect(screen.queryByRole("list", { name: "Peuvent rejoindre" })).toBeNull());
    expect((await screen.findByRole("button", { name: `Inviter ${alex.name}` })).getAttribute("aria-disabled")).toBeNull();
  });

  it("invité entré : « A rejoint », et Inviter reste possible (il a pu quitter la partie)", async () => {
    const svc = await hosted();
    const alex = (await svc.friendList())[0].friend;
    svc.setHostGuests([{ public_key: alex.public_key, origin: "request", entered: true, expires_at: null }]);
    mount(svc);
    await openWorld(svc);
    const list = await screen.findByRole("list", { name: "Peuvent rejoindre" });
    expect(within(list).getByText("A rejoint")).toBeTruthy();
    expect(screen.getByRole("button", { name: `Inviter ${alex.name}` }).getAttribute("aria-disabled")).toBeNull();
  });

  it("réglage open_to_all_friends actif : badge « Monde ouvert à tous tes amis »", async () => {
    const svc = await hosted();
    await svc.settingsSet({ ...(await svc.settingsGet()), open_to_all_friends: true });
    mount(svc);
    await screen.findByRole("list", { name: "Amis" });
    expect(screen.queryByText("Monde ouvert à tous tes amis"), "pas de badge sans monde ouvert").toBeNull();
    await openWorld(svc);
    expect(await screen.findByText("Monde ouvert à tous tes amis")).toBeTruthy();
  });

  it("Retirer qui échoue : le message s'affiche", async () => {
    const svc = await hosted();
    const alex = (await svc.friendList())[0].friend;
    svc.setHostGuests([{ public_key: alex.public_key, origin: "all_friends", entered: false, expires_at: Math.floor(Date.now() / 1000) + 200 }]);
    svc.hostGuestRevoke = async () => { throw new Error("Impossible de retirer l'accès."); };
    mount(svc);
    fireEvent.click(await screen.findByRole("button", { name: `Retirer l'accès de ${alex.name}` }));
    expect(await screen.findByRole("alert")).toBeTruthy();
  });
});

describe("Panneau en jeu : modpack en cours", () => {
  it("le modpack lancé a un bouton rouge Fermer qui ferme le jeu après confirmation ; les autres gardent Lancer", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    await svc.instancesLaunch("inst-atm10");
    mount(svc);
    const close = await screen.findByRole("button", { name: "Fermer All the Mods 10" });
    expect(close.className).toContain("cl-btn-danger");
    expect(screen.queryByRole("button", { name: "Lancer All the Mods 10" })).toBeNull();
    expect(screen.getByRole("button", { name: "Lancer Create: Aeronautics" })).toBeTruthy();

    fireEvent.click(close);
    expect(svc.gameCalls).toEqual([]);
    fireEvent.click(screen.getByRole("button", { name: "Fermer le jeu" }));
    await waitFor(() => expect(svc.gameCalls).toEqual([{ kind: "close", arg: "inst-atm10" }]));
    // le jeu terminé : le bouton redevient Lancer
    expect(await screen.findByRole("button", { name: "Lancer All the Mods 10" })).toBeTruthy();
  });

  it("sans jeu en cours : uniquement des boutons Lancer", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    mount(svc);
    await screen.findByRole("button", { name: "Lancer All the Mods 10" });
    expect(screen.queryByRole("button", { name: "Fermer All the Mods 10" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Fermer Create: Aeronautics" })).toBeNull();
  });

  it("ami en ligne sans invitation : Demander, puis Rejoindre une fois la demande acceptée", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    mount(svc);
    const alex = (await svc.friendList())[0].friend;
    fireEvent.click(await screen.findByRole("button", { name: `Demander à rejoindre ${alex.name}` }));
    expect(await screen.findByRole("button", { name: `Rejoindre ${alex.name}` })).toBeTruthy();
    expect(svc.joinRequestCalls).toEqual([{ kind: "ask", arg: alex.public_key }]);
    expect(screen.queryByRole("button", { name: `Demander à rejoindre ${alex.name}` })).toBeNull();
  });

  it("ami en ligne mais pas en jeu : ni Demander ni Rejoindre", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    const alex = (await svc.friendList())[0].friend;
    const base = svc.friendList;
    svc.friendList = async () => (await base()).map((f) => ({ ...f, in_game: false }));
    mount(svc);
    const friends = await screen.findByRole("list", { name: "Amis" });
    await within(friends).findByText(alex.name);
    expect(within(friends).queryByRole("button", { name: `Demander à rejoindre ${alex.name}` })).toBeNull();
    expect(within(friends).queryByRole("button", { name: `Rejoindre ${alex.name}` })).toBeNull();
  });

  it("ami qui héberge (invitation reçue) : Rejoindre directement, sans Demander", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    const alex = (await svc.friendList())[0].friend;
    svc.emitInvite({ public_key: alex.public_key });
    mount(svc);
    const friends = await screen.findByRole("list", { name: "Amis" });
    expect(await within(friends).findByRole("button", { name: `Rejoindre ${alex.name}` })).toBeTruthy();
    expect(within(friends).queryByRole("button", { name: `Demander à rejoindre ${alex.name}` })).toBeNull();
  });

  it("invitation reçue : Ignorer la retire du panneau et de la file (seul endroit où le bouton existe)", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    const alex = (await svc.friendList())[0].friend;
    svc.emitInvite({ public_key: alex.public_key });
    mount(svc);
    const list = await screen.findByRole("list", { name: "Invitations" });
    fireEvent.click(within(list).getByRole("button", { name: `Ignorer l'invitation de ${alex.name}` }));
    await waitFor(() => expect(screen.queryByRole("list", { name: "Invitations" })).toBeNull());
    expect(await svc.friendInvites()).toEqual([]);
  });

  it("hôte : la demande d'un ami apparaît avec Accepter / Refuser, et disparaît une fois décidée", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    mount(svc);
    const alex = (await svc.friendList())[0].friend;
    await screen.findByText("Amis · 1 en ligne");
    expect(screen.queryByRole("list", { name: "Demandes à rejoindre" })).toBeNull();
    act(() => { svc.emitJoinRequest({ public_key: alex.public_key }); });
    const list = await screen.findByRole("list", { name: "Demandes à rejoindre" });
    expect(list.textContent).toContain(alex.name);
    fireEvent.click(screen.getByRole("button", { name: `Accepter la demande de ${alex.name}` }));
    await waitFor(() => expect(screen.queryByRole("list", { name: "Demandes à rejoindre" })).toBeNull());
    expect(svc.joinRequestCalls).toEqual([{ kind: "answer", arg: alex.public_key, accept: true }]);

    act(() => { svc.emitJoinRequest({ public_key: alex.public_key }); });
    fireEvent.click(await screen.findByRole("button", { name: `Refuser la demande de ${alex.name}` }));
    await waitFor(() => expect(svc.joinRequestCalls.at(-1)).toEqual({ kind: "answer", arg: alex.public_key, accept: false }));
    await waitFor(() => expect(screen.queryByRole("list", { name: "Demandes à rejoindre" })).toBeNull());
  });

  it("hôte : une demande qui n'est plus valable affiche l'erreur", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    mount(svc);
    const alex = (await svc.friendList())[0].friend;
    act(() => { svc.emitJoinRequest({ public_key: alex.public_key }); });
    await screen.findByRole("list", { name: "Demandes à rejoindre" });
    svc.friendJoinRequestAnswer = async () => { throw new Error("Cette demande n'est plus valable."); };
    fireEvent.click(screen.getByRole("button", { name: `Accepter la demande de ${alex.name}` }));
    expect((await screen.findByRole("alert")).textContent).toContain("n'est plus valable");
  });
});
