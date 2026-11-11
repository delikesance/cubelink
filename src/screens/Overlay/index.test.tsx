import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Shell } from "../../app/shell";
import { resetHistory } from "../../app/router";
import { createFakeServices } from "../../services/fake";

beforeEach(() => { window.location.hash = "#/overlay"; resetHistory(); });
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

let ALEX = "";
let NOOR = "";

async function ready(svc: ReturnType<typeof createFakeServices>) {
  const l = await svc.friendList();
  ALEX = l[0]!.friend.public_key;
  NOOR = l[1]!.friend.public_key;
  render(<Shell services={svc} />);
  await screen.findByText("Alex");
  await screen.findByText("All the Mods 10");
}

describe("route overlay", () => {
  it("n'affiche ni barre de titre ni bannière de mise à jour, fond transparent, focus sur le panneau", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    await ready(svc);
    expect(document.querySelector(".titlebar")).toBeNull();
    expect(document.querySelector(".update-banner")).toBeNull();
    expect(document.documentElement.hasAttribute("data-overlay")).toBe(true);
    expect(screen.getByRole("heading", { name: "Cubelink", level: 1 })).toBeTruthy();
    expect(document.activeElement).toBe(screen.getByRole("heading", { name: "Cubelink", level: 1 }));
    expect(document.querySelector("[data-tauri-drag-region]")).not.toBeNull();
  });

  it("le bouton fermer et Échap appellent overlayToggle", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    const toggle = vi.spyOn(svc, "overlayToggle");
    await ready(svc);
    fireEvent.click(screen.getByRole("button", { name: "Fermer le panneau" }));
    expect(toggle).toHaveBeenCalledTimes(1);
    fireEvent.keyDown(document, { key: "Escape" });
    expect(toggle).toHaveBeenCalledTimes(2);
  });
});

describe("invitations", () => {
  it("sans invitation : message vide", async () => {
    await ready(createFakeServices({ tickMs: 0 }));
    expect(screen.getByText("Aucune invitation en attente.")).toBeTruthy();
  });

  it("une invitation reçue s'affiche (ami, monde, version) ; Ignorer l'écarte", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    const dismiss = vi.spyOn(svc, "friendInviteDismiss");
    await ready(svc);
    act(() => { svc.emitInvite({ public_key: ALEX, world_name: "Mon monde", minecraft: "1.20.1" }); });
    const list = await screen.findByRole("list", { name: "Invitations" });
    expect(within(list).getByText("Alex")).toBeTruthy();
    expect(within(list).getByText("Mon monde · 1.20.1")).toBeTruthy();
    fireEvent.click(within(list).getByRole("button", { name: "Ignorer l'invitation de Alex" }));
    await waitFor(() => expect(screen.getByText("Aucune invitation en attente.")).toBeTruthy());
    expect(dismiss).toHaveBeenCalledWith("inv-1");
  });

  it("Rejoindre sans jeu en cours : appel direct, sans confirmation", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    await ready(svc);
    act(() => { svc.emitInvite({ public_key: ALEX }); });
    const list = await screen.findByRole("list", { name: "Invitations" });
    fireEvent.click(within(list).getByRole("button", { name: "Rejoindre Alex" }));
    await waitFor(() => expect(svc.gameCalls).toEqual([{ kind: "join", arg: ALEX, instanceId: undefined }]));
    expect(screen.queryByRole("group", { name: "Confirmation" })).toBeNull();
  });

  it("Rejoindre avec un jeu en cours : confirmation, Annuler n'appelle rien, Confirmer appelle gameJoinFriend", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    await ready(svc);
    act(() => { svc.emitOverlayState({ game_running: true }); });
    act(() => { svc.emitInvite({ public_key: ALEX }); });
    const list = await screen.findByRole("list", { name: "Invitations" });
    fireEvent.click(within(list).getByRole("button", { name: "Rejoindre Alex" }));
    const box = screen.getByRole("group", { name: "Confirmation" });
    expect(box.textContent).toContain("Fermer le jeu en cours et rejoindre Alex ?");
    expect(document.activeElement).toBe(within(box).getByRole("button", { name: "Annuler" }));
    fireEvent.click(within(box).getByRole("button", { name: "Annuler" }));
    expect(svc.gameCalls).toEqual([]);
    fireEvent.click(within(list).getByRole("button", { name: "Rejoindre Alex" }));
    fireEvent.click(within(screen.getByRole("group", { name: "Confirmation" })).getByRole("button", { name: "Fermer et rejoindre" }));
    await waitFor(() => expect(svc.gameCalls).toEqual([{ kind: "join", arg: ALEX, instanceId: undefined }]));
    expect(screen.queryByRole("group", { name: "Confirmation" })).toBeNull();
  });

  it("Échap annule la confirmation sans fermer le panneau", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    const toggle = vi.spyOn(svc, "overlayToggle");
    await ready(svc);
    act(() => { svc.emitOverlayState({ game_running: true }); });
    fireEvent.click(screen.getByRole("button", { name: "Lancer All the Mods 10" }));
    expect(screen.getByRole("group", { name: "Confirmation" })).toBeTruthy();
    fireEvent.keyDown(document, { key: "Escape" });
    expect(screen.queryByRole("group", { name: "Confirmation" })).toBeNull();
    expect(toggle).not.toHaveBeenCalled();
  });
});

