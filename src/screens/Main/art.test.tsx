import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { ShellContext, type ShellContextValue } from "../../app/context";
import { createFakeServices } from "../../services/fake";
import type { Instance, Services } from "../../services";
import { clearArtCache } from "./artImage";
import Main from "./index";

beforeEach(clearArtCache);
afterEach(cleanup);

function setup(over: Partial<Services> = {}) {
  const services = Object.assign(createFakeServices({ tickMs: 0, requestCount: 0 }), over);
  const navigate = vi.fn();
  const value = { services, route: "main", params: {}, navigate, setTitle: () => {}, goBack: () => {}, canGoBack: false } as ShellContextValue;
  render(<ShellContext.Provider value={value}><Main /></ShellContext.Provider>);
  return { services, navigate };
}
const rail = () => screen.getByRole("navigation", { name: "Instances" });
const select = async (name: string) => {
  await screen.findByRole("heading", { level: 1 });
  fireEvent.click(within(rail()).getByRole("button", { name }));
  return screen.findByRole("heading", { level: 1, name });
};
const openMenu = async (name: string) => {
  fireEvent.contextMenu(within(rail()).getByRole("button", { name }));
  return screen.findByRole("menu");
};
const clickItem = async (name: string, item: string | RegExp) =>
  fireEvent.click(within(await openMenu(name)).getByRole("menuitem", { name: item }));
const AERO = "Create: Aeronautics";

const withArt = (art: Partial<NonNullable<Instance["art"]>>): Instance => ({
  id: "inst-aeronautics", name: AERO, minecraft: "1.21.1", loader: { loader: "neoforge", version: "21.1.77" }, ram_mb: 6144, mod_count: 94,
  art: { source: "modrinth", project_id: "p", title: "Pack", author: "Zed", summary: "Résumé trouvé", color: "#27ae60", icon: null, screenshots: [], page_url: null, ...art },
});

