import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Shell } from "./shell";
import { createFakeServices } from "../services/fake";
import type { AccountInfo } from "../services/types";

const STEVE: AccountInfo = { id: "ms-1", kind: "online", name: "Steve", uuid: "8667ba71-b85a-4004-af54-457a9734eed7" };
const NOOR: AccountInfo = { id: "off-noor", kind: "offline", name: "Noor_", uuid: "00000000-0000-3000-8000-000000000005" };
const svc2 = () => createFakeServices({ tickMs: 0, loginDelayMs: 0, accounts: [STEVE, NOOR] });

afterEach(() => { cleanup(); vi.restoreAllMocks(); });

describe("démarrage", () => {
  beforeEach(() => { window.location.hash = ""; });

  it("affiche un chargement puis l'accueil quand une session est conservée (sans écran de bienvenue)", async () => {
    render(<Shell services={svc2()} />);
    expect(screen.getByRole("status").textContent).toBe("Chargement…");
    expect(screen.queryByRole("button", { name: /Se connecter avec Microsoft/ })).toBeNull();
    await waitFor(() => expect(window.location.hash).toBe("#/main"));
    expect(screen.queryByText(/Bienvenue/)).toBeNull();
  });

  it("sans session : l'écran Connexion, sans message d'expiration", async () => {
    render(<Shell services={createFakeServices({ tickMs: 0 })} />);
    expect(await screen.findByRole("button", { name: /Se connecter avec Microsoft/ })).toBeTruthy();
    expect(screen.queryByText(/Session expirée/)).toBeNull();
    expect(window.location.hash).not.toContain("expired");
  });

  it("renouvellement échoué alors qu'un compte existe : Connexion + « Session expirée »", async () => {
    const svc = svc2();
    svc.authStatus = async () => null;
    render(<Shell services={svc} />);
    expect(await screen.findByText("Session expirée, reconnecte-toi.")).toBeTruthy();
    expect(screen.getByRole("button", { name: /Se connecter avec Microsoft/ })).toBeTruthy();
  });

  it("authStatus qui rejette est traité comme une absence de session", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    svc.authStatus = async () => { throw new Error("boom"); };
    render(<Shell services={svc} />);
    expect(await screen.findByRole("button", { name: /Se connecter avec Microsoft/ })).toBeTruthy();
  });

  it("arrivée volontaire sur #/login : aucun contrôle de session", async () => {
    window.location.hash = "#/login";
    const svc = svc2();
    const status = vi.spyOn(svc, "authStatus");
    render(<Shell services={svc} />);
    expect(screen.getByRole("button", { name: /Se connecter avec Microsoft/ })).toBeTruthy();
    expect(status).not.toHaveBeenCalled();
  });
});

