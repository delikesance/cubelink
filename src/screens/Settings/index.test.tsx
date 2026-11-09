import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Shell } from "../../app/shell";
import { createFakeServices } from "../../services/fake";
import { validateJavaPath, validateRam } from "./index";

beforeEach(() => { window.location.hash = "#/settings"; });
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

const relay = () => screen.getByLabelText(/Passer toujours par le relais/) as HTMLInputElement;

describe("validation", () => {
  it("mémoire 512-65536 et Java facultatif", () => {
    expect(validateRam(511)).toMatch(/512/);
    expect(validateRam(65537)).toMatch(/65536/);
    expect(validateRam(4096)).toBeNull();
    expect(validateJavaPath("")).toBeNull();
    expect(validateJavaPath("C:\\Java\\bin\\java.exe")).toBeNull();
    expect(validateJavaPath("a\nb")).toMatch(/invalides/);
  });
});

describe("écran Paramètres", () => {
  it("dialogue, 4 sections, flèches du clavier, liens", async () => {
    render(<Shell services={createFakeServices({ tickMs: 0 })} />);
    expect(screen.getByRole("dialog", { name: "Paramètres" })).toBeTruthy();
    const tabs = screen.getAllByRole("tab");
    expect(tabs.map((t) => t.textContent)).toEqual(["Réseau P2P", "Codes", "Cache et partage", "Crédits et licences"]);
    tabs[0].focus();
    fireEvent.keyDown(tabs[0], { key: "ArrowDown" });
    expect(tabs[1].getAttribute("aria-selected")).toBe("true");
    expect(document.activeElement).toBe(tabs[1]);
    expect(screen.getByRole("heading", { name: "Codes", level: 2 })).toBeTruthy();
    expect(screen.getAllByRole("link", { name: /Mentions légales/ })[0].getAttribute("href")).toBe("#/legal");
    fireEvent.click(screen.getByRole("button", { name: "Fermer les paramètres" }));
    await waitFor(() => expect(window.location.hash).toBe("#/main"));
  });

  it("enregistre à la modification, annonce « Enregistré » et persiste après rechargement", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    const first = render(<Shell services={svc} />);
    await waitFor(() => expect(relay().disabled).toBe(false));
    expect(relay().checked).toBe(false);
    fireEvent.click(relay());
    await waitFor(() => expect(screen.getAllByRole("status").some((s) => s.textContent === "Enregistré")).toBe(true));
    first.unmount();
    render(<Shell services={svc} />);
    await waitFor(() => expect(relay().checked).toBe(true));
  });

  it("échec d'enregistrement : message d'erreur", async () => {
    const base = createFakeServices({ tickMs: 0 });
    render(<Shell services={{ ...base, settingsSet: () => Promise.reject(new Error("Disque plein.")) }} />);
    await waitFor(() => expect(relay().disabled).toBe(false));
    fireEvent.click(relay());
    expect((await screen.findByRole("alert")).textContent).toBe("Disque plein.");
  });

  it("vider le cache demande confirmation et peut être annulé", () => {
    render(<Shell services={createFakeServices({ tickMs: 0 })} />);
    fireEvent.click(screen.getByRole("tab", { name: "Cache et partage" }));
    fireEvent.click(screen.getByRole("button", { name: /Vider le cache…/ }));
    expect(screen.getByRole("alertdialog")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Annuler" }));
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

describe("Échap", () => {
  it("ferme le dialogue", () => {
    render(<Shell services={createFakeServices({ tickMs: 0 })} />);
    fireEvent.keyDown(document.body, { key: "Escape" });
    expect(window.location.hash).toBe("#/main");
  });

  it("alertdialog : focus initial sur Annuler, Échap ferme seulement la confirmation", () => {
    render(<Shell services={createFakeServices({ tickMs: 0 })} />);
    fireEvent.click(screen.getByRole("tab", { name: "Cache et partage" }));
    fireEvent.click(screen.getByRole("button", { name: /Vider le cache…/ }));
    const ad = screen.getByRole("alertdialog");
    expect(ad.contains(document.activeElement)).toBe(true);
    expect((document.activeElement as HTMLElement).textContent).toBe("Annuler");
    fireEvent.keyDown(document.activeElement as HTMLElement, { key: "Escape" });
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(window.location.hash).toBe("#/settings");
  });
});
});
