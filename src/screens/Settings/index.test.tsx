import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Shell } from "../../app/shell";
import { createFakeServices } from "../../services/fake";
import { netVerdict, validateJavaPath, validateRam } from "./index";
import type { NetTest } from "../../services/types";

beforeEach(() => { window.location.hash = "#/settings"; });
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

const lan = () => screen.getByLabelText(/Ouvrir mon monde au réseau local automatiquement/) as HTMLInputElement;
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

describe("test de connexion", () => {
  const base: NetTest = { ipv6: false, port_mapped: true };
  it("verdicts : joignable par IPv6 ou par un port ouvert, sinon peu joignable (aucun relais)", () => {
    const v = netVerdict(base);
    expect([v.tone, v.label]).toEqual(["ok", "Joignable"]);
    expect(v.checks.map((c) => [c.label, c.ok])).toEqual([["Adresse IPv6 publique", false], ["Port ouvert sur la box", true]]);
    const lbl = (t: Partial<NetTest>) => { const r = netVerdict({ ...base, ...t }); return `${r.tone}:${r.label}`; };
    expect(lbl({ ipv6: true, port_mapped: false })).toBe("ok:Joignable");
    expect(lbl({ ipv6: false, port_mapped: false })).toBe("warn:Peu joignable");
    const w = netVerdict({ ipv6: false, port_mapped: false });
    expect(w.summary).toMatch(/même réseau local/);
    expect(JSON.stringify(w)).not.toMatch(/relais de secours|n0/i);
  });

  it("le bouton lance le vrai test et affiche le verdict", async () => {
    const services = createFakeServices({ tickMs: 0 });
    const spy = vi.spyOn(services, "netTest").mockResolvedValue({ ipv6: false, port_mapped: false });
    window.location.hash = "#/settings";
    render(<Shell services={services} />);
    goTab("Réseau");
    fireEvent.click(await screen.findByRole("button", { name: "Tester ma connexion" }));
    expect(await screen.findByText("Peu joignable")).toBeTruthy();
    expect(screen.getByText(/non ouvert/)).toBeTruthy();
    expect(spy).toHaveBeenCalledTimes(1);
  });

  it("une erreur est affichée telle quelle", async () => {
    const services = createFakeServices({ tickMs: 0 });
    vi.spyOn(services, "netTest").mockRejectedValue(new Error("Le réseau entre amis n'est pas démarré."));
    window.location.hash = "#/settings";
    render(<Shell services={services} />);
    goTab("Réseau");
    fireEvent.click(await screen.findByRole("button", { name: "Tester ma connexion" }));
    expect(await screen.findByText("Le réseau entre amis n'est pas démarré.")).toBeTruthy();
  });
});

describe("écran Paramètres", () => {
  it("titre, 6 onglets, flèches du clavier, lien Mentions légales", () => {
    render(<Shell services={createFakeServices({ tickMs: 0 })} />);
    expect(screen.getByRole("heading", { name: "Paramètres", level: 1 })).toBeTruthy();
    const tabs = screen.getAllByRole("tab");
    expect(tabs.map((t) => t.textContent)).toEqual(["Compte", "Amis", "Réseau", "Java", "Stockage", "À propos"]);
    expect(tabs[0].getAttribute("aria-selected")).toBe("true");
    tabs[0].focus();
    fireEvent.keyDown(tabs[0], { key: "ArrowRight" });
    expect(tabs[1].getAttribute("aria-selected")).toBe("true");
    expect(document.activeElement).toBe(tabs[1]);
    expect(screen.getAllByRole("tabpanel")).toHaveLength(1);
    fireEvent.keyDown(tabs[1], { key: "End" });
    expect(tabs[5].getAttribute("aria-selected")).toBe("true");
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
    await waitFor(() => expect(lan().disabled).toBe(false));
    expect(lan().checked).toBe(true);
    fireEvent.click(lan());
    await waitFor(() => expect(screen.getAllByRole("status").some((s) => s.textContent === "Enregistré")).toBe(true));
    first.unmount();
    render(<Shell services={svc} />);
    goTab("Réseau");
    await waitFor(() => expect(lan().checked).toBe(false));
  });

  it("échec d'enregistrement : message d'erreur", async () => {
    const base = createFakeServices({ tickMs: 0 });
    render(<Shell services={{ ...base, settingsSet: () => Promise.reject(new Error("Disque plein.")) }} />);
    goTab("Réseau");
    await waitFor(() => expect(lan().disabled).toBe(false));
    fireEvent.click(lan());
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

describe("n0 retiré", () => {
  it("plus de réglage de relais ni de découverte publique, et l'envoi ne transmet aucun ancien champ", async () => {
    const set = vi.fn(async (x) => x);
    const services = { ...createFakeServices({ tickMs: 0 }), settingsSet: set };
    render(<Shell services={services} />);
    goTab("Réseau");
    await screen.findByLabelText(/Ouvrir mon monde au réseau local automatiquement/);
    expect(screen.queryByLabelText(/Retrouver mes amis même si leur adresse change/)).toBeNull();
    expect(screen.queryByText(/serveurs n0/)).toBeNull();
    expect(screen.queryByLabelText(/Passer toujours par le relais/)).toBeNull();
    fireEvent.click(screen.getByLabelText(/Ouvrir mon monde au réseau local automatiquement/));
    await waitFor(() => expect(set).toHaveBeenCalled());
    for (const k of ["discovery", "allow_relay", "relay_only"]) expect(set.mock.calls[0][0]).not.toHaveProperty(k);
  });
});

describe("ouverture automatique au réseau local", () => {
  it("case cochée par défaut, envoie tous les champs", async () => {
    const set = vi.fn(async (x) => x);
    const services = { ...createFakeServices({ tickMs: 0 }), settingsSet: set };
    render(<Shell services={services} />);
    goTab("Réseau");
    const box = (await screen.findByLabelText(/Ouvrir mon monde au réseau local automatiquement/)) as HTMLInputElement;
    expect(box.checked).toBe(true);
    fireEvent.click(box);
    await waitFor(() => expect(set).toHaveBeenCalled());
    expect(set.mock.calls[0][0]).toMatchObject({ lan_auto: false, share_mods: true });
  });
});
