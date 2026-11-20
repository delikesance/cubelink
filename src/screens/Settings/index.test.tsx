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
  const base: NetTest = { ipv6: false, ipv4_public: false, port_mapped: true };
  it("verdicts : port ouvert = joignable ; adresse publique = probablement ; sinon peu joignable (aucun relais)", () => {
    const v = netVerdict(base);
    expect([v.tone, v.label]).toEqual(["ok", "Joignable"]);
    expect(v.checks.map((c) => [c.label, c.ok])).toEqual([["Adresse IPv6", false], ["Adresse IPv4 publique", false], ["Port ouvert sur la box", true]]);
    const lbl = (t: Partial<NetTest>) => { const r = netVerdict({ ...base, ...t }); return `${r.tone}:${r.label}`; };
    expect(lbl({ ipv6: true, port_mapped: false })).toBe("ok:Probablement joignable");
    expect(lbl({ ipv4_public: true, port_mapped: false })).toBe("ok:Probablement joignable");
    expect(lbl({ ipv6: true })).toBe("ok:Joignable");
    expect(lbl({ port_mapped: false })).toBe("warn:Peu joignable");
    const w = netVerdict({ ipv6: false, ipv4_public: false, port_mapped: false });
    expect(w.summary).toMatch(/même réseau local/);
    expect(JSON.stringify(w)).not.toMatch(/relais de secours|n0/i);
  });

  it("le bouton lance le vrai test et affiche le verdict", async () => {
    const services = createFakeServices({ tickMs: 0 });
    const spy = vi.spyOn(services, "netTest").mockResolvedValue({ ipv6: false, ipv4_public: false, port_mapped: false });
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

  it("Java : mémoire Automatique par défaut, champ Mo grisé, PC lu et affiché", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    render(<Shell services={svc} />);
    goTab("Java");
    const auto = screen.getByRole("checkbox", { name: /Mémoire automatique/ }) as HTMLInputElement;
    await waitFor(() => expect(auto.checked).toBe(true));
    expect((screen.getByLabelText("Mémoire par défaut (Mo)") as HTMLInputElement).disabled).toBe(true);
    expect(await screen.findByText(/≈\s16 Go sur ce PC/)).toBeTruthy();
  });

  it("Java : décocher Automatique enregistre une valeur manuelle, recocher revient à 0", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    const set = vi.spyOn(svc, "settingsSet");
    render(<Shell services={svc} />);
    goTab("Java");
    const auto = screen.getByRole("checkbox", { name: /Mémoire automatique/ }) as HTMLInputElement;
    await waitFor(() => expect(auto.disabled).toBe(false));
    fireEvent.click(auto);
    await waitFor(() => expect(set).toHaveBeenCalledTimes(1));
    expect(set.mock.calls[0]![0].default_ram_mb).toBe(4096);
    const ram = screen.getByLabelText("Mémoire par défaut (Mo)") as HTMLInputElement;
    await waitFor(() => expect(ram.disabled).toBe(false));
    expect(ram.value).toBe("4096");
    fireEvent.click(screen.getByRole("checkbox", { name: /Mémoire automatique/ }));
    await waitFor(() => expect(set).toHaveBeenCalledTimes(2));
    expect(set.mock.calls[1]![0].default_ram_mb).toBe(0);
  });

  it("Java : avertissement non bloquant au-delà de 75 % de la RAM du PC", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    const set = vi.spyOn(svc, "settingsSet");
    await svc.settingsSet({ ...(await svc.settingsGet()), default_ram_mb: 4096 });
    set.mockClear();
    render(<Shell services={svc} />);
    goTab("Java");
    const ram = screen.getByLabelText("Mémoire par défaut (Mo)") as HTMLInputElement;
    await waitFor(() => expect(ram.value).toBe("4096"));
    await waitFor(() => expect(ram.disabled).toBe(false));
    expect(screen.queryByText(/dépassent 75/)).toBeNull();
    fireEvent.change(ram, { target: { value: "12288" } }); // PC simulé : 16 Go, 75 % = 12 Go : pas au-dessus
    expect(screen.queryByText(/dépassent 75/)).toBeNull();
    fireEvent.change(ram, { target: { value: "12544" } });
    expect((await screen.findByText(/dépassent 75/)).textContent).toMatch(/12,3 Go.*16 Go/);
    fireEvent.blur(ram);
    await waitFor(() => expect(set).toHaveBeenCalledTimes(1)); // jamais bloquant : la valeur est enregistrée
    expect(set.mock.calls[0]![0].default_ram_mb).toBe(12544);
  });

  it("Java : mémoire par défaut validée puis enregistrée à la sortie du champ", async () => {
    const svc = createFakeServices({ tickMs: 0 });
    const set = vi.spyOn(svc, "settingsSet");
    await svc.settingsSet({ ...(await svc.settingsGet()), default_ram_mb: 4096 });
    set.mockClear();
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

describe("cache de mods", () => {
  const openStorage = () => { goTab("Stockage"); };

  it("affiche la taille, confirme avec le chiffre puis vide pour de bon", async () => {
    const services = createFakeServices({ tickMs: 0, cacheBytes: 12_400_000_000 });
    const clear = vi.spyOn(services, "cacheClear");
    render(<Shell services={services} />);
    openStorage();
    expect(await screen.findByText(/Libère 12,4 Go/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /Vider le cache…/ }));
    expect(within(screen.getByRole("alertdialog")).getByText(/Libère 12,4 Go/)).toBeTruthy();
    fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Vider le cache" }));
    expect(await screen.findByText("Cache vidé : 12,4 Go libérés.")).toBeTruthy();
    expect(clear).toHaveBeenCalledTimes(1);
    expect(screen.queryByText(/pas encore disponible/)).toBeNull();
    // La taille retombe à zéro : plus rien à libérer, le bouton est désactivé.
    await waitFor(() => expect((screen.getByRole("button", { name: /Vider le cache…/ }) as HTMLButtonElement).disabled).toBe(true));
    expect(screen.getByText(/Rien à libérer/)).toBeTruthy();
  });

  it("cache vide à l'ouverture : rien à libérer, bouton désactivé", async () => {
    render(<Shell services={createFakeServices({ tickMs: 0, cacheBytes: 0 })} />);
    openStorage();
    expect(await screen.findByText(/Rien à libérer/)).toBeTruthy();
    expect((screen.getByRole("button", { name: /Vider le cache…/ }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("l'échec du vidage est lu tel quel et le cache reste proposé", async () => {
    const base = createFakeServices({ tickMs: 0 });
    render(<Shell services={{ ...base, cacheClear: () => Promise.reject("Une partie est en cours.") }} />);
    openStorage();
    await screen.findByText(/Libère 12,4 Go/);
    fireEvent.click(screen.getByRole("button", { name: /Vider le cache…/ }));
    fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Vider le cache" }));
    expect(await screen.findByText("Une partie est en cours.")).toBeTruthy();
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect((screen.getByRole("button", { name: /Vider le cache…/ }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("taille illisible : le bouton reste utilisable, sans chiffre", async () => {
    const base = createFakeServices({ tickMs: 0 });
    render(<Shell services={{ ...base, cacheSize: () => Promise.reject(new Error("boom")) }} />);
    openStorage();
    await waitFor(() => expect(screen.getByText(/Fichiers partagés entre tes instances\./)).toBeTruthy());
    expect((screen.getByRole("button", { name: /Vider le cache…/ }) as HTMLButtonElement).disabled).toBe(false);
    expect(screen.queryByText(/Libère/)).toBeNull();
  });
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
