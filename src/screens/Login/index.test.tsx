import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Shell } from "../../app/shell";
import { createFakeServices } from "../../services/fake";
import type { Services } from "../../services/types";

// Arrivée volontaire sur Connexion (déconnexion) : pas de contrôle de session au démarrage.
beforeEach(() => { window.location.hash = "#/login"; });
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

  it("« Obtenir un nouveau code » relance une demande de code", async () => {
    const base = createFakeServices({ tickMs: 0, loginDelayMs: 500 });
    const begin = vi.fn(base.authBeginLogin);
    render(<Shell services={{ ...base, authBeginLogin: begin }} />);
    expect(screen.queryByRole("button", { name: "Obtenir un nouveau code" })).toBeNull();
    expect(screen.queryByRole("button", { name: /Problème lors de la connexion/ })).toBeNull();
    fireEvent.click(btn());
    fireEvent.click(await screen.findByRole("button", { name: "Obtenir un nouveau code" }));
    await waitFor(() => expect(begin).toHaveBeenCalledTimes(2));
  });

  /** Services dont le sondage ne répond qu'à la demande : permet de simuler une réponse tardive. */
  function lateServices() {
    const base = createFakeServices({ tickMs: 0 });
    let finish: (p: Awaited<ReturnType<Services["authPollLogin"]>>) => void = () => {};
    const svc: Services = {
      ...base,
      authBeginLogin: async () => ({ user_code: "ABCD-1234", verification_uri: "https://x.test", expires_in: 900 }),
      authPollLogin: () => new Promise((resolve) => { finish = resolve; }),
    };
    return { svc, finish: (p: Awaited<ReturnType<Services["authPollLogin"]>>) => finish(p) };
  }
  const profile: Awaited<ReturnType<Services["authPollLogin"]>> = { kind: "offline", name: "Alex", uuid: "00000000-0000-0000-0000-000000000000" };

  async function expectBackAtWelcome(finish: (p: typeof profile) => void) {
    expect(await screen.findByRole("button", { name: /Se connecter avec Microsoft/ })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Jouer hors ligne" })).toBeTruthy();
    expect(screen.queryByLabelText(/^Code :/)).toBeNull();
    // Une réponse tardive du sondage ne ramène ni l'écran « Connecté » ni l'accueil.
    await act(async () => { finish(profile); await Promise.resolve(); });
    expect(screen.queryByText(/^Bienvenue /)).toBeNull();
    await new Promise((r) => setTimeout(r, 50));
    expect(window.location.hash).toBe("#/login");
  }

  it("Annuler sur l'écran du code : retour à l'accueil, hors ligne accessible, réponse tardive ignorée", async () => {
    const { svc, finish } = lateServices();
    render(<Shell services={svc} />);
    fireEvent.click(btn());
    expect(await screen.findByLabelText(/^Code :/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Annuler" }));
    await expectBackAtWelcome(finish);
    expect(document.activeElement?.tagName).toBe("H1");
    fireEvent.click(screen.getByRole("button", { name: "Jouer hors ligne" }));
    expect(screen.getByLabelText("Pseudo hors ligne")).toBeTruthy();
  });

  it("Échap sur l'écran du code fait la même chose qu'Annuler", async () => {
    const { svc, finish } = lateServices();
    render(<Shell services={svc} />);
    fireEvent.click(btn());
    expect(await screen.findByLabelText(/^Code :/)).toBeTruthy();
    fireEvent.keyDown(document, { key: "Escape" });
    await expectBackAtWelcome(finish);
  });

  it("Annuler prévient Rust (authCancelLogin), pour qu'une validation tardive n'ajoute aucun compte", async () => {
    const { svc } = lateServices();
    const cancelLogin = vi.fn(async () => {});
    render(<Shell services={{ ...svc, authCancelLogin: cancelLogin }} />);
    fireEvent.click(btn());
    expect(await screen.findByLabelText(/^Code :/)).toBeTruthy();
    expect(cancelLogin).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Annuler" }));
    expect(cancelLogin).toHaveBeenCalledTimes(1);
  });

  it("Échap pendant la demande du code (réseau lent) quitte l'attente ; un second clic ne redemande pas de code", async () => {
    const base = createFakeServices({ tickMs: 0 });
    let release: (c: { user_code: string; verification_uri: string; expires_in: number }) => void = () => {};
    const begin = vi.fn(() => new Promise<{ user_code: string; verification_uri: string; expires_in: number }>((r) => { release = r; }));
    const cancelLogin = vi.fn(async () => {});
    render(<Shell services={{ ...base, authBeginLogin: begin, authCancelLogin: cancelLogin }} />);
    fireEvent.click(btn());
    fireEvent.click(btn());
    expect(begin).toHaveBeenCalledTimes(1);
    fireEvent.keyDown(document, { key: "Escape" });
    expect(cancelLogin).toHaveBeenCalledTimes(1);
    // La réponse tardive de la demande de code ne rouvre pas l'écran du code.
    await act(async () => { release({ user_code: "ZZZZ-0000", verification_uri: "https://x.test", expires_in: 900 }); await Promise.resolve(); });
    expect(screen.queryByLabelText(/^Code :/)).toBeNull();
    expect(screen.getByRole("button", { name: "Jouer hors ligne" })).toBeTruthy();
  });

  it("Échap déjà pris en charge par un autre composant (defaultPrevented) n'annule pas", async () => {
    const { svc } = lateServices();
    render(<Shell services={svc} />);
    fireEvent.click(btn());
    expect(await screen.findByLabelText(/^Code :/)).toBeTruthy();
    const taken = (e: KeyboardEvent) => e.preventDefault();
    document.addEventListener("keydown", taken, true);
    fireEvent.keyDown(document, { key: "Escape" });
    document.removeEventListener("keydown", taken, true);
    expect(screen.getByLabelText(/^Code :/)).toBeTruthy();
  });

  it("Échap sur l'accueil (rien à annuler) ne change rien", async () => {
    render(<Shell services={createFakeServices({ tickMs: 0 })} />);
    fireEvent.keyDown(document, { key: "Escape" });
    expect(btn()).toBeTruthy();
    expect(window.location.hash).toBe("#/login");
  });

  it("Annuler sur l'écran d'échec : retour à l'accueil", async () => {
    const base = createFakeServices({ tickMs: 0, loginDelayMs: 20 });
    const svc: Services = { ...base, authPollLogin: () => Promise.reject(new Error("Compte sans droit daccès.")) };
    render(<Shell services={svc} />);
    fireEvent.click(btn());
    expect((await screen.findByRole("alert")).textContent).toContain("Compte sans droit daccès.");
    fireEvent.click(screen.getByRole("button", { name: "Annuler" }));
    expect(await screen.findByRole("button", { name: "Jouer hors ligne" })).toBeTruthy();
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
    // Un code expiré s'annule aussi (Annuler ou Échap) : retour à l'accueil.
    fireEvent.keyDown(document, { key: "Escape" });
    expect(screen.getByRole("button", { name: "Jouer hors ligne" })).toBeTruthy();
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

  it("hors ligne : toujours proposé, même si offlineAvailable répond faux", async () => {
    const base = createFakeServices({ tickMs: 0 });
    render(<Shell services={{ ...base, offlineAvailable: async () => false }} />);
    expect(await screen.findByRole("button", { name: "Jouer hors ligne" })).toBeTruthy();
  });

  it("hors ligne : pseudo invalide refusé avec un message, valide accepté", async () => {
    const base = createFakeServices({ tickMs: 0 });
    const offline = vi.fn(base.authOfflineLogin);
    render(<Shell services={{ ...base, authOfflineLogin: offline }} />);
    fireEvent.click(await screen.findByRole("button", { name: "Jouer hors ligne" }));
    const field = screen.getByLabelText("Pseudo hors ligne");
    for (const bad of ["ab", "pseudo avec espace", "x".repeat(17), "é€"]) {
      fireEvent.change(field, { target: { value: bad } });
      fireEvent.click(screen.getByRole("button", { name: "Jouer" }));
      expect((await screen.findByRole("alert")).textContent).toMatch(/3 à 16/);
    }
    expect(offline).not.toHaveBeenCalled();
    fireEvent.change(field, { target: { value: "Bob_42" } });
    fireEvent.click(screen.getByRole("button", { name: "Jouer" }));
    await waitFor(() => expect(offline).toHaveBeenCalledWith("Bob_42"));
  });

  it("session expirée : message discret sous le bouton Microsoft", async () => {
    window.location.hash = "#/login?notice=expired";
    render(<Shell services={createFakeServices({ tickMs: 0 })} />);
    expect((await screen.findByRole("status")).textContent).toBe("Session expirée, reconnecte-toi.");
  });
});
