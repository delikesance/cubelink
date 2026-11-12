import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { Shell } from "../../app/shell";
import { resetHistory } from "../../app/router";
import { createFakeServices } from "../../services/fake";

let ALEX = "";
const key = async (svc: ReturnType<typeof createFakeServices>) => { ALEX = (await svc.friendList())[0]!.friend.public_key; };

beforeEach(() => { window.location.hash = "#/friends"; resetHistory(); });
afterEach(cleanup);

describe("Invitations dans l'écran Amis", () => {
  it("rien sans invitation ; une invitation reçue apparaît avec son compteur, Ignorer la retire", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    await key(svc);
    render(<Shell services={svc} />);
    await screen.findByRole("heading", { name: "Amis", level: 1 });
    expect(screen.queryByRole("heading", { name: /Invitations/ })).toBeNull();
    act(() => { svc.emitInvite({ public_key: ALEX, world_name: "Mon monde" }); });
    expect(await screen.findByRole("heading", { name: /Invitations/ })).toBeTruthy();
    expect(screen.getByLabelText("1 invitation en attente")).toBeTruthy();
    expect(screen.getByText("Alex t'invite")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Ignorer l'invitation de Alex" }));
    await waitFor(() => expect(screen.queryByRole("heading", { name: /Invitations/ })).toBeNull());
  });

  it("Rejoindre ouvre l'écran de jointure de l'ami", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    await key(svc);
    render(<Shell services={svc} />);
    await screen.findByRole("heading", { name: "Amis", level: 1 });
    act(() => { svc.emitInvite({ public_key: ALEX }); });
    await screen.findByText("Alex t'invite");
    fireEvent.click(within(screen.getByRole("region", { name: /Invitations/ })).getByRole("button", { name: "Rejoindre Alex" }));
    expect(window.location.hash).toBe(`#/join?friend=${ALEX}`);
  });

  it("Inviter par ami seulement si je suis hôte avec un monde ouvert", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    await key(svc);
    render(<Shell services={svc} />);
    await screen.findByText("Alex");
    expect(screen.queryByRole("button", { name: "Inviter Alex" })).toBeNull();
    act(() => { svc.emitHostState({ instance_id: "inst-atm10", sharing: true, world_port: 25565 }); });
    fireEvent.click(await screen.findByRole("button", { name: "Inviter Alex" }));
    await waitFor(() => expect(svc.gameCalls).toEqual([{ kind: "invite", arg: ALEX }]));
    expect(await screen.findByText("Invitation envoyée à Alex.")).toBeTruthy();
  });
});