describe("Main : illustrations", () => {
  it("instance avec art : crédit (auteur, source), méta et image de fond ; ni résumé ni galerie", async () => {
    setup();
    await select("All the Mods 10");
    expect(screen.getByText("par ATM Team")).toBeTruthy();
    expect(screen.getByText(/Source : CurseForge/)).toBeTruthy();
    expect(screen.getByText(/Minecraft 1\.21\.1/)).toBeTruthy();
    expect(screen.queryByText(/All the Mods 10 : un modpack/)).toBeNull();
    expect(screen.queryByRole("button", { name: /Agrandir la capture/ })).toBeNull();
    await waitFor(() => expect(document.querySelector("img.home-bg")).toBeTruthy());
  });

  it("lien « Page du modpack » sécurisé, seulement en http(s)", async () => {
    const inst = withArt({ page_url: "https://modrinth.com/modpack/pack" });
    setup({ instancesList: async () => [inst] });
    const link = (await screen.findByRole("link", { name: "Page du modpack" })) as HTMLAnchorElement;
    expect(link.target).toBe("_blank");
    expect(link.rel).toBe("noopener noreferrer");
    cleanup();
    setup({ instancesList: async () => [withArt({ page_url: "javascript:alert(1)" })] });
    await screen.findByRole("heading", { level: 1, name: AERO });
    expect(screen.queryByRole("link", { name: "Page du modpack" })).toBeNull();
  });

  it("instance sans art : image de blocs par défaut, pas de crédit, outils d'illustration dans le menu", async () => {
    setup();
    await screen.findByRole("heading", { level: 1, name: AERO });
    expect(document.querySelector("svg[data-default-art]")).toBeTruthy();
    expect(document.querySelector("img.home-bg")).toBeNull();
    expect(screen.queryByRole("link", { name: "Page du modpack" })).toBeNull();
    const menu = await openMenu(AERO);
    expect(within(menu).getByRole("menuitem", { name: "Choisir une illustration" })).toBeTruthy();
    expect(within(menu).getByRole("menuitem", { name: "Définir l'icône" })).toBeTruthy();
    expect(within(menu).queryByRole("menuitem", { name: /Voir les captures/ })).toBeNull();
  });

  it("image en erreur : la carte reste, l'image par défaut reste visible", async () => {
    setup({ instancesArtImage: async () => { throw new Error("boom"); } });
    await select("All the Mods 10");
    await waitFor(() => expect(document.querySelector("svg[data-default-art]")).toBeTruthy());
    expect(document.querySelector("img.home-bg")).toBeNull();
  });

  it("recherche automatique : chargement puis succès, crédit mis à jour", async () => {
    let done: () => void = () => {};
    setup({
      instancesArtLookup: () => new Promise((res) => { done = () => res(withArt({ title: "Pack Trouvé" })); }),
    });
    await screen.findByRole("heading", { level: 1, name: AERO });
    await clickItem(AERO, "Chercher automatiquement");
    expect((await screen.findAllByRole("status")).some((e) => /Recherche/.test(e.textContent ?? ""))).toBe(true);
    done();
    expect(await screen.findByText("par Zed")).toBeTruthy();
    expect(screen.queryByText(/Illustration trouvée/)).toBeNull();
  });

  it("recherche automatique : erreur affichée en role=alert", async () => {
    setup();
    await screen.findByRole("heading", { level: 1, name: AERO });
    await clickItem(AERO, "Chercher automatiquement");
    expect((await screen.findByRole("alert")).textContent).toMatch(/Aucun modpack correspondant/);
  });

  it("choix d'illustration : pickFiles puis instancesArtSet banner", async () => {
    const set = vi.fn(async () => withArt({ source: "custom", screenshots: ["cover.png"] }));
    const pick = vi.fn(async () => ["C:\\img\\cover.png"]);
    setup({ instancesArtSet: set, pickFiles: pick });
    await screen.findByRole("heading", { level: 1, name: AERO });
    await clickItem(AERO, "Choisir une illustration");
    await waitFor(() => expect(set).toHaveBeenCalledWith("inst-aeronautics", "C:\\img\\cover.png", "banner"));
    expect(pick).toHaveBeenCalledWith({ extensions: ["png", "jpg", "jpeg", "webp"] });
    expect(await screen.findByText("Illustration définie.")).toBeTruthy();
    const menu = await openMenu(AERO);
    expect(within(menu).getByRole("menuitem", { name: "Changer l'illustration" })).toBeTruthy();
    expect(within(menu).getByRole("menuitem", { name: /Voir les captures \(1\)/ })).toBeTruthy();
  });

  it("définir l'icône : role icon ; annulation du sélecteur = aucun appel", async () => {
    const set = vi.fn(async () => withArt({ icon: "icon.png" }));
    const pick = vi.fn(async () => ["C:\\img\\icon.png"]);
    setup({ instancesArtSet: set, pickFiles: pick });
    await screen.findByRole("heading", { level: 1, name: AERO });
    await clickItem(AERO, "Définir l'icône");
    await waitFor(() => expect(set).toHaveBeenCalledWith("inst-aeronautics", "C:\\img\\icon.png", "icon"));
    cleanup();
    set.mockClear();
    setup({ instancesArtSet: set, pickFiles: async () => [] });
    await screen.findByRole("heading", { level: 1, name: AERO });
    await clickItem(AERO, "Définir l'icône");
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
    expect(set).not.toHaveBeenCalled();
  });

  it("visionneuse (menu « Voir les captures ») : flèches, Échap, focus restauré", async () => {
    setup();
    await select("All the Mods 10");
    const trigger = within(rail()).getByRole("button", { name: "All the Mods 10" });
    trigger.focus();
    await clickItem("All the Mods 10", /^Voir les captures/);
    const dlg = await screen.findByRole("dialog");
    expect(dlg.getAttribute("aria-modal")).toBe("true");
    const counter = () => /(\d+) \/ (\d+)/.exec(screen.getByRole("dialog").querySelector(".home-viewer-count")?.textContent ?? "")!;
    expect(counter()[1]).toBe("1");
    const total = counter()[2];
    fireEvent.keyDown(document, { key: "ArrowRight" });
    expect(counter()[1]).toBe("2");
    fireEvent.keyDown(document, { key: "ArrowLeft" });
    fireEvent.keyDown(document, { key: "ArrowLeft" });
    expect(counter()[1]).toBe(total);
    fireEvent.keyDown(document, { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });

  it("menu contextuel : actions d'illustration, erreur de recherche annoncée", async () => {
    setup({ instancesArtLookup: async () => { throw new Error("hors-ligne"); } });
    await select("All the Mods 10");
    const menu = await openMenu("All the Mods 10");
    expect(within(menu).getByRole("menuitem", { name: "Changer l'illustration" })).toBeTruthy();
    fireEvent.click(within(menu).getByRole("menuitem", { name: "Chercher automatiquement" }));
    expect((await screen.findByRole("alert")).textContent).toBe("hors-ligne");
  });
});
