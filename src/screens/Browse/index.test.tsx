import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ShellContext } from "../../app/context";
import { goBack, navigate, resetHistory, useLocation } from "../../app/router";
import { createFakeServices } from "../../services/fake";
import type { BrowseHit, BrowsePage, BrowseVersion, Instance, Services } from "../../services/types";
import Browse, { DEBOUNCE_MS, formatDownloads } from "./index";

const hit = (o: Partial<BrowseHit> = {}): BrowseHit => ({
  source: "modrinth", id: "p1", kind: "modpack", name: "Super Pack", summary: "Un pack génial.",
  author: "alice", icon_url: null, downloads: 12345, page_url: "https://modrinth.com/modpack/p1", downloadable: true, ...o,
});
const version = (o: Partial<BrowseVersion> = {}): BrowseVersion => ({
  id: "v1", name: "1.0.0", minecraft: ["1.21.1"], loaders: ["neoforge"], published: "2026-01-01", downloadable: true, ...o,
});
const inst: Instance = { id: "i1", name: "Mon monde", minecraft: "1.21.1", loader: { loader: "fabric", version: "" }, ram_mb: 4096, mod_count: 0 };

function setup(over: Record<string, unknown> = {}, params: Record<string, string> = {}) {
  const base = createFakeServices({ tickMs: 0 });
  const svc = {
    ...base,
    browseSearch: vi.fn(async (): Promise<BrowsePage> => ({ hits: [hit()], total: 1 })),
    browseVersions: vi.fn(async () => [version()]),
    browseInstall: vi.fn(async () => ({ instance: inst, installed: ["a.jar"], missing: [] })),
    browseCheck: vi.fn(async () => ({ installed: [], recommended: "v1", verdicts: [{ version_id: "v1", ok: true, problems: [] }], blockers: [] })),
    onBrowseProgress: vi.fn(() => () => {}),
    instancesList: vi.fn(async () => [inst]),
    minecraftVersions: vi.fn(async () => ["1.21.1", "1.20.1"]),
    ...over,
  } as unknown as Services;
  if (Object.keys(params).length) window.location.hash = `#/browse?${new URLSearchParams(params).toString()}`;
  // Les params viennent de l'URL (comme dans le Shell) : la fiche s'ouvre et se ferme via le hash.
  function Harness() {
    const loc = useLocation();
    const value = { services: svc, route: "browse", params: loc.params, navigate, goBack, canGoBack: true, setTitle: () => {} };
    return <ShellContext.Provider value={value as never}><Browse /></ShellContext.Provider>;
  }
  render(<Harness />);
  return svc as unknown as Record<string, ReturnType<typeof vi.fn>> & Services;
}

beforeEach(() => { window.location.hash = "#/browse"; resetHistory(); });
afterEach(() => { cleanup(); vi.useRealTimers(); vi.restoreAllMocks(); });

describe("formatDownloads", () => {
  it("formate", () => {
    expect(formatDownloads(950)).toBe("950");
    expect(formatDownloads(12345)).toBe("12 k");
    expect(formatDownloads(2_500_000)).toBe("2,5 M");
  });
});

