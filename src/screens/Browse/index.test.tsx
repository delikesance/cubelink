import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ShellContext } from "../../app/context";
import { goBack, navigate } from "../../app/router";
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
    onBrowseProgress: vi.fn(() => () => {}),
    instancesList: vi.fn(async () => [inst]),
    minecraftVersions: vi.fn(async () => ["1.21.1", "1.20.1"]),
    ...over,
  } as unknown as Services;
  const value = { services: svc, route: "browse", params, navigate, goBack, canGoBack: true, setTitle: () => {} };
  render(<ShellContext.Provider value={value as never}><Browse /></ShellContext.Provider>);
  return svc as unknown as Record<string, ReturnType<typeof vi.fn>> & Services;
}

beforeEach(() => { window.location.hash = "#/browse"; });
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

describe("Parcourir : fiche et installation", () => {
  it("modpack : fiche, source + lien, installation puis navigation vers main", async () => {
    const svc = setup();
    fireEvent.click(await screen.findByRole("button", { name: /Super Pack/ }));
    const dlg = await screen.findByRole("dialog", { name: "Super Pack" });
    expect(dlg.getAttribute("aria-modal")).toBe("true");
    const link = screen.getByRole("link", { name: /Ouvrir la page sur Modrinth/ });
    expect(link.getAttribute("href")).toBe("https://modrinth.com/modpack/p1");
    expect(link.getAttribute("rel")).toContain("noopener");
    expect(link.getAttribute("target")).toBe("_blank");
    await screen.findByRole("option", { name: /1\.0\.0/ });
    fireEvent.click(screen.getByRole("button", { name: "Installer le modpack" }));
    await waitFor(() => expect(window.location.hash).toBe("#/main"));
    expect(svc.browseInstall).toHaveBeenCalledWith({ source: "modrinth", id: "p1", kind: "modpack", version_id: "v1", instance_id: undefined });
  });

  it("Échap ferme la fiche sans quitter l'écran", async () => {
    setup();
    fireEvent.click(await screen.findByRole("button", { name: /Super Pack/ }));
    const dlg = await screen.findByRole("dialog");
    fireEvent.keyDown(dlg, { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(window.location.hash).toBe("#/browse");
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
    fireEvent.click(screen.getByRole("button", { name: "Installer" }));
    await act(async () => { emit({ done: 1, total: 4, label: "Téléchargement de sodium.jar" }); });
    expect(screen.getByText("Téléchargement de sodium.jar")).toBeTruthy();
    expect(screen.getByRole("progressbar").getAttribute("aria-valuenow")).toBe("25");
    await act(async () => { resolve({ instance: inst, installed: ["sodium.jar"], missing: ["lib.jar"] }); });
    expect(await screen.findByText(/1 élément installé dans « Mon monde »/)).toBeTruthy();
    expect(screen.getByText("lib.jar")).toBeTruthy();
    expect(svc.browseInstall).toHaveBeenCalledWith(expect.objectContaining({ kind: "mod", instance_id: "i1" }));
  });

  it("non téléchargeable : Installer désactivé, message et lien Ouvrir sur CurseForge", async () => {
    setup({
      browseSearch: vi.fn(async () => ({ hits: [hit({ source: "curseforge", downloadable: false, page_url: "https://www.curseforge.com/x" })], total: 1 })),
    });
    fireEvent.click(await screen.findByRole("button", { name: /Super Pack/ }));
    await screen.findByRole("dialog");
    expect((screen.getByRole("button", { name: "Installer le modpack" }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText(/n'est pas téléchargeable/)).toBeTruthy();
    expect(screen.getByRole("link", { name: "Ouvrir sur CurseForge" }).getAttribute("href")).toBe("https://www.curseforge.com/x");
  });
});
