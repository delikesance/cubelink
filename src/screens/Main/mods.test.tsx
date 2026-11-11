import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { ShellContext, type ShellContextValue } from "../../app/context";
import { setImportWarning } from "../../app/importWarning";
import { createFakeServices } from "../../services/fake";
import Main from "./index";
import { UNDO_MS } from "./ModsPanel";

afterEach(() => { cleanup(); setImportWarning(null); });

function setup() {
  const services = createFakeServices({ tickMs: 0, requestCount: 0 });
  const navigate = vi.fn();
  const value = { services, route: "main", params: {}, navigate, setTitle: () => {}, goBack: () => {}, canGoBack: false } as ShellContextValue;
  render(<ShellContext.Provider value={value}><Main /></ShellContext.Provider>);
  return { services, navigate };
}

async function openMods() {
  const view = setup();
  await screen.findByRole("heading", { level: 1, name: "Create: Aeronautics" });
  fireEvent.click(screen.getByRole("button", { name: "Mods" }));
  const dialog = await screen.findByRole("dialog", { name: /^Mods/ });
  await within(dialog).findByRole("list", { name: "Mods installés" });
  return { ...view, dialog };
}

describe("Main : panneau Mods", () => {
  it("liste les mods avec taille, badge Personnalisé et compteur", async () => {
    const { dialog } = await openMods();
    expect(within(dialog).getAllByRole("listitem")).toHaveLength(9);
    expect(within(dialog).getByText("Personnalisé")).toBeTruthy();
    expect(within(dialog).getByText(/9 mods · 8 activés/)).toBeTruthy();
    await waitFor(() => expect(screen.getByRole("dialog", { name: "Mods · 9" })).toBeTruthy());
    expect(within(dialog).getAllByText(/Mo|Ko/).length).toBeGreaterThan(0);
  });

  it("filtre localement par nom", async () => {
    const { dialog } = await openMods();
    fireEvent.change(within(dialog).getByRole("searchbox"), { target: { value: "sodium" } });
    expect(within(dialog).getAllByRole("listitem")).toHaveLength(1);
    expect(within(dialog).getByText(/1 affiché/)).toBeTruthy();
    fireEvent.change(within(dialog).getByRole("searchbox"), { target: { value: "zzz" } });
    expect(within(dialog).getByText("Aucun mod ne correspond à la recherche.")).toBeTruthy();
  });

  it("l'interrupteur active/désactive le mod", async () => {
    const { dialog, services } = await openMods();
    const sw = within(dialog).getByRole("switch", { name: "Désactiver sodium-1.1.0.jar" });
    expect(sw.getAttribute("aria-checked")).toBe("true");
    fireEvent.click(sw);
    await waitFor(() => expect(within(dialog).getByRole("switch", { name: "Activer sodium-1.1.0.jar" }).getAttribute("aria-checked")).toBe("false"));
    expect((await services.modsList("inst-aeronautics")).find((m) => m.file === "sodium-1.1.0.jar")?.enabled).toBe(false);
    expect(within(dialog).getByText(/9 mods · 7 activés/)).toBeTruthy();
  });

  it("suppression annulable : Annuler rétablit le mod, sans appel au service", async () => {
    const { dialog, services } = await openMods();
    const rm = vi.spyOn(services, "modsRemove");
    fireEvent.click(within(dialog).getByRole("button", { name: "Supprimer mod-maison.jar" }));
    expect(within(dialog).getByText(/« mod-maison.jar » retiré\./)).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: /^Annuler \(\d+ s\)$/ }));
    expect(within(dialog).getByText("mod-maison.jar")).toBeTruthy();
    expect(rm).not.toHaveBeenCalled();
    expect(await services.modsList("inst-aeronautics")).toHaveLength(9);
  });

  it("suppression : appliquée après le délai d'annulation", async () => {
    const { dialog, services } = await openMods();
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      fireEvent.click(within(dialog).getByRole("button", { name: "Supprimer mod-maison.jar" }));
      await act(async () => { await vi.advanceTimersByTimeAsync(UNDO_MS + 50); });
    } finally { vi.useRealTimers(); }
    await waitFor(() => expect(within(dialog).queryByText("mod-maison.jar")).toBeNull());
    expect(await services.modsList("inst-aeronautics")).toHaveLength(8);
    expect(within(dialog).getByText(/8 mods/)).toBeTruthy();
  });

  it("Échap ferme le tiroir", async () => {
    const { dialog } = await openMods();
    fireEvent.keyDown(within(dialog).getByRole("searchbox"), { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("ajoute des mods avec le bouton (sélecteur de fichiers)", async () => {
    const { dialog, services } = await openMods();
    services.setPickedFiles(["C:\\x\\nouveau.jar"]);
    fireEvent.click(within(dialog).getByRole("button", { name: "Ajouter un .jar" }));
    expect(await within(dialog).findByText("nouveau.jar")).toBeTruthy();
    expect(within(dialog).getByText(/1 mod ajouté/)).toBeTruthy();
    expect(within(dialog).getByText(/10 mods/)).toBeTruthy();
  });

  it("ajoute les .jar déposés et signale les fichiers refusés", async () => {
    const { dialog, services } = await openMods();
    act(() => services.dropFiles(["/tmp/a.jar", "/tmp/notes.txt", "/tmp/b.JAR"]));
    expect(await within(dialog).findByText("a.jar")).toBeTruthy();
    expect(within(dialog).getByText("b.JAR")).toBeTruthy();
    expect(within(dialog).getByText(/2 mods ajoutés/)).toBeTruthy();
    expect(within(dialog).getByText(/refusé.*notes\.txt/)).toBeTruthy();
  });

  it("« Parcourir des mods » navigue vers browse avec l'instance", async () => {
    const { dialog, navigate } = await openMods();
    fireEvent.click(within(dialog).getByRole("button", { name: "Parcourir des mods" }));
    expect(navigate).toHaveBeenCalledWith("browse", { kind: "mod", instance: "inst-aeronautics" });
  });
});

describe("Main : entrées Parcourir", () => {
  it("plus de bouton Parcourir sur l'accueil (il est dans le « + »)", async () => {
    setup();
    await screen.findByRole("heading", { level: 1, name: "Create: Aeronautics" });
    expect(screen.queryByRole("button", { name: "Parcourir" })).toBeNull();
  });

  it("menu contextuel d'instance => mods de l'instance", async () => {
    const { navigate } = setup();
    await screen.findByRole("heading", { level: 1, name: "Create: Aeronautics" });
    fireEvent.contextMenu(within(screen.getByRole("navigation", { name: "Instances" })).getByRole("button", { name: "Create: Aeronautics" }));
    fireEvent.click(await screen.findByRole("menuitem", { name: "Parcourir des mods" }));
    expect(navigate).toHaveBeenCalledWith("browse", { kind: "mod", instance: "inst-aeronautics" });
  });
});