describe("Parcourir : recherche", () => {
  it("affiche les résultats, mention de la source, focus sur la recherche", async () => {
    const svc = setup();
    expect(await screen.findByText("Super Pack")).toBeTruthy();
    expect(screen.getByText("Modrinth", { selector: ".br-badge" })).toBeTruthy();
    expect(document.activeElement).toBe(screen.getByLabelText("Recherche"));
    expect(svc.browseSearch).toHaveBeenCalledWith(expect.objectContaining({ source: "modrinth", kind: "modpack", query: "", offset: 0 }));
  });

  it("debounce : un seul appel après la saisie, Entrée est immédiat", async () => {
    const svc = setup();
    await screen.findByText("Super Pack");
    vi.useFakeTimers();
    const input = screen.getByLabelText("Recherche");
    fireEvent.change(input, { target: { value: "c" } });
    fireEvent.change(input, { target: { value: "cr" } });
    expect(svc.browseSearch).toHaveBeenCalledTimes(1);
    await act(async () => { vi.advanceTimersByTime(DEBOUNCE_MS + 10); });
    expect(svc.browseSearch).toHaveBeenCalledTimes(2);
    expect(svc.browseSearch).toHaveBeenLastCalledWith(expect.objectContaining({ query: "cr" }));
    fireEvent.change(input, { target: { value: "cre" } });
    await act(async () => { fireEvent.submit(input.closest("form")!); });
    expect(svc.browseSearch).toHaveBeenLastCalledWith(expect.objectContaining({ query: "cre" }));
  });

  it("erreur puis Réessayer", async () => {
    const search = vi.fn()
      .mockRejectedValueOnce(new Error("Réseau indisponible."))
      .mockResolvedValue({ hits: [hit()], total: 1 });
    setup({ browseSearch: search });
    expect((await screen.findByRole("alert")).textContent).toMatch(/Réseau indisponible/);
    fireEvent.click(screen.getByRole("button", { name: "Réessayer" }));
    expect(await screen.findByText("Super Pack")).toBeTruthy();
    expect(search).toHaveBeenCalledTimes(2);
  });

  it("CurseForge sans clé API : message français, onglet signalé, bascule sur Modrinth", async () => {
    const search = vi.fn(async (q: { source: string }) => {
      if (q.source === "curseforge") throw new Error("entrée invalide : CurseForge n'est pas disponible : clé API de Cubelink absente");
      return { hits: [hit()], total: 1 };
    });
    setup({ browseSearch: search });
    expect(await screen.findByText("Super Pack")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /^CurseForge/ }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("CurseForge n'est pas configuré dans cette version de Cubelink. Utilise Modrinth.");
    expect(screen.getByRole("button", { name: /CurseForge · indisponible/ })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Utiliser Modrinth" }));
    expect(await screen.findByText("Super Pack")).toBeTruthy();
  });

  it("état vide", async () => {
    setup({ browseSearch: vi.fn(async () => ({ hits: [], total: 0 })) });
    expect(await screen.findByText(/Aucun résultat/)).toBeTruthy();
  });

  it("Charger plus ajoute la page suivante avec offset", async () => {
    const search = vi.fn(async (q: { offset: number }) => ({ hits: [hit({ id: `p${q.offset}`, name: `Pack ${q.offset}` })], total: 2 }));
    setup({ browseSearch: search });
    await screen.findByText("Pack 0");
    fireEvent.click(screen.getByRole("button", { name: "Charger plus" }));
    expect(await screen.findByText("Pack 1")).toBeTruthy();
    expect(search).toHaveBeenLastCalledWith(expect.objectContaining({ offset: 1 }));
    expect(screen.queryByRole("button", { name: "Charger plus" })).toBeNull();
  });
});

describe("Parcourir : onglets et source", () => {
  it("onglet initial via la route, flèches, changement de source", async () => {
    const svc = setup({}, { kind: "mod" });
    await screen.findByText("Super Pack");
    const tabs = screen.getAllByRole("tab");
    expect(tabs[1].getAttribute("aria-selected")).toBe("true");
    expect(svc.browseSearch).toHaveBeenCalledWith(expect.objectContaining({ kind: "mod" }));
    fireEvent.keyDown(tabs[1], { key: "ArrowRight" });
    expect(tabs[2].getAttribute("aria-selected")).toBe("true");
    expect(document.activeElement).toBe(tabs[2]);
    await waitFor(() => expect(svc.browseSearch).toHaveBeenLastCalledWith(expect.objectContaining({ kind: "resourcepack" })));
    fireEvent.click(screen.getByRole("button", { name: "CurseForge" }));
    await waitFor(() => expect(svc.browseSearch).toHaveBeenLastCalledWith(expect.objectContaining({ source: "curseforge" })));
  });

  it("instance de la route : préremplit minecraft et loader", async () => {
    const svc = setup({}, { kind: "mod", instance: "i1" });
    await waitFor(() => expect(svc.browseSearch).toHaveBeenLastCalledWith(expect.objectContaining({ minecraft: "1.21.1", loader: "fabric" })));
    expect((screen.getByLabelText("Instance") as HTMLSelectElement).value).toBe("i1");
  });
});

const packArt = (o: Partial<NonNullable<Instance["art"]>> = {}) => ({
  source: "modrinth" as const, project_id: "p1", title: "Super Pack", author: "alice", summary: "Résumé.",
  color: null, icon: null, screenshots: [], page_url: "https://modrinth.com/modpack/p1", ...o,
});

describe("Parcourir : fiche et installation", () => {
  it("modpack : la fiche s'ouvre comme une page (URL), lien source, installation puis Ouvrir le modpack", async () => {
    const svc = setup();
    fireEvent.click(await screen.findByRole("button", { name: /Super Pack/ }));
    expect(await screen.findByRole("heading", { name: "Super Pack" })).toBeTruthy();
    expect(window.location.hash).toContain("detail=modrinth%3Ap1");
    expect(screen.queryByRole("dialog")).toBeNull();
    const link = screen.getByRole("link", { name: /Ouvrir la page sur Modrinth/ });
    expect(link.getAttribute("href")).toBe("https://modrinth.com/modpack/p1");
    expect(link.getAttribute("rel")).toContain("noopener");
    expect(link.getAttribute("target")).toBe("_blank");
    await screen.findByRole("option", { name: /1\.0\.0/ });
    fireEvent.click(screen.getByRole("button", { name: "Installer le modpack" }));
    fireEvent.click(await screen.findByRole("button", { name: "Ouvrir le modpack" }));
    expect(window.location.hash).toBe("#/instance?id=i1");
    expect(svc.browseInstall).toHaveBeenCalledWith({ source: "modrinth", id: "p1", kind: "modpack", version_id: "v1", instance_id: undefined, ram_mb: 0 });
  });

  it("Retour et Échap ramènent à la liste", async () => {
    setup();
    fireEvent.click(await screen.findByRole("button", { name: /Super Pack/ }));
    await screen.findByRole("heading", { name: "Super Pack" });
    fireEvent.click(screen.getByRole("button", { name: "Retour aux résultats" }));
    await waitFor(() => expect(window.location.hash).toBe("#/browse"));
    expect(await screen.findByRole("tablist")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /Super Pack/ }));
    await screen.findByRole("heading", { name: "Super Pack" });
    fireEvent.keyDown(document.body, { key: "Escape" });
    await waitFor(() => expect(window.location.hash).toBe("#/browse"));
    expect(await screen.findByRole("tablist")).toBeTruthy();
  });

  it("ouverture directe par l'URL : fiche retrouvée dans les résultats chargés", async () => {
    setup({}, { detail: "modrinth:p1" });
    expect(await screen.findByRole("heading", { name: "Super Pack" })).toBeTruthy();
  });

  it("ouverture directe d'un pack installé sans résultat : fiche reconstruite, bouton Ouvrir", async () => {
    setup({
      browseSearch: vi.fn(async () => ({ hits: [], total: 0 })),
      instancesList: vi.fn(async () => [{ ...inst, art: packArt() }]),
    }, { detail: "modrinth:p1" });
    expect(await screen.findByRole("heading", { name: "Super Pack" })).toBeTruthy();
    fireEvent.click(await screen.findByRole("button", { name: "Ouvrir" }));
    expect(window.location.hash).toBe("#/instance?id=i1");
  });

  it("fiche introuvable : message et retour à la liste", async () => {
    setup({ browseSearch: vi.fn(async () => ({ hits: [], total: 0 })) }, { detail: "modrinth:zzz" });
    expect(await screen.findByText("Cette fiche est introuvable.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Retour aux résultats" }));
    await waitFor(() => expect(window.location.hash).toBe("#/browse"));
  });

  it("modpack déjà installé : Ouvrir (page instance) au lieu d'Installer, réinstaller en secondaire", async () => {
    const svc = setup({ instancesList: vi.fn(async () => [{ ...inst, art: packArt() }]) });
    fireEvent.click(await screen.findByRole("button", { name: /Super Pack/ }));
    await screen.findByRole("heading", { name: "Super Pack" });
    expect(await screen.findByText(/déjà installé/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Installer le modpack" })).toBeNull();
    expect(screen.getByRole("button", { name: "Réinstaller" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Ouvrir" }));
    expect(window.location.hash).toBe("#/instance?id=i1");
    expect(svc.browseInstall).not.toHaveBeenCalled();
  });

  it("mod sans instance : message, aucun appel", async () => {
    const svc = setup({ browseSearch: vi.fn(async () => ({ hits: [hit({ kind: "mod", name: "Sodium" })], total: 1 })) }, { kind: "mod" });
    fireEvent.click(await screen.findByRole("button", { name: /Sodium/ }));
    await screen.findByRole("option", { name: /1\.0\.0/ });
    fireEvent.click(screen.getByRole("button", { name: "Installer" }));
    expect((await screen.findByRole("alert")).textContent).toMatch(/instance cible/);
    expect(svc.browseInstall).not.toHaveBeenCalled();
  });

  it("mod avec instance : installation, progression et résumé installed/missing", async () => {
    let emit: (e: { done: number; total: number; label: string }) => void = () => {};
    let resolve: (r: unknown) => void = () => {};
    const svc = setup({
      browseSearch: vi.fn(async () => ({ hits: [hit({ kind: "mod", name: "Sodium" })], total: 1 })),
      onBrowseProgress: vi.fn((cb) => { emit = cb; return () => {}; }),
      browseInstall: vi.fn(() => new Promise((r) => { resolve = r; })),
    }, { kind: "mod", instance: "i1" });
    // Attend que les filtres de l'instance soient appliqués (la grille est rechargée).
    await waitFor(() => expect(screen.getByLabelText("Loader")).toHaveProperty("value", "fabric"));
    fireEvent.click(await screen.findByRole("button", { name: /Sodium/ }));
    await screen.findByRole("option", { name: /1\.0\.0/ });
    await waitFor(() => expect((screen.getByRole("button", { name: "Installer" }) as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(screen.getByRole("button", { name: "Installer" }));
    await act(async () => { emit({ done: 1, total: 4, label: "Téléchargement de sodium.jar" }); });
    expect(screen.getByText("Téléchargement de sodium.jar")).toBeTruthy();
    expect(screen.getByRole("progressbar").getAttribute("aria-valuenow")).toBe("25");
    await act(async () => { resolve({ instance: inst, installed: ["sodium.jar"], missing: ["lib.jar"] }); });
    expect(await screen.findByText(/1 élément installé dans « Mon monde »/)).toBeTruthy();
    expect(screen.getByText("lib.jar")).toBeTruthy();
    expect(svc.browseInstall).toHaveBeenCalledWith(expect.objectContaining({ kind: "mod", instance_id: "i1" }));
  });

  const modSetup = (over: Record<string, unknown>) => setup({
    browseSearch: vi.fn(async () => ({ hits: [hit({ kind: "mod", name: "Iris" })], total: 1 })),
    browseVersions: vi.fn(async () => [version({ id: "v2", name: "1.11.4" }), version({ id: "v1", name: "1.10.9" })]),
    ...over,
  }, { kind: "mod", instance: "i1" });

  it("mod : la version recommandée (compatible avec les autres mods) est choisie par défaut", async () => {
    const svc = modSetup({
      browseCheck: vi.fn(async () => ({ installed: [], recommended: "v1", blockers: [],
        verdicts: [{ version_id: "v2", ok: false, problems: ["Iris : Requiert Sodium 0.9.x"] }, { version_id: "v1", ok: true, problems: [] }] })),
    });
    await waitFor(() => expect(screen.getByLabelText("Loader")).toHaveProperty("value", "fabric"));
    fireEvent.click(await screen.findByRole("button", { name: /Iris/ }));
    await waitFor(() => expect((screen.getByLabelText("Version") as HTMLSelectElement).value).toBe("v1"));
    expect(screen.getByRole("option", { name: /1\.10\.9.*\(recommandée\)/ })).toBeTruthy();
    expect(screen.getByRole("option", { name: /1\.11\.4.*\(incompatible\)/ })).toBeTruthy();
    // Version incompatible choisie à la main : installation impossible, raison affichée.
    fireEvent.change(screen.getByLabelText("Version"), { target: { value: "v2" } });
    expect(await screen.findByText("Iris : Requiert Sodium 0.9.x")).toBeTruthy();
    expect((screen.getByRole("button", { name: "Installer" }) as HTMLButtonElement).disabled).toBe(true);
    expect(svc.browseCheck).toHaveBeenCalledWith("modrinth", "p1", "mod", "i1");
  });

  it("mod incompatible avec un mod présent : Installer bloqué, mods en cause listés", async () => {
    const svc = modSetup({
      browseCheck: vi.fn(async () => ({ installed: [], recommended: null, verdicts: [], blockers: ["Iris : Incompatible avec Embeddium."] })),
    });
    await waitFor(() => expect(screen.getByLabelText("Loader")).toHaveProperty("value", "fabric"));
    fireEvent.click(await screen.findByRole("button", { name: /Iris/ }));
    expect(await screen.findByText(/Aucune version n'est compatible avec les mods déjà présents/)).toBeTruthy();
    expect(screen.getByText("Iris : Incompatible avec Embeddium.")).toBeTruthy();
    expect((screen.getByRole("button", { name: "Installer" }) as HTMLButtonElement).disabled).toBe(true);
    expect(svc.browseInstall).not.toHaveBeenCalled();
  });

  it("mod déjà installé : Désinstaller à la place d'Installer", async () => {
    let installed = ["iris-1.10.9.jar"];
    const svc = modSetup({
      browseCheck: vi.fn(async () => ({ installed, recommended: "v1", verdicts: [{ version_id: "v1", ok: true, problems: [] }], blockers: [] })),
      modsRemove: vi.fn(async () => { installed = []; }),
    });
    await waitFor(() => expect(screen.getByLabelText("Loader")).toHaveProperty("value", "fabric"));
    fireEvent.click(await screen.findByRole("button", { name: /Iris/ }));
    fireEvent.click(await screen.findByRole("button", { name: "Désinstaller" }));
    expect(screen.queryByRole("button", { name: "Installer" })).toBeNull();
    await waitFor(() => expect(svc.modsRemove).toHaveBeenCalledWith("i1", "iris-1.10.9.jar"));
    expect(await screen.findByRole("button", { name: "Installer" })).toBeTruthy();
  });

  it("non téléchargeable : Installer désactivé, message et lien Ouvrir sur CurseForge", async () => {
    setup({
      browseSearch: vi.fn(async () => ({ hits: [hit({ source: "curseforge", downloadable: false, page_url: "https://www.curseforge.com/x" })], total: 1 })),
    });
    fireEvent.click(await screen.findByRole("button", { name: /Super Pack/ }));
    await screen.findByRole("heading", { name: "Super Pack" });
    expect((screen.getByRole("button", { name: "Installer le modpack" }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText(/n'est pas téléchargeable/)).toBeTruthy();
    expect(screen.getByRole("link", { name: "Ouvrir sur CurseForge" }).getAttribute("href")).toBe("https://www.curseforge.com/x");
  });
});