describe("amis", () => {
  it("Inviter n'est visible que si je suis hôte avec un monde ouvert", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    await ready(svc);
    expect(screen.queryByRole("button", { name: "Inviter Alex" })).toBeNull();
    act(() => { svc.emitHostState({ instance_id: "inst-atm10", sharing: true, world_port: null }); });
    expect(screen.queryByRole("button", { name: "Inviter Alex" })).toBeNull();
    act(() => { svc.emitHostState({ instance_id: "inst-atm10", sharing: true, world_port: 25565 }); });
    const btn = await screen.findByRole("button", { name: "Inviter Alex" });
    expect(screen.queryByRole("button", { name: "Inviter Noor" })).toBeNull(); // Noor est hors ligne
    fireEvent.click(btn);
    await waitFor(() => expect(svc.gameCalls).toEqual([{ kind: "invite", arg: ALEX }]));
    act(() => { svc.emitHostState({ instance_id: "inst-atm10", sharing: false, world_port: null }); });
    expect(screen.queryByRole("button", { name: "Inviter Alex" })).toBeNull();
  });

  it("badge « Héberge » et bouton Rejoindre seulement pour un ami en ligne qui héberge", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    await ready(svc);
    // « Héberge » est déduit d'une invitation non expirée.
    act(() => { svc.emitInvite({ public_key: ALEX }); svc.emitInvite({ public_key: NOOR }); }); // Noor hors ligne : ignoré
    await screen.findByRole("list", { name: "Invitations" });
    const list = screen.getByRole("list", { name: "Amis" });
    expect(within(list).getAllByText("Héberge")).toHaveLength(1);
    fireEvent.click(within(list).getByRole("button", { name: "Rejoindre Alex" }));
    await waitFor(() => expect(svc.gameCalls).toEqual([{ kind: "join", arg: ALEX, instanceId: undefined }]));
    expect(within(list).queryByRole("button", { name: "Rejoindre Noor" })).toBeNull();
  });

  it("une invitation expirée ne compte pas comme hébergement", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    await ready(svc);
    act(() => { svc.emitInvite({ public_key: ALEX, expires_at: 1 }); });
    await screen.findByRole("list", { name: "Invitations" });
    expect(screen.queryByText("Héberge")).toBeNull();
    expect(within(screen.getByRole("list", { name: "Amis" })).queryByRole("button", { name: "Rejoindre Alex" })).toBeNull();
  });

  it("sans invitation : ni badge ni Rejoindre", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    await ready(svc);
    expect(screen.queryByText("Héberge")).toBeNull();
    expect(screen.queryByRole("button", { name: "Rejoindre Alex" })).toBeNull();
  });
});

describe("modpacks", () => {
  it("sans jeu en cours : Lancer appelle gameSwitch directement", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    await ready(svc);
    fireEvent.click(screen.getByRole("button", { name: "Lancer Prominence II" }));
    await waitFor(() => expect(svc.gameCalls).toEqual([{ kind: "switch", arg: "inst-prominence" }]));
  });

  it("avec un jeu en cours : confirmation « Fermer le jeu et lancer … ? » puis gameSwitch", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    await ready(svc);
    act(() => { svc.emitOverlayState({ game_running: true }); });
    fireEvent.click(screen.getByRole("button", { name: "Lancer Prominence II" }));
    const box = screen.getByRole("group", { name: "Confirmation" });
    expect(box.textContent).toContain("Fermer le jeu et lancer Prominence II ?");
    expect(box.textContent).toContain("Les changements non sauvegardés seront perdus");
    expect(svc.gameCalls).toEqual([]);
    fireEvent.click(within(box).getByRole("button", { name: "Fermer et lancer" }));
    await waitFor(() => expect(svc.gameCalls).toEqual([{ kind: "switch", arg: "inst-prominence" }]));
  });

  it("une erreur de commande s'affiche en alerte", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    vi.spyOn(svc, "gameSwitch").mockRejectedValue(new Error("Impossible de fermer le jeu."));
    await ready(svc);
    fireEvent.click(screen.getByRole("button", { name: "Lancer Prominence II" }));
    expect((await screen.findByRole("alert")).textContent).toBe("Impossible de fermer le jeu.");
  });
});

describe("plein écran exclusif", () => {
  it("affiche un avertissement discret", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    await ready(svc);
    expect(screen.queryByRole("note")).toBeNull();
    act(() => { svc.emitOverlayState({ fullscreen_exclusive: true }); });
    expect(screen.getByRole("note").textContent).toContain("En plein écran exclusif, le panneau peut être masqué");
  });
});
