import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Shell } from "../../app/shell";
import { createFakeServices } from "../../services/fake";
import { validateJavaPath, validateRam } from "./index";

beforeEach(() => { window.location.hash = "#/settings"; });
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

const relay = () => screen.getByLabelText(/Passer toujours par le relais/) as HTMLInputElement;
const goTab = (name: string) => fireEvent.click(screen.getByRole("tab", { name }));

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
  it("titre, 5 onglets, flèches du clavier, lien Mentions légales", () => {
    render(<Shell services={createFakeServices({ tickMs: 0 })} />);
    expect(screen.getByRole("heading", { name: "Paramètres", level: 1 })).toBeTruthy();
    const tabs = screen.getAllByRole("tab");
    expect(tabs.map((t) => t.textContent)).toEqual(["Compte", "Réseau", "Java", "Stockage", "À propos"]);
    expect(tabs[0].getAttribute("aria-selected")).toBe("true");
    tabs[0].focus();
    fireEvent.keyDown(tabs[0], { key: "ArrowRight" });
    expect(tabs[1].getAttribute("aria-selected")).toBe("true");
    expect(document.activeElement).toBe(tabs[1]);
    expect(screen.getAllByRole("tabpanel")).toHaveLength(1);
    fireEvent.keyDown(tabs[1], { key: "End" });
    expect(tabs[4].getAttribute("aria-selected")).toBe("true");
    expect(screen.getByRole("link", { name: "Mentions légales" }).getAttribute("href")).toBe("#/legal");
  });

  it("l'onglet initial vient du paramètre tab", () => {
    window.location.hash = "#/settings?tab=java";
    render(<Shell services={createFakeServices({ tickMs: 0 })} />);
    expect(screen.getByRole("tab", { name: "Java" }).getAttribute("aria-selected")).toBe("true");
  });

  it("Compte : un compte hors ligne est listé comme actif ; retirer le dernier ramène à la connexion", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    await svc.authOfflineLogin("Alex");
    const out = vi.spyOn(svc, "authRemove");
    render(<Shell services={svc} />);
    const list = await screen.findByRole("list");
    expect(within(list).getByText("Alex")).toBeTruthy();
    expect(within(list).getByText("Hors ligne")).toBeTruthy();
    expect(within(list).getByText("Actif")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Retirer Alex" }));
    fireEvent.click(screen.getByRole("button", { name: "Confirmer le retrait de Alex" }));
    await waitFor(() => expect(out).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(window.location.hash).toBe("#/login"));
  });

  it("Java : mémoire par défaut validée puis enregistrée à la sortie du champ", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    const set = vi.spyOn(svc, "settingsSet");
    render(<Shell services={svc} />);
    goTab("Java");
    const ram = screen.getByLabelText("Mémoire par défaut (Mo)") as HTMLInputElement;
    await waitFor(() => expect(ram.disabled).toBe(false));
    fireEvent.change(ram, { target: { value: "100" } });
    fireEvent.blur(ram);
    expect((await screen.findByRole("alert")).textContent).toMatch(/512/);
    expect(set).not.toHaveBeenCalled();
    fireEvent.change(ram, { target: { value: "6144" } });
    fireEvent.blur(ram);
    await waitFor(() => expect(set).toHaveBeenCalledTimes(1));
    expect(set.mock.calls[0]![0].default_ram_mb).toBe(6144);
  });

  it("enregistre à la modification, annonce « Enregistré » et persiste après rechargement", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    const first = render(<Shell services={svc} />);
    goTab("Réseau");
    await waitFor(() => expect(relay().disabled).toBe(false));
    expect(relay().checked).toBe(false);
    fireEvent.click(relay());
    await waitFor(() => expect(screen.getAllByRole("status").some((s) => s.textContent === "Enregistré")).toBe(true));
    first.unmount();
    render(<Shell services={svc} />);
    goTab("Réseau");
    await waitFor(() => expect(relay().checked).toBe(true));
  });

  it("échec d'enregistrement : message d'erreur", async () => {
    const base = createFakeServices({ tickMs: 0 });
    render(<Shell services={{ ...base, settingsSet: () => Promise.reject(new Error("Disque plein.")) }} />);
    goTab("Réseau");
    await waitFor(() => expect(relay().disabled).toBe(false));
    fireEvent.click(relay());
    expect((await screen.findByRole("alert")).textContent).toBe("Disque plein.");
  });

  it("vider le cache demande confirmation et peut être annulé", () => {
    render(<Shell services={createFakeServices({ tickMs: 0 })} />);
    fireEvent.click(screen.getByRole("tab", { name: "Stockage" }));
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
    fireEvent.click(screen.getByRole("tab", { name: "Stockage" }));
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

describe("découverte des amis", () => {
  it("case cochée par défaut, décochable, aide mentionnant le redémarrage", async () => {
    render(<Shell services={createFakeServices({ tickMs: 0 })} />);
    goTab("Réseau");
    const box = (await screen.findByLabelText(/Retrouver mes amis même si leur adresse change/)) as HTMLInputElement;
    expect(box.checked).toBe(true);
    expect(screen.getByText(/Appliqué au prochain démarrage/)).toBeTruthy();
    fireEvent.click(box);
    await waitFor(() => expect(box.checked).toBe(false));
  });
});

describe("ouverture automatique au réseau local", () => {
  it("case cochée par défaut, envoie tous les champs sans écraser discovery", async () => {
    const set = vi.fn(async (x) => x);
    const services = { ...createFakeServices({ tickMs: 0 }), settingsSet: set };
    render(<Shell services={services} />);
    goTab("Réseau");
    const box = (await screen.findByLabelText(/Ouvrir mon monde au réseau local automatiquement/)) as HTMLInputElement;
    expect(box.checked).toBe(true);
    fireEvent.click(box);
    await waitFor(() => expect(set).toHaveBeenCalled());
    expect(set.mock.calls[0][0]).toMatchObject({ lan_auto: false, discovery: true, share_mods: true });
  });
});
