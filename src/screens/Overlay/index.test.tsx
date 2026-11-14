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

  it("ami qui héberge (invitation reçue) : Rejoindre directement, sans Demander", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    const alex = (await svc.friendList())[0].friend;
    svc.emitInvite({ public_key: alex.public_key });
    mount(svc);
    const friends = await screen.findByRole("list", { name: "Amis" });
    expect(await within(friends).findByRole("button", { name: `Rejoindre ${alex.name}` })).toBeTruthy();
    expect(within(friends).queryByRole("button", { name: `Demander à rejoindre ${alex.name}` })).toBeNull();
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
