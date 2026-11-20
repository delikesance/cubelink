import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Shell } from "../../app/shell";
import { createFakeServices } from "../../services/fake";
import { netVerdict, validateJavaPath, validateRam } from "./index";
import type { NetTest } from "../../services/types";

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

describe("test de connexion", () => {
  const base: NetTest = { relay_connected: true, relay: "euc1-1.relay.n0.iroh.link", relay_ms: 28, udp: true, ipv6: false, direct: true, relay_only: false, partial: false };
  it("verdicts", () => {
    const v = netVerdict(base);
    expect([v.tone, v.label]).toEqual(["ok", "Excellente"]);
    expect(v.checks).toEqual([
      { label: "Connexion directe", ok: true, detail: "possible" },
      { label: "Relais de secours", ok: true, detail: "joignable · Europe · 28 ms" },
    ]);
    const lbl = (t: Partial<NetTest>) => { const r = netVerdict({ ...base, ...t }); return `${r.tone}:${r.label}`; };
    expect(lbl({ direct: false })).toBe("warn:Correcte");
    expect(lbl({ udp: false, direct: false })).toBe("warn:Limitée");
    expect(lbl({ relay_connected: false, udp: false })).toBe("danger:Hors ligne");
    expect(lbl({ relay_connected: false })).toBe("warn:Bonne");
    expect(lbl({ relay_connected: false, direct: false })).toBe("danger:Fragile");
    expect(lbl({ relay_only: true })).toBe("ok:Relais uniquement");
    expect(lbl({ relay_only: true, relay_connected: false })).toBe("danger:Hors ligne");
    expect(lbl({ partial: true })).toBe("neutral:Test incomplet");
  });

  it("le bouton lance le vrai test et affiche le verdict", async () => {
    const services = createFakeServices({ tickMs: 0 });
    const spy = vi.spyOn(services, "netTest").mockResolvedValue({ ...base, direct: false });
    window.location.hash = "#/settings";
    render(<Shell services={services} />);
    goTab("Réseau");
    fireEvent.click(await screen.findByRole("button", { name: "Tester ma connexion" }));
    expect(await screen.findByText("Correcte")).toBeTruthy();
    expect(screen.getByText(/certaines parties passeront par le relais/)).toBeTruthy();
    expect(screen.getByText("gênée par ton routeur")).toBeTruthy();
    expect(screen.getByText("joignable · Europe · 28 ms")).toBeTruthy();
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
