import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { Shell } from "./shell";
import { JoinRequestBanner, joinBannerVisible } from "./JoinRequestBanner";
import { createFakeServices } from "../services/fake";
import type { StageState } from "../services";

const DETACHED: StageState = { attached: false, game: false, panel: false };
beforeEach(() => { window.location.hash = "#/main"; });
afterEach(cleanup);

describe("joinBannerVisible", () => {
  it("masqué sans demande, et seulement quand le panneau « en jeu » est ouvert sur le jeu", () => {
    expect(joinBannerVisible(DETACHED, 0)).toBe(false);
    expect(joinBannerVisible(DETACHED, 1)).toBe(true);
    expect(joinBannerVisible({ attached: true, game: true, panel: false }, 1)).toBe(true);
    expect(joinBannerVisible({ attached: true, game: true, panel: true }, 1)).toBe(false);
    // repli ou Scène sans jeu : le panneau n'est pas monté, le bandeau est la seule trace
    expect(joinBannerVisible({ attached: false, game: false, panel: true }, 1)).toBe(true);
    expect(joinBannerVisible({ attached: true, game: false, panel: true }, 1)).toBe(true);
  });
});

describe("Bandeau des demandes à rejoindre (hors panneau « en jeu »)", () => {
  it("sans Scène : la demande s'affiche, Accepter et Refuser répondent, la ligne disparaît", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    render(<JoinRequestBanner services={svc} stage={DETACHED} />);
    const alex = (await svc.friendList())[0].friend;
    expect(screen.queryByRole("region", { name: "Demandes à rejoindre" })).toBeNull();
    act(() => { svc.emitJoinRequest({ public_key: alex.public_key }); });
    const region = await screen.findByRole("region", { name: "Demandes à rejoindre" });
    expect(region.textContent).toContain(alex.name);
    expect(region.textContent).toContain("Veut rejoindre ta partie");
    fireEvent.click(screen.getByRole("button", { name: `Accepter la demande de ${alex.name}` }));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Demandes à rejoindre" })).toBeNull());
    expect(svc.joinRequestCalls).toEqual([{ kind: "answer", arg: alex.public_key, accept: true }]);

    act(() => { svc.emitJoinRequest({ public_key: alex.public_key }); });
    fireEvent.click(await screen.findByRole("button", { name: `Refuser la demande de ${alex.name}` }));
    await waitFor(() => expect(svc.joinRequestCalls.at(-1)).toEqual({ kind: "answer", arg: alex.public_key, accept: false }));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Demandes à rejoindre" })).toBeNull());
  });

  it("demande déjà expirée côté hôte : l'erreur s'affiche et les boutons restent utilisables", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    render(<JoinRequestBanner services={svc} stage={DETACHED} />);
    const alex = (await svc.friendList())[0].friend;
    act(() => { svc.emitJoinRequest({ public_key: alex.public_key }); });
    await screen.findByRole("region", { name: "Demandes à rejoindre" });
    // le faux service refuse la réponse quand la demande n'est plus dans sa file
    svc.friendJoinRequestAnswer = async () => { throw new Error("Cette demande n'est plus valable."); };
    fireEvent.click(screen.getByRole("button", { name: `Accepter la demande de ${alex.name}` }));
    expect((await screen.findByRole("alert")).textContent).toContain("plus valable");
    expect((screen.getByRole("button", { name: `Refuser la demande de ${alex.name}` }) as HTMLButtonElement).disabled).toBe(false);
  });
});

describe("Coque : le bandeau ne double pas le panneau « en jeu »", () => {
  async function mount() {
    const svc = createFakeServices({ tickMs: 0, stageSupported: true });
    render(<Shell services={svc} />);
    await waitFor(() => expect(svc.stageCalls.some((c) => c.cmd === "layout")).toBe(true));
    return svc;
  }
  const lines = (name: string) => screen.queryAllByRole("button", { name });

  it("Scène absente : une seule ligne, dans le bandeau", async () => {
    const svc = await mount();
    const alex = (await svc.friendList())[0].friend;
    act(() => { svc.emitJoinRequest({ public_key: alex.public_key }); });
    await screen.findByRole("region", { name: "Demandes à rejoindre" });
    expect(lines(`Accepter la demande de ${alex.name}`)).toHaveLength(1);
  });

  it("jeu affiché, panneau fermé : le bandeau reste ; panneau ouvert : une seule ligne, dans le panneau", async () => {
    const svc = await mount();
    const alex = (await svc.friendList())[0].friend;
    act(() => svc.emitStageState({ attached: true, game: true, panel: false }));
    await act(async () => {}); // le panneau a fini sa lecture initiale des demandes
    act(() => { svc.emitJoinRequest({ public_key: alex.public_key }); });
    await screen.findByRole("region", { name: "Demandes à rejoindre" });
    act(() => svc.emitStageState({ attached: true, game: true, panel: true }));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Demandes à rejoindre" })).toBeNull());
    expect(lines(`Accepter la demande de ${alex.name}`)).toHaveLength(1);
    expect(screen.getByRole("list", { name: "Demandes à rejoindre" })).toBeTruthy();
  });

  it("repli de la Scène : le jeu a sa fenêtre, le bandeau reprend la demande", async () => {
    const svc = await mount();
    const alex = (await svc.friendList())[0].friend;
    act(() => svc.emitStageState({ attached: true, game: true, panel: true }));
    await act(async () => {});
    act(() => { svc.emitJoinRequest({ public_key: alex.public_key }); });
    expect(screen.queryByRole("region", { name: "Demandes à rejoindre" })).toBeNull();
    act(() => svc.emitStageState({ attached: false, game: false, panel: false }));
    await screen.findByRole("region", { name: "Demandes à rejoindre" });
    expect(lines(`Accepter la demande de ${alex.name}`)).toHaveLength(1);
  });
});
