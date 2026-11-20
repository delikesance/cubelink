import { act, cleanup, render, screen, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { TunnelNotice } from "./TunnelNotice";
import { tunnelMessage } from "./useTunnelState";
import { Shell } from "./shell";
import { resetHistory } from "./router";
import { createFakeServices } from "../services/fake";

// Faux service : tunnel://state est émis à la main ; on compte les abonnements actifs.
let svc: ReturnType<typeof createFakeServices>;
let active = 0;
let alex = "";
const tunnel = (state: "ok" | "reconnecting" | "lost" | "closed", key?: string) => act(() => { svc.emitTunnelState({ state, friend_public_key: key }); });

beforeEach(async () => {
  window.location.hash = "";
  resetHistory();
  svc = createFakeServices({ tickMs: 0 });
  alex = (await svc.friendList()).find((f) => f.friend.name === "Alex")!.friend.public_key;
  active = 0;
  const subscribe = svc.onTunnelState;
  svc.onTunnelState = (cb) => {
    active += 1;
    const off = subscribe(cb);
    return () => { active -= 1; off(); };
  };
});
afterEach(cleanup);

describe("tunnelMessage", () => {
  it("nomme l'ami, sinon reste générique sans clé", () => {
    expect(tunnelMessage("reconnecting", "Alex")).toMatch(/^Connexion à Alex perdue, reconnexion…/);
    expect(tunnelMessage("lost", "Alex")).toBe("Alex a fermé son monde. Relance la partie pour le rejoindre.");
    expect(tunnelMessage("reconnecting", null)).toContain("ton ami");
    expect(tunnelMessage("lost", null)).toMatch(/^Ton ami a fermé son monde/);
  });
});

describe("TunnelNotice : liaison avec l'hôte", () => {
  it("rien tant qu'aucun événement", async () => {
    render(<TunnelNotice services={svc} />);
    await waitFor(() => expect(active).toBe(1));
    expect(screen.queryByRole("status")).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("reconnecting avec le pseudo de l'ami, ok retire le bandeau", async () => {
    render(<TunnelNotice services={svc} />);
    await waitFor(() => expect(active).toBe(1));
    tunnel("reconnecting", alex);
    expect(screen.getByRole("status").textContent).toContain("reconnexion");
    await waitFor(() => expect(screen.getByRole("status").textContent).toContain("Connexion à Alex perdue"));
    expect(screen.queryByRole("button")).toBeNull();
    tunnel("ok", alex);
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("clé inconnue : message générique, jamais la clé", async () => {
    render(<TunnelNotice services={svc} />);
    await waitFor(() => expect(active).toBe(1));
    const unknown = "ab".repeat(32);
    tunnel("reconnecting", unknown);
    await act(async () => { await svc.friendList(); });
    expect(screen.getByRole("status").textContent).toContain("ton ami");
    expect(screen.getByRole("status").textContent).not.toContain(unknown);
  });

  it("lost : alerte persistante avec Fermer, qui l'écarte", async () => {
    render(<TunnelNotice services={svc} />);
    await waitFor(() => expect(active).toBe(1));
    tunnel("reconnecting", alex);
    tunnel("lost", alex);
    await waitFor(() => expect(screen.getByRole("alert").textContent).toContain("Alex a fermé son monde"));
    fireEvent.click(screen.getByRole("button", { name: "Fermer" }));
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("closed retire le bandeau (fin de partie, tunnel remplacé)", async () => {
    render(<TunnelNotice services={svc} />);
    await waitFor(() => expect(active).toBe(1));
    tunnel("reconnecting", alex);
    expect(screen.getByRole("status")).toBeTruthy();
    tunnel("closed");
    expect(screen.queryByRole("status")).toBeNull();
    tunnel("lost", alex);
    expect(screen.getByRole("alert")).toBeTruthy();
    tunnel("closed");
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("friendList en erreur : message générique", async () => {
    svc.friendList = () => Promise.reject(new Error("hors ligne"));
    render(<TunnelNotice services={svc} />);
    await waitFor(() => expect(active).toBe(1));
    tunnel("lost", alex);
    expect(screen.getByRole("alert").textContent).toContain("Ton ami a fermé son monde");
  });

  it("se désabonne au démontage", async () => {
    const { unmount } = render(<TunnelNotice services={svc} />);
    await waitFor(() => expect(active).toBe(1));
    unmount();
    expect(active).toBe(0);
  });

  it("la coquille monte le bandeau, aussi hors écran de connexion", async () => {
    render(<Shell services={svc} />);
    await waitFor(() => expect(active).toBe(1));
    tunnel("reconnecting", alex);
    expect(document.querySelector(".tunnel-notice")).not.toBeNull();
  });
});
