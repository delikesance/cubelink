import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Shell } from "../../app/shell";
import { createFakeServices } from "../../services/fake";
import type { Services } from "../../services/types";

beforeEach(() => { window.location.hash = ""; });
afterEach(() => { cleanup(); vi.useRealTimers(); });

const btn = () => screen.getByRole("button", { name: /Se connecter avec Microsoft/ });

describe("écran Connexion", () => {
  it("affichage initial", () => {
    render(<Shell services={createFakeServices({ tickMs: 0 })} />);
    expect(btn()).toBeTruthy();
    expect(screen.getByRole("link", { name: "Mentions légales" }).getAttribute("href")).toBe("#/legal");
  });

  it("flux réussi : code affiché puis navigation vers main", async () => {
    const svc = createFakeServices({ tickMs: 0, loginDelayMs: 800 });
    render(<Shell services={svc} />);
    fireEvent.click(btn());
    const expected = await svc.authBeginLogin();
    expect((await screen.findByLabelText(/^Code :/)).textContent).toBe(expected.user_code);
    expect(screen.getByTestId("countdown").textContent).toMatch(/^\d+:\d\d$/);
    expect(screen.getByRole("link", { name: /^Ouvrir / })).toBeTruthy();
    // Connecté : « Bienvenue » puis accueil.
    expect((await screen.findByText(/^Bienvenue /)).textContent).toMatch(/Bienvenue \S+/);
    await waitFor(() => expect(window.location.hash).toBe("#/main"), { timeout: 3000 });
  });

  it.each([
    ["ce compte ne possède pas Minecraft Java", "no-license"],
    ["connexion refusée par l'utilisateur", "cancelled"],
    ["réseau injoignable (offline)", "offline"],
  ])("échec « %s » : écran Erreur %s", async (msg, state) => {
    const base = createFakeServices({ tickMs: 0, loginDelayMs: 20 });
    const svc: Services = { ...base, authPollLogin: () => Promise.reject(new Error(msg)) };
    render(<Shell services={svc} />);
    fireEvent.click(btn());
    await waitFor(() => expect(window.location.hash).toBe(`#/error?state=${state}`));
  });

  it("« Problème lors de la connexion ? » relance une demande de code", async () => {
    const base = createFakeServices({ tickMs: 0, loginDelayMs: 500 });
    const begin = vi.fn(base.authBeginLogin);
    render(<Shell services={{ ...base, authBeginLogin: begin }} />);
    expect(screen.queryByRole("button", { name: /Problème lors de la connexion/ })).toBeNull();
    fireEvent.click(btn());
    fireEvent.click(await screen.findByRole("button", { name: /Problème lors de la connexion/ }));
    await waitFor(() => expect(begin).toHaveBeenCalledTimes(2));
  });

  it("flux en échec : message clair et Réessayer", async () => {
    const base = createFakeServices({ tickMs: 0, loginDelayMs: 20 });
    let calls = 0;
    const svc: Services = {
      ...base,
      authPollLogin: () => (++calls === 1 ? Promise.reject(new Error("Compte sans droit daccès.")) : base.authPollLogin()),
    };
    render(<Shell services={svc} />);
    fireEvent.click(btn());
    expect((await screen.findByRole("alert")).textContent).toContain("Compte sans droit daccès.");
    fireEvent.click(screen.getByRole("button", { name: "Réessayer" }));
    await waitFor(() => expect(window.location.hash).toBe("#/main"), { timeout: 3000 });
  });

  it("expiration du code", async () => {
    vi.useFakeTimers();
    const base = createFakeServices({ tickMs: 0 });
    const svc: Services = {
      ...base,
      authBeginLogin: async () => ({ user_code: "ABCD-1234", verification_uri: "https://x.test", expires_in: 3 }),
      authPollLogin: () => new Promise(() => {}),
    };
    render(<Shell services={svc} />);
    fireEvent.click(btn());
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    expect(screen.getByLabelText(/^Code :/).textContent).toBe("ABCD-1234");
    await act(async () => { await vi.advanceTimersByTimeAsync(3100); });
    expect(screen.getByRole("alert").textContent).toContain("expiré");
    expect(screen.getByRole("button", { name: "Réessayer" })).toBeTruthy();
  });

  it("jouer hors ligne : pseudo requis puis accueil", async () => {
    const base = createFakeServices({ tickMs: 0 });
    const offline = vi.fn(base.authOfflineLogin);
    render(<Shell services={{ ...base, authOfflineLogin: offline }} />);
    const toggle = await screen.findByRole("button", { name: "Jouer hors ligne" });
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(toggle);
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    fireEvent.click(screen.getByRole("button", { name: "Jouer" }));
    expect((await screen.findByRole("alert")).textContent).toContain("pseudo");
    expect(offline).not.toHaveBeenCalled();
    fireEvent.change(screen.getByLabelText("Pseudo hors ligne"), { target: { value: "Alex" } });
    fireEvent.click(screen.getByRole("button", { name: "Jouer" }));
    await waitFor(() => expect(offline).toHaveBeenCalledWith("Alex"));
    await waitFor(() => expect(window.location.hash).toBe("#/main"));
  });

  it("hors ligne indisponible (application normale) : le bouton n'est pas proposé", async () => {
    const base = createFakeServices({ tickMs: 0 });
    render(<Shell services={{ ...base, offlineAvailable: async () => false }} />);
    await screen.findByRole("button", { name: "Se connecter avec Microsoft" });
    await new Promise((r) => setTimeout(r, 20));
    expect(screen.queryByRole("button", { name: "Jouer hors ligne" })).toBeNull();
  });
});