describe("sélecteur de compte", () => {
  beforeEach(() => { window.location.hash = "#/main"; });

  it("en deux clics : ouvrir puis choisir un autre compte", async () => {
    const svc = svc2();
    const sw = vi.spyOn(svc, "authSwitch");
    render(<Shell services={svc} />);
    const trigger = await screen.findByRole("button", { name: /Compte actif : Steve/ });
    fireEvent.click(trigger);
    const menu = screen.getByRole("menu", { name: "Changer de compte" });
    const items = within(menu).getAllByRole("menuitemradio");
    expect(items.map((i) => i.getAttribute("aria-checked"))).toEqual(["true", "false"]);
    expect(within(menu).getByText("Hors ligne")).toBeTruthy();
    fireEvent.click(items[1]);
    await waitFor(() => expect(sw).toHaveBeenCalledWith("off-noor"));
    expect(await screen.findByRole("button", { name: /Compte actif : Noor_/ })).toBeTruthy();
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("clavier : flèches, Échap referme et rend le focus", async () => {
    render(<Shell services={svc2()} />);
    const trigger = await screen.findByRole("button", { name: /Compte actif/ });
    fireEvent.click(trigger);
    const items = screen.getAllByRole("menuitemradio");
    await waitFor(() => expect(document.activeElement).toBe(items[0]));
    fireEvent.keyDown(items[0], { key: "ArrowDown" });
    expect(document.activeElement).toBe(items[1]);
    fireEvent.keyDown(items[1], { key: "Escape" });
    expect(screen.queryByRole("menu")).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });

  it("« Gérer les comptes » ouvre Réglages ; absent sur l'écran Connexion", async () => {
    render(<Shell services={svc2()} />);
    fireEvent.click(await screen.findByRole("button", { name: /Compte actif/ }));
    fireEvent.click(screen.getByRole("menuitem", { name: /Gérer les comptes/ }));
    await waitFor(() => expect(window.location.hash).toBe("#/settings?tab=compte"));
    act(() => { window.location.hash = "#/login"; });
    await waitFor(() => expect(screen.queryByRole("button", { name: /Compte actif/ })).toBeNull());
  });
});

describe("Réglages > Comptes", () => {
  beforeEach(() => { window.location.hash = "#/settings"; });

  it("liste, utiliser un compte, retirer avec confirmation (annulable)", async () => {
    const svc = svc2();
    const rm = vi.spyOn(svc, "authRemove");
    render(<Shell services={svc} />);
    const list = await screen.findByRole("list");
    expect(within(list).getAllByRole("listitem")).toHaveLength(2);
    fireEvent.click(screen.getByRole("button", { name: "Utiliser Noor_" }));
    await waitFor(() => expect(screen.queryByRole("button", { name: "Utiliser Noor_" })).toBeNull());
    expect(screen.getByRole("button", { name: "Utiliser Steve" })).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Retirer Steve" }));
    fireEvent.click(screen.getByRole("button", { name: "Annuler" }));
    expect(rm).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Retirer Steve" }));
    fireEvent.click(screen.getByRole("button", { name: "Confirmer le retrait de Steve" }));
    await waitFor(() => expect(rm).toHaveBeenCalledWith("ms-1"));
    await waitFor(() => expect(within(screen.getByRole("list")).getAllByRole("listitem")).toHaveLength(1));
    expect(window.location.hash).toBe("#/settings");
  });

  it("ajout hors ligne : pseudo validé puis compte listé", async () => {
    render(<Shell services={svc2()} />);
    await screen.findByRole("list");
    fireEvent.click(screen.getByRole("button", { name: "Ajouter un compte hors ligne" }));
    fireEvent.change(screen.getByLabelText("Pseudo hors ligne"), { target: { value: "a!" } });
    fireEvent.click(screen.getByRole("button", { name: "Ajouter" }));
    expect((await screen.findByRole("alert")).textContent).toMatch(/3 à 16/);
    fireEvent.change(screen.getByLabelText("Pseudo hors ligne"), { target: { value: "Creeper_9" } });
    fireEvent.click(screen.getByRole("button", { name: "Ajouter" }));
    await waitFor(() => expect(within(screen.getByRole("list")).getByText("Creeper_9")).toBeTruthy());
    expect(screen.getByText("Compte Creeper_9 ajouté.")).toBeTruthy();
    expect(screen.queryByLabelText("Pseudo hors ligne")).toBeNull();
  });

  it("ajout Microsoft : code d'appareil affiché puis compte ajouté", async () => {
    const svc = createFakeServices({ tickMs: 0, loginDelayMs: 50, accounts: [NOOR] });
    render(<Shell services={svc} />);
    await screen.findByRole("list");
    fireEvent.click(screen.getByRole("button", { name: "Ajouter un compte Microsoft" }));
    expect(await screen.findByLabelText(/^Code :/)).toBeTruthy();
    expect(screen.getByRole("link", { name: /^Ouvrir / })).toBeTruthy();
    await waitFor(() => expect(within(screen.getByRole("list")).getByText("Steve")).toBeTruthy());
    expect(screen.queryByLabelText(/^Code :/)).toBeNull();
  });
});
